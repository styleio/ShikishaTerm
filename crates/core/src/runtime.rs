//! The loop.
//!
//! One turn: take what the shell reported, act on it, let the tabs move, fire
//! whatever automation that woke, answer whoever is asking over the network,
//! and hand back a picture. It ran inside the window's own file until the
//! runtime became a crate; what it needs from a window is now written down as
//! `crate::host::Shell`, and a runtime with no window uses the same loop with
//! `Headless` in that place.

use crate::hooks::{Command, HookEngine, TabCtx};
use crate::host::Shell;
use crate::keymap::{key_to_bytes_with, named_key};
use crate::send::{PendingSend, Step, paste_chunks};
use crate::tab::{CopyState, RecordedStep, Tab, extract_text};
use crate::view::{
    RESULT_TAB, ScreenPush, Size, Surface, Ui, pty_dims, remote_floor, screen_push, surface_key,
    surface_moves, surfaces_of, surfaces_written, terminal_size, title_of,
};
use crate::desk::{
    apply_ws_config, build_engine, extract_env_block, open_declared_browsers, panel_places, reseat_desks,
    spawn_desk, surface_of_id, switch_desk,
};
use crate::{
    api, ball, bridge, caps, config, crypto, exchange, folders, grants, hooks, i18n, layout,
    netaddr, notify, placed, profile, remote, reply, sessionfind, ssh, tab, tailscale, update,
    watch, webui,
};
use crate::detect::TabState;
// Names only the tests at the bottom of this file reach for
#[cfg(test)]
use crate::{
    keymap::key_to_bytes,
    resume_plan_of,
    send::{PASTE_ACK_MS, PASTE_CHUNK, SUBMIT_GIVE_UP_MS, SUBMIT_QUIET_MS},
    desk::{TabAuto, automation_by_pane, carried_conversation},
};
use crate::{FIXED_TOKEN_MIN, append_hook_log, random_hex, remote_token};
use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const STATUS_BAR_HEIGHT: u16 = 1;
/// Say it, and offer to go where the answer is.
///
/// An address in a message box is an address somebody has to copy out by hand,
/// onto a machine where this program will not start. When the fix is a page,
/// opening the page is the fix -- and the browser is there even when the
/// runtime is not, because it is part of Windows.
/// Whether to quit now. Yes without a word when nothing is at work; when an
/// AI is, the person is asked, because quitting ends it mid-sentence -- the
/// conversation comes back next time, the work it was doing does not.
///
/// A native box rather than the board's own, so it is there for a quit asked
/// A rectangle on screen
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}
/// Absolute line position counted from the bottom of the screen
pub fn abs_line(offset: usize, rows: u16, cursor_row: u16) -> usize {
    offset + rows.saturating_sub(1).saturating_sub(cursor_row) as usize
}
/// Generates a tab name from argv ("ssh" -> "SSH")
/// Where thanks go. The Store's own review page for the Store's copy; the
/// repository for the zip's
pub const STORE_REVIEW_URL: &str = "ms-windows-store://review/?ProductId=9PB8XQVM87Z0";
pub const REPO_URL: &str = "https://github.com/styleio/ShikishaTerm";
/// Which first-run pointer to show, and what to remember about it.
///
/// Two steps and no more: "add a folder" while there is none, then "press
/// this folder's +" while there is one folder and nothing has been started in
/// it. `seen` is how far the pointing has got on this machine (0, 1 or 2),
/// and comes back moved on when the step it names is over -- shown once, or
/// done -- so a person who has been past a step is never pointed at it again.
/// Somebody with folders and no record has been here since before the
/// pointer existed, and is not pointed at anything
pub fn coach_step(folders: usize, seen: u8, past_the_plus: bool) -> (Option<u8>, u8) {
    match (folders, seen) {
        // Up until a folder exists, however many frames that takes
        (0, 0) | (0, 1) => (Some(1), 1),
        (1, 1) if !past_the_plus => (Some(2), 1),
        (_, 1) if past_the_plus => (None, 2),
        _ => (None, seen),
    }
}
/// The same for a folder on another machine. What it is cannot be asked of the
/// disk here, so the dialog says whether it is a repository: it listed it
fn add_remote_to_desk(desk: Option<&config::Desk>, host: &str, at: &str, project: Option<&str>) -> Result<Added, String> {
    let at = at.trim().trim_end_matches('/');
    let at = if at.is_empty() { "/" } else { at };
    let name = at.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(at).to_string();
    let here = desk.is_some_and(|w| {
        w.folders.iter().any(|f| {
            f.host.as_ref().is_some_and(|h| h.name == host)
                && f.cwd.as_ref().is_some_and(|c| c.to_string_lossy().trim_end_matches('/') == at)
        })
    });
    if here {
        return Ok(Added::Already(i18n::tp("msg.project.already", &[("name", &name)])));
    }
    let desk_name = desk.map(|w| w.name.clone()).unwrap_or_default();
    config::append_folder_starting(&desk_name, None, std::path::Path::new(at), None, &config::Start::Same, Some(host))
        .map_err(|e| format!("{e:#}"))?;
    // The folder is the project's checkout on that machine, which its
    // worktrees there are cut from: a project of its own, named for the
    // folder, or the one it was asked for from
    let project = project.map(str::trim).filter(|p| !p.is_empty()).map(str::to_string).unwrap_or_else(|| {
        let taken = |n: &str| desk.is_some_and(|w| w.projects.iter().any(|p| p.name == n && p.home_on(host).is_some()));
        match taken(&name) {
            false => name.clone(),
            true => (2..).map(|i| format!("{name} {i}")).find(|n| !taken(n)).expect("endless"),
        }
    });
    let home = config::ProjectHome { host: host.to_string(), at: at.to_string(), ..Default::default() };
    let desk_id = desk.map(|w| w.id.clone()).unwrap_or_default();
    // A project worked out from its checkout here, and not written down yet,
    // is written down with that checkout -- or its folders here would stop
    // being its folders
    let here = desk.and_then(|w| {
        w.folders
            .iter()
            .filter(|f| f.host.is_none())
            .filter_map(|f| f.cwd.as_deref().and_then(crate::repo::main_checkout))
            .find(|m| m.file_name().is_some_and(|n| n.to_string_lossy() == project))
            .map(|m| m.display().to_string().replace('\\', "/"))
    });
    config::set_project_home(&desk_id, &project, &home, here.as_deref()).map_err(|e| format!("{e:#}"))?;
    config::set_folder_far(&desk_name, std::path::Path::new(at), host, Some(&project), None).map_err(|e| format!("{e:#}"))?;
    Ok(Added::New(i18n::tp("msg.project.remote_added", &[("name", &name), ("host", host)])))
}

/// A worktree being made from the dialog, from the press until its card is on
/// the desk. The making runs on a thread; what it is written down as, and
/// where, is decided here when it is pressed, so a desk switched in between
/// still gets the folder it was asked for
struct Pending {
    id: u64,
    making: crate::worktree::Making,
    desk: String,
    /// The same desk by its id, which is how its projects are written
    desk_id: String,
    /// On a MicroVM: the project's checkout machine this making made has been
    /// written down as the project's -- which happens the moment it is made,
    /// whether or not the worktree after it is
    checkout_noted: bool,
    /// When writing that checkout down was last tried, so a failure is tried
    /// again a while later rather than on every pass
    checkout_tried: Option<Instant>,
    /// The folder the dialog was opened on
    from: std::path::PathBuf,
    /// What the folder's card is called. What the person wrote, in whatever
    /// letters they wrote it in -- this label is the app's own and never
    /// reaches git or the disk, so it is not held to the branch's letters
    label: String,
    /// The project's shared git folder, which puts its row under its heading
    family: String,
    start: config::Start,
    /// The issue or pull request it is for, when it is for one
    link: serde_json::Value,
    /// Whether it names and describes itself from what its AIs are asked
    auto: bool,
    /// The branch name this app drew, when nobody typed one. Written into the
    /// folder's source, where it is what lets the work rename the branch once
    /// (see [`crate::worktree::auto_rename_plan`]). A name somebody typed
    /// leaves this empty and the branch is theirs
    drawn: Option<String>,
    carry: Vec<crate::worktree::Carry>,
    /// The folder is there; only writing it down is left (or failed)
    made: bool,
    /// Why it failed, once it has
    error: Option<String>,
    /// The folder git will not touch until it is written down as one to trust,
    /// spelled as git asked for it. Set when git refused the project the
    /// branch is cut from, and when it refuses the branch's own folder
    /// afterwards -- a project on another machine's share is owned over there,
    /// and git stops at both
    trust: Option<String>,
    /// The folders that were to be shared with the project and could not be,
    /// waiting for the person to say whether to copy them in instead. A
    /// project on another machine's share is where this happens: what the
    /// branch would have pointed at cannot be pointed at from here
    unlinked: Vec<String>,
    /// Copying those in, once the person said to, and which ones. Runs on a
    /// thread of its own, because a folder on a share takes as long to copy as
    /// the branch took to make
    copying: Option<(crate::worktree::Making, Vec<String>)>,
    /// When it was written into the settings. The row stays until the desk
    /// that was read back lists the folder, so a card takes its place in the
    /// same frame the row goes
    written: Option<Instant>,
    /// Stopped and taken back: the row goes
    gone: bool,
}

impl Pending {
    fn state(&self) -> crate::uistate::MakingState {
        let plan = &self.making.plan;
        crate::uistate::MakingState {
            id: self.id,
            family: self.family.clone(),
            // What the card it turns into will be called, so the name does
            // not change under the person at the moment the row becomes a card
            name: self.label.clone(),
            folder: plan.folder.display().to_string(),
            stage: match (&self.error, &self.trust, self.made || self.written.is_some()) {
                (Some(_), _, _) => "failed".into(),
                // The folder is there and git will not go into it: not a
                // failure, and not something to leave unsaid either
                (None, Some(_), _) => "untrusted".into(),
                // ...and the same for what the branch was to share with the
                // project and could not. Asked after git, which is the one
                // that stops everything else
                (None, None, _) if !self.unlinked.is_empty() => "unlinked".into(),
                // While the answer is being carried out, the row says so
                (None, None, _) if self.copying.is_some() => {
                    crate::worktree::Stage::SettingUp.key().into()
                }
                (None, None, true) => crate::worktree::Stage::SettingUp.key().into(),
                (None, None, false) => self.making.stage().key().into(),
            },
            error: self.error.clone().unwrap_or_default(),
            trust: self.trust.clone().unwrap_or_default(),
            trust_line: self.trust.as_deref().map(crate::trust::line).unwrap_or_default(),
            trust_file: match self.trust.is_some() {
                true => crate::trust::file().display().to_string(),
                false => String::new(),
            },
            unlinked: self.unlinked.clone(),
        }
    }

    /// On a MicroVM: the project's checkout machine, once this making has
    /// made one, written down as the project's -- with a folder of its own on
    /// the desk, which is where the machine every later worktree is copied
    /// from is signed in to and set up
    fn note_checkout(&mut self) -> anyhow::Result<()> {
        if self.checkout_noted {
            return Ok(());
        }
        let plan = self.making.plan.clone();
        let (Some(host), Some(id)) = (plan.host.as_ref(), self.making.machines().checkout) else { return Ok(()) };
        // Tried again a while later when writing it failed, rather than on
        // every pass -- and never given up on: a checkout that is written
        // nowhere is a machine the next worktree makes a second of
        if self.checkout_tried.is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
            return Ok(());
        }
        self.checkout_tried = Some(Instant::now());
        let at = plan.main.to_string_lossy().to_string();
        let home = config::ProjectHome {
            host: host.name.clone(),
            at: at.clone(),
            placement: None,
            sandbox: Some(id.clone()),
            prepared: self.making.machines().prepared,
        };
        // A project nobody had written down is written down with its checkout
        // here, so the folders here stay in it
        let here = crate::repo::main_checkout(&self.from).map(|m| m.display().to_string().replace('\\', "/"));
        config::set_project_home(&self.desk_id, &plan.project, &home, here.as_deref())?;
        // Written down: what follows is the folder for it, which is tried
        // once -- a second try would add the folder twice
        self.checkout_noted = true;
        // The AI it was made with is the project's from now on, said in its
        // settings rather than left to be asked again
        config::set_project_value(
            &self.desk_id,
            &plan.project,
            "machine_ai",
            Some(plan.preparing.ai.as_deref().unwrap_or(crate::microvm::NO_AI)),
        )?;
        config::append_folder_starting(
            &self.desk,
            None,
            &plan.main,
            None,
            &crate::microvm::start_with(plan.preparing.ai.as_deref()),
            Some(&host.name),
        )?;
        config::set_folder_far(&self.desk, &plan.main, &host.name, Some(&plan.project), Some(&id))?;
        Ok(())
    }

    /// Writes the made folder into the settings, beside the folder it was
    /// asked from, running what the dialog chose
    fn write_down(&self) -> anyhow::Result<()> {
        let plan = &self.making.plan;
        // On a MicroVM the dialog's choice is what the machine was given: the
        // AI it was prepared with, or its shell. What this PC runs is not on
        // that machine
        let start = match (plan.host.as_ref(), &self.start) {
            (Some(h), _) if h.is_made() => crate::microvm::start_with(plan.preparing.ai.as_deref()),
            // On a server, "the same" is the same as the server's checkout it
            // is cut from -- its AI and terminals there -- as a worktree here
            // takes its checkout's. The folder the ask came from is this PC's
            (Some(_), config::Start::Same) => config::Start::SameAs(plan.main.clone()),
            _ => self.start.clone(),
        };
        config::append_folder_starting(
            &self.desk,
            Some(plan.like(&self.from)),
            &plan.folder,
            Some(&self.label),
            &start,
            plan.host.as_ref().map(|h| h.name.as_str()),
        )?;
        // A folder on another machine says which project it is a piece of --
        // nothing here can read that off its git folder -- and on a MicroVM,
        // which machine it is
        if let Some(h) = plan.host.as_ref() {
            config::set_folder_far(
                &self.desk,
                &plan.folder,
                &h.name,
                Some(&plan.project).filter(|p| !p.is_empty()).map(String::as_str),
                self.making.machines().worktree.as_deref(),
            )?;
        }
        // The branch it is really on, and whether this app is the one that
        // thought of that name. The line above writes the label as the branch,
        // which is right for a card and wrong for git: a label keeps what was
        // typed, in whatever letters it was typed in
        if plan.host.is_none() {
            config::set_folder_branch(&self.desk, &plan.folder, &plan.branch, self.drawn.as_deref())?;
        }
        // Named for its branch until something is asked in it -- on another
        // machine too, from what the input bar hands its AIs
        if self.auto {
            config::set_folder_auto_label(&self.desk, &plan.folder, true)?;
        }
        Ok(())
    }
}

/// A worktree deleted from its right-click, from the press until its folder is
/// gone. When the folder will not go, it stays until the person says what to do
/// with what is left. It is drawn as the same row as a worktree being made, and
/// it answers through the same numbers
struct Leaving {
    id: u64,
    removal: crate::worktree::Removal,
    desk: String,
    /// The project's shared git folder, which puts its row under its heading
    family: String,
    /// What its row calls it: its branch, or its folder's name
    name: String,
    /// The project's own checkout, told to forget a worktree left behind
    main: Option<std::path::PathBuf>,
    /// Where it stood in the list and what it said there, to put it back
    taken: Option<config::TakenFolder>,
    /// A project's checkout on a MicroVM, let go of by the project as its
    /// folder went: put back with the folder, if the folder is
    checkout: Option<(String, String, config::ProjectHome)>,
    /// Why the folder is still there, once it is known to be
    error: Option<String>,
    /// Put back in the list: the row waits for the card, as a made one does
    restored: Option<Instant>,
    gone: bool,
}

impl Leaving {
    fn state(&self) -> crate::uistate::MakingState {
        crate::uistate::MakingState {
            id: self.id,
            family: self.family.clone(),
            name: self.name.clone(),
            folder: self.removal.folder.display().to_string(),
            stage: match self.error {
                Some(_) => "unremoved".into(),
                None => "removing".into(),
            },
            error: self.error.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
}

/// Work on a MicroVM under way, from the press until what it made is written
/// down: a project being cloned onto one, or a checkout's machine being
/// prepared with the AI and the machine setup. Drawn as a row of the same
/// kind as a worktree being made, under the project, and answered through
/// the same numbers
/// The sign-in step of a project just cloned onto a MicroVM, while it is
/// pending: the checkout, its machine, and the AI to be signed in to there.
/// `shown` is whether the machine has answered once and the step is on the
/// board; before that, a machine that says "signed in" is never shown it
struct LoginPending {
    seq: u64,
    folder: String,
    host: config::HostSpec,
    home: config::ProjectHome,
    ai: String,
    name: String,
    shown: bool,
}

/// How often the machine is asked again while the sign-in step is open
const LOGIN_FRESH: Duration = Duration::from_secs(10);

/// The sign-in step for a server's git: a clone onto the server could not
/// read the repository, and the server's git is to be given a sign-in to
/// GitHub by the person, in a terminal on the server, with the commands the
/// step drafts. The terminal stands in the folder the clone was to go in,
/// put on the desk for the step when it was not there, and taken off again
/// when the step closes. "Clone again" tries the clone's row again
struct GitSignIn {
    seq: u64,
    /// The clone's row, tried again from the step
    row: u64,
    desk: String,
    host: config::HostSpec,
    spec: crate::ssh::Spec,
    url: String,
    /// What the server is, once it has been looked at
    probe: std::sync::Arc<std::sync::Mutex<Option<Result<crate::addproject::ServerLook, String>>>>,
    look: Option<crate::addproject::ServerLook>,
    /// Whether the terminal's folder was put on the desk for the step
    added: bool,
    /// Whether the server's git can read the repository now: when it was
    /// last asked, what it said, and whether an asking is on its way
    access: std::sync::Arc<std::sync::Mutex<(Option<Instant>, Option<bool>, bool)>>,
}

/// A terminal's rows as one text, each row with whether the terminal itself
/// wrapped it into the next, joined where a line went on: a row the
/// terminal wrapped, and a row written out to its last column -- a program
/// that draws its own screen breaks a long line at the width itself, and
/// the terminal never knows those two rows were one
pub fn join_rows(rows: &[(String, bool)], cols: u16) -> String {
    let mut text = String::new();
    for (row, wrapped) in rows {
        let full = row.chars().count() >= usize::from(cols);
        text.push_str(row);
        if !(*wrapped || full) {
            text.push('\n');
        }
    }
    text
}

/// The first web address in a terminal's text: from `https://` to the
/// first blank or quote. Empty when there is none
pub fn web_address_in(text: &str) -> String {
    let Some(at) = text.find("https://") else { return String::new() };
    text[at..]
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`'))
        .next()
        .unwrap_or_default()
        .trim_end_matches(['.', ',', ')', ']'])
        .to_string()
}

/// Delete a machine a row made and is putting away unwritten, unless the
/// settings point at it after all (a write that got halfway). On a thread:
/// the service is asked, and the board does not wait for it
fn let_go_if_nobodys(id: String) {
    std::thread::spawn(move || {
        // Kept when the settings cannot be read whole: whether it is
        // somebody's cannot be told, and a machine is not deleted on a guess
        match config::machines_in_use() {
            Ok(used) if !used.contains_key(&id) => {}
            Ok(_) => return,
            Err(why) => {
                append_hook_log(&format!("kept machine {id}: {why}"));
                return;
            }
        }
        if let Some(key) = crate::e2b::key() {
            crate::e2b::throw_away(&key, &id);
        }
    });
}

/// The phase of a row cloning onto a server over SSH
const PHASE_SSH_CLONING: &str = "ssh_cloning";

struct VmJob {
    id: u64,
    desk: String,
    desk_id: String,
    project: String,
    /// The entry, naming the machine once there is one
    host: config::HostSpec,
    /// The checkout on it
    at: String,
    work: VmWork,
    /// Why it failed, once it has
    error: Option<String>,
    /// Asked to stop: the row says so until it has
    stopping: bool,
    gone: bool,
}

enum VmWork {
    Clone {
        job: crate::microvm::Checkout,
        add: MicrovmAdd,
        /// What it is made from, kept to try again
        url: String,
        sign_in: config::FarSignIn,
        preparing: crate::microvm::Preparing,
    },
    Prepare {
        job: crate::microvm::Prepare,
        home: config::ProjectHome,
        preparing: crate::microvm::Preparing,
        /// The checkout to open the worktree dialog on once it is prepared
        follow: Option<String>,
    },
    /// A project cloned onto a server reached over SSH, by the server's own
    /// git. The same row as a MicroVM's clone: the dialog closes, and the
    /// row says how it is going, stops it, and tries it again
    SshClone {
        job: crate::addproject::Job,
        spec: crate::ssh::Spec,
        /// What it is cloned from and where, kept to try again
        url: String,
        parent: String,
    },
}

impl VmJob {
    /// What it made, once it has finished making it: the machine of a
    /// MicroVM's clone or preparation, the folder of a server's clone
    fn finished(&self) -> Option<String> {
        let done = match &self.work {
            VmWork::Clone { job, .. } => job.outcome(),
            VmWork::Prepare { job, .. } => job.outcome(),
            VmWork::SshClone { job, .. } => {
                return match job.outcome() {
                    crate::addproject::Outcome::Done(at) => Some(at.to_string_lossy().to_string()),
                    _ => None,
                };
            }
        };
        match done {
            crate::microvm::Outcome::Done { sandbox, .. } => Some(sandbox),
            _ => None,
        }
    }

    fn state(&self) -> crate::uistate::MakingState {
        let phase = match &self.work {
            VmWork::Clone { job, .. } => job.outcome(),
            VmWork::Prepare { job, .. } => job.outcome(),
            // Said in the words a MicroVM's phases are, so the row reads the same
            VmWork::SshClone { .. } => crate::microvm::Outcome::Running(PHASE_SSH_CLONING),
        };
        crate::uistate::MakingState {
            id: self.id,
            // Under the project's heading once its checkout there is on the
            // desk; a project being cloned has no heading yet, and its row
            // stands on its own
            family: match &self.work {
                VmWork::Clone { .. } | VmWork::SshClone { .. } => String::new(),
                VmWork::Prepare { .. } => crate::uistate::far_family(&self.host.name, &self.at),
            },
            name: match &self.work {
                VmWork::Clone { .. } | VmWork::SshClone { .. } => self.project.clone(),
                VmWork::Prepare { .. } => self.host.name.clone(),
            },
            folder: self.at.clone(),
            stage: match (&self.error, self.stopping, phase) {
                (Some(_), _, _) => "failed".into(),
                (None, true, _) => "stopping".into(),
                (None, false, crate::microvm::Outcome::Running(p)) => p.into(),
                (None, false, _) => crate::microvm::PHASE_PREPARING.into(),
            },
            error: self.error.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
}

/// How long a written-down worktree's row waits for the desk to list it
/// before it goes anyway. A reload that never comes must not leave it forever
const MAKING_CARD_WAIT: Duration = Duration::from_secs(10);

/// The state file holding the projects whose found worktrees are kept hidden,
/// one shared git folder to a line
const WORKTREES_KEPT: &str = "worktrees-kept";

fn load_kept() -> std::collections::BTreeSet<String> {
    std::fs::read_to_string(config::state_path(WORKTREES_KEPT))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn save_kept(kept: &std::collections::BTreeSet<String>) {
    let text: String = kept.iter().map(|k| format!("{k}\n")).collect();
    let _ = crate::crypto::write_atomic(&config::state_path(WORKTREES_KEPT), &text);
}

/// What adding a folder to the desk came to
enum Added {
    /// It is on the desk now; the words to say so
    New(String),
    /// It was already there; the words to say so
    Already(String),
}

/// Put a folder on the desk as a project -- the one way every door (the
/// picker, a clone, a new project) ends. Once only: a second press on the same
/// folder would put two headings over one place, each with its own tabs. The
/// words say which it is, a git repository or a plain folder
fn add_to_desk(desk: Option<&config::Desk>, at: &std::path::Path, start: &config::Start) -> Result<Added, String> {
    let name = at.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| at.display().to_string());
    let here = desk.is_some_and(|w| {
        w.folders.iter().filter_map(|f| f.cwd.as_ref()).any(|c| crate::uistate::same_folder(c, at))
    });
    if here {
        return Ok(Added::Already(i18n::tp("msg.project.already", &[("name", &name)])));
    }
    let desk_name = desk.map(|w| w.name.clone()).unwrap_or_default();
    // Open, running the default command: a project added is somewhere to work
    // at once, not a card with nothing in it
    config::append_folder_starting(&desk_name, None, at, None, start, None).map_err(|e| format!("{e:#}"))?;
    Ok(Added::New(match crate::repo::family_of(at).is_some() {
        true => i18n::tp("msg.project.added", &[("name", &name)]),
        false => i18n::tp("msg.folder.added", &[("name", &name)]),
    }))
}

/// The state file that says the first-start setup has been answered
const SETUP_ANSWERED: &str = "setup";

/// The desk the first-start setup makes, and the name of the git account it
/// gives that desk when GitHub CLI is installed
const FIRST_DESK: &str = "DESK";
const FIRST_DESK_GH: &str = "gh";

/// Whether the first-start setup is asked. Only on a first start -- a machine
/// that already has settings has already chosen -- and only until it has been
/// answered once. A first start can happen more than once: until a folder is
/// added the settings file does not count as settings, so the answer is kept
/// on its own rather than inferred from that file
pub fn setup_wanted(first_run: bool, answered: bool) -> bool {
    first_run && !answered
}

/// What the setup's "Refresh" found, said about the page it was pressed on:
/// the AIs on the first, GitHub CLI on the second
fn setup_found(now: &crate::uistate::SetupState, step: u8) -> String {
    if step >= 2 {
        return i18n::t(if now.gh { "msg.setup.gh.found" } else { "msg.setup.gh.found_none" });
    }
    let names: Vec<&str> = now.installed.iter().map(|a| a.name.as_str()).collect();
    match names.is_empty() {
        true => i18n::t("msg.setup.found_none"),
        false => i18n::tp("msg.setup.found", &[("names", &names.join(", "))]),
    }
}

/// The AIs this machine can start, one per profile whose command is on PATH.
///
/// Offered the way the settings' own form would launch them: with the CLI's
/// "act without asking" flag, because an AI started to work in a folder of
/// its own is started to work unattended. A profile whose command is not
/// installed is not offered -- a choice that fails on pressing is worse than
/// no choice
pub fn startable_ais() -> Vec<crate::uistate::AiChoice> {
    let mut out = Vec::new();
    for pf in crate::profile::files() {
        let Some(cmd) = pf.command_match.first().map(|c| c.trim().to_string()) else { continue };
        if cmd.is_empty() || crate::tab::resolve_command(&cmd).is_none() {
            continue;
        }
        let command = match crate::tab::bypass_flag(&cmd) {
            Some(flag) => format!("{cmd} {flag}"),
            None => cmd.clone(),
        };
        out.push(crate::uistate::AiChoice { key: cmd, name: pf.name.clone(), command });
    }
    out
}
/// A project on its way onto a MicroVM from the add-a-project dialog: what it
/// is written down as once its checkout is there
#[derive(Clone)]
struct MicrovmAdd {
    host: String,
    project: String,
    /// The account the dialog said it signs in as, written as the project's
    /// own when it is one the person set up -- so every worktree after signs
    /// in the same way, and the settings say so
    account: Option<String>,
    /// The AI its machine is given, written as the project's own
    ai: Option<String>,
}

/// The account a project from `url` signs in to its git server as, before
/// the project is written down: the one of the desk's that says it is for
/// the address's owner, else the way git on this PC signs in
fn account_for_url(desk: Option<&config::Desk>, url: &str) -> Option<String> {
    let (host, owner) = crate::microvm::host_and_owner(url)?;
    let desk = desk?;
    config::git_accounts_for(&desk.git_accounts, Some(&host), Some(&owner))
        .into_iter()
        .find(|(_, fits)| *fits)
        .map(|(a, _)| a.name)
}

/// The household a worktree being made belongs to, which puts its row under
/// its project's heading: the git folder here, or -- on another machine --
/// the one its checkout there has, named with the machine
fn making_family(plan: &crate::worktree::Plan, from: &std::path::Path) -> String {
    match plan.host.as_ref() {
        Some(h) => crate::uistate::far_family(&h.name, &plan.main.to_string_lossy()),
        None => crate::repo::family_of(from).map(|f| f.display().to_string()).unwrap_or_default(),
    }
}

/// What the worktree dialog sends for "this PC" from a folder that is on
/// another machine, where saying nothing means that machine
const HERE: &str = "@here";

/// A project on another machine, as a worktree there is planned from: what
/// [`crate::worktree::Far`] borrows, held
struct FarProject {
    host: config::HostSpec,
    home: Option<config::ProjectHome>,
    project: String,
    origin: String,
    env: Option<crate::devcontainer::Env>,
    sign_in: config::FarSignIn,
    preparing: crate::microvm::Preparing,
}

impl FarProject {
    fn of(&self) -> crate::worktree::Far<'_> {
        crate::worktree::Far {
            host: &self.host,
            home: self.home.as_ref(),
            project: &self.project,
            origin: &self.origin,
            env: self.env.clone(),
            sign_in: self.sign_in.clone(),
            preparing: self.preparing.clone(),
        }
    }
}

/// A project on `host`, for cutting a worktree there from the folder `from`:
/// its checkout there, what it is called, where it is fetched from, and --
/// on a MicroVM -- what it signs in to its git server as, with the account's
/// name as the dialog says it. A project nobody has written down is called
/// what its checkout here is called, the way the settings would name it.
/// `machine_ai` is the AI the dialog chose for a checkout made on a MicroVM
/// (empty: what the project says)
fn far_of(
    desk: Option<&config::Desk>,
    project: Option<&config::ProjectSpec>,
    host: &config::HostSpec,
    from: &std::path::Path,
    checkout: &Option<std::path::PathBuf>,
    env: Option<crate::devcontainer::Env>,
    machine_ai: &str,
) -> (FarProject, String, Result<config::FarSignIn, String>) {
    let name = project
        .map(|p| p.name.clone())
        .or_else(|| checkout.as_deref().and_then(|c| c.file_name()).map(|n| n.to_string_lossy().to_string()))
        .or_else(|| from.to_string_lossy().trim_end_matches('/').rsplit(['/', '\\']).next().map(str::to_string))
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| "project".into());
    let origin = checkout
        .as_deref()
        .and_then(crate::repo::remote_url_of)
        .map(|u| crate::worktree::fetchable(&u))
        .unwrap_or_default();
    let git = desk.map(|d| d.git_use(project.and_then(|p| p.git_account.as_deref()))).unwrap_or_default();
    let account = match &git {
        config::GitUse::Unset | config::GitUse::Pc(None) => i18n::t("tui.branch.signin.pc"),
        config::GitUse::Pc(Some(login)) | config::GitUse::Gh { login, .. } => login.clone(),
        config::GitUse::Account { spec } => spec.name.clone(),
        config::GitUse::Missing(n) => n.clone(),
    };
    let sign_in = match host.is_made() {
        true => git.far(&|k| crate::git::secret(k)),
        false => Ok(config::FarSignIn::Nobody),
    };
    let far = FarProject {
        host: host.clone(),
        home: project.and_then(|p| p.home_on(&host.name)).cloned(),
        project: name,
        origin,
        env,
        // One that cannot be had is said in the dialog, and the machine signs
        // in to nothing: a public repository still clones
        sign_in: sign_in.clone().unwrap_or_default(),
        preparing: crate::microvm::Preparing::of(
            match machine_ai.trim() {
                "" => project.and_then(|p| p.machine_ai.as_deref()),
                chosen => Some(chosen),
            },
            project.and_then(|p| p.machine_setup.as_deref()),
        ),
    };
    (far, account, sign_in)
}

/// Every MicroVM folder's machine, with what it should sign in to git as: its
/// project's account, the way a worktree of it is made with it
fn microvm_sign_ins(desks: &[config::Desk]) -> Vec<(String, config::FarSignIn)> {
    let mut out = Vec::new();
    for d in desks {
        for f in &d.folders {
            let Some(id) = f.host.as_ref().filter(|h| h.is_made()).and_then(|h| h.instance.clone()) else { continue };
            let chosen = f
                .project
                .as_deref()
                .and_then(|n| d.projects.iter().find(|p| p.name == n))
                .and_then(|p| p.git_account.as_deref());
            if let Ok(far) = d.git_use(chosen).far(&|k| crate::git::secret(k)) {
                out.push((id, far));
            }
        }
    }
    out
}

/// What a new folder should run, from the word the dialog sent.
///
/// Empty is "the same as the folder it is cut from", `none` is nothing, and
/// anything else names one of the AIs offered. A name that is not on the list
/// -- an AI uninstalled between the offer and the press -- runs nothing rather
/// than a command that would fail on screen
pub fn start_of(said: &str, ais: &[crate::uistate::AiChoice]) -> config::Start {
    match said.trim() {
        "" => config::Start::Same,
        "none" => config::Start::Nothing,
        key => ais
            .iter()
            .find(|a| a.key == key)
            .map(|a| config::Start::One { name: a.key.clone(), command: a.command.clone() })
            .unwrap_or(config::Start::Nothing),
    }
}
/// How long a message stays part of the state. The screen it lands on is what
/// decides when the toast actually fades (up to nine seconds for a long
/// warning); this is the slightly later moment the app stops carrying it, so
/// nothing stale is handed to a surface that arrives afterwards.
pub const FLASH_LIFE: Duration = Duration::from_secs(12);
/// Every tab's identity and state, in the shape the automation engine reads.
///
/// Built in one place because two callers need the same thing. One is the
/// detection tick, so a script can read `shikisha.state`. The other is the
/// external API — and there it is not a convenience: a call arrives naming
/// nothing but its own key, and the engine works out which tab that is by
/// looking the name up in THIS list. An engine that has not been given it
/// attributes the call to nobody, which is how a hook comes to report a
/// conversation id "from tab0: no such tab" and lose it.
/// Whose call this is, for the permission table.
///
/// Nobody says who they are: the external API mints a key per tab, so a call
/// arrives already attached to the tab whose key opened the connection, and
/// this only has to look up what is running there. A caller holding no tab's
/// key is a program the person started themselves, which is why "outside every
/// tab" counts as the person -- the same reading the chain brake already takes.
/// A key naming a tab that is no longer there is nobody, and answers as the AI:
/// the side that cannot do harm if the guess is wrong
pub fn subject_of(caller: Option<&str>, tabs: &[Tab]) -> grants::Subject {
    let Some(name) = caller else {
        return grants::Subject::Human;
    };
    let keys: Vec<hooks::TabKey> = tab_states(tabs).into_iter().map(|(k, _)| k).collect();
    match hooks::TabRef::Name(name.to_string())
        .resolve(&keys)
        .and_then(|i| tabs.get(i - 1))
    {
        Some(t) if t.is_ai() => grants::Subject::Ai,
        Some(_) => grants::Subject::Human,
        None => grants::Subject::Ai,
    }
}
pub fn tab_states(tabs: &[Tab]) -> Vec<(hooks::TabKey, String)> {
    tabs.iter()
        .map(|t| (t.key(), t.state.label().to_string()))
        .collect()
}
/// One sentence a person can read, out of what Lua reported.
///
/// An error arriving from a primitive carries the marks of where it came from
/// -- "runtime error:" in front and a stack traceback behind. Both belong in
/// the log and in a script author's hands; neither belongs on a panel
pub fn plain_error(said: &str) -> String {
    said.split("stack traceback:")
        .next()
        .unwrap_or(said)
        .trim()
        .trim_start_matches("runtime error:")
        .trim()
        .to_string()
}
/// Where each tab is working, in the same order as the states.
///
/// A tab with no folder of its own gets an empty path rather than the app's
/// own folder: falling back would mean `git_status()` from a tab that is
/// nowhere quietly answers about the app's own repository. Nothing that runs
/// a program on this PC arrives here without one -- a tab out of the settings
/// with no folder is held rather than started (`TabOptions::hold`) -- so the
/// empty path is a model conversation or a terminal on another machine, and
/// both of those are genuinely in no folder of ours
pub fn tab_places(tabs: &[Tab]) -> Vec<hooks::TabPlace> {
    tabs.iter().map(tab_place).collect()
}
/// The folder the git panel called `panel` reports on: a panel launched as a
/// tab of its own, or the column standing beside a tab. `None` for one that
/// works in no folder, so that nothing quietly answers about the app's own
fn panel_place_dir(surfaces: &[Surface], tabs: &[Tab], panel: &str) -> Option<std::path::PathBuf> {
    panel_places(surfaces)
        .into_iter()
        .chain(tab_places(tabs).into_iter().filter(|p| !p.dir.as_os_str().is_empty()))
        .find(|p| p.key.matches(panel))
        .map(|p| p.dir)
}
/// Where every screen is working, in the order the screens are in.
///
/// This is the list `origin` -- "which screen is asking" -- is a number into.
/// A screen number is not a tab number: a page, a git panel, an editor or a
/// file panel takes a place in the row of screens, so the moment one of them
/// sits anywhere but last, the third screen and the third tab are two
/// different things. Handing the engine the tabs with the panels bolted on the
/// end made them agree only by luck, and when they disagreed a tab's own
/// report -- the conversation it is running -- was written down against
/// whoever happened to be standing in that position.
///
/// So: one row per screen, in screen order, always. A screen that works in no
/// folder still gets a row, under the name automation knows it by, because
/// what has to hold is that row *n* is screen *n*.
pub fn places_by_surface(surfaces: &[Surface], tabs: &[Tab]) -> Vec<hooks::TabPlace> {
    surfaces
        .iter()
        .zip(surface_keys(surfaces, tabs))
        .map(|(s, key)| match s {
            Surface::Session(i) => tabs
                .get(*i)
                .map(tab_place)
                .unwrap_or(hooks::TabPlace { key, ..Default::default() }),
            panel => crate::desk::panel_place(panel)
                .unwrap_or(hooks::TabPlace { key, ..Default::default() }),
        })
        .collect()
}
/// Where one tab is working, and what its folder guards.
/// One file of a working tree, as text, read where the tree is: on this PC,
/// or on the machine `at` names, where `dir` is the folder as that machine
/// spells it
fn tree_file_read(dir: &std::path::Path, rel: &str, at: Option<&crate::elsewhere::Elsewhere>) -> anyhow::Result<String> {
    match at {
        None => Ok(std::fs::read_to_string(dir.join(rel))?),
        Some(at) => {
            let path = format!("{}/{rel}", crate::desk::far_path(dir).trim_end_matches('/'));
            match crate::elsewhere::files(at, crate::ssh::FileJob::Read { path }, TREE_FILE_WAIT_MS)? {
                crate::ssh::FileAnswer::Bytes(b) => Ok(String::from_utf8_lossy(&b).into_owned()),
                _ => anyhow::bail!(crate::i18n::t("err.files.binary")),
            }
        }
    }
}
/// The same file written back, where the tree is
fn tree_file_write(dir: &std::path::Path, rel: &str, at: Option<&crate::elsewhere::Elsewhere>, text: &str) -> anyhow::Result<()> {
    match at {
        None => Ok(std::fs::write(dir.join(rel), text)?),
        Some(at) => {
            let to = format!("{}/{rel}", crate::desk::far_path(dir).trim_end_matches('/'));
            let job = crate::ssh::FileJob::Write { to, bytes: text.as_bytes().to_vec() };
            crate::elsewhere::files(at, job, TREE_FILE_WAIT_MS).map(|_| ())
        }
    }
}
/// How long a file of a tree on another machine may take to come or go
const TREE_FILE_WAIT_MS: u64 = 60_000;
fn tab_place(t: &Tab) -> hooks::TabPlace {
    // A folder on another machine is spelled the way that machine spells
    // it, and stays so: `/home/user/proj` is not a relative path of this PC
    // to put under the current folder, it is where the tab is over there
    let dir = match (t.remote_cwd(), t.cwd().map(std::path::Path::to_path_buf)) {
        (Some(far), _) => std::path::PathBuf::from(far),
        (None, Some(p)) if p.is_absolute() => p,
        (None, Some(p)) => std::env::current_dir().map(|c| c.join(&p)).unwrap_or(p),
        (None, None) => std::path::PathBuf::new(),
    };
    hooks::TabPlace {
        key: t.key(),
        dir,
        remote: tab_machine(t),
        // The folder over there, for a tab whose folder is there. A terminal
        // tab given only an address has none: a shell starts wherever
        // signing in puts it, so there is nothing on that end for a path to
        // be outside of, and the fence that does hold is this tab's own
        // working folder. A file panel is the one given both (`desk::panel_place`)
        remote_dir: t.remote_cwd().unwrap_or_default().to_string(),
        protect: t.protect().to_vec(),
        git: t.git_use.clone(),
    }
}
/// The machine a tab's terminal is on, when it is not this one
fn tab_machine(t: &Tab) -> Option<crate::elsewhere::Elsewhere> {
    match (t.remote(), t.cloud()) {
        (Some(spec), _) => Some(crate::elsewhere::Elsewhere::Ssh(spec.clone())),
        (None, Some(host)) => Some(crate::elsewhere::Elsewhere::Cloud(host.clone())),
        (None, None) => None,
    }
}
pub fn resume_plan(t: &Tab, alone: bool, keep: bool) -> (tab::Resume, Option<&'static str>) {
    if !keep {
        return (tab::Resume::Fresh, None);
    }
    let Some(spec) = t.resume.as_ref() else {
        // Only an AI has a conversation to lose. A shell restarted is a shell
        // restarted, and "this CLI cannot carry a conversation" said about one
        // is a sentence about something that was never there
        return (tab::Resume::Fresh, t.is_ai().then_some("msg.resume.unsupported"));
    };
    // Nothing has happened in this tab yet, and it was having a conversation
    // when the app last closed. "Carry the conversation over" can only mean
    // that one — which is why this needs no key of its own
    let want = match (t.spoke(), t.previous.clone()) {
        (false, Some(before)) => Some(before),
        _ => t.session.clone(),
    };
    if let Some(s) = want
        && !spec.with_id.is_empty() {
            // A conversation can be deleted between one run and the next. Ask
            // before handing the CLI an id it has never heard of: it would say
            // so in its own words, in red, in a place the person has no reason
            // to connect with the key they just pressed
            // On another machine the record is there, and the line typed
            // there asks for it (see `tab::far_launch`)
            let far = t.remote().is_some() || t.cloud().is_some();
            let gone = !far
                && spec
                    .verify
                    .as_ref()
                    .is_some_and(|v| !sessionfind::exists(v, &s.id));
            if gone {
                append_hook_log(&format!("\"{}\" no longer has {}", t.title, s.short()));
                return (tab::Resume::Fresh, Some("msg.resume.gone"));
            }
            return (tab::Resume::Id(s), None);
        }
    if !spec.newest_here.is_empty() {
        if alone {
            return (tab::Resume::NewestHere, None);
        }
        return (tab::Resume::Fresh, Some("msg.resume.ambiguous"));
    }
    (tab::Resume::Fresh, Some("msg.resume.unknown"))
}
/// Try again to start a tab that could not start, when that is what the
/// surface is. The same road the settings take to launch what they name, so an
/// install made a minute ago is found the way the first attempt looked for it.
/// None for any other surface, which the ordinary restart handles
pub fn retry_failed(
    at: usize,
    surfaces: &[Surface],
    tabs: &mut Vec<Tab>,
    desk: Option<&config::Desk>,
    rows: u16,
    cols: u16,
    carry: Option<&crate::lastsession::Saved>,
) -> Option<String> {
    let Some(Surface::Failed { name, .. }) = surfaces.get(at.checked_sub(1)?) else {
        return None;
    };
    let desk = desk?;
    let mut errors = Vec::new();
    crate::desk::apply_ws_config(tabs, desk, rows, cols, &mut errors, &mut Default::default(), carry);
    Some(match crate::desk::launch_failure(&desk.name, name) {
        Some(still) => still.why,
        None => i18n::tp("msg.failed.started", &[("name", name)]),
    })
}

/// Relaunch whatever one surface holds, and answer with what to say about it.
///
/// The one restart in the app. Three doors reach it — Ctrl+B r / Ctrl+B R, the
/// ↻ pair in a pane's caption, and the phone's ↻ — and they must do the same
/// thing, so none of them carries logic of its own.
///
/// A session relaunches its command, carrying the conversation when `keep` and
/// when that can be done safely. A page has no process to relaunch: opening it
/// again exactly as it was opened is the same act — a fresh page object, back
/// at the URL it started on, with whatever the page had built up gone. Not yet
/// a fresh identity; see `browser_spec` on why the private profile isn't
/// reaching WebView2. Anything else (the board, the app's own screens) has
/// nothing to put back and is left alone.
#[allow(clippy::too_many_arguments)]
pub fn restart_surface(
    at: usize,
    keep: bool,
    tabs: &mut [Tab],
    surfaces: &[Surface],
    engine: &mut Option<HookEngine>,
    caps: &hooks::Caps,
    rows: u16,
    cols: u16,
) -> Option<String> {
    // Whatever this tab had queued or was waiting on dies with the process it
    // was waiting on. Done before the kill, while the index still means what
    // the engine thinks it means
    if let Some(eng) = engine.as_mut() {
        eng.cancel_tab(at);
    }
    let alone = session_at(surfaces, at)
        .map(|i| only_one_here(tabs, i))
        .unwrap_or(false);
    if let Some(t) = session_mut(tabs, surfaces, at) {
        return Some(restart_tab(t, alone, keep, rows, cols));
    }
    let name = restartable_page(surfaces, at, caps)?;
    Some(
        match caps
            .browser_spec(&name)
            .ok_or_else(|| anyhow::anyhow!("no spec"))
            .and_then(|(url, profile)| {
                caps.browser_close(&name)?;
                caps.browser_open(&name, &url, profile)
            }) {
            Ok(()) => i18n::tp("msg.restarted", &[("name", &name)]),
            Err(e) => i18n::tp("msg.restart_failed", &[("error", &format!("{e:#}"))]),
        },
    )
}
/// Whether this tab is the only one that could have left "the newest
/// conversation in this folder" — same program, same folder.
///
/// Worked out before the tab is borrowed to restart it, because by then the
/// others are out of reach
pub fn only_one_here(tabs: &[Tab], index: usize) -> bool {
    let Some(me) = tabs.get(index) else {
        return false;
    };
    !tabs.iter().enumerate().any(|(i, o)| {
        i != index
            && o.program() == me.program()
            && match (o.cwd(), me.cwd()) {
                (Some(a), Some(b)) => crate::sessionfind::same_folder(a, b),
                (a, b) => a.is_none() && b.is_none(),
            }
    })
}
/// Restart one tab, carrying its conversation when that can be done safely, and
/// answer with what to tell the person.
pub fn restart_tab(t: &mut Tab, alone: bool, keep: bool, rows: u16, cols: u16) -> String {
    let (plan, why) = resume_plan(t, alone, keep);
    restarted(t, plan, why, rows, cols)
}

/// Put one tab into a conversation somebody picked for it.
///
/// The same restart, with the conversation named rather than worked out: the
/// person is looking at a list of what was said in this folder and has chosen
/// one. Down the same road, so a tab put back this way is an ordinary resumed
/// tab in every other respect
/// What was said before in the folder of the tab on screen `which`.
///
/// `which` is the number the screen drew the tab under, which counts pages and
/// panels too. Read as a place in the tab list instead, a page anywhere in
/// front of the tab pointed it at the tab after it, or past the last one --
/// and then no answer came back at all, so the list sat open under its
/// heading with nothing in it
///
/// A tab on another machine has its CLI's records there: the list is asked of
/// that machine on a thread (`tx`), and says it is being asked until it comes
fn past_of(
    surfaces: &[Surface],
    tabs: &[Tab],
    which: usize,
    tx: &std::sync::mpsc::Sender<(usize, Vec<crate::vault::Hit>)>,
) -> Option<crate::uistate::PastState> {
    let t = tabs.get(session_at(surfaces, which)?)?;
    if let (Some(at), Some(cwd)) = (tab_machine(t), t.cwd()) {
        let (program, cwd, tx) = (t.program().to_string(), cwd.to_path_buf(), tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send((which, crate::vault::here_far(&program, &at, &cwd, 12)));
        });
        return Some(crate::uistate::PastState { tab: which, name: t.title.clone(), hits: Vec::new(), asking: true });
    }
    Some(crate::uistate::PastState {
        tab: which,
        name: t.title.clone(),
        hits: t
            .cwd()
            .map(|c| crate::vault::here(t.program(), c, 12))
            .unwrap_or_default(),
        asking: false,
    })
}

pub fn resume_tab_into(t: &mut Tab, id: String, rows: u16, cols: u16) -> String {
    let s = tab::Session { id, source: tab::SessionSource::Store };
    restarted(t, tab::Resume::Id(s), None, rows, cols)
}

/// The restart itself, and what to say about it.
fn restarted(t: &mut Tab, plan: tab::Resume, why: Option<&'static str>, rows: u16, cols: u16) -> String {
    let carried = matches!(plan, tab::Resume::Id(_) | tab::Resume::NewestHere);
    match t.restart_as(rows, cols, plan) {
        Ok(()) => {
            if let Some(s) = t.session.as_ref() {
                append_hook_log(&format!("restarted \"{}\" carrying {}", t.title, s.short()));
            }
            match (carried, why) {
                // Say the way back at the moment it is wanted: the one time
                // resuming is wrong is when the conversation is what broke the
                // CLI, and that is exactly when this message is on screen
                (true, _) => i18n::tp("msg.resumed", &[("name", &t.title)]),
                (false, Some(k)) => i18n::tp(k, &[("name", &t.title)]),
                (false, None) => i18n::tp("msg.restarted", &[("name", &t.title)]),
            }
        }
        Err(e) => i18n::tp("msg.restart_failed", &[("error", &t.launch_hint(&e.to_string()))]),
    }
}
/// Divide the focused pane and answer with the surface now under the cursor.
///
/// Two doors ask for this — `Ctrl+B %` and the ⊞ / ⊟ in a pane's caption — and
/// they must divide identically: which surface the new half shows, and where
/// focus lands, are decisions, not details of whichever door was used
/// The editor the file list opens when there is nowhere else to put a file.
///
/// One name, so pressing ten files leaves one editor. It is not written to the
/// settings: it exists while it is open and is gone when it is closed.
pub const EDITOR_SCRATCH: &str = "editor.here";

pub fn run(shell: &mut dyn crate::host::Shell) -> Result<()> {
    // The mode flag is not a command to launch.
    // Forgetting to filter it out would send us looking for a program named `--window`.
    let cmd_args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !matches!(a.as_str(), "--settings"))
        .collect();
    let start = Instant::now();
    // Width comes from config if given; otherwise it's auto-computed from tab names
    // (finalized once tabs are launched).
    let (mut rows, mut cols) = pty_dims(shell.size()?);

    // Tab layout precedence: CLI args (debug) > config.json > default (1 PowerShell tab)
    let cfg = if cmd_args.is_empty() {
        config::load()
    } else {
        None
    };
    let mut startup_errors: Vec<String> = Vec::new();
    let mut desks: Vec<config::Desk> = Vec::new();
    if let Some(c) = &cfg {
        let (desk, errs) = c.resolve_desks();
        desks = desk;
        startup_errors.extend(errs);
    }
    crate::microvm::keep_sign_ins_current(microvm_sign_ins(&desks));

    // The external control API. Opened before the first tab, because a tab's
    // process is handed the way in as it is launched — one started earlier
    // would spend its whole life unable to call back
    let mut api_server = match api::ApiServer::start(
        cfg.as_ref().map(|c| c.external_api.access).unwrap_or_default(),
    ) {
        Ok(s) => s,
        Err(e) => {
            // Not worth refusing to start over. Say so plainly in the log
            // rather than leaving a silent absence
            append_hook_log(&format!("external API did not start: {e}"));
            None
        }
    };

    let mut tabs: Vec<Tab> = Vec::new();
    let remembered = config::load_last_desk();
    let mut desk_index = starting_desk(
        cfg.as_ref().and_then(|c| c.restore_desk).unwrap_or(true),
        remembered.as_deref(),
        &desks,
    );
    if let Some(w) = desks.get(desk_index) {
        // Knowing where we started is a handy clue later, when tracking down "why is
        // this the screen we're on".
        append_hook_log(&format!(
            "Startup: desk \"{}\" ({})",
            w.name,
            match remembered.as_deref() {
                Some(r) if r == w.id || r == w.name => "resuming last session",
                _ => "first desk",
            }
        ));
    }
    // If secrets are encrypted, ask for the master password -- before anything
    // that needs one is started. A tab is handed its git account's token as it
    // is born and cannot be handed one afterwards, so a store still locked at
    // that moment is a terminal that spends its whole life signing in as
    // nobody. The same goes for the model connections below
    let mut password: Option<String> = None;
    if let Some(path) = cfg.as_ref().and_then(|c| c.secrets_path())
        && std::fs::read_to_string(&path)
            .map(|t| crypto::is_encrypted(&t))
            .unwrap_or(false)
        {
            let mut refused = false;
            for attempt in 1..=3 {
                let note = if attempt == 1 {
                    i18n::t("prompt.password.note")
                } else {
                    i18n::t("prompt.password.retry")
                };
                match shell.ask_password(&i18n::t("prompt.password.title"), &note)? {
                    Some(pw) => {
                        let ok = std::fs::read_to_string(&path)
                            .ok()
                            .and_then(|t| serde_json::from_str::<crypto::Envelope>(&t).ok())
                            .map(|env| crypto::decrypt(&env, &pw).is_ok())
                            .unwrap_or(false);
                        if ok {
                            password = Some(pw);
                            refused = false;
                            break;
                        }
                        refused = true;
                    }
                    // On cancel, continue without secrets (only notifications become unusable)
                    None => {
                        startup_errors
                            .push(i18n::t("prompt.password.skipped"));
                        break;
                    }
                }
            }
            // A password was given and it was not the one. Said as that: a
            // service handed a wrong credential otherwise came up saying only
            // that "a master password is required", which reads as though none
            // had arrived and sends somebody to check the wrong thing
            if refused {
                startup_errors.push(i18n::t("prompt.password.wrong"));
            }
        }

    // What was on screen when the app last closed. Two things are taken from
    // it, and they are taken at different moments. The conversations are needed
    // HERE, before the first process starts: carrying one over is a decision
    // the launch itself makes, and asking afterwards would mean minting a
    // conversation only to throw it away. The division of the screen is put
    // back further down, once there are tabs for the panes to point at
    // What the SSH tabs sign in with, handed to the connection thread before
    // anything is launched: a tab that comes up before its password is known
    // would be told there is none (the store lives on this thread, the
    // connections on another -- see `ssh::use_secrets`)
    if let Some(c) = cfg.as_ref() {
        let tokens = c.resolve_tokens(password.as_deref());
        ssh::use_secrets(tokens.clone());
        // The sandbox service's key travels with the rest, under its own name
        crate::e2b::use_key(tokens.get("e2b_api_key").cloned());
        // ...and what a git typed in a terminal signs in with, for the same
        // reason: the tab is handed its account's token as it starts
        crate::git::use_secrets(tokens);
    }
    let mut last_session = crate::lastsession::Saved::load();
    if !cmd_args.is_empty() {
        tabs.push(Tab::spawn(
            title_of(&cmd_args),
            &cmd_args,
            None,
            rows,
            cols,
            // Somebody standing in a folder asked for this command, the way
            // any terminal is asked. That folder is written down rather than
            // left to be guessed at the launch, so this tab says where it
            // works the same way every other tab does
            tab::TabOptions { cwd: std::env::current_dir().ok(), ..Default::default() },
        )?);
    } else if let Some(w) = desks.get(desk_index) {
        // If we're resuming where we left off, launch that same desk too.
        // Hard-coding this to the first desk would restore only the name while
        // showing a screen with different contents.
        // The model connections, before any tab starts. The full hand-over
        // comes further down, once there is a notifier to hand
        if let Some(c) = cfg.as_ref() {
            let tokens = c.resolve_tokens(password.as_deref());
            bridge::use_connections(config::app_providers(c, &|k| tokens.get(k).cloned()));
        }
        spawn_desk(w, rows, cols, &mut tabs, &mut startup_errors, Some(&last_session));
    }
    // No config yet = first run. Guide the user so the experience isn't just
    // "a single shell opens and nothing else happens", leaving them unsure what to do.
    let first_run = cmd_args.is_empty() && cfg.is_none();
    if tabs.is_empty() && desks.is_empty() {
        let argv = vec!["powershell.exe".to_string()];
        tabs.push(Tab::spawn(
            "SHELL".into(),
            &argv,
            None,
            rows,
            cols,
            // The same as a command asked for by hand: there are no settings
            // yet to name a folder, so the one the app was started from is the
            // answer, and it is written down rather than guessed
            tab::TabOptions { cwd: std::env::current_dir().ok(), ..Default::default() },
        )?);
    }

    // Re-fit the PTY size now that every tab exists
    (rows, cols) = pty_dims(shell.size()?);
    for t in &tabs {
        let _ = t.resize(rows, cols);
    }

    // The Lua hook engine is per-desk (shared variables are scoped inside it too).
    // Unused desks don't get one built; it's created on demand when switched to.
    let mut max_chain = cfg.as_ref().and_then(|c| c.max_chain).unwrap_or(10);
    let mut done_confirm_ms = cfg
        .as_ref()
        .and_then(|c| c.done_confirm_ms)
        .unwrap_or(profile::DEFAULT_DONE_CONFIRM_MS);
    // Notification destinations. Empty until the desk on screen is handed over
    // below: each desk registers its own, and there are none of the app's
    if let Some(e) = cfg.as_ref().and_then(|c| c.secrets_problem(password.as_deref())) {
        startup_errors.push(e);
    }
    let notifier = notify::Notifier::new(Default::default(), None);
    // Names inside the secrets file changed shape; a file written by an
    // earlier version is brought forward here rather than in the ordinary
    // migration steps, because those run before anyone has said the master
    // password and this one may have to open an encrypted store
    if let Some(c) = cfg.as_ref()
        && let Some(path) = c.secrets_path() {
            match config::migrate_secrets(&path, password.as_deref(), &desks) {
                Ok(true) => append_hook_log("secrets: names brought forward to the new shape"),
                Ok(false) => {}
                Err(e) => startup_errors.push(format!("secrets: {e:#}")),
            }
        }
    // Capabilities granted to automation (empty by default). An advanced feature that
    // can only be enabled by writing it into the config file.
    let caps: hooks::Caps = std::rc::Rc::new(match cfg.as_ref() {
        // The doors and the permission table start closed and standard; the
        // desk on screen hands its own over below, before anything runs
        Some(c) => caps::Capabilities::new(
            Default::default(),
            config_file_dir(),
            c.resolve_tokens(password.as_deref()),
            c.resolve_secret_terms(password.as_deref()),
            Default::default(),
        ),
        None => caps::Capabilities::disabled(),
    });
    let mut engines: Vec<Option<HookEngine>> = (0..desks.len().max(1)).map(|_| None).collect();
    // If we have a window, put browsers inside it
    caps.set_host(shell.host());
    // ...and if pages can be drawn either here or on a connected device, the
    // person's setting says which
    shell.draw_pages(placed::Draw::of(
        cfg.as_ref().and_then(|c| c.browser_draw.as_deref()).unwrap_or_default(),
    ));
    caps.set_desk(desk_index);
    // Somewhere to ask about pull requests, on its own thread. Quiet and
    // harmless when there is no GitHub token: it simply never knows anything,
    // and no row grows a line
    let prs = crate::pr::Watch::start();
    // The app's deciding AI, for a script that asks for a decision without
    // naming a model. Said again whenever the settings are read
    if let Some(c) = cfg.as_ref() {
        caps.set_words_models(config::app_words(c));
    }
    if let Some(w) = desks.get(desk_index) {
        // Everything this desk answers for, handed over in one act -- the
        // same one a switch uses, so the first desk is not a special case.
        // Before the engine below is built, because the Lua it compiles belongs
        // to this desk and must meet this desk's doors
        crate::desk::hand_over(w, &caps, &notifier, &prs);
        engines[desk_index] = build_engine(cfg.as_ref(), Some(w), &mut startup_errors, &caps);
        // Declared browsers are NOT opened here: placing a page occupies the
        // window thread, and at startup the person is often already clicking.
        // The board goes up first; the loop opens them right after (below).
    } else {
        engines[0] = build_engine(cfg.as_ref(), None, &mut startup_errors, &caps);
    }
    let mut open_browsers_after_first_paint = true;
    let mut first_paint_done = false;
    let slot = desk_index.min(engines.len().saturating_sub(1));
    let mut engine = engines[slot].take();
    // The current ad-hoc "operate a target" attachment, as (source pane, target),
    // so a repeated goal to the same target doesn't re-brief from scratch.
    let mut operating: Option<(usize, usize)> = None;
    // The page currently being driven from words (🗣), as (its pane, its key).
    // One at a time: a second one would be a second thing typing into pages
    // while the person watches only one of them
    let mut driving: Option<(usize, String)> = None;
    // A words run taken out of an engine that was built again, waiting to be
    // taken up by the new one (HookEngine::words_carry)
    let mut words_carried: Option<serde_json::Value> = None;
    // Whether somebody is being asked why the last run ended badly, and the
    // way their answer gets back to the loop
    let mut asking_why = false;
    let (why_tx, why_rx) = std::sync::mpsc::channel::<String>();
    // ✨ finished command suggestions arrive from worker threads (the
    // assistant AI call takes seconds); polled once per tick below
    let (suggest_tx, suggest_rx) = std::sync::mpsc::channel::<String>();
    // The git panel's slow half: fetch, pull and push answer from a thread
    let (git_tx, git_rx) = std::sync::mpsc::channel::<String>();
    // The git panel's questions, one line per folder (see `GitLine`)
    let mut git_lines = GitLines::new();
    // Answers to the Issue tab, from the threads that waited for GitHub
    let (issues_tx, issues_rx) = std::sync::mpsc::channel::<String>();
    // A search of past conversations, answered from another machine
    let (vault_far_tx, vault_far_rx) = std::sync::mpsc::channel::<(u64, Vec<crate::vault::Hit>)>();
    let mut vault_seq: u64 = 0;
    // What was said before in a tab's folder on another machine, read there
    let (past_tx, past_rx) = std::sync::mpsc::channel::<(usize, Vec<crate::vault::Hit>)>();
    // A MicroVM folder asked to be deleted, checked on its machine for work
    // that would go with it; and the ones that passed, to go on
    let (far_discard_tx, far_discard_rx) = std::sync::mpsc::channel::<(String, Result<(), String>)>();
    let mut far_discard_checked: std::collections::HashSet<String> = Default::default();
    // A pull request's draft prompt for a folder on another machine, whose
    // commits and change are read there on a thread: (prompt, shape)
    let (pr_draft_tx, pr_draft_rx) = std::sync::mpsc::channel::<(String, String)>();
    // A pull request's base brought into its folder: the merge on a thread, and
    // a conflict handed to an AI tab back here, where the tabs are
    let (pr_tx, pr_rx) = std::sync::mpsc::channel::<PrCatchUp>();
    // A failed CI run's checks and logs, read from GitHub on a thread, and the
    // tab that fixes them opened back here
    let (ci_tx, ci_rx) = std::sync::mpsc::channel::<CiFix>();
    // Everything the file panel asks of a server, which is all of it: a folder
    // on the far end is a network round trip and the window cannot wait for one
    let (sftp_tx, sftp_rx) = std::sync::mpsc::channel::<String>();
    // The same, for the column's file list and the editor when their folder is
    // on another machine
    let (far_tx, far_rx) = std::sync::mpsc::channel::<FarFiles>();
    // What each editor showing a file on another machine last heard about it,
    // and when each last asked. The disk here is asked every pass; a machine
    // over there is a round trip, so it is asked on a clock of its own
    let mut far_seen: std::collections::HashMap<String, FarSeen> = std::collections::HashMap::new();
    let mut far_polls: std::collections::HashMap<String, FarPoll> = std::collections::HashMap::new();
    // When each MicroVM was last given its minutes again (see `keep_machines_up`)
    let mut kept_up: std::collections::HashMap<String, Instant> = std::collections::HashMap::new();
    // 🔍 environment cards: per tab (by id), the captured output of the last
    // survey the person ran. Ride along with every ✨ suggestion so the AI
    // keeps knowing the environment long after the survey scrolled away
    let mut env_cards: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // A survey in flight: (tab id, give-up time). The tick below watches the
    // tab's screen for the probe's end marker — event-paced, no sleeps
    let mut pending_survey: Option<(String, std::time::Instant)> = None;

    // Remote UI (monitor/control from a phone, etc). Only starts listening when
    // enabled in config. Status is also handed to the settings page so the QR code
    // can be viewed in a browser.
    let remote_info: Arc<Mutex<webui::RemoteInfo>> = Arc::new(Mutex::new(Default::default()));
    // The network bind can stall — a lingering earlier instance can hold the
    // port for up to a second — and nothing else at startup needs it. Bind on
    // a background thread; the loop installs the server when it lands, and
    // every click in between gets answered instead of waiting on a socket.
    let mut remote_ui: Option<remote::RemoteUi> = None;
    let mut remote_rx = start_remote_bg(cfg.as_ref(), password.as_deref());
    publish_remote(&remote_info, &remote_ui);


    // Where focus is currently directed. None = never moved it yet.
    let mut focused: Option<Option<String>> = None;

    // The location of the page being viewed (name inside the window, URL, can-go-back,
    // can-go-forward). Only the window knows this, so we ask and cache it.
    let mut where_now: Option<(String, String, bool, bool)> = None;
    // Per-id loading state (currently loading, time it most recently started).
    // The indicator stays lit for a minimum duration from the start so even
    // instantaneous network activity remains visible.
    let mut loading_now: std::collections::HashMap<String, (bool, std::time::Instant)> =
        std::collections::HashMap::new();
    let mut asked_where_ms: u64 = 0;

    let mut auto_enabled = true;
    // How often a still-working tab is mentioned to automation again. None
    // unless somebody asked for it
    let mut busy_repeat_ms: Option<u64> = cfg
        .as_ref()
        .and_then(|c| c.busy_repeat_sec)
        .filter(|s| *s > 0)
        .map(|s| s * 1000);
    let mut started_fired = vec![false; tabs.len()];
    // When each still-working tab is due to be mentioned to automation again,
    // for the tabs automation was told about in the first place. Empty unless
    // the interval is set, and emptied for a tab the moment it stops working
    let mut busy_again: std::collections::HashMap<usize, u64> =
        std::collections::HashMap::new();
    // The "invisible ball" of the automation chain. Used in the display to show
    // which tab currently holds the work.
    let mut ball = ball::Ball::default();
    // Holding area for hand-offs the recipient can't accept yet
    let mut waiting: Vec<Waiting> = Vec::new();
    // A reservation to send submit (Enter) later, for text that's already been sent
    let mut pending_send: Vec<PendingSend> = Vec::new();
    // Tabs that look like they've finished responding, and the time that gets confirmed.
    // We hold off firing until we've verified it stayed quiet, so we don't fire on a
    // mid-response pause for breath.
    let mut pending_done: Vec<(usize, u64)> = Vec::new();
    // The name drawn for a worktree nobody named, by the folder the dialog was
    // opened on. Kept for as long as that dialog keeps asking, so the name on
    // screen is the name that gets made
    // What making a folder had to say, kept for a moment: making one writes the
    // settings, and the reload that follows would otherwise put "settings
    // reloaded" over what could not come along
    let mut said_before_reload: Option<(std::time::Instant, String)> = None;
    // Whether the Issue tab has been opened. Once it has, it stays among the
    // rows until the app is closed: like INDEX it is not closed, and it asks
    // GitHub nothing while it is not in view
    let mut issues_open = false;
    let mut issues_front = false;
    // What the Issue tab's answers were read against: the desk, and how many
    // times the settings have changed. When either moves, what it shows -- a
    // project's account, "no account chosen" -- may no longer be so, and it is
    // told to ask again rather than left saying it until somebody presses reload
    let mut settings_gen: u64 = 0;
    let mut issues_basis: (usize, u64) = (0, 0);
    // Words waiting for the input bar of a worktree just made for an issue or
    // a pull request: (the folder, the words, since when)
    let mut pending_drafts: Vec<(std::path::PathBuf, String, Instant)> = Vec::new();
    // What people asked the AIs in each folder, for the folders that name and
    // describe themselves from it (`crate::labels`)
    let mut heard = crate::labels::Board::default();
    // How far each AI's own record of its conversation has been read
    // (`crate::asks`). Kept by file rather than by tab: the file is what is
    // being read, and a tab that restarts on the same conversation carries on
    // rather than starting again
    let mut read_asks: std::collections::HashMap<std::path::PathBuf, u64> =
        std::collections::HashMap::new();
    let mut label_jobs: Vec<LabelJob> = Vec::new();
    let mut label_seq: u64 = 0;
    let mut label_look = Instant::now();
    let mut drawn_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // Whether automation may switch which tab is on screen (see ViewMove)
    let mut auto_switch = cfg.as_ref().and_then(|c| c.auto_switch).unwrap_or(true);
    // Whether the ✕ puts the window away rather than quitting (see the loop)
    let mut resident = cfg.as_ref().and_then(|c| c.resident).unwrap_or(true);
    // What each AI's subscription has left, each on a thread of its own.
    // Nothing is read until a tab of that AI exists (limits::Meter::want)
    let limits: Vec<(crate::limits::Source, crate::limits::Meter)> = crate::limits::Source::ALL
        .into_iter()
        .map(|s| (s, crate::limits::Meter::start(s)))
        .collect();
    let mut ai_usage_on = cfg.as_ref().and_then(|c| c.ai_usage).unwrap_or(true);
    // The last time a human touched the screen. Don't auto-follow right after that.
    let mut view_touched_ms: u64 = 0;
    // Clickable spots on INDEX. Rebuilt every frame at draw time.

    // 0 = INDEX, 1.. = sessions. Start on INDEX (the screen with onboarding guidance) at first.
    let mut active: usize = if tabs.is_empty() || first_run { 0 } else { 1 };
    // Whether INDEX is covering the window.
    //
    // A screen, not a pane. The board is a view OF the running things, not
    // one of them: it has no process, no state, no folder, nothing a pane is
    // for. It used to be surface 0 and could be put in a pane -- usually not
    // on purpose, because a division with no free tab to fill it reached for
    // the board as a fallback -- and there, unfocused, it drew nothing at all,
    // since a pane's read-only copy is a terminal's text and the board is not
    // a terminal. It covers the window now and the panes wait underneath
    let mut board_open = tabs.is_empty() || first_run;
    // How the content area is divided. It starts undivided, which is the shape
    // every code path that knows only `active` was written for: the focused
    // pane's surface *is* `active`, and the two are re-synced once per frame
    // below, so splitting the screen adds panes without rewriting the loop.
    let mut pane_layout = crate::layout::Layout::single(active);
    // The other half of what was remembered (the conversations were used at
    // launch, above). The division of the screen is put back unconditionally:
    // it is a shape, not a conversation, and nobody is surprised to find their
    // panes where they left them. `previous` is filled in whether or not the
    // conversations were carried, because with carrying turned off it is what
    // Ctrl+B r reaches for — the way back stays available, it is just not taken
    // for you
    if let Some(desk) = desks.get(desk_index) {
        for t in tabs.iter_mut() {
            t.previous = last_session.conversation_for(desk, t);
        }
        if let Some(saved) = last_session.panes_for(desk) {
            // Whether those panes still point at surfaces that exist is not
            // decided here: the loop clamps the tree to what is on screen every
            // frame, which is the one place that knows
            pane_layout = saved;
            active = pane_layout.focused_surface();
        }
    }
    // When to next write down what is on screen. Rare events (a conversation
    // learned, a desk switched) are worth writing at once; a divider being
    // dragged is not, and a delay keeps a drag from writing a file per frame
    let mut save_at: Option<std::time::Instant> = None;
    // The arrangement each split row owns, parked while that row is not in
    // front, and the name of the one in front now.
    //
    // This is where the division of the screen lives. It used to belong to the
    // desk -- one of them, shared by every folder on it, owned by nobody --
    // and a split made while working in one folder was waiting in the next
    // one, showing tabs from a folder nobody had opened
    let mut splits = crate::splits::Splits::new();
    let mut open_split: Option<String> = None;
    // A row was pressed in the list or the strip, so whatever arrangement is
    // in front is being left for it (see `take_selects`)
    let mut left_split = false;
    // A folder was pressed that had nothing running in it yet, so something
    // was started there and the press is waiting for it to arrive
    let mut going_to: Option<(std::path::PathBuf, Instant)> = None;
    // What automation asked of the panes, waiting for the loop's next turn to
    // be carried out where dividing and closing are written once
    let mut lua_splits: Vec<(crate::layout::PaneId, bool)> = Vec::new();
    let mut lua_shuts: Vec<crate::layout::PaneId> = Vec::new();
    // What was last written down for the row in front, and a division waiting
    // to be written. Dragging a divider changes the arrangement on every frame
    // of the drag, and each one of those is not a setting somebody made
    let mut split_written: Option<crate::layout::Kept> = None;
    let mut split_save: Option<(String, Instant)> = None;
    // The zoom level waiting to be written down, and when to write it
    let mut font_size: Option<u8> = None;
    // The editors as they stand: which folder each works in, and which file it
    // is showing. Held here rather than in the settings, because the file
    // somebody opened this afternoon is not a setting
    let mut editors: Vec<crate::view::EditorOpen> = Vec::new();
    // The one just asked for, to be brought into view at the top of the pass
    // (the surfaces were worked out before the press arrived)
    let mut open_editor: Option<String> = None;
    let mut tab_width: Option<u16> = None;
    let mut side_width: Option<u16> = None;
    let mut font_save_at: Option<std::time::Instant> = None;
    let mut tab_save_at: Option<std::time::Instant> = None;
    let mut side_save_at: Option<std::time::Instant> = None;
    // Whether the composer is shut, as the window's own page last said. The
    // pen a placed page draws for itself follows it
    let mut composer_shut = false;
    // The placed page currently showing that pen, if any
    let mut pen_shown: Option<String> = None;
    // A pane waiting for the tab it asked for, and the rows that were on the
    // list when it asked (by `surface_key`, so a row that only moved is still
    // the same row). Cleared when the tab arrives or the form is shut
    let mut awaiting_tab: Option<(u32, Vec<String>)> = None;
    // Which key does what, this run. Read once and re-read when the settings
    // change, the same as everything else that can be edited while running
    // When to look again at where the tabs are. Starts now so the first frame
    // already knows, rather than showing a sidebar that fills in a beat later
    let mut place_at = std::time::Instant::now();
    let mut sign_ins_at = std::time::Instant::now() + std::time::Duration::from_secs(600);
    // Where each git tab's folder pushes to, on the same beat: read off the
    // disk, so not every frame
    let mut git_repos: Vec<(std::path::PathBuf, String)> = Vec::new();
    // What each tab costs the machine, measured on the same 2-second beat as
    // where it is. The meter keeps last time's totals so processor use comes
    // out as a rate rather than a running sum
    let mut meter = crate::usage::Meter::default();
    // The AIs that can be started here. Read once: it asks the disk which
    // commands exist, and the answer does not change while the app runs
    // except by somebody installing one, which a settings save also notices
    let mut ai_choices = startable_ais();
    // What has been pointed at on this machine, and whether thanks were asked
    let mut coach_seen: u8 = std::fs::read_to_string(config::state_path("coach"))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let mut thanks_asked = config::state_path("thanks-asked").exists();
    // Whether this machine has been told what a tab that was opened as a shell
    // does not do with the conversation an AI started in it
    let mut guest_told = config::state_path("guest-told").exists();
    // The first-start setup: up on a first start until it is answered, and
    // never on a machine that already had settings. Which AIs are installed is
    // asked once, here, like the list above
    let mut setup_view = setup_wanted(first_run, config::state_path(SETUP_ANSWERED).exists())
        .then(crate::webui::setup_state);
    // When the setup last wrote the settings itself, so the reload that follows
    // does not announce it
    let mut setup_reload: Option<std::time::Instant> = None;
    // A project being cloned, with the dialog's number for the attempt, and
    // what the dialog is told about it. Where such a project goes by default
    // is asked once
    // A clone onto this PC; one onto a server is a row on the board (VmWork::SshClone)
    let mut add_job: Option<(u64, crate::addproject::Job)> = None;
    // The work on MicroVMs under way -- a project being cloned onto one, a
    // checkout's machine being prepared -- each a row on the board under its
    // project, like a worktree being made
    let mut vm_jobs: Vec<VmJob> = Vec::new();
    let mut add_view: Option<crate::uistate::AddProjectState> = None;
    // A folder on another machine, walked from the same dialog. Listed on a
    // thread: a machine that does not answer takes as long as its timeout
    let mut remote_view: Option<crate::uistate::RemoteListState> = None;
    // The public addresses of a folder on a MicroVM, and the answers on their
    // way from the thread that asks the machine
    let mut far_ports_view: Option<crate::uistate::FarPortsState> = None;
    // The AIs a MicroVM can be given, as the profiles say: read once
    let machine_ais: Vec<crate::uistate::MachineAiChoice> = crate::profile::machine_ais()
        .into_iter()
        .map(|a| crate::uistate::MachineAiChoice { key: a.key, name: a.name })
        .collect();
    let (far_ports_tx, far_ports_rx) = std::sync::mpsc::channel::<crate::uistate::FarPortsState>();
    let (listing_tx, listing_rx) = std::sync::mpsc::channel::<crate::uistate::RemoteListState>();
    // The aliases a new machine can be filled in from, read again with the settings
    let mut ssh_aliases = crate::discover::ssh_aliases();
    let project_home = crate::addproject::projects_root().display().to_string();
    // The Assistant AI setting. Read from the file itself, once: on a first
    // start the setup writes it before there are settings that count as
    // loaded. Kept up to date where it changes -- the setup, a settings reload
    let mut assistant_ai = config::assistant_written();
    // The projects whose found worktrees somebody chose to keep hidden
    let mut worktrees_kept = load_kept();
    // The working folders put out of sight until the next launch. Held here
    // and nowhere else: nothing is written down, so starting the program again
    // shows every one of them, which is what "hide while it runs" means
    let mut folders_hidden: std::collections::BTreeSet<std::path::PathBuf> =
        std::collections::BTreeSet::new();
    // Whether the view is looking at something else than it was because it was
    // moved, rather than because somebody asked it to move. Set by the places
    // that shuffle the rows on their own -- rows following the settings, a desk
    // replaced under the view -- and answered once a pass, just before drawing
    // (`view::settle`). It is the whole difference between "show me that tab"
    // and "the tab you were on is gone", and only the first may bring a folder
    // back from out of sight
    let mut view_drifted = false;
    // The row the view was left on the last time that question was answered.
    // Anything else it is on when the rows next move is somebody having asked
    // for it in between
    let mut view_settled_at = 0usize;
    // A folder being put back on this machine while the window keeps drawing
    let mut putting: Option<folders::Putting> = None;
    // Worktrees being made, each a row under its project's heading
    let mut makings: Vec<Pending> = Vec::new();
    let mut leavings: Vec<Leaving> = Vec::new();
    let mut making_seq: u64 = 0;
    let mut thanks_show = false;
    // Where thanks would go: the Store's review page for the Store's copy, the
    // repository for the zip's
    let thanks_kind = if config::packaged() { "store" } else { "github" };
    // What the Vault overlay is showing right now: the last search and its
    // hits. Kept across frames so the results stay put until the next search,
    // and dropped from the state entirely while the overlay is closed
    let mut vault_view: Option<crate::uistate::VaultState> = None;
    // What was said before in one tab's folder, for the tab that came up on a
    // conversation of nobody's. Held the same way, and for the same reason:
    // the list stays put while the person reads it
    let mut past_view: Option<crate::uistate::PastState> = None;
    // What making a branch would do. Answered while the name is being typed,
    // and cleared once the folder exists so the dialog can close itself
    let mut branch_view: Option<crate::uistate::BranchPlan> = None;
    // What a MicroVM would sign in as, still being found out for the dialog
    // on screen: the account's name and how it is handed over. Asked again
    // each turn until it is known, since it is found on a thread
    let mut signin_waiting: Option<(String, Result<config::FarSignIn, String>)> = None;
    // Whose AI sign-in the open worktree dialog is about: the checkout's
    // machine, looked at again while the dialog is open, so a sign-in done
    // in the checkout's tab meanwhile is seen without closing it
    let mut ai_signin_watch: Option<(config::HostSpec, Option<config::ProjectHome>, Option<String>)> = None;
    // The folder on another machine whose branches the open worktree dialog is
    // waiting for, asked of git there on a thread
    let mut bases_watch: Option<(config::HostSpec, String)> = None;
    // The sign-in step of a project just cloned onto a MicroVM: the checkout
    // whose AI is to be signed in to, before its first worktree is cut
    let mut login_pending: Option<LoginPending> = None;
    let mut git_signin: Option<GitSignIn> = None;
    // The SSH clone page's question, which GitHub accounts GitHub CLI on the
    // chosen server holds: (the page's number, the answer), filled in by a
    // thread since asking a server takes a moment
    let ssh_accounts_answer: std::sync::Arc<std::sync::Mutex<Option<(u64, Result<Vec<String>, String>)>>> = Default::default();
    let mut login_view: Option<crate::uistate::LoginStepState> = None;
    let mut login_seq: u64 = 0;
    // What it would take to have a missing working folder here. Answered when
    // one is opened, and cleared once the folder exists so the dialog closes
    let mut repair_view: Option<crate::uistate::RepairPlan> = None;
    // The folders being looked through, while somewhere new is being chosen
    let mut browse_view: Option<crate::uistate::BrowseState> = None;
    // What this whole app is costing the machine, refreshed on the same beat as
    // the per-tab figures. Shown in the board's header
    let mut self_cost: Option<String> = None;
    let (mut keymap, key_errs) = crate::keys::Keys::load(cfg.as_ref());
    startup_errors.extend(key_errs);
    let mut prefix_active = false;
    // The last state drawn. This is what gets handed to the phone (keeps the
    // assembly point to a single spot).
    // What we last pushed to remote viewers over the state socket, so we only
    // send on change. The screen is also rate-limited (see below) so a burst of
    // AI output doesn't flood a slow phone link the way pushing every frame would.
    let mut last_remote_ui: Option<String> = None;
    let mut last_remote_rows: Vec<String> = Vec::new();
    let mut last_remote_push = Instant::now() - Duration::from_secs(1);
    // The panes, for the viewers from afar that lay the board out in them
    let mut pane_relay = PaneRelay::default();
    /// How often a viewer that has said nothing is written to anyway.
    const BEAT: Duration = Duration::from_secs(3);
    // When the last heartbeat went out. A phone is only ever found to be gone
    // by a write to it failing, so on a quiet screen -- nothing running, no
    // output -- a phone that was closed or fell asleep would be counted as
    // watching for as long as the quiet lasted, and the terminals would stay
    // cut to a screen nobody was holding. A few bytes every few seconds means
    // it is noticed within one beat of leaving; it also keeps the socket from
    // being timed out as idle by whatever sits between the two.
    let mut last_beat = Instant::now();
    // Whether an overlaid browser is currently being shown. Leaving it up would
    // permanently hide the terminal, so it's hidden by default.
    // The board flashes only the first of these, and a flash fades. Every one
    // of them goes to the log as well, so that a script that never ran can be
    // told apart, afterwards, from a script that ran and did nothing
    // Once each: two readers of one locked store both say it is locked, and the
    // same sentence twice reads like two problems
    let mut said_already = std::collections::HashSet::new();
    for e in startup_errors.iter().filter(|e| said_already.insert(e.as_str())) {
        append_hook_log(&format!("Startup: {e}"));
    }
    let mut flash: Option<String> = startup_errors
        .first()
        .map(|e| i18n::tp("msg.startup_failed", &[("error", e)]))
        .or_else(|| plaintext_secrets_warning(cfg.as_ref()));
    // A message is a toast, and a toast goes away by itself. The screen fades
    // it after a few seconds (src/toast.rs), and it stops being part of the
    // state shortly after that — otherwise it would sit in `flash` until the
    // next keystroke and be handed, still looking fresh, to a phone that
    // connected an hour later. Timed here by watching the value change rather
    // than at the sixty-odd places that set one.
    let mut flash_shown: Option<String> = None;
    let mut flash_at = Instant::now();
    let mut last_detect = Instant::now() - Duration::from_secs(1);
    // The browser currently being screen-relayed (only streams while someone's watching)
    let mut casting: Option<String> = None;
    // Desks use a virtual-desktop model: switching means hiding, not stopping.
    // Each desk keeps its own set of tabs, launched the first time it's activated.
    // Launched tabs live in `tabs`; the shelf reserves space for the remaining desks.
    let mut desk_tabs: Vec<Vec<Tab>> = Vec::new();
    desk_tabs.resize_with(desks.len(), Vec::new);
    // Watch the config file for changes (saving takes effect without a restart)
    let mut watcher = watch::Watcher::new(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
    // Look for a newer version: now, and once a day while this runs. Looking
    // is all it does by itself; installing is a button on the settings screen
    update::start(cfg.as_ref().and_then(|c| c.update_check).unwrap_or(true));
    let mut cfg = cfg;
    // The quick commands as the launcher draws them. Laid out once per read of
    // the settings rather than per frame: the drawing of every button is looked
    // up while doing it, and nothing about them changes in between
    let quick_of = |c: Option<&config::Config>| {
        std::sync::Arc::new(crate::quick::view(&c.map(|c| c.quick_commands.clone()).unwrap_or_default()))
    };
    let mut quick_view = quick_of(cfg.as_ref());
    // Lines waiting for a tab a quick command has just opened (see `PendingQuick`)
    let mut pending_quicks: Vec<PendingQuick> = Vec::new();
    // Where a command with no folder in front opens. Looked up once: it is
    // asked on every frame the launcher could be showing
    let home = home_folder();
    // Whether the page has something up over everything (the quick commands,
    // the ideas). Pages placed in the window are windows of their own and
    // nothing drawn can cover them, so they step aside while it is, the way
    // they do for the help and the desk list
    let mut page_covered = false;

    let mut desk_open = false;
    let mut help_open = false;
    let mut qr_open = false;
    // While the settings overlay is up, automation (ball-follow, ShowTab) must
    // not yank the screen to another tab — settings is a place of its own, not a
    // tab you get pushed out of. Only an explicit human tab/desk pick, or
    // "close settings", leaves it.
    let mut settings_open = false;
    // Where the settings page stands while it is open: the whole window, a
    // sheet over the board (a link that named one thing: a card, a folder, a
    // tab), or the dialog the board's + opens. Only meaningful while
    // `settings_open`
    let mut settings_place = SettingsPlace::Full;
    // The guide's panel: whether it is up, and where it was put
    let mut guide_open = false;
    let mut guide_at = PanelAt::default();
    // Flag for dragging the tab-bar border (lets the mouse adjust its width)
    // The settings web GUI (launched via INDEX's [e], stopped when the app exits)
    let mut web: Option<webui::WebUi> = None;
    // Share the master password held by the main app with the settings GUI
    // (used to encrypt secrets). Never sent to the page; only read server-side,
    // within the same process. Kept in sync on change.
    let web_password: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(password.clone()));
    let config_file = config::config_file_path();
    // Whether the remote server's settings reverse-proxy has been pointed at the
    // local config server yet. Done once per remote instance (reset when remote
    // is (re)started), so a phone's `/cfg` can reach the config UI.
    let mut settings_linked = false;
    // Tabs closed from the tab bar, kept so they can be opened again
    let mut closed_tabs = crate::closed::Closed::load();
    // A tab's ✕ waiting for its answer, and how many have been asked, so the
    // page opens each question once
    let mut close_ask: Option<crate::uistate::CloseAskState> = None;
    let mut close_asked: u64 = 0;
    // Tabs to end at the top of the next pass, by serial. Not there and then:
    // the rows for that pass are already worked out by tab position, and
    // taking a tab out from under them would point the rest of the pass at its
    // neighbours
    let mut ending: Vec<u64> = Vec::new();
    // Conversations to hand tabs as they are started again, by automation name
    let mut resume_for: std::collections::HashMap<String, tab::Session> =
        std::collections::HashMap::new();
    // A reopened tab to bring into view once it is on screen, and until when
    // to keep looking for it
    let mut reveal: Option<(String, Instant)> = None;
    // What each row was on the last pass, and on which desk, so a pane can
    // follow its tab when the rows move
    let mut rows_were: (String, Vec<String>) = (String::new(), Vec::new());
    // The rows that have been in front on this desk, by key, the most recent
    // last (the board is the empty key). A row goes to the end each time it
    // comes to the front, and off the list when it goes from the rows. Where
    // the view goes back to when the row in front closes on an undivided
    // screen: the thing the person was looking at before this took the
    // screen, and not the neighbour by number (`view::back_row`)
    let mut fronts: Vec<String> = Vec::new();

    loop {
        // Install the remote server the moment its background bind lands.
        // Errors and notes surface exactly as the old synchronous path did.
        if let Some(rx) = &remote_rx
            && let Ok((ui, mut errs)) = rx.try_recv() {
                remote_ui = ui;
                remote_rx = None;
                // Pages drawn on a connected device are driven through this
                if let (Some(r), Some(line)) = (remote_ui.as_ref(), shell.far_pages()) {
                    r.set_page_line(line);
                }
                if let Some(r) = remote_ui.as_ref() {
                    shell.board_is_at(&r.url);
                }
                publish_remote(&remote_info, &remote_ui);
                last_remote_ui = None;
                if flash.is_none() {
                    flash = errs
                        .first()
                        .map(|e| i18n::tp("msg.startup_failed", &[("error", e)]));
                }
                startup_errors.append(&mut errs);
            }

        // Open the desk's declared browsers on the iteration AFTER the
        // first full draw: the board answers clicks first, then the window
        // thread pays the (brief) cost of placing pages.
        if open_browsers_after_first_paint && first_paint_done {
            open_browsers_after_first_paint = false;
            if let Some(w) = desks.get(desk_index) {
                open_declared_browsers(w, &caps, &mut startup_errors);
            }
        }
        first_paint_done = true;

        // Point the remote settings proxy at the (loopback) config server. Starting
        // it here, lazily but eagerly-once, means the phone can open settings even
        // before anyone has opened it on the PC. The config UI stays on loopback.
        if !settings_linked
            && let Some(r) = remote_ui.as_ref()
                && let Ok(u) = ensure_web_url(&mut web, &config_file, &remote_info, &web_password, &caps)
                    && let (Some(origin), Some(tok)) =
                        (u.split("/?").next(), u.split("token=").nth(1))
                    {
                        r.set_settings_backend(origin.to_string(), tok.to_string());
                        settings_linked = true;
                    }

        // Tabs closed on the last pass end here, before the rows are worked out
        if !ending.is_empty() {
            let mut at = 0;
            tabs.retain_mut(|t| {
                let goes = ending.contains(&t.serial());
                if goes {
                    t.kill();
                    if at < started_fired.len() {
                        started_fired.remove(at);
                    }
                } else {
                    at += 1;
                }
                !goes
            });
            ending.clear();
        }

        // What's laid out on screen, in the order written in config.
        // The upper bound of pressable numbers needs more than just the session count.
        let hosted = caps.hosted_names();
        let titles: Vec<&str> = tabs.iter().map(|t| t.title.as_str()).collect();
        let surfaces = surfaces_of(desks.get(desk_index), &titles, &hosted, &editors, issues_open);
        let surface_count = surfaces.len();
        // The rows moved since the last pass: keep every pane on what it was
        // showing. Only on one desk -- switching is another set of rows
        // altogether, with an arrangement of its own
        {
            let desk_now = desks.get(desk_index).map(|d| d.name.clone()).unwrap_or_default();
            let keys: Vec<String> = surfaces.iter().map(|s| surface_key(s, &tabs)).collect();
            if rows_were.0 == desk_now && !rows_were.1.is_empty() && rows_were.1 != keys {
                let moves = surface_moves(&rows_were.1, &keys);
                let was_focused = pane_layout.focused_surface();
                // What the view was looking at, by name. Read before the panes
                // follow, while the old numbers still mean something
                let was_on = active.checked_sub(1).and_then(|i| rows_were.1.get(i)).cloned();
                // Somebody asked for another row since the last time the view
                // was settled -- a key, a row pressed, a tab opened and brought
                // to the front. Then this is not the view being carried about
                // by rows it did not choose, and the row they asked for is
                // followed like any other
                let asked = active != view_settled_at;
                // The row in front went, and it had the screen to itself:
                // there is no other pane to fall back on, so the view goes
                // back to what was in front before it, if that is still here.
                // Read before the panes follow, which is what moves the view
                // on to a neighbour
                let went_alone = pane_layout.is_single()
                    && !asked
                    && active.checked_sub(1).and_then(|i| moves.get(i)).is_some_and(Option::is_none);
                let apart: Vec<usize> = surfaces
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| crate::view::drawn_apart(s))
                    .map(|(i, _)| i + 1)
                    .collect();
                pane_layout.follow(&moves, &apart);
                // `active` is the focused pane's row, and follows it. When
                // something asked for another row on the last pass it is that
                // row, which has moved like any other
                active = match active {
                    0 => 0,
                    n if n == was_focused => pane_layout.focused_surface(),
                    n => moves.get(n - 1).copied().flatten().unwrap_or(pane_layout.focused_surface()),
                };
                if went_alone
                    && let Some(back) = crate::view::back_row(&fronts, &keys)
                {
                    active = back;
                    board_open = back == 0;
                    pane_layout.show(active);
                    // Closing the row in front is an ask, and this is where
                    // it leads: the view is not adrift, it is home
                    view_drifted = false;
                    view_touched_ms = start.elapsed().as_millis() as u64;
                } else if !asked && was_on != active.checked_sub(1).and_then(|i| keys.get(i)).cloned() {
                    // Looking at something else than it was, and nobody asked
                    // for it: the row it was on went, or the numbers moved out
                    // from under it. Said here and settled once, before
                    // drawing, so a folder somebody put out of sight is never
                    // brought back by the view merely landing on one of its
                    // rows
                    view_drifted = true;
                }
            }
            // The rows that have been in front, brought up to date: another
            // desk is another list, whose rows say nothing about this one; a
            // row that went is not a place to go back to; and the row in
            // front now is the most recent, wherever it stood before
            if rows_were.0 != desk_now {
                fronts.clear();
            }
            fronts.retain(|k| k.is_empty() || keys.contains(k));
            // The board in front is the board, whatever row it is drawn over
            let now = match board_open {
                true => String::new(),
                false => active.checked_sub(1).and_then(|i| keys.get(i)).cloned().unwrap_or_default(),
            };
            if fronts.last() != Some(&now) {
                fronts.retain(|k| *k != now);
                fronts.push(now);
            }
            rows_were = (desk_now, keys);
        }
        // Past the end of the list in hand: the rows it was numbered against
        // are gone and no move could be followed -- another desk's rows, or a
        // list read while this one was being drawn. Cut down here, where the
        // number and the list are the same list, and never in a pass that is
        // holding a different one
        if active > surface_count {
            active = surface_count;
            board_open |= active == 0;
            view_drifted = true;
        }
        // The Issue tab, asked for on the last pass: to the front once its row
        // is here -- after the rows that moved have been followed, which would
        // otherwise take the view back to where it was
        if std::mem::take(&mut issues_front)
            && let Some(at) = surfaces.iter().position(|s| matches!(s, Surface::Issues { .. }))
        {
            active = at + 1;
            board_open = false;
            settings_open = false;
            // Asked for, so whatever the rows did a moment ago is not what
            // put the view here (`view::settle`)
            view_drifted = false;
            view_touched_ms = start.elapsed().as_millis() as u64;
        }
        // The folder somebody pressed, which had nothing in it until the
        // press started something. Given up on after a while, the same as a
        // reopened tab: a wait that is never answered must not outlive the
        // afternoon
        if let Some((folder, until)) = going_to.clone() {
            let is_it = |f: &std::path::Path| crate::uistate::same_folder(f, &folder);
            if let Some(n) = (1..=surface_count)
                .find(|&s| surface_folder(&surfaces, &tabs, s).is_some_and(is_it))
            {
                active = n;
                left_split = true;
                going_to = None;
                board_open = false;
                view_drifted = false;
                view_touched_ms = start.elapsed().as_millis() as u64;
            } else if until.elapsed() > Duration::from_secs(20) {
                going_to = None;
            }
        }
        // A tab just opened again, now that it is here
        if let Some((name, until)) = &reveal {
            if let Some(n) = crate::closed::row_named(&surfaces, &tabs, name) {
                active = n;
                board_open = false;
                view_drifted = false;
                view_touched_ms = start.elapsed().as_millis() as u64;
                reveal = None;
            } else if Instant::now() > *until {
                reveal = None;
            }
        }
        // A question about a row that is no longer there has nothing to ask
        if close_ask.as_ref().is_some_and(|a| {
            a.tab
                .checked_sub(1)
                .and_then(|i| surfaces.get(i))
                .map(|s| surface_key(s, &tabs))
                .as_deref()
                != Some(a.key.as_str())
        }) {
            close_ask = None;
        }
        // Keep the tree and `active` in step. Anything in the loop may set
        // `active` (a digit, an automation, the settings screen closing); the
        // focused pane follows it, and moving focus between panes sets `active`
        // at the point it happens. One sync point, so neither can drift.
        pane_layout.clamp(surface_count);
        // An editor was asked for. If it is already in a pane, look at it;
        // otherwise divide the pane in front and put it beside -- the file and
        // what is running stay on screen together, which is the whole point of
        // reading it here rather than in another program
        if let Some(key) = open_editor.take()
            && let Some(n) = surfaces
                .iter()
                .position(|s| matches!(s, Surface::Editor { key: k, .. } if *k == key))
                .map(|i| i + 1)
            {
                // The keyboard is standing in a pane with nothing in it:
                // that is where it goes, and no room has to be made. This is
                // what a file dropped on an empty pane arrives as -- the drop
                // puts the keyboard there first, and the rest of this is about
                // finding room where there is none
                let into_empty = (pane_layout.focused_surface() == 0).then(|| pane_layout.focus());
                match into_empty.or_else(|| pane_layout.pane_of(n)) {
                    // Already on screen: look at it rather than opening a
                    // second window onto the same file
                    Some(id) => {
                        pane_layout.put(id, n);
                        pane_layout.focus_pane(id);
                    }
                    None => {
                        // Divide the screen only when it is whole. Once there
                        // are two, the file goes into the one that is not being
                        // looked at -- an empty one first. Splitting every time
                        // is how you end up with six slivers and nothing
                        // readable in any of them
                        let focus = pane_layout.focus();
                        let other = pane_layout
                            .leaves()
                            .into_iter()
                            .filter(|(id, _)| *id != focus)
                            .min_by_key(|(_, s)| usize::from(*s != 0))
                            .map(|(id, _)| id);
                        match other {
                            Some(id) => {
                                pane_layout.set_surface(id, n);
                                pane_layout.focus_pane(id);
                            }
                            None => {
                                pane_layout.split(layout::Dir::Row, n);
                            }
                        }
                    }
                }
                active = pane_layout.focused_surface();
                // The editor was asked for
                view_drifted = false;
                view_touched_ms = start.elapsed().as_millis() as u64;
            }
        // A working folder's name was pressed: to that folder's first row.
        // Already the folder in front, nothing moves -- the press is somebody
        // finding their place, not asking to be moved.
        //
        // It used to put back the arrangement that folder was last seen in,
        // kept in a cache of its own that nothing on screen mentioned. An
        // arrangement is a row now (`splits.rs`), so it is in the folder's own
        // list with a name and a ✕, and pressing the folder goes to the folder
        for want in shell.mail().take_folder_views() {
            let want = std::path::PathBuf::from(want);
            let is_want = |f: &std::path::Path| crate::uistate::same_folder(f, &want);
            if folder_press_moves(surface_folder(&surfaces, &tabs, active), &want, board_open || settings_open) {
                {
                    {
                        let Some(n) = (1..=surface_count)
                            .find(|&s| surface_folder(&surfaces, &tabs, s).is_some_and(is_want))
                        else {
                            // Nothing runs there yet: pressing it opens the
                            // default command in it, which is what a folder
                            // with a card and no tab is waiting for
                            let desk = desks.get(desk_index);
                            let listed = desk.is_some_and(|d| {
                                d.folders.iter().any(|f| f.host.is_none() && f.cwd.as_deref().is_some_and(is_want))
                            });
                            if listed && let config::Start::One { name, command } =
                                config::default_shell_start(cfg.as_ref().and_then(|c| c.default_shell.as_deref()))
                            {
                                let tab = serde_json::json!({"name": name, "command": config::command_value(&command)});
                                let desk_name = desk.map(|d| d.name.clone()).unwrap_or_default();
                                if config::append_tab(&desk_name, tab, Some(&want)) {
                                    said_before_reload = Some((Instant::now(), i18n::tp("msg.shell.opened", &[("name", &name)])));
                                    // It arrives with the settings this write
                                    // sets off, and the press was to go there.
                                    // Without this the press opened something
                                    // and left the person where they were --
                                    // in a split of another folder, which is
                                    // the one place that is most confusing
                                    going_to = Some((want.clone(), Instant::now()));
                                }
                            }
                            continue;
                        };
                        // Where to go, and nothing else. Laying the screen
                        // out here as well is how a folder press could leave a
                        // split of ANOTHER folder still counting as the thing
                        // in front: the tree it owned had been replaced under
                        // it, and the next pass wrote the replacement back into
                        // that split as its arrangement
                        active = n;
                        left_split = true;
                    }
                }
            }
            board_open = false;
            settings_open = false;
            // A press is an ask, whatever the rows did on the way here
            view_drifted = false;
            view_touched_ms = start.elapsed().as_millis() as u64;
        }
        // What is on screen belongs to the row in front. A split row owns an
        // arrangement; every other row is itself, undivided. This is the one
        // place that decides it, so that everything which moves the person --
        // the list, a folder's name, a key, a hand-off, automation -- can say
        // only WHERE to go and never have to lay the screen out as well.
        //
        // The three tests below, in order: a split that is no longer there, a
        // split being left, a split being entered.
        //
        // `pressed` is taken every pass, whether or not there is a split to
        // leave. Taken inside the test that uses it, a press made with none
        // open stayed set, and then left the split that press was on its way
        // INTO the moment it was entered
        let pressed = std::mem::take(&mut left_split);
        // A split that is not on this desk's list any more is not in front,
        // whatever this still says: the desk was switched, the settings were
        // read again, the row was closed from somewhere else. Checked against
        // the rows rather than cleared by each of those in turn, because the
        // one that forgets is the one that is added next
        if open_split
            .as_deref()
            .is_some_and(|k| !surfaces.iter().any(|s| matches!(s, Surface::Split { key, .. } if key == k)))
        {
            open_split = None;
            split_save = None;
            split_written = None;
            pane_layout = crate::layout::Layout::single(active);
        }
        if let Some(key) = open_split.clone()
            && (pressed || pane_layout.pane_of(active).is_none())
        {
            splits.park(&key, pane_layout.clone());
            open_split = None;
            pane_layout = crate::layout::Layout::single(active);
        }
        if open_split.is_none()
            && let Some(key) = split_at(&surfaces, active)
        {
            let written = desks
                .get(desk_index)
                .and_then(|d| d.tabs.iter().find(|t| t.cfg.id.as_deref() == Some(key.as_str())))
                .and_then(|t| t.cfg.panes.clone());
            let keyed = surface_keys(&surfaces, &tabs);
            pane_layout = splits.take(&key, written.as_ref(), &pane_layout, |k| {
                keyed.iter().position(|t| t.matches(k)).map(|i| i + 1)
            });
            open_split = Some(key);
            active = pane_layout.focused_surface();
        }
        if open_split.is_none() && (!pane_layout.is_single() || pane_layout.focused_surface() != active) {
            pane_layout = crate::layout::Layout::single(active);
        }
        if pane_layout.focused_surface() != active {
            pane_layout.show(active);
        }
        // The arrangement in front, written down beside the row that owns it,
        // so the next start finds it as it was left. Held to the same delay a
        // dragged divider is -- this runs every pass, and a file per frame is
        // not a saved setting, it is a disk being worn out
        if let Some(key) = open_split.clone() {
            let keyed = surface_keys(&surfaces, &tabs);
            let now = crate::splits::Splits::written(&pane_layout, |s| {
                keyed.get(s - 1).and_then(|k| k.id.clone())
            });
            // Only ever an arrangement, never the absence of one. A split
            // is two panes or more, so "nothing to write" here means the tree
            // in hand is not this row's -- somebody replaced it -- and writing
            // that would take away, on a timer, the arrangement a person made.
            // The one thing that legitimately ends an arrangement is closing
            // the row, and that takes the whole row with it
            if now.is_some() && now != split_written {
                split_written = now;
                split_save = Some((key, Instant::now()));
            }
        }
        if let Some((key, at)) = split_save.clone()
            && at.elapsed() >= SPLIT_SAVE_AFTER
        {
            split_save = None;
            if let Some(kept) = split_written.as_ref() {
                config::save_tab_panes(&key, Some(kept));
            }
        }
        // Who the terminals are cut to, settled once per pass rather than by
        // whichever viewer last reported (see `terminal_size`). Both viewers
        // re-measure and re-report as they redraw, so reading it here — from
        // who is actually looking — is what keeps the two of them from taking
        // the terminal off each other.
        let watched_afar = remote_ui.as_ref().is_some_and(|r| r.watched());
        (rows, cols) = pty_dims(terminal_size(
            (shell.geom_rows(), shell.geom_cols()),
            shell.phone_size(),
            watched_afar,
        ));
        // This is the only place a terminal is resized — two places deciding
        // meant a split pane was told its size twice per frame, and whichever
        // ran last won.
        {
            let want = tab_sizes(
                tabs.len(),
                &pane_layout,
                &surfaces,
                // The panes behind the one in front, drawn to the same
                // viewer's numbers the one in front is
                crate::view::panes_geom(shell.geom_panes(), shell.phone_panes(), watched_afar),
                (rows, cols),
            );
            for (t, (r, c)) in tabs.iter().zip(want) {
                let now = {
                    let pr = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                    pr.screen().size()
                };
                if now != (r, c) {
                    let _ = t.resize(r, c);
                }
            }
        }
        // Reload and apply once the config is saved (no app restart needed)
        if watcher.changed()
            && let Some(newcfg) = config::load() {
                let (new_ws, errs) = newcfg.resolve_desks();
                startup_errors.extend(errs);
                // Which desk was active before this reload. Its live tabs are
                // in `tabs` (not the cache), so it's skipped when re-keying below.
                let prev_ws_index = desk_index;
                // The language is only read at startup, so changing it in settings
                // doesn't apply to the current screen. Add a note to the board's
                // notification prompting the user to close and reopen.
                // (the settings GUI's alert doesn't show inside the in-app WebView,
                // so we convey it here instead)
                let lang_restart = i18n::would_change(newcfg.language.as_deref());
                // Every desk's running tabs, carried over to the settings just
                // read (see `reseat_desks`). `None` is the desk on screen gone
                let viewed = reseat_desks(&desks, &new_ws, prev_ws_index, &mut tabs, &mut desk_tabs);
                // Apply immediately to the desk being viewed; others get it on switch
                let target = viewed.unwrap_or(0);
                let mut msg = i18n::t("msg.config_reloaded");
                if viewed.is_none() {
                    // What is on screen now is the desk that took its place,
                    // shown the way a switch shows one: its own running tabs,
                    // or its first launch when it has none yet
                    if let Some(w) = new_ws.get(target) {
                        if tabs.is_empty() {
                            spawn_desk(w, rows, cols, &mut tabs, &mut startup_errors, Some(&last_session));
                            for t in tabs.iter_mut() {
                                t.previous = last_session.conversation_for(w, t);
                            }
                        }
                        caps.set_desk(target);
                        config::save_last_desk(&w.id);
                    }
                    active = if tabs.is_empty() { 0 } else { 1 };
                    pane_layout = crate::layout::Layout::single(active);
                    // The desk went out from under the view; nobody asked to
                    // be moved to row 1 of whatever took its place
                    view_drifted = true;
                }
                if let Some(w) = new_ws.get(target) {
                    if viewed.is_some() {
                        let before = startup_errors.len();
                        msg = apply_ws_config(
                            &mut tabs,
                            w,
                            rows,
                            cols,
                            &mut startup_errors,
                            &mut resume_for,
                            Some(&last_session),
                        );
                        // A tab that the save asked for and could not start is what
                        // there is to say, not that the settings were read. The tab
                        // itself stays on screen saying the same (Surface::Failed)
                        if let Some(why) = startup_errors.get(before) {
                            msg = why.clone();
                        }
                    }
                    desk_index = target;
                    // Bring browsers in line with config too: open added ones, close
                    // removed ones, redraw the bar and band. If reopening were required
                    // to take effect, editing settings would be pointless
                    // (pages already open are left untouched).
                    open_declared_browsers(w, &caps, &mut startup_errors);
                }
                // The per-desk Lua engine cache is indexed by position, and that
                // position shifts whenever desks are added/removed here. Reset it
                // to match the new count (all None) so switching to a newly added
                // desk can't index out of bounds; each inactive desk's engine
                // is rebuilt on demand on the next switch (the active one is rebuilt below).
                engines = (0..new_ws.len().max(1)).map(|_| None).collect();
                desks = new_ws;
                // A token replaced in the settings reaches the machines
                // already made, not only the ones made after it
                crate::microvm::keep_sign_ins_current(microvm_sign_ins(&desks));
                // A project's git account is part of each tab's place, so the
                // places are looked at again against the settings just read
                place_at = std::time::Instant::now();
                settings_gen += 1;
                ai_choices = startable_ais();
                max_chain = newcfg.max_chain.unwrap_or(10);
                assistant_ai = newcfg.ai_engine.clone().unwrap_or_default();
                ssh_aliases = crate::discover::ssh_aliases();
                auto_switch = newcfg.auto_switch.unwrap_or(true);
                resident = newcfg.resident.unwrap_or(true);
                ai_usage_on = newcfg.ai_usage.unwrap_or(true);
                update::set_auto(newcfg.update_check.unwrap_or(true));
                busy_repeat_ms = newcfg.busy_repeat_sec.filter(|s| *s > 0).map(|s| s * 1000);
                busy_again.clear();
                done_confirm_ms = newcfg
                    .done_confirm_ms
                    .unwrap_or(profile::DEFAULT_DONE_CONFIRM_MS);
                // Rebuild capabilities and automation scripts. The destinations
                // are the desk's, handed over again below
                if let Some(e) = newcfg.secrets_problem(password.as_deref()) {
                    startup_errors.push(e);
                }
                // Only swap out the parts that come from config. Rebuilding it
                // entirely would leave nobody aware of pages already placed in the
                // window, so they'd stay stuck on screen with no way to remove them
                // (this used to happen: the moment settings were saved, the settings
                // screen would stick around and tabs would stop responding).
                caps.set_secrets(
                    newcfg.resolve_tokens(password.as_deref()),
                    newcfg.resolve_secret_terms(password.as_deref()),
                );
                // ...and the same for the connections and the git accounts,
                // which keep their own copy: a password taken out of the
                // settings stops working, and so does a token
                ssh::use_secrets(newcfg.resolve_tokens(password.as_deref()));
                crate::git::use_secrets(newcfg.resolve_tokens(password.as_deref()));
        crate::e2b::use_key(newcfg.resolve_tokens(password.as_deref()).get("e2b_api_key").cloned());
                // Everything that is the desk's rather than the app's, said
                // again now that the settings have been read afresh. It has to
                // come after set_config, not before: that call puts the app's
                // own doors and permission table in, and the desk on screen
                // has the last word on both. It also needs the secrets set_config
                // just loaded, because this is where the git accounts' tokens are read
                if let Some(w) = desks.get(desk_index) {
                    crate::desk::hand_over(w, &caps, &notifier, &prs);
                }
                caps.set_words_models(config::app_words(&newcfg));
                if let Some(eng) = engine.as_ref() {
                    eng.set_ai_engine(newcfg.ai_engine.clone().filter(|s| !s.is_empty()));
                }
                // A words run goes on across the reload: the settings are
                // written in the middle of one as a matter of course
                if let Some((pane, _)) = driving.as_ref() {
                    words_carried = engine.as_ref().and_then(|e| e.words_carry(*pane));
                    if words_carried.is_none() {
                        driving = None;
                    }
                }
                engine = build_engine(
                    Some(&newcfg),
                    desks.get(desk_index),
                    &mut startup_errors,
                    &caps,
                );
                started_fired.clear();
                started_fired.resize(tabs.len(), false);
                // `active` is a row of the list this pass is holding, and the
                // settings just read are a list nobody has drawn yet. Cut down
                // to fit that one, the number would then be followed a second
                // time on the next pass -- once for the rows that went, once
                // for the cut -- and land two rows away from what the person
                // was looking at. Deleting a worktree did exactly that, and
                // what it landed on was a folder somebody had put out of
                // sight, which came back for being looked at.
                //
                // So nothing is renumbered here. The next pass builds the new
                // list, knows both by name, and moves the view by name
                // (`surface_moves`); a number left pointing past the end is
                // cut down there, in the numbering it belongs to
                // Apply remote UI config changes (enable/disable takes effect here too)
                let mut remote_changed: Option<String> = None;
                let want = newcfg.remote.clone();
                let now = cfg.as_ref().map(|c| c.remote.clone()).unwrap_or_default();
                if (want.enabled, &want.bind, want.port, want.allow_public, &want.password, want.sticky_token, &want.fixed_token)
                    != (now.enabled, &now.bind, now.port, now.allow_public, &now.password, now.sticky_token, &now.fixed_token)
                {
                    if let Some(r) = &remote_ui {
                        r.shutdown();
                    }
                    // Same background bind as startup — the QR/status appear a
                    // moment later when the loop installs the result.
                    remote_ui = None;
                    remote_rx = start_remote_bg(Some(&newcfg), password.as_deref());
                    publish_remote(&remote_info, &remote_ui);
                    // A fresh remote server needs its settings proxy re-pointed.
                    settings_linked = false;
                    // Fresh server = fresh viewers; forget what the old one pushed.
                    last_remote_ui = None;
                    last_remote_rows = Vec::new();
                    pane_relay = PaneRelay::default();
                    // Announce the INTENT (the bind hasn't landed yet); a bind
                    // failure still surfaces as a flash from the install above.
                    remote_changed = Some(if want.enabled {
                        i18n::t("msg.remote_enabled")
                    } else {
                        i18n::t("msg.remote_stopped")
                    });
                }
                // The external API answers the same way: saving is the switch.
                // Tabs already running keep the keys they were born with (the
                // keys outlive the server, the pipe does not), so turning it
                // off and back on doesn't strand the agents mid-task
                let want_api = newcfg.external_api.access;
                if want_api != cfg.as_ref().map(|c| c.external_api.access).unwrap_or_default() {
                    if let Some(a) = api_server.as_mut() {
                        a.shutdown();
                    }
                    api_server = match api::ApiServer::start(want_api) {
                        Ok(s) => s,
                        Err(e) => {
                            append_hook_log(&format!("external API did not start: {e}"));
                            None
                        }
                    };
                }
                cfg = Some(newcfg);
                quick_view = quick_of(cfg.as_ref());
                // Re-resolve the model bridge's connection info, and hand it to
                // the tabs — including the ones parked in desks that are
                // not on screen, which are just as open as the ones that are
                if let Some(c) = &cfg {
                    let tokens = c.resolve_tokens(password.as_deref());
                    reload_providers(c, &|k| tokens.get(k).cloned(), &mut tabs, &mut desk_tabs);
                }
                watcher.retarget(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
                let mut note = remote_changed.unwrap_or(msg);
                if let Some((at, said)) = said_before_reload.take()
                    && at.elapsed() < std::time::Duration::from_secs(10)
                {
                    note = said;
                }
                if lang_restart {
                    note.push_str(&i18n::t("msg.lang_restart"));
                }
                // The setup wrote the desk itself. What the reload would say
                // about that -- settings read, the first shell closed -- is
                // about something nobody did, on the first screen anybody sees
                let from_setup = setup_reload
                    .take()
                    .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(10));
                if !from_setup {
                    flash = Some(format!(">> {note}"));
                }
                // A settings save may have changed the quick actions — push them
                // into the shell so the composer updates without a reload.
                shell.push_actions(&crate::shell::actions_json());
                shell.push_theme();
                shell.settings_reloaded();
                let (next, errs) = crate::keys::Keys::load(cfg.as_ref());
                keymap = next;
                startup_errors.extend(errs);
            }

        // Settings that could not be carried into this version, said once.
        // Shown once the screen is free, so it doesn't overwrite other output.
        if flash.is_none()
            && let Some((from, path)) = update::take_carry_failure() {
                flash = Some(i18n::tp("msg.update.carry_failed", &[("version", &from), ("path", &path)]));
            }
        // A notification that could not be sent, said here too. It used to go
        // only to hooks.log, and a person whose phone stayed quiet had nothing
        // on screen to say why.
        if flash.is_none() {
            flash = notify::take_failed();
        }
        // A script that asked for a password it may not have. It used to fail
        // quietly -- the sentence went to the script and to hooks.log, and the
        // person watching automation stop had nothing in front of them
        if flash.is_none() {
            flash = crate::caps::take_refusal();
        }

        // Check every tab's state every 200ms (completion of inactive tabs is
        // reflected on INDEX too)
        if last_detect.elapsed() >= Duration::from_millis(200) {
            last_detect = Instant::now();
            let mut transitions = Vec::with_capacity(tabs.len());
            for (i, t) in tabs.iter_mut().enumerate() {
                let (old, new) = t.tick(start);
                transitions.push((i + 1, old, new));
            }
            // What people asked the AIs, gathered by folder, and which folders
            // an AI just finished a turn in: what a folder's automatic name is
            // written from, and when. Heard whatever the emergency stop says --
            // it stops AIs acting, and this is not an AI acting
            for t in tabs.iter_mut() {
                let asked = std::mem::take(&mut t.asked);
                if let Some(at) = t.cwd() {
                    for text in asked {
                        heard.hear(at, &text);
                    }
                }
            }
            for &(idx, old, new) in &transitions {
                if old == TabState::Busy
                    && new.turn_ended()
                    && let Some(at) = tabs.get(idx - 1).and_then(|t| t.cwd())
                {
                    heard.turn_ended(at);
                }
            }
            // The first answer an AI has ever finished on this machine is the
            // moment to ask for a star: something worked. Busy first, so a
            // conversation put back at startup does not count as an answer
            if !thanks_asked && !thanks_show {
                thanks_show = transitions.iter().any(|&(idx, old, new)| {
                    old == TabState::Busy
                        && new.turn_ended()
                        && tabs.get(idx - 1).is_some_and(|t| t.is_ai())
                });
            }
            // The first time an AI turns up in a tab that was opened as a
            // shell, say what that tab will and will not do afterwards. Once
            // on this machine: it is the same answer every time, and a notice
            // that comes back is a notice people learn to look past
            if !guest_told
                && let Some(t) = tabs.iter().find(|t| t.guest().is_some())
            {
                guest_told = true;
                let _ = crate::crypto::write_atomic(&config::state_path("guest-told"), "1");
                flash = Some(i18n::tp("msg.guest.found", &[("name", t.profile_name())]));
            }

            // A tab whose launch command changed in settings is flagged for
            // restart, but only actually restarted here once it is idle — so a
            // running AI is never cut off. This makes "swap the AI in settings"
            // take effect on an idle tab on its own, instead of quietly keeping
            // the old process alive. The new session is treated as a fresh
            // launch (started_fired cleared) so its on_start briefing fires again.
            // A staged change to the launch conditions (encoding, scrollback…)
            // is not a reason to lose the conversation
            // A folder that has turned up since its tabs were held back. Nobody
            // announces that -- it is made by the dialog, by hand in Explorer,
            // or by a stick being plugged in -- so it is asked here, off the
            // table the watch already keeps, and a tab that is free to run goes
            // back through the ordinary restart below (see `Tab::reconsider`)
            for t in tabs.iter_mut().chain(desk_tabs.iter_mut().flatten()) {
                t.reconsider();
            }
            let alone: Vec<bool> = (0..tabs.len()).map(|i| only_one_here(&tabs, i)).collect();
            for (i, t) in tabs.iter_mut().enumerate() {
                // Ask what to do about the conversation only once it is
                // actually being restarted. Working it out first would mean
                // deciding — and looking on disk — five times a second for
                // every tab, to answer a question nobody had asked
                if !(t.needs_restart && t.state != TabState::Busy) {
                    continue;
                }
                let (plan, _) = resume_plan(t, alone.get(i).copied().unwrap_or(false), true);
                if t.restart_as(rows, cols, plan).is_ok()
                    && let Some(f) = started_fired.get_mut(i) {
                        *f = false;
                    }
            }

            // What a program asked us to notice, in the escapes every terminal
            // understands. Nothing had to be set up for this: a CLI that has
            // never heard of this app, running over ssh or in a container,
            // still knows how to ring a terminal.
            //
            // It always lands on the tab that sent it, where it stays until
            // something newer replaces it. The toast is the part that is held
            // back when the person is already looking at that tab — telling
            // someone what is in front of them is noise, not news
            // Output that is not UTF-8, said once per tab.
            //
            // The characters go wrong on screen and nothing else happens: the
            // program is fine, the terminal is fine, and the one setting that
            // would fix it is the one nobody knows to look for. So the tab says
            // what happened and which encoding this machine most likely meant.
            // It is a sentence, not a switch -- guessing and re-decoding by
            // ourselves would be wrong the moment a program really did send
            // something that is not text.
            for i in 0..tabs.len() {
                let Some(t) = tabs.get_mut(i) else { continue };
                if !t.take_not_utf8() {
                    continue;
                }
                let said = match crate::discover::legacy_console_encoding() {
                    Some((name, _)) => i18n::tp("msg.encoding.not_utf8", &[("enc", name)]),
                    None => i18n::t("msg.encoding.not_utf8.plain"),
                };
                append_hook_log(&format!("tab{} is not UTF-8", i + 1));
                t.set_status("notify", &said);
                flash = Some(format!("{} — {said}", t.title));
            }

            // A Windows notification that was clicked. The whole point of the
            // banner is that the person is not looking at this window, so the
            // answer to a click is to put the window in front of them, showing
            // the tab the notification was about.
            if let Some(tab) = notify::banner_clicked_tab() {
                // Put away, the window is not among the visible ones `raise`
                // looks through; it has to be brought back before it can be raised
                if shell.is_hidden() {
                    shell.show();
                }
                notify::banner_raise();
                append_hook_log(&format!("wintoast: clicked (tab{tab})"));
                if tab >= 1 {
                    shell.mail().selects.push(tab);
                }
            }

            // What the AI in a tab on another machine reported through its
            // hook, said to the terminal (see `agenthook::far_line`): taken as
            // a hook here is taken. Only from a tab that is on another machine
            // -- a program here has its own pipe, and has no business speaking
            // for itself through the screen
            for t in tabs.iter_mut() {
                let heard = t.take_far_hooks();
                if heard.is_empty() || tab_machine(t).is_none() {
                    continue;
                }
                for (kind, sent, v) in heard {
                    let report = crate::agenthook::report_of(&kind, &v);
                    if let Some(id) = report.id {
                        let s = tab::Session { id, source: tab::SessionSource::Hook };
                        append_hook_log(&format!("\"{}\" (on another machine) is running {}", t.title, s.short()));
                        t.session = Some(s);
                    }
                    if let Some(prompt) = report.prompt {
                        t.heard(&prompt);
                    }
                    if let Some(known) = report.state.as_deref().and_then(TabState::from_label) {
                        let sent = if sent == 0 { crate::hooks::epoch_ms() } else { sent };
                        t.hook_says(known, sent);
                    }
                }
            }

            // A tab on a MicroVM on a screen -- in any pane; a phone looks at
            // what the window shows -- opens its terminal, which starts its
            // machine. One nobody is looking at leaves its machine alone
            for (_, s) in pane_layout.leaves() {
                if let Some(t) = session_at(&surfaces, s).and_then(|i| tabs.get(i))
                    && let Some(id) = t.cloud().and_then(|h| h.instance.as_deref())
                {
                    crate::e2b::shown(id);
                }
            }
            // The sign-in step shows the checkout's own terminal in its dialog
            // (the checkout's machine by the project's record of it: the
            // settings entry carries no machine)
            if let Some(id) = login_pending.as_ref().filter(|p| p.shown).and_then(|p| p.home.sandbox.as_deref()) {
                crate::e2b::shown(id);
            }

            let mut fired_notes: Vec<(usize, String)> = Vec::new();
            for i in 0..tabs.len() {
                let showing = session_at(&surfaces, active) == Some(i);
                let notes = match tabs.get_mut(i) {
                    Some(t) => t.take_notes(),
                    None => continue,
                };
                for (title, body) in notes {
                    let said = match (title.trim(), body.trim()) {
                        ("", b) => b.to_string(),
                        (a, "") => a.to_string(),
                        (a, b) => format!("{a}: {b}"),
                    };
                    if said.is_empty() {
                        continue;
                    }
                    if let Some(t) = tabs.get_mut(i) {
                        append_hook_log(&format!("tab{} \"{}\" says: {said}", i + 1, t.title));
                        t.set_status("notify", &said);
                        if !showing {
                            flash = Some(format!("{} — {said}", t.title));
                        }
                    }
                    fired_notes.push((i, said));
                }
            }
            // A notification is an event too. When a program rings the terminal
            // -- a bell, an OSC notify, even over ssh where nothing of ours is
            // installed -- the automation gets an on_notify(tab, text) so it can
            // do what the toast cannot: forward it to a phone, route it, log it.
            // The toast still shows; this is additive. Without a hook it is a
            // no-op, and firing it costs nothing
            if !fired_notes.is_empty()
                && let Some(eng) = engine.as_mut() {
                    for (i, said) in fired_notes {
                        let ctx = tab_ctx(&tabs[i], surface_at(&surfaces, i + 1));
                        eng.fire("on_notify", &ctx, Some(&said));
                    }
                    // Whatever the hook asked for -- forward it, set a status --
                    // is drained and carried out here, the same way every other
                    // hook's commands are after it fires
                    let cmds = eng.drain_commands();
                    if !cmds.is_empty() {
                        let now_ms = start.elapsed().as_millis() as u64;
                        exec_commands(
                            cmds,
                            &mut tabs,
                            &surfaces,
                            &mut pane_layout,
                            max_chain,
                            auto_enabled,
                            now_ms,
                            rows,
                            cols,
                            &notifier,
                            &mut flash,
                            &mut ball,
                            &mut pending_send,
                            &mut waiting,
                            &mut active,
                            &mut lua_splits,
                            &mut lua_shuts,
                            ViewMove { allowed: auto_switch, touched_ms: view_touched_ms, settings_open },
                        );
                    }
                }

            // Where each tab is: the branch it sits on, the ports it opened.
            //
            // Both are cheap to know and expensive to ask for -- someone with
            // six agents running has six answers to "which one is serving on
            // 3000", and every one of them costs a tab switch and a command.
            //
            // Asked for all the tabs at once and only every couple of seconds.
            // The ports come from one table of the whole machine's listeners
            // and one walk of its process tree; doing that per tab would be
            // paying several times over for the same reply, and doing it every
            // frame would be paying it sixty times a second for an answer that
            // changes when someone starts a server
            // A token can change without the settings file changing -- the
            // secrets are written apart, and this PC's git keeps its own --
            // so what the MicroVMs sign in as is looked at again now and then.
            // Nothing is sent to a machine unless it changed
            if std::time::Instant::now() >= sign_ins_at {
                sign_ins_at = std::time::Instant::now() + std::time::Duration::from_secs(600);
                crate::microvm::keep_sign_ins_current(microvm_sign_ins(&desks));
            }
            if std::time::Instant::now() >= place_at {
                place_at = std::time::Instant::now() + std::time::Duration::from_secs(2);
                let mut roots: Vec<(usize, u32)> = tabs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, t)| t.pid.map(|p| (i, p)))
                    .collect();
                // Our own process is a root too, under a key no tab can have,
                // so the same one look measures what this app costs all in --
                // terminal, agents, embedded browser -- as honestly as it
                // measures each agent
                roots.push((usize::MAX, std::process::id()));
                let ports = crate::repo::ports_below(&roots);
                let cost = meter.sample(&roots);
                self_cost = cost.get(&usize::MAX).and_then(|u| u.line());
                git_repos = surfaces
                    .iter()
                    .filter_map(|s| match s {
                        Surface::Git { dir: Some(d), .. } => {
                            // A folder on another machine: its remote as git
                            // there last said (asked on a thread, see
                            // `github::far_origin`); nothing here to read
                            let far = desks.get(desk_index).and_then(|w| {
                                w.folders
                                    .iter()
                                    .find(|f| f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, d)))
                                    .and_then(|f| f.host.clone())
                            });
                            match far {
                                Some(host) => crate::github::far_origin(&host, d).map(|r| (d.clone(), r)),
                                None => crate::repo::origin_of(d).map(|r| (d.clone(), r)),
                            }
                        }
                        _ => None,
                    })
                    .collect();
                let now_ms = start.elapsed().as_millis() as u64;
                // The AI in a tab on another machine reports through a hook
                // written there, once per machine and AI in this run
                for t in tabs.iter().filter(|t| t.is_ai() && t.had_output()) {
                    let Some(at) = tab_machine(t) else { continue };
                    // Written when the machine is up for its own reasons
                    if t.cloud().and_then(|h| h.instance.as_deref()).is_some_and(crate::e2b::asleep) {
                        continue;
                    }
                    let machine = match &at {
                        crate::elsewhere::Elsewhere::Cloud(h) => h.instance.clone().unwrap_or_else(|| h.name.clone()),
                        other => other.address(),
                    };
                    crate::agenthook::ensure_far(at, machine, t.profile_name().to_string());
                }
                for (i, t) in tabs.iter_mut().enumerate() {
                    t.usage = cost.get(&i).copied().unwrap_or_default();
                    // A folder on another machine: what git there last said,
                    // asked again only while its terminal shows the machine
                    // is up (see `git::far_place`)
                    let far = match (tab_machine(t), t.cwd()) {
                        (Some(at), Some(c)) => {
                            let asleep = t.cloud().and_then(|h| h.instance.as_deref()).is_some_and(crate::e2b::asleep);
                            let awake = !asleep && t.ms_since_change(now_ms) < 120_000;
                            Some(crate::git::far_place(&at, c, awake))
                        }
                        _ => None,
                    };
                    let branch = match &far {
                        Some((b, _)) => b.clone(),
                        None => t.cwd().and_then(crate::repo::branch_of),
                    };
                    // Where it pushes to is only worth working out when there
                    // is a branch to ask about, and only worth asking about
                    // when GitHub is where it lives
                    let repo = match &far {
                        Some((_, r)) => branch.as_ref().and(r.clone()),
                        None => branch
                            .as_ref()
                            .and_then(|_| t.cwd())
                            .and_then(crate::repo::origin_of),
                    };
                    // What is known right now, and a nudge to find out. The
                    // asking happens elsewhere; a row that waited on GitHub
                    // would be a window that stops drawing
                    // Which git account the column beside this folder signs in
                    // with: its project's choice, looked up again with the rest
                    // of the place so a choice made a moment ago is in force
                    (t.git_use, t.git_project) = match (desks.get(desk_index), t.cwd()) {
                        (Some(d), Some(c)) => d.git_use_of_folder(c),
                        _ => (Default::default(), None),
                    };
                    // ...and the pull request number is read as that account
                    let pr = match (&repo, &branch, t.git_use.pr_account()) {
                        (Some(r), Some(b), Some(who)) => prs.of(&who, r, b).map(|p| p.short()),
                        _ => None,
                    };
                    // Which project this folder belongs to, and whether it is
                    // the checkout or a branch cut from it. Same kind of look
                    // as the branch above -- a file read, not a git run
                    let (family, linked) = match t.cwd() {
                        Some(c) => (crate::repo::family_of(c), crate::repo::is_linked(c)),
                        None => (None, false),
                    };
                    t.place = crate::repo::Place {
                        branch,
                        ports: ports.get(&i).cloned().unwrap_or_default(),
                        repo,
                        pr,
                        family,
                        linked,
                    };
                }
            }

            // Look for the conversation a CLI started but never announced.
            //
            // Only for a tab that could not have been anyone else: these
            // records say which folder they belong to and never which tab, so
            // with two of the same CLI in one folder there is nothing here to
            // tell them apart — and a tab that comes back holding someone
            // else's conversation is worse than one that comes back empty
            for i in 0..tabs.len() {
                let alone = only_one_here(&tabs, i);
                let Some(t) = tabs.get_mut(i) else { continue };
                let Some((at, left)) = t.session_probe else { continue };
                if std::time::Instant::now() < at {
                    continue;
                }
                let spec = t.resume.as_ref().and_then(|r| r.record.clone());
                let found = match (&spec, alone) {
                    (Some(spec), true) => sessionfind::find(spec, t.cwd(), t.born()),
                    _ => None,
                };
                match found {
                    Some(id) => {
                        let s = tab::Session { id, source: tab::SessionSource::Store };
                        append_hook_log(&format!(
                            "tab{} \"{}\" appears to be running {}",
                            i + 1,
                            t.title,
                            s.short()
                        ));
                        t.session = Some(s);
                        t.session_probe = None;
                    }
                    // Stop only where there is nothing that could ever be
                    // found. NOT after a while: one of these CLIs writes its
                    // record when the first thing is said, and a tab can sit
                    // open for an hour before anyone says it
                    None if !alone || spec.is_none() => {
                        // Said out loud, because this is the moment the tab
                        // quietly stops being able to come back tomorrow. The
                        // settings screen still shows its "carry the
                        // conversation over" tick, and nothing else on screen
                        // would ever mention that it cannot be honoured here
                        append_hook_log(&format!(
                            "tab{} \"{}\": not looking for a conversation ({})",
                            i + 1,
                            t.title,
                            match alone {
                                false => "another tab runs the same program in the same folder",
                                true => "this CLI keeps no records to read it from",
                            }
                        ));
                        t.session_probe = None;
                    }
                    None => {
                        // Eager at first, then patient. Looking is cheap —
                        // yesterday's folders are skipped unread — but not free
                        let wait = if left > 0 { 2 } else { 15 };
                        // The one pass where eagerness runs out is where this
                        // is worth saying: by now the CLI has long written its
                        // record, so still not knowing means the two sides
                        // disagree about something -- and which two things
                        // failed to meet is exactly what nobody could see
                        if left == 1 {
                            let spec = spec.as_ref().expect("checked above");
                            let seen = sessionfind::folders_seen(spec, t.born(), 5);
                            append_hook_log(&format!(
                                "tab{} \"{}\": still cannot tell which conversation {} is having \
                                 (looked under {} for a record whose folder is {}; {})",
                                i + 1,
                                t.title,
                                t.program(),
                                spec.dir,
                                t.cwd().map(|c| c.display().to_string()).unwrap_or_else(|| {
                                    "(none: the tab has no folder, so nothing can be attributed \
                                     to it)"
                                        .into()
                                }),
                                match seen.is_empty() {
                                    true => "it has written no records since this tab started"
                                        .to_string(),
                                    false =>
                                        format!("the records it has written say: {}", seen.join(", ")),
                                }
                            ));
                        }
                        t.session_probe = Some((
                            std::time::Instant::now() + Duration::from_secs(wait),
                            left.saturating_sub(1),
                        ))
                    }
                }
            }

            // Write down what is on screen, a moment after it last changed.
            // Delayed on purpose: dragging a divider changes it sixty times a
            // second, and none of those is worth a file.
            //
            // Worked out every time rather than when the screen looks
            // different, because what goes in the file does not only depend on
            // the screen: which conversation is worth keeping also turns on
            // whether the CLI has written that conversation down yet, and that
            // becomes true quietly, minutes after the tab started. Skipping on
            // a mark made of the panes and the tabs' ids meant the file was
            // written in the one moment the answer was still "no" and never
            // again, so a tab restarted mid-afternoon was still remembered
            // under the conversation it had that morning -- and that is the
            // dead id the next start handed the CLI. Nothing is written when
            // nothing changed; `write` compares with the file itself
            if save_at.is_none_or(|at| std::time::Instant::now() >= at) {
                if let Some(desk) = desks.get(desk_index) {
                    last_session.remember(desk, &tabs, Some(&pane_layout));
                    last_session.write();
                }
                save_at = Some(std::time::Instant::now() + Duration::from_secs(3));
            }

            // Retire the API keys of tabs that are gone. Told the live set
            // rather than each closure: tabs leave in several ways, and a key
            // that outlives its tab is a working key nobody is watching
            if let Some(a) = api_server.as_ref() {
                a.retain_tabs(
                    &tabs.iter().map(|t| t.called().to_string()).collect::<Vec<_>>(),
                );
            }

            // Fire hooks -> resume waiting coroutines -> run the queued operations
            if let Some(eng) = engine.as_mut() {
                // Let the loop read the current state (shikisha.state)
                eng.set_ai_engine(cfg.as_ref().and_then(|c| c.ai_engine.clone()).filter(|s| !s.is_empty()));
                eng.set_states(tab_states(&tabs));
                // Every screen, in screen order: naming any one of them names
                // the folder it is looking at, and its position is the number
                // its own calls arrive under
                eng.set_places(places_by_surface(&surfaces, &tabs));
                // ...and each tab's latest reply, so an operator can read the AI
                // tab it's driving (shikisha.tab_output).
                eng.set_outputs(
                    tabs.iter()
                        .map(|t| (t.key(), t.last_response.clone().unwrap_or_default()))
                        .collect(),
                );
                // ...and what each one has on its screen, which for a program
                // that draws instead of printing is the only output there is
                eng.set_screens(
                    tabs.iter()
                        .map(|t| (t.key(), t.last_screen.clone()))
                        .collect(),
                );
                // ...and where each one is being recorded, for reading a long
                // run back in pieces
                // Every tab is listed, recorded or not: the list is what tab
                // numbers are resolved against, and leaving one out would make
                // "tab 2" mean the second recorded tab
                eng.set_logs(
                    tabs.iter()
                        .map(|t| (t.key(), t.log_path.clone().unwrap_or_default()))
                        .collect(),
                );
                // ...and where a phone can reach this app, for "a human is
                // needed" notifications (shikisha.remote_url)
                eng.set_remote_url(remote_ui.as_ref().map(|r| r.url.clone()));
                eng.set_replies(
                    remote_ui
                        .as_ref()
                        .map(|r| (r.origin().to_string(), r.tickets())),
                );
                // Discard waiting loops belonging to exited tabs (don't leave infinite loops behind)
                for &(idx, old, new) in &transitions {
                    if new == TabState::Exited && old != TabState::Exited {
                        eng.cancel_tab(surface_at(&surfaces, idx));
                    }
                }
                let now_ms = start.elapsed().as_millis() as u64;
                if auto_enabled {
                    for (i, fired) in started_fired.iter_mut().enumerate() {
                        // Sending right after launch gets dropped, since the AI CLI
                        // hasn't drawn its input box yet. Wait until it's ready before
                        // flushing it in.
                        if !*fired && tabs[i].ready_for_startup_hook(now_ms) {
                            *fired = true;
                            eng.fire(
                                "on_start",
                                &tab_ctx(&tabs[i], surface_at(&surfaces, i + 1)),
                                None,
                            );
                        }
                    }
                    for &(idx, old, new) in &transitions {
                        if old == new {
                            continue;
                        }
                        let t = &tabs[idx - 1];
                        append_hook_log(&format!(
                            "State tab{idx} {}->{} [{}] said={:?} prompted={} working={} answered={} submit_pending={}",
                            old.label(),
                            new.label(),
                            t.profile_name(),
                            // What the program said about itself, if it says
                            // anything: the one line that tells a state read
                            // off the screen from a state it was told outright
                            t.hook_word().map(|w| w.label()),
                            t.was_prompted(),
                            t.saw_working_flag(),
                            t.answered_since_submit(),
                            pending_send.iter().any(|p| p.tab == idx)
                        ));
                    }

                    // Once a follow-up starts, cancel any pending completion confirmation
                    for &(idx, _, new) in &transitions {
                        if new == TabState::Busy || new == TabState::Exited {
                            pending_done.retain(|&(t, _)| t != idx);
                        }
                    }
                    for &(idx, old, new) in &transitions {
                        if old == new {
                            continue;
                        }
                        // If it restarted, redo on_start (resume automation after an SSH reconnect)
                        if new != TabState::Exited && old == TabState::Exited
                            && let Some(f) = started_fired.get_mut(idx - 1) {
                                *f = false;
                            }
                        let ctx = tab_ctx(&tabs[idx - 1], surface_at(&surfaces, idx));
                        // Even just the startup banner's output makes the screen move
                        // then settle, so every tab is guaranteed to pass through DONE
                        // once with nobody having asked anything. To avoid forwarding
                        // that output as a response, only treat it as one once there's
                        // been input. A tab where submit (Enter) hasn't arrived yet is
                        // merely showing a pasted draft. Going quiet doesn't make that a response.
                        let submitting = pending_send.iter().any(|p| p.tab == idx);
                        // If nothing came out after submit, it never arrived.
                        // Don't read a screen that's just showing the pasted draft as a response.
                        let answering = tabs[idx - 1].was_prompted()
                            && !submitting
                            && tabs[idx - 1].answered_since_submit();
                        match new {
                            TabState::Busy if answering => {
                                eng.fire("on_busy", &ctx, None);
                                // ...and from here it may be mentioned again
                                // while it is still working (below)
                                if let Some(every) = busy_repeat_ms {
                                    busy_again.insert(idx, now_ms + every);
                                }
                            }
                            _ if new.turn_ended() && old == TabState::Busy && !answering => {
                                append_hook_log(&format!(
                                    "Ignoring done tab{idx} [{}] prompted={} submitting={} answered={}",
                                    tabs[idx - 1].profile_name(),
                                    tabs[idx - 1].was_prompted(),
                                    submitting,
                                    tabs[idx - 1].answered_since_submit()
                                ));
                            }
                            _ if new.turn_ended() && answering && old == TabState::Busy => {
                                append_hook_log(&format!(
                                    "Awaiting done confirmation tab{idx} [{}]",
                                    tabs[idx - 1].profile_name()
                                ));
                                // Don't fire yet here. AI output pauses for breath
                                // partway through, so going quiet alone doesn't mean it's done.
                                // Use the AI-specific setting if given, otherwise the base config.
                                let wait = tabs[idx - 1].done_confirm_ms().unwrap_or(done_confirm_ms);
                                let at = now_ms + wait;
                                pending_done.retain(|&(t, _)| t != idx);
                                pending_done.push((idx, at));
                            }
                            TabState::Question => {
                                let screen =
                                    tabs[idx - 1].parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents();
                                eng.fire("on_question", &ctx, Some(&screen));
                            }
                            TabState::Exited => eng.fire("on_exit", &ctx, None),
                            _ => {}
                        }
                    }
                    // Fire only the ones that stayed quiet as truly done
                    let (ready, waiting): (Vec<_>, Vec<_>) =
                        pending_done.iter().partition(|&&(_, at)| now_ms >= at);
                    pending_done = waiting;
                    for (idx, _) in ready {
                        if let Some(t) = tabs.get_mut(idx.wrapping_sub(1)) {
                            if !t.state.turn_ended() {
                                continue;
                            }
                            // One response per submit. Waiting for the next one requires another submit.
                            t.finish_response();
                        }
                        let ctx = tab_ctx(&tabs[idx - 1], surface_at(&surfaces, idx));
                        // Narrowing the width makes vt100 truncate each line to that
                        // width, so if it got narrower while waiting for a response,
                        // the text is missing pieces. We can't undo that, but keeping
                        // the truncated text is better than silently handing over gaps.
                        if tabs[idx - 1].resized_while_waiting() {
                            append_hook_log(&format!(
                                "Warning tab{idx}: the screen width narrowed while a response was in \
                                 progress. The terminal truncates lines to fit, so the response may be missing content."
                            ));
                        }
                        let ended = tabs[idx - 1].state;
                        append_hook_log(&format!(
                            "{} fired tab{idx} [{}]: response {} chars: {}",
                            crate::hooks::ending_hook(ended),
                            ended.label(),
                            ctx.output.chars().count(),
                            log_excerpt(&ctx.output, 100)
                        ));
                        eng.fire_ending(ended, &ctx);
                        // Beginner-friendly "notify me when this AI answers": a
                        // per-tab shortcut for an on_done that calls notify.
                        if let Some(dest) = tabs[idx - 1].notify_on_done.clone() {
                            // Three lines, and each one earns its place. The
                            // name, because a phone buzzing without saying
                            // which tab finished is a phone that sends you to
                            // the PC to find out. The opening of the answer,
                            // because most of the time that IS the answer and
                            // the walk can be skipped entirely. And where the
                            // board is -- but never the key to it: a paired
                            // phone opens this and is already signed in from
                            // its own storage, while the same link in a shared
                            // channel hands over nothing.
                            let reply = match (tabs[idx - 1].notify_reply, remote_ui.as_ref())
                            {
                                (true, Some(r)) => Some(r.reply_link(reply::Ticket::new(
                                    tabs[idx - 1].id.clone(),
                                    idx,
                                    tabs[idx - 1].title.clone(),
                                    ctx.output.clone(),
                                    dest.clone(),
                                ))),
                                _ => None,
                            };
                            let msg = on_done_message(
                                &tabs[idx - 1].title,
                                &ctx.output,
                                reply.as_deref(),
                            );
                            // Which tab, so that a banner on this PC can be
                            // clicked back to the thing it is about.
                            let status = notifier.send_about(&dest, &msg, Some(idx));
                            append_hook_log(&format!("notify_on_done tab{idx} \"{dest}\": {status}"));
                        }
                    }

                    // A tab that has been working a long time without a word is
                    // either thinking or hung, and nothing here can tell those
                    // apart. The automation that asked for the work can, so it is
                    // told again while the work is still running -- but only about
                    // tabs it was told about in the first place, and only when
                    // somebody asked for it. A hook that starts running on a timer
                    // by itself is a hook that surprises whoever wrote it
                    if let Some(every) = busy_repeat_ms {
                        let states: Vec<TabState> = tabs.iter().map(|t| t.state).collect();
                        for idx in busy_repeat_due(now_ms, every, &states, &mut busy_again) {
                            let ctx = tab_ctx(&tabs[idx - 1], surface_at(&surfaces, idx));
                            append_hook_log(&format!(
                                "on_busy again tab{idx}: still working after {}s",
                                every / 1000
                            ));
                            eng.fire("on_busy", &ctx, None);
                        }
                    }

                    // Automation addresses things by screen number; the contents live in sessions
                    eng.tick_pending(&|pane| {
                        session_at(&surfaces, pane)
                            .and_then(|i| tabs.get(i))
                            .map(|t| t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents())
                    });
                }
                // What a person started from a panel is not automation, and the
                // brake on automation does not hold it: a folder sent from the
                // file panel goes on with automation stopped, the way a single
                // file always has
                if !auto_enabled {
                    eng.tick_panel_pending();
                }
                // A report aimed at a file panel goes to that panel. Taken out
                // here because `exec_commands` knows about tabs and a panel is
                // not one -- and because this is where the panel's other news
                // is already pushed from
                let cmds = panel_progress_out(eng.drain_commands(), &surfaces, &sftp_tx);
                if !cmds.is_empty() {
                    let now_ms = start.elapsed().as_millis() as u64;
                    exec_commands(
                        cmds,
                        &mut tabs,
                        &surfaces,
                        &mut pane_layout,
                        max_chain,
                        auto_enabled,
                        now_ms,
                        rows,
                        cols,
                        &notifier,
                        &mut flash,
                        &mut ball,
                        &mut pending_send,
                        &mut waiting,
                        &mut active,
                        &mut lua_splits,
                        &mut lua_shuts,
                        ViewMove { allowed: auto_switch, touched_ms: view_touched_ms, settings_open },
                    );
                }
            }

            // Hand the current status to the remote UI and run any operations it sent
            if let Some(r) = remote_ui.as_ref() {
                let snap = remote::Snapshot {
                    // What was built at draw time, read back from where the
                    // window keeps it. `ui` doesn't exist here yet, and
                    // building it again would be a second place that assembles
                    // state -- and one more full build of it every frame.
                    ui: shell.last_drawn().cloned(),
                    // What the screen push last sent, so a viewer that joins now
                    // is handed the same picture the ones already here can see
                    screen_html: last_remote_rows.join("\n"),
                    panes: if r.has_pane_clients() { pane_relay.seed() } else { Vec::new() },
                    desk: desks
                        .get(desk_index)
                        .map(|w| w.name.clone())
                        .unwrap_or_default(),
                    auto_enabled,
                    cols,
                    // Numbered by SCREEN position (the 1-based index the phone
                    // shows and sends back, e.g. /api/attach's `tab`), not by
                    // session slot: browser surfaces sit in the list too, so the
                    // two numberings drift apart after the first browser tab
                    tabs: tabs
                        .iter()
                        .enumerate()
                        .map(|(i, t)| remote::RemoteTab {
                            index: surface_at(&surfaces, i + 1),
                            name: t.title.clone(),
                            state: t.state.label().to_string(),
                            locked: t.locked,
                            output: trim_for_phone(
                                &t.last_response.clone().unwrap_or_default(),
                                200,
                            ),
                            // Carries appearance, so read line by line rather than via contents()
                            screen: trim_for_phone(
                                &tab::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen()),
                                200,
                            ),
                            cwd: tab_cwd_abs(t),
                            machine: match (t.remote(), t.cloud()) {
                                (Some(spec), _) => Some(crate::elsewhere::Elsewhere::Ssh(spec.clone())),
                                (None, Some(host)) => Some(crate::elsewhere::Elsewhere::Cloud(host.clone())),
                                (None, None) => None,
                            },
                            remote_cwd: t.remote_cwd().unwrap_or_default().to_string(),
                            // Two strings, no filesystem: this runs every tick,
                            // and finding the record means walking a folder.
                            // The reader resolves the path when it is asked
                            record_id: t
                                .session
                                .as_ref()
                                .map(|s| s.id.clone())
                                .unwrap_or_default(),
                            record_glob: t
                                .resume
                                .as_ref()
                                .and_then(|r| r.verify.clone())
                                .unwrap_or_default(),
                        })
                        .collect(),
                };
                // Push what changed to any state-socket viewers. The UI goes out
                // whenever it changes; the screen goes out as the rows that
                // moved, no faster than the viewer's line is draining
                // (idle = nothing sent).
                if r.has_state_clients() {
                    let ui_json = serde_json::to_string(&snap.ui).unwrap_or_default();
                    if last_remote_ui.as_deref() != Some(ui_json.as_str()) {
                        r.push_state(format!("{{\"ui\":{ui_json}}}"));
                        last_remote_ui = Some(ui_json);
                    }
                    // The heartbeat. Carries nothing the page needs -- it reads
                    // it as "the line is alive" and drops it -- and exists so
                    // that a viewer that has gone is found to be gone.
                    if last_beat.elapsed() >= BEAT {
                        r.push_state("{\"beat\":1}".to_string());
                        last_beat = Instant::now();
                    }
                }
                *r.snapshot.lock().unwrap() = snap;
            }

            // auto_restart: automatically bring exited tabs back
            let alone: Vec<bool> = (0..tabs.len()).map(|i| only_one_here(&tabs, i)).collect();
            for (i, t) in tabs.iter_mut().enumerate() {
                if t.state == TabState::Exited && t.auto_restart {
                    let (plan, _) = resume_plan(t, alone.get(i).copied().unwrap_or(false), true);
                    match t.restart_as(rows, cols, plan) {
                        Ok(()) => {
                            append_hook_log(&format!("auto-restart tab{}", i + 1));
                            flash = Some(i18n::tp("msg.restarted", &[("name", &t.title)]));
                        }
                        Err(e) => flash = Some(i18n::tp("msg.restart_failed", &[("error", &t.launch_hint(&e.to_string()))])),
                    }
                }
            }
        }

        // Calls waiting on the external API's pipe. Answered here, on the loop,
        // because the Lua state belongs to this thread — the caller is holding
        // its line open for the answer, so this is drained every turn (16ms)
        // rather than on the 200ms detection tick
        if let Some(a) = api_server.as_ref() {
            while let Ok(call) = a.rx.try_recv() {
                // A desk with no Lua of its own still has an engine's
                // worth of commands to offer; make one rather than answer
                // "not available" (the same gap-filler as 🎯 operate and ▶)
                if engine.is_none() {
                    match crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)) {
                        Ok(eng) => engine = Some(eng),
                        Err(e) => {
                            let _ = call.reply.send(Err(e.to_string()));
                            continue;
                        }
                    }
                }
                let answer = match engine.as_ref() {
                    Some(eng) => {
                        // Who exists, before anything is answered. An engine
                        // made a line ago to serve this very call has been
                        // told nothing yet, and "which tab is calling" is
                        // answered out of the screens — the first call from a
                        // tab used to be credited to nobody and thrown away,
                        // and the first call is the one carrying the id of the
                        // conversation to come back to.
                        //
                        // The screens, not the tabs: a caller is credited the
                        // number its report is then carried out under, and the
                        // two lists are not the same one (`places_by_surface`)
                        eng.set_states(tab_states(&tabs));
                        eng.set_places(places_by_surface(&surfaces, &tabs));
                        let who = subject_of(call.caller.as_deref(), &tabs);
                        eng.call_primitive_as(
                            call.caller.as_deref(),
                            who,
                            &call.method,
                            &call.params,
                        )
                    }
                    None => Err("no engine".to_string()),
                };
                let _ = call.reply.send(answer);
            }
        }

        // Process remote operations and frame delivery every iteration (waiting 200ms
        // would let finger-swipe traces bunch up and arrive all at once, breaking swipe playback)
        if let Some(r) = remote_ui.as_ref() {
            let now_ms = start.elapsed().as_millis() as u64;
            // The browser currently being viewed (target for Inject / relay)
            let shown_browser = match surfaces.get(active.wrapping_sub(1)) {
                Some(Surface::Browser { key, .. }) => Some(key.clone()),
                _ => None,
            };
            while let Ok(cmd) = r.rx.try_recv() {
                match cmd {
                    // Treat input from remote as a human operation
                    // (resets the auto-chain, and is rejected while locked)
                    remote::RemoteCmd::Send { tab, text } => {
                        let excerpt = log_excerpt(&text, 120);
                        if hand_line(
                            &mut tabs, &surfaces, tab, text, now_ms,
                            &mut pending_send, &mut ball,
                        ) {
                            append_hook_log(&format!("remote send tab{tab}: {excerpt}"));
                        }
                    }
                    // An answer typed on a reply page. The same act as
                    // typing into the tab here -- it goes in as a person's
                    // words and breaks the automatic chain -- with two
                    // differences. The target is found by the tab's own id
                    // first, because numbers shift while a phone sits in a
                    // pocket and a "yes" delivered to whatever is third in the
                    // list now is worse than one that arrives nowhere. And the
                    // chat that carried the link is told what happened, either
                    // way: somebody who pressed send on a train has no other
                    // way to learn whether it landed
                    remote::RemoteCmd::Reply { tab_id, tab, name, dest, text } => {
                        let by_number = |n: usize| session_at(&surfaces, n).and_then(|i| tabs.get(i));
                        let target = (1..=tabs.len())
                            .find(|n| {
                                tab_id.as_deref().is_some_and(|want| {
                                    by_number(*n).and_then(|t| t.id.as_deref()) == Some(want)
                                })
                            })
                            .or_else(|| {
                                // No id to go on: the number stands, but only
                                // if the tab there is still the one that asked
                                (by_number(tab).map(|t| t.title.as_str()) == Some(name.as_str()))
                                    .then_some(tab)
                            });
                        let said = log_excerpt(&text, 120);
                        let to = (!dest.is_empty()).then_some(dest.as_str());
                        let landed = target.is_some_and(|n| {
                            hand_line(
                                &mut tabs, &surfaces, n, text.clone(), now_ms,
                                &mut pending_send, &mut ball,
                            )
                        });
                        let told = match landed {
                            true => i18n::tp("msg.notify.replied", &[("name", &name)]),
                            false => i18n::tp("msg.notify.reply_lost", &[("name", &name)]),
                        };
                        append_hook_log(&format!(
                            "reply -> \"{name}\" ({}): {said}",
                            if landed { "sent" } else { "no such tab" }
                        ));
                        notifier.send_opt(to, &format!("{told}\n{text}"));
                    }
                    remote::RemoteCmd::Keys { tab, keys } => {
                        if let Some(t) = session_at(&surfaces, tab).and_then(|i| tabs.get_mut(i)) {
                            if t.locked {
                                continue;
                            }
                            t.chain_depth = 0;
                            t.last_manual_ms = Some(now_ms);
                            let _ = t.write_bytes(keys.as_bytes());
                        }
                    }
                    // Input on the relay screen is injected as real input into the browser being viewed
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Inject { input, .. }) => {
                        if let Some(key) = &shown_browser {
                            let _ = caps.browser_inject(key, input);
                        }
                    }
                    // The top bar (back/forward/refresh/URL) doesn't turn into terminal
                    // keystrokes. Just like the window, push it onto `gos` and let the
                    // shared handling below pass it to the browser. Routing it through
                    // `keys_for` used to silently drop `Go` as unmatched.
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Go { go }) => {
                        shell.mail().gos.push(go);
                    }
                    // Scrolling back through history isn't a keystroke, so keys_for()
                    // can't carry it — it would be dropped, leaving the phone stuck on
                    // the current screen with no way to review earlier output. Push it
                    // onto the very queue the window's own wheel feeds, so both are
                    // applied identically below (into a full-screen TUI's own scroll,
                    // or our kept scrollback for a plain shell).
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Scroll { by, row, col }) => {
                        shell.mail().scrolls.push((by, row, col));
                    }
                    // The phone fits the terminal to its own screen. Its numbers are
                    // kept as the phone's own -- not written over the window's, which
                    // is what the window falls back to the moment nobody is watching
                    // from afar (see `terminal_size`). Its `area` is not taken either:
                    // that positions the window's own browser child view, which the
                    // phone doesn't use (it watches the relay), so the window keeps
                    // the placement it measured for itself.
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Resize { rows, cols, panes, .. }) => {
                        shell.set_phone_size(Some((rows, cols)));
                        // ...and how it has laid the division out, which it
                        // draws too now (`view::panes_geom`). Dropped here, a
                        // pane nobody was typing into kept whatever width the
                        // window had given it and never got another
                        shell.set_phone_panes(panes);
                        shell.queue_input(Event::Resize(cols, rows));
                    }
                    // A Lua quick-action fired from the phone. It's not a keystroke,
                    // so route it straight to the same queue the window's ipc path
                    // fills (drained and run against the active tab below).
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::RunAction { path }) => {
                        shell.mail().run_actions.push(path);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Words { on, goal, agree }) => {
                        shell.mail().words.push((on, goal, agree));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::WhyStopped { ask }) => {
                        shell.mail().why_stopped.push(ask);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Operate { target, goal }) => {
                        shell.mail().operates.push((target, goal));
                    }
                    // 📼 / ▶ from the phone's composer: same queues as the window's.
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Record { on }) => {
                        shell.mail().record_arms.push(on);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Git { panel, act, args }) => {
                        shell.mail().gits.push((panel, act, args));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Making { id, act }) => {
                        shell.mail().makings.push((id, act));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Found { family, act }) => {
                        shell.mail().found.push((family, act));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::GitAccount { panel, account }) => {
                        shell.mail().git_accounts.push((panel, account));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Issues { act, args }) => {
                        shell.mail().issues.push((act, args));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::OpenIssues) => {
                        shell.mail().open_issues = true;
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Ideas { act, args }) => {
                        shell.mail().ideas.push((act, args));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Files { panel, act, args }) => {
                        shell.mail().files.push((panel, act, args));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::EditOpen { panel, path, diff }) => {
                        shell.mail().edits.push((panel, path, diff));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Sftp { panel, act, args }) => {
                        shell.mail().sftps.push((panel, act, args));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::RunLua { code }) => {
                        shell.mail().run_luas.push(code);
                    }
                    // ✨ a suggestion request from the phone: same queue as the
                    // window's (keys_for would silently drop it, like Go once was)
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Suggest { text }) => {
                        shell.mail().suggests.push(text);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Survey) => {
                        shell.mail().surveys += 1;
                    }
                    // A line the phone finished in the composer. Not a
                    // keystroke -- the recipient may be a model bridge, which
                    // has no keyboard -- so it goes to the same queue the
                    // window's composer fills. Without this it fell through to
                    // keys_for and was dropped, which the loop's own
                    // fall-through guard had been saying all along.
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Say { tab, text }) => {
                        shell.mail().says.push((tab, text));
                    }
                    // A quick command pressed on the phone: the same queue the
                    // window's press fills, looked up and sent on this machine
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Quick { id, tab }) => {
                        shell.mail().quicks.push((id, tab));
                    }
                    // The bar's button, pressed on the phone: the same queue the
                    // board's press fills. A person's answer from wherever they
                    // are looking
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Button { from: Some(name) }) => {
                        shell.mail().presses.push(name);
                    }
                    remote::RemoteCmd::Ui(ev @ shikisha_shared::Ev::VaultSearch { .. })
                    | remote::RemoteCmd::Ui(ev @ shikisha_shared::Ev::VaultOpen { .. })
                    | remote::RemoteCmd::Ui(ev @ shikisha_shared::Ev::PastList { .. })
                    | remote::RemoteCmd::Ui(ev @ shikisha_shared::Ev::PastResume { .. }) => {
                        shell.queue_ui(ev);
                    }
                    // Giving a branch its own folder, and putting a working
                    // folder back on this machine. Neither is a keystroke, so
                    // neither can be turned into one -- they go to the same
                    // queues the window's dialogs fill
                    remote::RemoteCmd::Ui(ev @ shikisha_shared::Ev::Branch { .. }) => {
                        shell.mail().branches.extend(shikisha_shared::BranchAsk::of(ev));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::BringLines { from, lines }) => {
                        shell.mail().bring_lines.push((from, lines));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Repair {
                        folder,
                        choose,
                        branch,
                        take,
                    }) => {
                        shell.mail().repairs.push((folder, choose, branch, take));
                    }
                    // Walking the folders to open another one: the list the
                    // phone has instead of a dialog. Same queue as the window's
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Browse { path, open, make }) => {
                        shell.mail().browses.push((path, open, make));
                    }
                    // The update card and the first-run pointer, answered on
                    // the phone: the same fields the window's presses fill
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Update { open }) => {
                        shell.mail().update_card = Some(open);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Coach { step }) => {
                        shell.mail().coach_done = Some(step);
                    }
                    // The pane's own restart, pressed from afar. The window
                    // has filled this queue since panes existed; nothing
                    // filled it from a phone, so the button did nothing and
                    // said nothing
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::RestartPane { id, keep }) => {
                        shell.mail().restart_panes.push((id, keep));
                    }
                    // Putting away a tab's usage-limit notice. Reading it is
                    // the whole act, and it is usually read from a phone --
                    // where, until now, putting it away put nothing away
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::LimitAck { tab }) => {
                        shell.mail().limit_acks.push(tab);
                    }
                    // Picking a tab from afar. By number, as at the window
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Select { tab }) => {
                        shell.mail().selects.push(tab);
                    }
                    // A working folder pressed in the list, from afar: the same
                    // queue as the window's
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderView { folder }) => {
                        shell.mail().folder_views.push(folder);
                    }
                    // Arranging the screen, from a device with room to
                    // arrange it. The same queues the window's own presses
                    // fill -- one place decides what a split means
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FocusPane { id }) => {
                        shell.mail().focus_panes.push(id);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::ClosePane { id }) => {
                        shell.mail().close_panes.push(id);
                    }
                    // A tab's ✕, the answer to its question, and opening a
                    // closed one again: the window's queues, so the phone is
                    // asked the same question the window is
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::CloseTab { tab, key, sure }) => {
                        shell.mail().close_tabs.push((tab, key, sure));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::CloseTabBack) => {
                        shell.mail().close_tab_back = true;
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::ReopenTab { id }) => {
                        shell.mail().reopen_tabs.push(id);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::SplitPane { id, down }) => {
                        shell.mail().pane_splits.push((id, down));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::PaneRatio { divider, ratio }) => {
                        shell.mail().pane_ratios.push((divider, ratio));
                    }
                    // The tab bar's +. Two things, as at the window: where the
                    // tab should land, and the keystroke that opens the form
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::AddTab { pane, folder }) => {
                        let mail = shell.mail();
                        mail.add_tab_pane = pane.or(mail.add_tab_pane);
                        if let Some(f) = folder {
                            mail.add_tab_folder = Some(f);
                        }
                        for e in keys_for(&shikisha_shared::Ev::AddTab { pane: None, folder: None }) {
                            shell.inject(e);
                        }
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::TabName { tab, name }) => {
                        shell.mail().tab_names.push((tab, name));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::TabFolder { tab, folder }) => {
                        shell.mail().tab_folders.push((tab, folder));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderName { folder, name }) => {
                        shell.mail().folder_names.push((folder, name));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderClose { folder }) => {
                        shell.mail().folder_closes.push(folder);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderDiscard { folder, unasked }) => {
                        shell.mail().folder_discards.push((folder, unasked));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FarPorts { folder }) => {
                        shell.mail().far_ports.push(folder);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FarPage { folder, port }) => {
                        shell.mail().far_pages.push((folder, port));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Login { folder, act }) => {
                        shell.mail().logins.push((folder, act));
                    }
                    // The add-a-project dialog, from a phone: the same queues
                    // the window's dialog fills (see `main.rs`)
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::AddProject {
                        how,
                        text,
                        parent,
                        ask,
                        host,
                        project,
                        ai,
                        account,
                    }) => {
                        shell.mail().add_projects.push(crate::mailbox::AddAsk {
                            how,
                            text,
                            parent,
                            ask,
                            host,
                            project,
                            ai,
                            account,
                        });
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::RemoteList { host, path, ask }) => {
                        shell.mail().remote_lists.push((host, path, ask));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::AddHost { name, at, key, password, ask }) => {
                        shell.mail().add_hosts.push(crate::mailbox::HostAsk { name, at, key, password, ask });
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderColor { folder, color }) => {
                        shell.mail().folder_colors.push((folder, color));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderHide { folder, hide }) => {
                        shell.mail().folder_hides.push((folder, hide));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderMove { folder, to }) => {
                        shell.mail().folder_moves.push((folder, to));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::KeepEnv { from }) => {
                        shell.mail().keep_envs.push(from);
                    }
                    // How big the text is, and how wide the tab bar is, as the
                    // person looking wants them
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FontSize { px }) => {
                        shell.mail().font_size = Some(px);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::TabWidth { px }) => {
                        shell.mail().tab_width = Some(px);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::SideWidth { px }) => {
                        shell.mail().side_width = Some(px);
                    }
                    // Convert other screen operations into the same keystrokes that come from the window
                    remote::RemoteCmd::Ui(ev) => {
                        let keys = keys_for(&ev);
                        if keys.is_empty() {
                            // Every new intent kind must be routed above
                            // explicitly — a fall-through here has silently
                            // swallowed Go, Suggest and Survey before. Never
                            // let the next one vanish without a trace
                            append_hook_log(&format!(
                                "remote UI event fell through unrouted: {ev:?}"
                            ));
                        }
                        for e in keys {
                            shell.inject(e);
                        }
                    }
                    remote::RemoteCmd::SetAuto(on) => {
                        auto_enabled = on;
                        if !on
                            && let Some(eng) = engine.as_mut() {
                                eng.cancel_all();
                            }
                        flash = Some(i18n::t(if on {
                            "msg.remote_auto_on"
                        } else {
                            "msg.remote_auto_off"
                        }));
                    }
                }
            }
            // Deliver only the newest of the accumulated relay frames (drop the older ones).
            // Keeps the connection and the phone from being flooded when the sender is fast;
            // always shows the latest picture.
            if let Some(jpeg) = shell.mail().take_frames().pop() {
                r.push_frame(jpeg);
            }
            // Relay if the browser being viewed has viewers, otherwise stop
            let want = if r.has_frame_clients() {
                shown_browser
            } else {
                None
            };
            if want != casting {
                if let Some(old) = &casting {
                    let _ = caps.browser_screencast(old, false);
                }
                if let Some(new) = &want {
                    let _ = caps.browser_screencast(new, true);
                }
                // And whose sound goes with it, once the window has had the
                // message above -- it has not yet, so this is nobody's for
                // now and asked again below
                r.sound_comes_from(0);
                casting = want;
            } else if let Some(key) = &casting {
                // Even if the target hasn't changed, push out one frame of the current
                // screen when a new viewer joins. Otherwise a static page would leave
                // them waiting for a change forever, staring at nothing.
                if r.take_keyframe_request() {
                    let _ = caps.browser_screencast(key, true);
                }
                // Whose sound goes with this picture, asked until there is an
                // answer. The window is told to start casting by a message and
                // only knows which browser plays the page once it has acted on
                // it, so asking in the same breath as sending it always
                // answered "nobody" -- and a phone tapping the speaker was
                // told the PC did not know what was playing
                if !r.sound_is_known() {
                    r.sound_comes_from(caps.browser_sound_from(key).unwrap_or(0));
                }
            }
        }

        // Flush held hand-offs once the recipient becomes ready to receive them.
        // Even ones we give up on aren't silently discarded — the worst outcome
        // is for something to vanish without a trace.
        if !waiting.is_empty() {
            let now_ms = start.elapsed().as_millis() as u64;
            let keys = surface_keys(&surfaces, &tabs);
            let mut ready: Vec<Command> = Vec::new();
            let mut keep: Vec<Waiting> = Vec::new();
            for w in std::mem::take(&mut waiting) {
                let can = target_of(&w.cmd)
                    .and_then(|r| r.resolve(&keys))
                    .and_then(|p| session_at(&surfaces, p))
                    .and_then(|i| tabs.get(i))
                    .map(|t| ready_to_receive(t, now_ms))
                    .unwrap_or(false);
                if can {
                    ready.push(w.cmd);
                } else if now_ms >= w.give_up_ms {
                    let to = target_of(&w.cmd);
                    append_hook_log(&format!("Timed out never becoming ready to receive: {to:?}"));
                    flash = Some(i18n::tp(
                        "msg.handoff_timeout",
                        &[("target", &format!("{to:?}"))],
                    ));
                } else {
                    keep.push(w);
                }
            }
            waiting = keep;
            if !ready.is_empty() {
                exec_commands(
                    ready,
                    &mut tabs,
                    &surfaces,
                    &mut pane_layout,
                    max_chain,
                    auto_enabled,
                    now_ms,
                    rows,
                    cols,
                    &notifier,
                    &mut flash,
                    &mut ball,
                    &mut pending_send,
                    &mut waiting,
                    &mut active,
                    &mut lua_splits,
                    &mut lua_shuts,
                    ViewMove { allowed: auto_switch, touched_ms: view_touched_ms, settings_open },
                );
            }
        }

        // Feed out the pastes in flight, and press Enter once the recipient has
        // taken the whole thing in
        if !pending_send.is_empty() {
            let now_ms = start.elapsed().as_millis() as u64;
            // One at a time per tab, from the front. Two messages to the same
            // tab used to go over interleaved -- the second one's text arriving
            // before the first one's Enter, so both were sent as one and the
            // second Enter went out onto an empty line. Sending in turn is what
            // makes two messages two messages.
            let mut holding: Vec<usize> = Vec::new();
            pending_send.retain_mut(|p| {
                if holding.contains(&p.tab) {
                    return true;
                }
                holding.push(p.tab);
                let Some(t) = session_at(&surfaces, p.tab).and_then(|i| tabs.get(i)) else {
                    return false;
                };
                match p.step(t.output_count(), now_ms) {
                    Step::Wait => true,
                    Step::Hand(chunk) => {
                        let _ = t.write_passthrough(&chunk);
                        true
                    }
                    Step::Submit { settled } => {
                        if p.submit {
                            let _ = t.write_bytes(b"\r");
                            append_hook_log(&format!(
                                "submit tab{} ({})",
                                p.tab,
                                if settled { "after intake finished" } else { "sent while still unsettled" }
                            ));
                        }
                        false
                    }
                }
            });
        }

        // chain_depth resets to 0 when a human types. Make the ball follow that too
        // (checked from the holder's side, so we don't need to add more places that reset it).
        // Don't clear a ball that's waiting on a human here. Even if the chain has
        // ended, the work still belongs to the holder. It gets cleared on the
        // touched side once a human touches it.
        if ball.holder > 0
            && !ball.awaiting_human
            && !session_at(&surfaces, ball.holder)
                .and_then(|i| tabs.get(i))
                .map(|t| t.chain_depth > 0)
                .unwrap_or(false)
        {
            ball.reset();
        }
        ball.clamp_to(surfaces.len());

        // The controls shown over the browser being viewed.
        //
        // Whether to show them is decided by config or Lua; whether they're pressable
        // is answered by the window. The answer arrives with a delay, so show them
        // looking unpressable until it comes in.
        let drawn_ms = start.elapsed().as_millis() as u64;
        let showing = match surfaces.get(active.wrapping_sub(1)) {
            Some(Surface::Browser { key, .. }) => Some(key.clone()),
            _ => None,
        };
        let nav = showing.as_deref().and_then(|key| {
            let spec = caps.nav_of(key)?;
            let w = where_now.as_ref().filter(|w| w.0 == key);
            Some(crate::uistate::NavState {
                back: spec.back,
                forward: spec.forward,
                reload: spec.reload,
                reload_hard: spec.reload_hard,
                edit: spec.url,
                point: spec.point,
                can_back: w.is_some_and(|w| w.2),
                can_forward: w.is_some_and(|w| w.3),
                at: w.map(|w| w.1.clone()).unwrap_or_default(),
                // Lit while loading, or if it started less than 0.5s ago (covers instantaneous requests)
                loading: loading_now.get(key).is_some_and(|(busy, since)| {
                    *busy || since.elapsed() < std::time::Duration::from_millis(500)
                }),
            })
        });
        // Only the window knows the current location. Ask at a reasonable interval,
        // and only while the controls are shown. Pages returned to via history don't
        // always announce a load, so relying on "ask when it loads" alone would leave
        // the back button stale.
        if let (Some(key), true) = (
            &showing,
            nav.is_some() && drawn_ms.saturating_sub(asked_where_ms) >= WHERE_EVERY_MS,
        ) {
            asked_where_ms = drawn_ms;
            let _ = caps.browser_where(key);
        }

        // If the current desk is a discussion, find the opening speaker
        // (first participant) so the dashboard can offer a "start" card.
        let (discuss_start, discuss_start_name) = desks
            .get(desk_index)
            .and_then(|w| {
                let d = w.discuss.as_ref()?;
                if d.agents.iter().filter(|s| !s.trim().is_empty()).count() < 2 {
                    return None;
                }
                let first = d.agents.iter().find(|s| !s.trim().is_empty())?;
                let pane = surface_of_id(w, first)?;
                let name = w
                    .tabs
                    .iter()
                    .find(|t| {
                        t.cfg.id.as_deref() == Some(first.as_str())
                            || t.cfg.name.as_deref() == Some(first.as_str())
                    })
                    .and_then(|t| t.cfg.name.as_deref().filter(|x| !x.is_empty()))
                    .map(str::to_string)
                    .unwrap_or_else(|| first.clone());
                Some((pane, name))
            })
            .map_or((None, None), |(p, n)| (Some(p), Some(n)));
        // The first-run pointer: worked out from what is on screen, and what
        // has been pointed at before. Written down the moment it moves on, so
        // the next start does not point at the same thing twice
        let folder_count = desks
            .get(desk_index)
            .map(|w| w.folders.iter().filter(|f| f.cwd.is_some()).count())
            .unwrap_or(0);
        let past_the_plus = tabs.iter().any(|t| t.is_ai() || t.place.linked);
        let (coach, seen) = coach_step(folder_count, coach_seen, past_the_plus);
        if seen != coach_seen {
            coach_seen = seen;
            let _ = crate::crypto::write_atomic(&config::state_path("coach"), &seen.to_string());
        }
        // Read only while a tab of that AI exists and the setting is on;
        // shown only while such a tab is in view (the page decides that)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let usage = limits
            .iter()
            .filter_map(|(source, meter)| {
                meter.want(ai_usage_on && tabs.iter().any(|t| t.ai_kind().as_deref() == Some(source.key())));
                let l = meter.current()?;
                Some((source.key().to_string(), crate::uistate::UsageState::of(source.name(), &l, now)))
            })
            .collect();
        // Where the view has come to rest, asked once, after everything that
        // could have moved it. A tab somebody asked for brings its folder back
        // -- out of sight is not out of reach, and typing into a tab nobody can
        // see is the outcome this exists to prevent. A view merely put on one
        // of those rows steps off instead, and the folder stays away
        let drifted = std::mem::take(&mut view_drifted);
        if !folders_hidden.is_empty() {
            match crate::view::settle(drifted, active, &surfaces, &tabs, &folders_hidden) {
                crate::view::Settled::Stay => {}
                crate::view::Settled::Bring(dir) => {
                    folders_hidden.retain(|h| !crate::uistate::same_folder(h, &dir));
                }
                crate::view::Settled::Show(n) => {
                    active = n;
                    pane_layout.show(active);
                }
                crate::view::Settled::Board => {
                    active = 0;
                    board_open = true;
                }
            }
        }
        view_settled_at = active;
        let ui = Ui {
            ais: ai_choices.clone(),
            split_open: open_split.clone(),
            // What is still being typed into a tab, so the composer can say so
            // rather than emptying and leaving the person guessing. Taken from
            // the sends themselves, here, where they are: a second tally kept
            // alongside them would be a second thing to get wrong
            sending: pending_send
                .iter()
                .map(|p| {
                    let (share, chars) = p.sending();
                    (p.tab, crate::uistate::SendingState { share, chars })
                })
                .collect(),
            past: past_view.clone(),
            // The pointer waits behind the setup: it points at the list, and
            // the setup is in front of the list
            coach: coach.filter(|_| setup_view.is_none()),
            discard_unasked: cfg.as_ref().is_some_and(|c| c.confirm_worktree_delete == Some(false)),
            setup: setup_view.clone(),
            add_project: add_view.clone(),
            worktrees_kept: worktrees_kept.clone(),
            folders_hidden: folders_hidden.clone(),
            making: makings
                .iter()
                .map(Pending::state)
                .chain(leavings.iter().map(Leaving::state))
                .chain(vm_jobs.iter().map(VmJob::state))
                .collect(),
            hosts: cfg
                .as_ref()
                .map(|c| {
                    c.hosts
                        .iter()
                        .filter(|h| !h.name.trim().is_empty())
                        .map(|h| crate::uistate::HostChoice {
                            name: h.name.clone(),
                            at: h.at.clone(),
                            kind: if h.is_made() { "microvm" } else { "ssh" }.into(),
                            project: desks
                                .get(desk_index)
                                .and_then(|d| d.projects.iter().rev().find_map(|p| p.home_on(&h.name)))
                                .map(|home| home.at.clone())
                                .unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ssh_aliases: ssh_aliases.clone(),
            remote_list: remote_view.clone(),
            far_ports: far_ports_view.clone(),
            login_step: login_view.clone(),
            machine_ais: machine_ais.clone(),
            project_home: project_home.clone(),
            assistant: assistant_ai.clone(),
            git_accounts: desks
                .get(desk_index)
                .map(|d| {
                    d.git_accounts
                        .iter()
                        .map(|a| crate::uistate::GitAccountChoice {
                            name: a.name.clone(),
                            label: a.label.as_deref().map(str::trim).filter(|l| !l.is_empty()).unwrap_or(&a.name).to_string(),
                            owners: a.owners.iter().map(|o| o.trim().to_ascii_lowercase()).collect(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            usage,
            thanks: thanks_show.then(|| thanks_kind.to_string()),
            update: update::ask(),
            close_ask: close_ask.clone(),
            closed: desks.get(desk_index).map(|d| closed_tabs.shown(&d.name)).unwrap_or_default(),
            quick: quick_view.clone(),
            // Where each kind of button would go right now, for the launcher
            // to say so before anything is pressed. Only the kinds the buttons
            // actually are, and asked of the same function the press asks
            quick_to: quick_view
                .dests
                .iter()
                .map(|key| {
                    let (kind, ai) = match key.strip_prefix("ai:") {
                        Some(ai) => (crate::quick::Kind::Ai, ai),
                        None => (crate::quick::Kind::Terminal, ""),
                    };
                    let go = quick_go(kind, ai, &surfaces, &tabs, active, board_open || settings_open,
                                      &ai_choices, home.as_deref(), desks.get(desk_index));
                    (key.clone(), quick_dest(&go))
                })
                .collect(),
            first_run,
            settings_gen,
            // Any desk: a phone registers itself once, for whichever desk
            // sends to it
            push_wanted: desks
                .iter()
                .flat_map(|d| d.notify.values())
                .any(|d| matches!(d, notify::Destination::Phone {})),
            asking_why,
            ask_why_by: crate::webui::assistant_ai(
                cfg.as_ref().and_then(|c| c.ai_engine.as_deref()),
            )
            .map(|(_, label)| label.to_string())
            .unwrap_or_default(),
            active,
            board: board_open,
            settings: settings_open,
            settings_float: settings_place.floats(),
            // The flag itself, engine or no engine. It used to be sent only
            // while a Lua engine existed, which left the bar saying AUTO ON
            // after an emergency stop in a desk with no automation of
            // its own -- and the stop still means something there: it is
            // what interrupted the AIs, and what keeps a hand-over from
            // starting until it is turned back on
            auto: Some(auto_enabled),
            desk_names: desks.iter().map(|w| w.name.clone()).collect(),
            desk_ids: desks.iter().map(|w| w.id.clone()).collect(),
            desk_index,
            desk_open,
            help_open,
            // Only worth carrying while it is on screen; it is the same list
            // every frame otherwise
            help_rows: match help_open {
                true => keymap
                    .help_rows()
                    .into_iter()
                    .map(|(k, d)| (k, d.to_string()))
                    .collect(),
                false => Vec::new(),
            },
            vault: vault_view.clone(),
            branch: branch_view.clone(),
            repair: repair_view.clone(),
            browse: browse_view.clone(),
            folder_colors: cfg
                .as_ref()
                .map(|c| c.folder_colors.clone())
                .unwrap_or_default(),
            folders: desks
                .get(desk_index)
                .map(|w| {
                    w.folders
                        .iter()
                        .filter_map(|f| {
                            f.cwd.clone().map(|c| (c, f.name.clone().unwrap_or_default()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            folder_projects: desks
                .get(desk_index)
                .map(|w| {
                    w.folders
                        .iter()
                        .filter_map(|f| f.cwd.clone().zip(f.project.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            folder_items: desks
                .get(desk_index)
                .map(|w| w.folders.iter().filter_map(|f| f.cwd.clone().zip(f.work_item.clone())).collect())
                .unwrap_or_default(),
            folder_labels: desks
                .get(desk_index)
                .map(|w| {
                    w.folders
                        .iter()
                        .filter_map(|f| {
                            f.cwd.clone().map(|folder| crate::uistate::FolderLabel {
                                folder,
                                summary: f.summary.clone(),
                                auto: f.auto_label,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default(),
            drafts: {
                // Only for a while: the words are for the first look at a tab
                // made a moment ago, not for every time it is opened after
                pending_drafts.retain(|(_, _, at)| at.elapsed() < Duration::from_secs(30 * 60));
                pending_drafts.iter().map(|(f, d, _)| (f.clone(), d.clone())).collect()
            },
            git_repos: git_repos.clone(),
            folders_elsewhere: desks
                .get(desk_index)
                .map(|w| {
                    w.folders
                        .iter()
                        .filter(|f| f.host.is_some())
                        .filter_map(|f| f.cwd.clone())
                        .collect()
                })
                .unwrap_or_default(),
            folder_hosts: desks
                .get(desk_index)
                .map(|w| {
                    w.folders
                        .iter()
                        .filter_map(|f| f.cwd.clone().zip(f.host.as_ref().map(|h| h.name.clone())))
                        .collect()
                })
                .unwrap_or_default(),
            folder_far: desks
                .get(desk_index)
                .map(|w| {
                    w.folders
                        .iter()
                        .filter_map(|f| {
                            let (cwd, host) = (f.cwd.clone()?, f.host.as_ref()?);
                            let home = f
                                .project
                                .as_deref()
                                .and_then(|n| w.projects.iter().find(|p| p.name == n))
                                .and_then(|p| p.home_on(&host.name))?;
                            let checkout = home.at.trim_end_matches('/');
                            let linked = cwd.to_string_lossy().trim_end_matches('/') != checkout;
                            Some((cwd, crate::uistate::far_family(&host.name, checkout), linked))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            folder_machines: desks
                .get(desk_index)
                .map(|w| {
                    w.folders
                        .iter()
                        .filter_map(|f| {
                            // A server by who and where; a MicroVM by the
                            // entry's name, which is what its folders share --
                            // each worktree is a machine of its own, and a mark
                            // is for the place they all are
                            let machine = match crate::elsewhere::Elsewhere::of(f.host.as_ref()?).ok()? {
                                crate::elsewhere::Elsewhere::Ssh(spec) => spec.machine(),
                                crate::elsewhere::Elsewhere::Cloud(h) => format!("microvm:{}", h.name),
                            };
                            f.cwd.clone().zip(Some(machine))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            server_marks: cfg.as_ref().map(|c| c.server_marks.clone()).unwrap_or_default(),
            self_cost: self_cost.clone(),
            // With a stand-in laid out there is a link to show even when
            // nothing is listening — that is the whole point of it (netaddr::demo_link)
            qr: if qr_open {
                remote_ui
                    .as_ref()
                    .map(|r| r.url.clone())
                    .or_else(netaddr::demo_link)
            } else {
                None
            },
            // A board put up for this machine's own window is not remote
            // access, and the badge that says so must not claim it is: a
            // person reading REMOTE believes their board is reachable from
            // the network, and this one is not
            remote_on: remote_ui.as_ref().is_some_and(|r| !r.local_only),
            remote_conn: remote_ui.as_ref().is_some_and(|r| r.has_state_clients()),
            remote_sticky: cfg.as_ref().is_some_and(|c| c.remote.sticky_token),
            aim: aim_of(desks.get(desk_index), &surfaces, &tabs, active),
            nav,
            asks: caps.asks_now(),
            away: caps.drawn_away(),
            // Worked out from the same settings a run is started from, so the
            // board's "choose a model first" and the refusal cannot disagree
            words_unset: desks
                .get(desk_index)
                .map(|w| {
                    surfaces
                        .iter()
                        .filter_map(|s| match s {
                            Surface::Browser { key, .. } => Some(key.clone()),
                            _ => None,
                        })
                        .filter(|k| !w.words_models(cfg.as_ref(), Some(k)).complete())
                        .collect()
                })
                .unwrap_or_default(),
            // The same settings again, asked who picks each page's moves
            words_fast: desks
                .get(desk_index)
                .map(|w| {
                    surfaces
                        .iter()
                        .filter_map(|s| match s {
                            Surface::Browser { key, .. } => Some(key.clone()),
                            _ => None,
                        })
                        .filter(|k| {
                            w.words_models(cfg.as_ref(), Some(k))
                                .choose()
                                .is_some_and(|n| crate::bridge::decides_fast(&n))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            scrolled: session_at(&surfaces, active)
                .and_then(|i| tabs.get(i))
                .map(|t| {
                    t.parser
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .screen()
                        .scrollback()
                })
                .unwrap_or(0),
            ball,
            max_chain,
            now_ms: start.elapsed().as_millis() as u64,
            surfaces: surfaces.clone(),
            // What the disk says about each open file, as of now. One metadata
            // call per open editor per pass -- cheap enough to ask every time,
            // and asking every time is what makes "somebody else wrote this"
            // something the editor notices rather than something it is told
            editors: editors
                .iter()
                .map(|e| {
                    let mut e = e.clone();
                    e.stamp = match (&e.at, &e.dir, &e.showing) {
                        // A file on another machine: what it said last time it
                        // was asked (see `far_stamp_polls`)
                        (Some(_), _, Some(rel)) => far_seen
                            .get(&e.key)
                            .filter(|seen| &seen.path == rel)
                            .map(|seen| seen.stamp.clone()),
                        (None, Some(d), Some(rel)) => local_under(d, rel).map(|at| crate::files::stamp_of(&at)),
                        _ => None,
                    }
                    .filter(|s| !s.is_empty());
                    e
                })
                .collect(),
            layout: pane_layout.clone(),
            // Whether the thing in view can be put back the way it started. A
            // session always can; a page only if we know how it was opened.
            // Decided here so the button the screen draws and the keystroke it
            // stands for can never disagree about where it applies
            restartable: session_at(&surfaces, active).is_some()
                || restartable_page(&surfaces, active, &caps).is_some(),
            discuss_start,
            discuss_start_name,
        };
        if flash != flash_shown {
            flash_shown = flash.clone();
            flash_at = Instant::now();
            // Shown by the screen with `toast(S.flash)`, like every other
            // message; over a page, the screen hands it to the page to draw
            // (Ev::PageToast below). Said once, there, for every message
        }
        // Messages the screen could not draw over the page in view: drawn by
        // that page. A page placed in the focused pane is a window of its own,
        // and anything of ours stands behind it (or, in a split, is cut off at
        // the pane's edge)
        for (text, warn) in shell.mail().page_toasts.drain(..).collect::<Vec<_>>() {
            if let Some(key) = focused_page(&pane_layout, &ui.surfaces) {
                let _ = caps.browser_toast(&key, &text, warn);
            }
        }
        // Comfortably longer than the longest the screen shows one for, so the
        // page is what decides when a message fades and this only clears up after it
        if flash.is_some() && flash_at.elapsed() >= FLASH_LIFE {
            flash = None;
            flash_shown = None;
        }
        shell.draw(&tabs, &ui, flash.as_deref())?;
        // The screen goes to any watching phone or browser here, on every turn
        // of the loop, rather than inside the 200ms state check below.
        //
        // It lived in that check for a long time, which quietly capped a remote
        // screen at five frames a second however generous the rate limit was --
        // and five frames is what scrolling from a phone looked like. Detection
        // is cheap to do slowly; a screen is not.
        if let Some(r) = remote_ui.as_ref()
            && r.has_state_clients() && last_remote_push.elapsed() >= remote_floor(r.max_pending()) {
                let now: Vec<String> = tabs
                    .get(session_at(&surfaces, active).unwrap_or(usize::MAX))
                    .map(|t| {
                        let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                        crate::shell::screen_rows(p.screen())
                    })
                    .unwrap_or_default();
                match screen_push(&last_remote_rows, &now) {
                    ScreenPush::Nothing => {}
                    ScreenPush::Rows(moved) => {
                        let rows = {
                            let list: Vec<(usize, &str)> =
                                moved.iter().map(|&i| (i, now[i].as_str())).collect();
                            serde_json::to_string(&list)
                        };
                        if let Ok(rows) = rows {
                            r.push_state(format!("{{\"rows\":{rows}}}"));
                            last_remote_rows = now;
                            last_remote_push = Instant::now();
                        }
                    }
                    ScreenPush::Whole => {
                        if let Ok(scr) = serde_json::to_string(&now.join("
")) {
                            r.push_state(format!("{{\"screen_html\":{scr}}}"));
                            last_remote_rows = now;
                            last_remote_push = Instant::now();
                        }
                    }
                }
                // The division of the content area and the panes not in front,
                // to the viewers that draw them. With none of those watching,
                // nothing is built -- and what was said is forgotten, so the
                // first one to arrive is told everything rather than a
                // difference from a picture it never saw
                if r.has_pane_clients() {
                    for m in pane_relay.changes(&pane_layout, &surfaces, &tabs) {
                        r.push_panes(m);
                    }
                } else {
                    pane_relay = PaneRelay::default();
                }
            }
        // The window's size can change. If we don't hand it back over, a placed
        // page stays at its previous size.
        caps.set_area(shell.geom_area());
        // Place only the one currently selected at the terminal content's position.
        // The OS handles minimize and stacking order via ownership, but position
        // still needs to be tracked by us.
        // Move keyboard focus to whatever is currently visible.
        //
        // A page's internal focus (activeElement) and what the OS considers focused
        // are different things. Right after the window is created, the OS side
        // hasn't settled yet — keystrokes arrive, but only the Japanese IME
        // conversion window would show up in the corner of the screen (a telltale
        // sign; moving the window even slightly fixes it). Re-set focus from our
        // side every time what's visible changes.
        {
            let want = match surfaces.get(active.wrapping_sub(1)) {
                Some(Surface::Browser { key, .. }) => Some(key.clone()),
                _ => None,
            };
            if focused.as_ref() != Some(&want) {
                focused = Some(want.clone());
                match &want {
                    Some(name) => {
                        let _ = caps.browser_focus(name);
                    }
                    None => {
                        shell.take_keyboard_back();
                    }
                }
            }
        }
        // Focus follows a click on a pane, the way it follows a click in the
        // tab bar. `active` moves with it so every existing path stays right.
        for id in shell.mail().take_focus_panes() {
            if pane_layout.focus_pane(id) {
                active = pane_layout.focused_surface();
                view_touched_ms = start.elapsed().as_millis() as u64;
            }
        }
        // The pen over a placed page is drawn by that page: nothing of ours can
        // be stacked above a window of its own. Only one page ever shows it --
        // the one in the focused pane, and only while the composer is shut --
        // so this names that page and turns the previous one off. Recomputed
        // rather than told, since focus moves for reasons the page never hears
        if let Some(on) = shell.mail().take_pen() {
            composer_shut = on;
        }
        let wants_pen = composer_shut
            .then(|| focused_page(&pane_layout, &ui.surfaces))
            .flatten();
        if wants_pen != pen_shown {
            if let Some(old) = pen_shown.take() {
                let _ = caps.browser_pen(&old, false);
            }
            if let Some(new) = wants_pen.clone() {
                let _ = caps.browser_pen(&new, true);
            }
            pen_shown = wants_pen;
        }

        // A pane asked for a tab and the form has produced one. It is the row
        // that was not on the list when the pane asked, so the pane that asked
        // takes that row, and asks for nothing more
        // The form is a surface too, from the moment it opens. It is not the
        // answer to the question -- it IS the question -- so it is left out of
        // both lists below. Counting it made the baseline move under its own
        // feet: it was already there when the wait began and gone again by the
        // time the tab arrived, so the total came back to where it started and
        // the new tab looked like nothing new
        let is_form = |n: usize| {
            matches!(ui_surface_at(&surfaces, n), Some(Surface::Browser { key, .. })
                if key == SETTINGS_TAB)
        };
        // Every row on the list, by what it is rather than where it stands
        // (see `arrived_row`)
        let rows_now: Vec<(usize, String)> = (1..=surface_count)
            .filter(|n| !is_form(*n))
            .filter_map(|n| ui_surface_at(&surfaces, n).map(|s| (n, surface_key(s, &tabs))))
            .collect();
        if let Some(id) = shell.mail().take_add_tab_pane() {
            awaiting_tab = Some((id, rows_now.iter().map(|(_, k)| k.clone()).collect()));
        }
        // Nothing calls the wait off. Not the form closing -- saving CLOSES it,
        // and the tab it wrote does not exist until the settings file has been
        // read back, so ending the wait there meant the tab arrived to find
        // nobody waiting. Not the pane filling up either: it fills with the
        // form itself for as long as that is open, and reading that as "filled"
        // ended the wait one frame after it began. A wait that is never
        // answered simply never fires, and an empty pane stays empty, which is
        // exactly what it was before anybody asked
        let arrived = awaiting_tab
            .as_ref()
            .and_then(|(id, was)| arrived_row(was, &rows_now).map(|n| (*id, n)));
        if let Some((id, fresh)) = arrived {
            pane_layout.set_surface(id, fresh);
            // Made here, so the keyboard belongs here
            pane_layout.focus_pane(id);
            active = pane_layout.focused_surface();
            board_open = false;
            awaiting_tab = None;
            // The form was opened to make this one tab, and it has. Leaving
            // it up would leave it sitting in the pane the tab was made for
            let _ = caps.browser_close(SETTINGS_TAB);
            settings_open = false;
        }

        // Someone clicked into a page placed in the window. That press never
        // reaches the pane underneath -- the page is a window of its own -- so
        // the pane it sits in is focused from the page's own report instead.
        // Without this a browser pane could only be entered by its caption
        for child in shell.mail().take_touches() {
            let Some(key) = caps.name_of_child(&child) else {
                continue;
            };
            let at = ui.surfaces.iter().position(
                |s| matches!(s, Surface::Browser { key: k, .. } if *k == key),
            );
            let Some(pane) = at.and_then(|i| pane_layout.pane_of(i + 1)) else {
                continue;
            };
            if pane_layout.focus_pane(pane) {
                active = pane_layout.focused_surface();
                view_touched_ms = start.elapsed().as_millis() as u64;
            }
        }
        for (divider, ratio) in shell.mail().take_pane_ratios() {
            pane_layout.set_divider(divider, ratio);
        }
        // The terminal was zoomed. The page has already redrawn itself; this
        // is only so it opens that size next time. Written on a delay because
        // a wheel sends a notch at a time and a settings file is not a place
        // to write sixty times a second
        if let Some(px) = shell.mail().take_font_size() {
            font_size = Some(px);
            font_save_at = Some(std::time::Instant::now() + Duration::from_secs(2));
        }
        if font_save_at.is_some_and(|at| std::time::Instant::now() >= at) {
            font_save_at = None;
            if let Some(px) = font_size.take() {
                config::save_appearance("font_size", serde_json::json!(px));
                // Our own write is not news. Without this the watcher sees the
                // settings change and announces a reload, which is a strange
                // thing to be told by a window you just zoomed
                watcher.retarget(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
                append_hook_log(&format!("terminal font size is now {px}"));
            }
        }
        // The tab bar was dragged to a new width, or put away. Held back the
        // same way and for the same reason: a drag is a stream of widths, and
        // a settings file is not a place to write one per frame
        if let Some(px) = shell.mail().take_tab_width() {
            tab_width = Some(config::clamp_tab_bar(px));
            tab_save_at = Some(std::time::Instant::now() + Duration::from_secs(2));
        }
        if tab_save_at.is_some_and(|at| std::time::Instant::now() >= at) {
            tab_save_at = None;
            if let Some(px) = tab_width.take() {
                config::save_setting(&["tab_bar_width"], serde_json::json!(px));
                watcher.retarget(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
                append_hook_log(&if px == 0 {
                    "the tab bar is put away".to_string()
                } else {
                    format!("the tab bar is now {px}px wide")
                });
            }
        }
        // The right-hand column, held back the same way and for the same reason
        if let Some(px) = shell.mail().take_side_width() {
            side_width = Some(config::clamp_side_bar(px));
            side_save_at = Some(std::time::Instant::now() + Duration::from_secs(2));
        }
        if side_save_at.is_some_and(|at| std::time::Instant::now() >= at) {
            side_save_at = None;
            if let Some(px) = side_width.take() {
                config::save_setting(&["side_bar_width"], serde_json::json!(px));
                watcher.retarget(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
                append_hook_log(&if px == 0 {
                    "the side column is put away".to_string()
                } else {
                    format!("the side column is now {px}px wide")
                });
            }
        }

        // ⊞ / ⊟ in a pane's caption. Divides that pane, not whichever one had
        // focus: the button is attached to a pane, so it must mean that one
        for (id, down) in std::mem::take(&mut lua_splits) {
            shell.mail().pane_splits.push((id, down));
        }
        for id in std::mem::take(&mut lua_shuts) {
            shell.mail().close_panes.push(id);
        }
        for (id, down) in shell.mail().take_pane_splits() {
            if !pane_layout.focus_pane(id) {
                continue;
            }
            let dir = if down { layout::Dir::Col } else { layout::Dir::Row };
            // Inside a split, it divides that split. Outside one it makes a
            // split: a row of its own in this row's folder, holding what was
            // in front and an empty pane beside it. That row is where the
            // arrangement lives from then on -- it has a name, a place in a
            // list, and a ✕ that takes it away again
            match open_split.is_some() {
                true => {
                    pane_layout.split(dir, 0);
                    active = pane_layout.focused_surface();
                }
                // Nothing is in front -- the board itself is, or the last
                // row went. A split of nothing is nothing: two empty panes,
                // in no folder, under a row nobody asked for. Pressed from
                // there it says so instead
                false if active == 0 => flash = Some(i18n::t("msg.split.nothing")),
                false => {
                    // Empty, not filled with whatever row happens to come
                    // next. Pulling in the next one was how a split in one
                    // folder came up showing another folder's tabs, and
                    // nobody had asked for either of them
                    let mut made = crate::layout::Layout::single(active);
                    made.split(dir, 0);
                    let keyed = surface_keys(&surfaces, &tabs);
                    let panes = made.keep(|s| keyed.get(s - 1).and_then(|k| k.id.clone()));
                    let here = surface_folder(&surfaces, &tabs, active).map(|d| d.to_path_buf());
                    let desk_name = desks.get(desk_index).map(|d| d.name.clone()).unwrap_or_default();
                    // Named here rather than left to the write, because this
                    // row is gone to the moment it arrives and the way to go
                    // to a row is by the name automation calls it
                    let taken = std::fs::read_to_string(config::config_file_path())
                        .ok()
                        .and_then(|t| serde_json::from_str::<serde_json::Value>(config::without_bom(&t)).ok())
                        .map(|v| config::tab_ids_in(&v))
                        .unwrap_or_default();
                    let id = config::pet_id(&taken);
                    let row = serde_json::json!({
                        "name": i18n::t("tui.split.name"),
                        "id": id,
                        "command": "split",
                        "panes": panes,
                    });
                    if config::append_tab(&desk_name, row, here.as_deref()) {
                        // It arrives with the settings this write sets off
                        reveal = Some((id, Instant::now() + Duration::from_secs(20)));
                    } else {
                        flash = Some(i18n::t("msg.split.not_written"));
                    }
                }
            }
            view_touched_ms = start.elapsed().as_millis() as u64;
        }
        // ↻ / ⟲ in a pane's caption. Focus moves to that pane first, and not
        // as a side effect: the restart itself, the engine's cancel and the
        // "is anyone else running this CLI here" test are all written in terms
        // of the focused surface, and moving there is how the button means the
        // pane it is drawn on rather than the pane you happened to be in
        for (id, keep) in shell.mail().take_restart_panes() {
            if !pane_layout.focus_pane(id) {
                continue;
            }
            active = pane_layout.focused_surface();
            view_touched_ms = start.elapsed().as_millis() as u64;
            if let Some(msg) = retry_failed(active, &surfaces, &mut tabs, desks.get(desk_index), rows, cols, Some(&last_session))
                .or_else(|| restart_surface(active, keep, &mut tabs, &surfaces, &mut engine, &caps, rows, cols))
            {
                flash = Some(msg);
            }
        }
        for id in shell.mail().take_close_panes() {
            let closed = pane_layout.close(id);
            if closed {
                active = pane_layout.focused_surface();
                view_touched_ms = start.elapsed().as_millis() as u64;
            }
            // A split is two panes or more. Down to one it is a row pretending
            // to be an arrangement: pressing it puts you on the one thing it
            // holds, which is where pressing that thing's own row already puts
            // you, so it looks like a row that cannot be pressed at all.
            //
            // So the row goes as soon as there is nothing left to divide --
            // whether the last ✕ took the second pane (`closed`) or was aimed
            // at the only one (`!closed`). Taking it away ends nothing: what it
            // was showing goes on standing in its own folder, which is the
            // whole point of the row. Through the same door that row's own ✕
            // goes through, so it leaves the settings in one place and not two
            let alone = pane_layout.is_single();
            if let Some(key) = open_split.clone().filter(|_| alone)
                && let Some(at) = surfaces
                    .iter()
                    .position(|s| matches!(s, Surface::Split { key: k, .. } if *k == key))
                    .map(|i| i + 1)
            {
                let gone = surface_key(&surfaces[at - 1], &tabs);
                shell.mail().close_tabs.push((at, gone, true));
                splits.forget(&key);
                split_save = None;
                split_written = None;
                open_split = None;
                // What it was showing is in front now, on its own. Left to the
                // row going, the screen would first flash whatever the list
                // put in its place
                active = pane_layout.focused_surface();
                pane_layout = crate::layout::Layout::single(active);
                view_touched_ms = start.elapsed().as_millis() as u64;
            } else if !closed {
                flash = Some(i18n::t("msg.pane_last"));
            }
        }
        // A tab's ✕, its middle click, the key, the phone: one door for all of
        // them, so none of them can close a working AI without the question.
        // The rows are worked out again here because a reload earlier in this
        // pass may have changed them; the key a press carries is checked
        // against these
        let closing = shell.mail().take_close_tabs();
        if !closing.is_empty() {
            let titles: Vec<&str> = tabs.iter().map(|t| t.title.as_str()).collect();
            let rows = surfaces_written(desks.get(desk_index), &titles, &caps.hosted_names(), &editors, issues_open);
            for (at, key, sure) in closing {
                close_asked += 1;
                match crate::closed::close(
                    at,
                    &key,
                    sure,
                    &rows,
                    &tabs,
                    desks.get(desk_index),
                    &caps,
                    &mut closed_tabs,
                    close_asked,
                ) {
                    crate::closed::Closing::Nothing => {}
                    crate::closed::Closing::Ask(ask) => close_ask = Some(ask),
                    crate::closed::Closing::Closed { note, settings, ends } => {
                        close_ask = None;
                        match ends {
                            crate::closed::Ends::Tab(serial) => ending.push(serial),
                            crate::closed::Ends::Editor(key) => editors.retain(|e| e.key != key),
                            crate::closed::Ends::Nothing => {}
                        }
                        if settings {
                            // The reload that takes the line out says this
                            // instead of "settings reloaded"
                            said_before_reload = Some((Instant::now(), note));
                            watcher.poke();
                        } else {
                            flash = Some(note);
                        }
                    }
                    crate::closed::Closing::Failed(why) => {
                        close_ask = None;
                        flash = Some(why);
                    }
                }
            }
        }
        if shell.mail().take_close_tab_back() {
            close_ask = None;
        }
        for which in shell.mail().take_reopen_tabs() {
            let Some(d) = desks.get(desk_index) else { continue };
            match crate::closed::reopen(which, d, &caps, &mut closed_tabs) {
                crate::closed::Reopening::Reopened { note, settings, reveal: name, resume } => {
                    if let Some((id, s)) = resume {
                        resume_for.insert(id, s);
                    }
                    reveal = Some((name, Instant::now() + Duration::from_secs(10)));
                    if settings {
                        said_before_reload = Some((Instant::now(), note));
                        watcher.poke();
                    } else {
                        flash = Some(note);
                    }
                }
                crate::closed::Reopening::Nothing(why) | crate::closed::Reopening::Failed(why) => {
                    flash = Some(why);
                }
            }
        }
        // Place every browser that has a pane, at that pane's rectangle.
        // Collapsed to nothing when it has no pane — the page stays alive, so
        // coming back to it doesn't reload it.
        {
            let geom = &shell.geom_panes();
            // An overlay is drawn by the page, and a browser is not: it is a
            // window of its own living inside ours, and no amount of stacking
            // puts a drawn thing over it. So while something is being shown
            // over the screen, the browsers step aside. They keep their pages;
            // being given no rectangle is all that happens to them
            // The question a tab's ✕ asks is one of those things
            let covered = help_open || desk_open || qr_open || page_covered || close_ask.is_some();
            // The settings form is a screen, not a pane: it covers the content
            // area and the layout waits underneath. It asks about the whole
            // app, so seating it in one corner of the app made as little sense
            // as seating the board there -- and once the panes were hidden to
            // let it cover, the pane it was sitting in had no size to give it
            // Nothing is placed in a rectangle with no size: before the page
            // has measured itself there is no window to cover, and a page
            // given nothing is a page nobody can find again
            let room = shell.geom_full().2 > 0 && shell.geom_full().3 > 0;
            // The panel floats over whatever is underneath, so it is placed
            // with them rather than instead of them. Hidden by the same things
            // that hide a placed page: a dialog the board drew is drawn by the
            // board, and a page has no way to be under it
            let panel: Vec<(String, (i32, i32, i32, i32))> = match guide_open && !covered && room {
                true => vec![(GUIDE_TAB.to_string(), guide_at.rect(shell.geom_full()))],
                false => Vec::new(),
            };
            if settings_open && !covered && room {
                let full = shell.geom_full();
                let at = settings_place.rect(full);
                caps.show_at(&[vec![(SETTINGS_TAB.to_string(), at)], panel].concat());
            } else {
            let shown: Vec<(String, (i32, i32, i32, i32))> = pane_layout
                .leaves()
                .into_iter()
                .filter_map(|(id, s)| {
                    let Some(Surface::Browser { key, .. }) = surfaces.get(s.wrapping_sub(1)) else {
                        return None;
                    };
                    // Before the page has measured anything (the very first
                    // frames), the whole content area is the only rectangle we
                    // know, and it is the right one while undivided.
                    let rect = geom
                        .iter()
                        .find(|g| g.id == id)
                        .map(|g| g.rect)
                        .unwrap_or(shell.geom_area());
                    Some((key.clone(), rect))
                })
                // INDEX covers the panes. The board keeps their last
                // measurements while it does (a covered pane has not changed
                // size), so a page asked for its rectangle there would stand
                // over INDEX where its pane was, and take the presses meant
                // for the board
                .filter(|_| !covered && !board_open)
                .collect();
            caps.show_at(&[shown, panel].concat());
            }
        }
        // Hand off that the bar's button was pressed. The board (or the phone)
        // names the page the bar stands under, by the name automation gives it;
        // the bar is only ever drawn for the desk in view, so that name is
        // this desk's
        for name in shell.mail().take_presses() {
            caps.note_press(&name);
            append_hook_log(&format!("Bar pressed {name}"));
            if !auto_enabled {
                flash = Some(i18n::t("msg.press_auto_off"));
                continue;
            }
            let Some((eng, page)) = engine
                .as_mut()
                .zip(page_ctx(&surfaces, &name, String::new(), true))
            else {
                continue;
            };
            // Showing a pressable-looking control with nothing to receive it just
            // looks broken.
            if !eng.has_page_hook("on_press", page.index) {
                flash = Some(i18n::tp("msg.press_nowhere", &[("name", &page.name)]));
                append_hook_log("Not doing anything, since no on_press is written");
                continue;
            }
            eng.fire_page("on_press", &page);
        }
        // The wheel was scrolled. Only the visible tab moves.
        for (by, row, col) in shell.mail().take_scrolls() {
            if by == 0 {
                continue;
            }
            if let Some(t) = session_at(&surfaces, active).and_then(|i| tabs.get(i)) {
                scroll_by(t, by, row, col);
                // If the screen jumps while scrolling back, you lose track of what you were reading
                view_touched_ms = start.elapsed().as_millis() as u64;
            }
        }

        // Lines a person finished in the composer or the topic box, each for
        // the tab it names.
        for (tab, line) in shell.mail().take_says() {
            let now_ms = start.elapsed().as_millis() as u64;
            let to = if tab == 0 { active } else { tab };
            if !hand_line(&mut tabs, &surfaces, to, line, now_ms, &mut pending_send, &mut ball) {
                append_hook_log(&format!("say went nowhere: tab{to} is not a session"));
            }
        }

        // Something the page draws over everything went up or down (see `page_covered`)
        if let Some(on) = shell.mail().take_covered() {
            page_covered = on;
        }
        // Quick commands pressed, on the window or the phone. Each opens a tab
        // of its own (`quick_go`); what it sends is read from the settings here
        // -- the press only named it -- and any secret it names is put in now,
        // through the door a script's secrets go through: the desk on screen,
        // and open to whoever receives it. A shell is the person's own hands;
        // an AI is an AI reading it. A refusal about a secret is said by that
        // door (`caps::take_refusal`)
        for (id, _tab) in shell.mail().take_quicks() {
            let spec = crate::quick::arrange(&cfg.as_ref().map(|c| c.quick_commands.clone()).unwrap_or_default());
            // A folder is opened on the page and sends nothing; its id arriving
            // here, or one that is gone, is a page out of step with the settings
            let Some(item) = spec.find(&id).filter(|i| i.kind != crate::quick::Kind::Folder).cloned() else {
                flash = Some(i18n::t("msg.quick.gone"));
                continue;
            };
            let label = item.label.clone();
            if item.body.trim().is_empty() {
                flash = Some(i18n::tp("msg.quick.empty", &[("label", &label)]));
                continue;
            }
            let go = quick_go(
                item.kind, &item.ai, &surfaces, &tabs, active, board_open || settings_open,
                &ai_choices, home.as_deref(), desks.get(desk_index),
            );
            let QuickGo::Open { cwd, command, program } = go else {
                if let QuickGo::Refuse(why) = go {
                    append_hook_log(&format!("quick command \"{label}\" not sent: {why}"));
                    flash = Some(i18n::tp("msg.quick.refused", &[("label", &label), ("why", &i18n::t(why))]));
                }
                continue;
            };
            // Whoever receives it is who a secret in it is for: an AI tab is an
            // AI reading it, a shell is the person's own
            let who = match item.kind {
                crate::quick::Kind::Ai => grants::Subject::Ai,
                _ => grants::Subject::Human,
            };
            // Put in any secret it names. A refusal is said by the door itself
            // (`caps::take_refusal`), in the same words a script would get
            let text = match crate::quick::expand(&item.body, |name| {
                caps.script_secret(name, who).map(|(value, _)| value)
            }) {
                Ok(text) => text,
                Err(e) => {
                    append_hook_log(&format!("quick command \"{label}\" not sent: {e}"));
                    continue;
                }
            };
            // The log says the button and what it was written as. The text
            // that goes out may hold a secret's value, so that is never logged
            let Some(desk) = desks.get(desk_index).map(|d| d.name.clone()) else { continue };
            match open_and_say(&desk, &cwd, &command, &program, &label, text, item.enter, &tabs, &mut pending_quicks, &mut reveal) {
                Some(title) => {
                    append_hook_log(&format!(
                        "quick command \"{label}\" -> new tab \"{title}\" in {}: {}",
                        cwd.display(),
                        log_excerpt(&item.body, 120)
                    ));
                    watcher.poke();
                }
                None => flash = Some(i18n::tp("msg.quick.open_failed", &[("label", &label)])),
            }
        }
        // Lines waiting for a tab a quick command opened: handed over once
        // the program in it has started and is holding still, the same "ready"
        // a startup hook waits for. A tab that never comes, or never settles,
        // is said rather than waited on for ever
        if !pending_quicks.is_empty() {
            let now_ms = start.elapsed().as_millis() as u64;
            let mut still = Vec::new();
            for p in std::mem::take(&mut pending_quicks) {
                let at = (1..=surfaces.len()).find(|&s| {
                    session_at(&surfaces, s)
                        .and_then(|i| tabs.get(i))
                        .is_some_and(|t| t.id.as_deref() == Some(p.id.as_str()))
                });
                let ready = at
                    .and_then(|s| session_at(&surfaces, s).and_then(|i| tabs.get(i)))
                    .is_some_and(|t| quick_ready(t, now_ms));
                match (at, ready) {
                    (Some(s), true) => {
                        hand_over(&mut tabs, &surfaces, s, p.text, p.submit, now_ms, &mut pending_send, &mut ball);
                    }
                    _ if Instant::now() > p.until => {
                        flash = Some(i18n::tp("msg.quick.never_ready", &[("label", &p.label)]));
                    }
                    _ => still.push(p),
                }
            }
            pending_quicks = still;
        }

        // Lua quick-actions tapped in the bar: look up the code (kept server-side)
        // by where the action stands, folders included, and run it against the
        // active tab. Its commands drain with the hooks'.
        for path in shell.mail().take_run_actions() {
            let Some(code) = cfg
                .as_ref()
                .and_then(|c| config::action_at(&c.actions, &path))
                .filter(|a| a.lua)
                .map(|a| a.body.clone())
            else {
                continue;
            };
            // Run with the active tab as context — a session tab, or a browser
            // (so a Lua action can drive the browser it's shown over). INDEX and
            // settings have no action context, so drop it there.
            let ctx = match session_at(&surfaces, active).and_then(|i| tabs.get(i)) {
                Some(t) => tab_ctx(t, active),
                None => match surfaces.get(active.wrapping_sub(1)) {
                    Some(Surface::Browser { key, .. }) => browser_ctx(active, key),
                    _ => continue,
                },
            };
            // A desk with no automation of its own has no engine until one
            // is needed; a quick action is such a need, as a words run is
            if engine.is_none() {
                engine = crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)).ok();
            }
            if let Some(eng) = engine.as_mut() {
                eng.fire_action(&code, &ctx);
            }
        }

        // 📼 record-mode toggles: arm the shown browser's recorder (off silences
        // recording everywhere — caps keeps it to one recorder at a time).
        for on in shell.mail().take_record_arms() {
            if let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) {
                let _ = caps.browser_record(key, on);
            } else if !on {
                // "Off" must land even when the browser tab is no longer shown
                // (e.g. the tab switch that caused it) — it names no page.
                let _ = caps.browser_record("", false);
            }
        }

        // What the git panel asked for.
        //
        // The everyday ones are one primitive call each, made in the person's
        // name: the panel is a screen they opened, so it reaches exactly what
        // their own automation would. The ones that talk to a server are the
        // exception and say so out loud -- they ask the same permission table
        // and then run on a thread, because the engine lives on the main loop
        // and a window cannot wait three minutes on somebody's network.
        // A git account chosen for a project from the worktree dialog, when
        // GitHub could not be read with the one it had: `folder:<path>` names
        // the folder whose project it is. Written into the settings and read
        // back from there like any other change, so the settings screen and
        // the next push agree
        for (panel, account) in shell.mail().take_git_accounts() {
            let Some(desk) = desks.get(desk_index) else { continue };
            let account = account.trim().to_string();
            // Only a name this desk has, or the PC's own git (as one of its
            // accounts or not), or one of GitHub CLI's, or nothing
            if !(account.is_empty()
                || config::pc_choice(&account).is_some()
                || config::gh_choice(&account).is_some()
                || desk.git_accounts.iter().any(|a| a.name == account))
            {
                continue;
            }
            let saved = match panel.strip_prefix("folder:").map(std::path::PathBuf::from) {
                Some(dir) => config::save_git_account(&desk.id, config::GitChoiceAt::Folder(&dir), &account),
                None => false,
            };
            append_hook_log(&format!(
                "git account for {panel}: {} ({})",
                if account.is_empty() { "(none)" } else { &account },
                if saved { "saved" } else { "not saved" }
            ));
            if saved {
                // Looked at again now rather than at the next beat
                place_at = std::time::Instant::now();
                settings_gen += 1;
            } else {
                let js = serde_json::json!({
                    "act": "account", "ok": false, "error": i18n::t("err.git.account.not_saved"),
                })
                .to_string();
                shell.push_git(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"git\":{js}}}"));
                }
            }
        }
        for (panel, act, args) in shell.mail().take_gits() {
            let paths: Vec<String> = args
                .get("paths")
                .and_then(|p| p.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let text = args.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_string();
            if act == "message" {
                // Lua the person can add to or replace entirely. It runs in the
                // engine because `ai_ask` suspends there rather than blocking:
                // the window keeps drawing while the AI thinks
                if engine.is_none() {
                    engine =
                        crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)).ok();
                }
                let Some(eng) = engine.as_mut() else { continue };
                eng.set_states(tab_states(&tabs));
                eng.set_places(places_by_surface(&surfaces, &tabs));
                // The project's, for the folder this panel reports on: a
                // team's rules are its repository's (see Desk::git_of). A
                // panel standing in no folder is told the app's own
                let spec = match (desks.get(desk_index), panel_place_dir(&surfaces, &tabs, &panel)) {
                    (Some(desk), Some(dir)) => desk.git_of(&dir),
                    _ => config::GitSpec::default(),
                };
                let code = spec
                    .message_lua
                    .clone()
                    .filter(|l| !l.trim().is_empty())
                    .unwrap_or_else(|| crate::hooks::COMMIT_MESSAGE_LUA.to_string());
                let _ = eng.call_primitive_as(
                    None,
                    grants::Subject::Human,
                    "set_var",
                    &[serde_json::json!("git_tab"), serde_json::json!(panel)],
                );
                // The whole prompt, as written in the settings (or the default
                // shown there). The name of the AI that will answer goes in
                // where the prompt asks for it; the change goes in inside the Lua
                let ai = cfg.as_ref().and_then(|c| c.ai_engine.clone()).filter(|s| !s.is_empty());
                let prompt = spec
                    .commit_prompt()
                    .replace("{ai}", crate::webui::local_ai_label(ai.as_deref()).unwrap_or("an AI"))
                    // The language the screen is in, named in itself, for a
                    // team whose rule is "the message in our language"
                    .replace("{language}", &i18n::t("lang.self"));
                let _ = eng.call_primitive_as(
                    None,
                    grants::Subject::Human,
                    "set_var",
                    &[serde_json::json!("git_prompt"), serde_json::json!(prompt)],
                );
                shell.push_git(&serde_json::json!({"act": "message", "busy": true}).to_string());
                eng.start_snippet("message", &code);
                continue;
            }
            // The whole git panel as a tab of its own, for what the column keeps
            // out of the way -- the branches, the history. The one this folder
            // already has is brought forward rather than a second one opened
            if act == "open_tab" {
                let place = panel_places(&surfaces)
                    .into_iter()
                    .chain(tab_places(&tabs).into_iter().filter(|p| !p.dir.as_os_str().is_empty()))
                    .find(|p| p.key.matches(&panel));
                let answer = match (place, desks.get(desk_index)) {
                    (None, _) | (_, None) => Err(i18n::t("err.git.no_tab")),
                    (Some(place), Some(desk)) => {
                        let open = surfaces.iter().find_map(|s| match s {
                            Surface::Git { key, dir: Some(d), .. } if crate::uistate::same_folder(d, &place.dir) => Some(key.clone()),
                            _ => None,
                        });
                        match open {
                            Some(key) => {
                                reveal = Some((key, Instant::now() + Duration::from_secs(20)));
                                Ok(serde_json::json!({"already": true}))
                            }
                            None => {
                                let (title, id) = quick_tab_names("Git", "Git", &tabs);
                                let line = serde_json::json!({"name": title, "id": id, "command": "git"});
                                if config::append_tab(&desk.name, line, Some(&place.dir)) {
                                    reveal = Some((id, Instant::now() + Duration::from_secs(20)));
                                    watcher.poke();
                                    Ok(serde_json::json!({"already": false}))
                                } else {
                                    Err(i18n::tp("msg.quick.open_failed", &[("label", "Git")]))
                                }
                            }
                        }
                    }
                };
                let js = match answer {
                    Ok(data) => serde_json::json!({"act": act, "ok": true, "data": data}),
                    Err(why) => serde_json::json!({"act": act, "ok": false, "error": why}),
                }
                .to_string();
                shell.push_git(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"git\":{js}}}"));
                }
                continue;
            }
            // A merge that stopped, handed to an AI tab in the same folder with
            // the desk's instruction as its first message. What it names -- the
            // branch, the base, the files -- is read from the folder, not taken
            // from the page
            if act == "resolve_tab" {
                let place = panel_places(&surfaces)
                    .into_iter()
                    .chain(tab_places(&tabs).into_iter().filter(|p| !p.dir.as_os_str().is_empty()))
                    .find(|p| p.key.matches(&panel));
                let ai = cfg.as_ref().and_then(|c| c.ai_engine.clone()).unwrap_or_default();
                let answer = match (place, desks.get(desk_index)) {
                    (None, _) | (_, None) => Err(i18n::t("err.git.no_tab")),
                    (Some(place), Some(desk)) => match ai_for_folder(Some(desk), &place.dir, &ai, &ai_choices) {
                    Err(why) => Err(why),
                    Ok(choice) => {
                        let choice = &choice;
                        crate::git::there(&place.dir, place.remote.as_ref());
                        let branch = crate::git::branch(&place.dir).ok().flatten().unwrap_or_default();
                        let base = crate::git::recorded_base(&place.dir, &branch).unwrap_or_default();
                        let said = resolve_in_tab(desk, &place.dir, &base, choice, &tabs, &mut pending_quicks, &mut reveal);
                        if said.as_ref().is_ok_and(|v| v["already"] == false) {
                            watcher.poke();
                        }
                        said
                    }
                    },
                };
                let js = match answer {
                    Ok(data) => serde_json::json!({"act": act, "ok": true, "data": data}),
                    Err(why) => serde_json::json!({"act": act, "ok": false, "error": why}),
                }
                .to_string();
                shell.push_git(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"git\":{js}}}"));
                }
                continue;
            }
            if matches!(act.as_str(), "fetch" | "pull" | "push" | "resolve" | "catch_up") {
                // "message" is the AI writing one, which is a read of the diff
                // followed by a wait on a program -- the same reason as the
                // network ones for not doing it on this thread
                let name = match act.as_str() {
                    // Untangling writes the file back, which is the same reach
                    // as anything else that edits the tree
                    "resolve" => "git_apply".to_string(),
                    _ => format!("git_{act}"),
                };
                // A panel of its own first, then the tab being looked at: the
                // column on the right reports on whatever folder the person is
                // working in, and names that tab rather than a surface. A tab
                // with no folder is not an answer, so it is not offered as one
                let place = panel_places(&surfaces)
                    .into_iter()
                    .chain(tab_places(&tabs).into_iter().filter(|p| !p.dir.as_os_str().is_empty()))
                    .find(|p| p.key.matches(&panel));
                // Who it signs in as: the account chosen for that tab or
                // project, read out of the store here, on this thread, and
                // never handed to anything but the git it is for. Untangling
                // talks to no server, so it needs no account
                let who = place.as_ref().map(|p| {
                    p.git.to_git(act != "resolve", &|k| caps.secret_value(k).ok())
                });
                // A refusal, and whether it is one the project's git account
                // settings put right -- then the folder those settings are
                // found by, so the panel can put a way there under the words
                let mut fix_at: Option<std::path::PathBuf> = None;
                let answer = match (caps.allows(&name, grants::Subject::Human), place, who) {
                    (false, ..) => Some(i18n::tp(
                        "err.hooks.not_permitted",
                        &[("name", &name), ("who", &i18n::t("grant.who.human"))],
                    )),
                    (true, None, _) | (true, _, None) => Some(i18n::t("err.git.no_tab")),
                    // An account that cannot sign in -- gone from the desk,
                    // its token never entered, its key file missing -- before
                    // git is started
                    (true, Some(place), Some(Err(why))) => {
                        fix_at = Some(place.dir);
                        Some(why)
                    }
                    (true, Some(place), Some(Ok(who))) => {
                        let dir = place.dir;
                        // Where the folder is, said to git on this thread for
                        // what is asked here, and again on the thread that does
                        // the work (see `git::there`)
                        let at = place.remote;
                        crate::git::there(&dir, at.as_ref());
                        // Bringing the latest in needs a base: the one written down
                        // for the branch, or the one just chosen -- written down now,
                        // so it is not asked for again
                        let mut base = String::new();
                        if act == "catch_up" {
                            let here = crate::git::branch(&dir).ok().flatten().unwrap_or_default();
                            let chosen = args.get("base").and_then(|b| b.as_str()).unwrap_or_default().trim().to_string();
                            if !chosen.is_empty() && !here.is_empty() {
                                let _ = crate::git::record_base(&dir, &here, &chosen);
                            }
                            base = crate::git::recorded_base(&dir, &here).unwrap_or(chosen);
                            if base.is_empty() {
                                let js = serde_json::json!({"act": act, "ok": false, "why": "no_base",
                                    "error": i18n::t("git.catch_up.pick")}).to_string();
                                shell.push_git(&js);
                                if let Some(r) = remote_ui.as_ref() {
                                    r.push_state(format!("{{\"git\":{js}}}"));
                                }
                                continue;
                            }
                        }
                        // Say it started, so a button that will be a while
                        // does not look like a button that did nothing --
                        // wherever it was pressed
                        let js = serde_json::json!({"act": act, "busy": true}).to_string();
                        shell.push_git(&js);
                        if let Some(r) = remote_ui.as_ref() {
                            r.push_state(format!("{{\"git\":{js}}}"));
                        }
                        let tx = git_tx.clone();
                        let act2 = act.clone();
                        let ai = cfg
                            .as_ref()
                            .and_then(|c| c.ai_engine.clone())
                            .filter(|s| !s.is_empty());
                        let folder = dir.display().to_string();
                        std::thread::spawn(move || {
                            crate::git::there(&dir, at.as_ref());
                            let done = match act2.as_str() {
                                "fetch" => crate::git::fetch(&dir, &who),
                                "pull" => crate::git::pull(&dir, &who),
                                "push" => crate::git::push(&dir, &who),
                                "catch_up" => crate::git::catch_up(&dir, &base, &who)
                                    .map(|n| serde_json::json!({"taken": n, "base": base}).to_string()),
                                #[allow(unreachable_patterns)]
                                // Every file git left marked, one at a time.
                                // Nothing is staged and nothing is committed:
                                // what comes back is written into the tree, and
                                // the person reads it as a diff like any other
                                "resolve" => crate::git::tangled(&dir).and_then(|files| {
                                    let mut done: Vec<String> = Vec::new();
                                    let mut failed: Vec<String> = Vec::new();
                                    for f in &files {
                                        // The file where the tree is: read
                                        // and written back over there when
                                        // the folder is on another machine
                                        let said = tree_file_read(&dir, f, at.as_ref())
                                            .and_then(|body| {
                                                crate::webui::resolve_conflict(f, &body, ai.as_deref())
                                            })
                                            .and_then(|text| tree_file_write(&dir, f, at.as_ref(), &text));
                                        match said {
                                            Ok(()) => done.push(f.clone()),
                                            Err(e) => failed.push(format!("{f}: {e}")),
                                        }
                                    }
                                    if done.is_empty() && !failed.is_empty() {
                                        anyhow::bail!("{}", failed.join("\n"));
                                    }
                                    Ok(crate::i18n::tp(
                                        "msg.git.resolved",
                                        &[("n", &done.len().to_string())],
                                    ) + if failed.is_empty() { "" } else { "\n" }
                                        + &failed.join("\n"))
                                }),
                                _ => Ok(String::new()),
                            };
                            let js = match done {
                                Ok(said) => serde_json::json!({
                                    "act": act2, "ok": true, "data": said.trim(),
                                }),
                                // Uncommitted work in the way of a pull is named,
                                // so the panel can put those files in front
                                Err(e) => match (e.downcast_ref::<crate::git::PullBlocked>(), e.downcast_ref::<crate::git::CatchUpStop>()) {
                                    (Some(blocked), _) => serde_json::json!({
                                        "act": act2, "ok": false, "error": e.to_string(),
                                        "why": "in_the_way", "paths": blocked.0,
                                    }),
                                    // A merge that stopped names the base and its files,
                                    // so the panel can offer to hand it over
                                    (_, Some(crate::git::CatchUpStop::Conflict { base, files })) => serde_json::json!({
                                        "act": act2, "ok": false, "error": e.to_string(),
                                        "why": "conflict", "base": base, "files": files,
                                    }),
                                    (_, Some(crate::git::CatchUpStop::Dirty)) => serde_json::json!({
                                        "act": act2, "ok": false, "error": e.to_string(), "why": "dirty",
                                    }),
                                    // A sign-in the server refused, or one that
                                    // could not be made: put right on the
                                    // project's page, which the panel offers
                                    _ if crate::git::is_sign_in_trouble(&e) => serde_json::json!({
                                        "act": act2, "ok": false, "error": e.to_string(),
                                        "why": "account", "folder": folder,
                                    }),
                                    _ => serde_json::json!({
                                        "act": act2, "ok": false, "error": plain_error(&e.to_string()),
                                    }),
                                },
                            };
                            let _ = tx.send(js.to_string());
                        });
                        None
                    }
                };
                // A refusal is an answer the phone needs as much as the window:
                // a button pressed there that says nothing looks broken
                if let Some(said) = answer {
                    let js = match fix_at {
                        Some(dir) => serde_json::json!({
                            "act": act, "ok": false, "error": said,
                            "why": "account", "folder": dir.display().to_string(),
                        }),
                        None => serde_json::json!({"act": act, "ok": false, "error": said}),
                    }
                    .to_string();
                    shell.push_git(&js);
                    if let Some(r) = remote_ui.as_ref() {
                        r.push_state(format!("{{\"git\":{js}}}"));
                    }
                }
                continue;
            }
            let files = serde_json::json!(paths);
            let (method, params): (&str, Vec<serde_json::Value>) = match act.as_str() {
                "status" => ("git_status", vec![]),
                "branch" => ("git_branch", vec![]),
                "branches" => ("git_branches", vec![]),
                "diff" => (
                    "git_diff",
                    vec![
                        serde_json::json!({
                            "path": paths.first().cloned().unwrap_or_default(),
                            "staged": args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false),
                            "encoding": args.get("encoding").and_then(|v| v.as_str()).unwrap_or(""),
                        }),
                    ],
                ),
                "graph" => (
                    "git_graph",
                    vec![
                        serde_json::json!({
                            "all": args.get("all").and_then(|v| v.as_bool()).unwrap_or(true),
                            "remotes": args.get("remotes").and_then(|v| v.as_bool()).unwrap_or(false),
                            "branch": args.get("branch").and_then(|v| v.as_str()).unwrap_or(""),
                        }),
                    ],
                ),
                "detail" => ("git_detail", vec![serde_json::json!(text)]),
                "hunks" => (
                    "git_hunks",
                    vec![
                        serde_json::json!({
                            "path": paths.first().cloned().unwrap_or_default(),
                            "staged": args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false),
                            "commit": args.get("commit").and_then(|v| v.as_str()).unwrap_or(""),
                            "encoding": args.get("encoding").and_then(|v| v.as_str()).unwrap_or(""),
                        }),
                    ],
                ),
                // One piece of a diff, put where the button said
                "hunk" => (
                    "git_apply",
                    vec![
                        serde_json::json!(text),
                        serde_json::json!({
                            "cached": args.get("cached").and_then(|v| v.as_bool()).unwrap_or(false),
                            "reverse": args.get("reverse").and_then(|v| v.as_bool()).unwrap_or(false),
                            "encoding": args.get("encoding").and_then(|v| v.as_str()).unwrap_or(""),
                        }),
                    ],
                ),
                "stage" => ("git_stage", vec![files]),
                "unstage" => ("git_unstage", vec![files]),
                // Throwing a change away. Asked twice: once with `plan`, for
                // the command lines the question is put in, and once for real
                // once somebody has read them and pressed the button
                "discard" => (
                    "git_discard",
                    vec![
                        files,
                        serde_json::json!({
                            "staged": args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false),
                            "plan": args.get("plan").and_then(|v| v.as_bool()).unwrap_or(false),
                        }),
                    ],
                ),
                "commit" => (
                    "git_commit",
                    vec![
                        serde_json::json!(text),
                        serde_json::json!({
                            "amend": args.get("amend").and_then(|v| v.as_bool()).unwrap_or(false),
                        }),
                    ],
                ),
                "checkout" => ("git_checkout", vec![serde_json::json!(text)]),
                "merge" => ("git_merge", vec![serde_json::json!(text)]),
                "remote_branches" => ("git_remote_branches", vec![]),
                // "make a branch and commit there" -- the answer to a refusal
                // rather than a way around it
                "branch_new" => ("git_branch_create", vec![serde_json::json!(text)]),
                _ => continue,
            };
            // Asked on the folder's own line (see `GitLine`), after the same
            // permission a script is asked for. Where the folder is, whose
            // branches it guards and who commits there are settled here, from
            // the screens as they are now; git itself runs on the line
            let place = panel_places(&surfaces)
                .into_iter()
                .chain(tab_places(&tabs).into_iter().filter(|p| !p.dir.as_os_str().is_empty()))
                .find(|p| p.key.matches(&panel));
            let refused = match (caps.allows(method, grants::Subject::Human), &place) {
                (false, _) => Some(i18n::tp(
                    "err.hooks.not_permitted",
                    &[("name", method), ("who", &i18n::t("grant.who.human"))],
                )),
                (true, None) => Some(i18n::t("err.git.no_tab")),
                (true, Some(_)) => None,
            };
            let who = match (crate::gitops::signs_in(method), &place) {
                (Some(to_server), Some(p)) => match p.git.to_git(to_server, &|k| caps.secret_value(k).ok()) {
                    Ok(w) => Some(w),
                    Err(why) => {
                        let js = serde_json::json!({"act": act, "ok": false, "error": why, "why": "account",
                            "folder": p.dir.display().to_string(), "panel": panel}).to_string();
                        shell.push_git(&js);
                        if let Some(r) = remote_ui.as_ref() {
                            r.push_state(format!("{{\"git\":{js}}}"));
                        }
                        continue;
                    }
                },
                _ => None,
            };
            let (Some(place), None) = (place, refused.clone()) else {
                let js = serde_json::json!({"act": act, "ok": false, "error": refused.unwrap_or_default(), "panel": panel}).to_string();
                shell.push_git(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"git\":{js}}}"));
                }
                continue;
            };
            git_lines.ask(GitJob {
                panel: panel.clone(),
                act: act.clone(),
                method,
                dir: place.dir,
                at: place.remote,
                protect: place.protect,
                who,
                params,
                seq: 0,
            });
        }
        // What the folders' git lines answered: handed to the panel, save an
        // answer to a question asked again since, and one that took too long
        // said as that -- a line that does not answer is not waited on
        for js in git_lines.answers() {
            shell.push_git(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"git\":{js}}}"));
            }
        }
        // Folders that name and describe themselves from what their AIs were
        // asked (`crate::labels`): looked at once a second, and each one that
        // is due handed to the Lua that writes it (`hooks::LABEL_LUA`)
        if label_look.elapsed() >= Duration::from_secs(1) {
            label_look = Instant::now();
            let now = Instant::now();
            // An answer that never comes -- the engine was built again under
            // it, or the desk it ran in has been put away for long -- does not
            // hold the folder forever. Its requests are read next time
            let (stale, live): (Vec<LabelJob>, Vec<LabelJob>) =
                label_jobs.drain(..).partition(|j| now.duration_since(j.started) > LABEL_GIVE_UP);
            label_jobs = live;
            for job in stale {
                append_hook_log(&format!("folder label for {} never came back", job.folder.display()));
                heard.finished(&job.folder, now, Some(job.asks));
            }
            if let Some(desk) = desks.get(desk_index) {
                // A folder on another machine asks like any other. Of the
                // three roads below, the record the AI keeps of its own
                // conversation is on that machine and cannot be read from
                // here, so such a folder is named from what the input bar
                // handed its AIs -- which is what a person working there
                // from the board types
                let mut wanted = Vec::new();
                for f in &desk.folders {
                    let Some(cwd) = f.cwd.as_ref() else { continue };
                    match f.auto_label {
                        true => wanted.push((cwd.clone(), f.summary.is_some())),
                        // Heard for nothing: a folder that does not ask keeps nothing
                        false => heard.forget(cwd),
                    }
                }
                // What people asked the AIs, read out of the record each AI
                // keeps of its own conversation (`crate::asks`). The other two
                // roads only cover some of the asking: the input bar knows
                // what it handed over, and a hook reports what it was
                // installed to report. Somebody typing straight into a CLI
                // that has had nothing put into its settings travels neither,
                // and that is how most work is asked for -- folders left
                // unnamed for weeks were all of them this
                for t in tabs.iter() {
                    let Some(at) = t.cwd() else { continue };
                    if !wanted.iter().any(|(w, _)| crate::uistate::same_folder(w, at)) {
                        continue;
                    }
                    let Some(spec) = t.resume.as_ref() else { continue };
                    let (Some(how), Some(verify), Some(session)) =
                        (spec.asks.as_ref(), spec.verify.as_deref(), t.session.as_ref())
                    else {
                        continue;
                    };
                    let Some(file) = sessionfind::locate(verify, &session.id) else { continue };
                    let from = *read_asks
                        .entry(file.clone())
                        .or_insert_with(|| crate::asks::begin_at(&file));
                    let (said, now_at) = crate::asks::read_from(&file, how, from);
                    read_asks.insert(file, now_at);
                    let at = at.to_path_buf();
                    for text in said {
                        heard.hear(&at, &text);
                    }
                }
                let due = heard.due(now, &wanted);
                if !due.is_empty() && engine.is_none() {
                    engine = crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)).ok();
                }
                if let Some(eng) = engine.as_mut() {
                    for at in due {
                        let Some(f) = desk.folders.iter().find(|f| {
                            f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, &at))
                        }) else {
                            continue;
                        };
                        let asks = heard.take(&at);
                        // What it is called and said to be now, when that was
                        // written from requests before. A folder never
                        // described is only its branch's name, which says
                        // nothing about the work and would be kept for that
                        let none = i18n::t("ai.label.none");
                        let (name_now, summary_now) = match f.summary.as_deref() {
                            Some(s) => (f.name.clone().unwrap_or_else(|| none.clone()), s.to_string()),
                            None => (none.clone(), none.clone()),
                        };
                        let listed = asks
                            .iter()
                            .enumerate()
                            .map(|(i, a)| format!("{}. {a}", i + 1))
                            .collect::<Vec<_>>()
                            .join("\n\n");
                        let prompt = i18n::tp(
                            "ai.label.prompt",
                            &[
                                ("name", &name_now),
                                ("summary", &summary_now),
                                ("asks", &listed),
                                ("language", &i18n::t("ai.label.language")),
                            ],
                        );
                        // The desk's own answer, then the app's, then the
                        // assistant AI. Three steps rather than two because
                        // the app-wide one is where a person who has one
                        // assistant AI for code and another for this says so
                        let ai = desk
                            .summary_ai
                            .clone()
                            .filter(|a| !a.trim().is_empty())
                            .or_else(|| cfg.as_ref().and_then(|c| c.summary_ai.clone()))
                            .filter(|a| !a.trim().is_empty())
                            .or_else(|| cfg.as_ref().and_then(|c| c.ai_engine.clone()))
                            .filter(|a| !a.trim().is_empty())
                            .unwrap_or_default();
                        for (name, value) in [("label_prompt", prompt), ("label_ai", ai)] {
                            let _ = eng.call_primitive_as(
                                None,
                                grants::Subject::Human,
                                "set_var",
                                &[serde_json::json!(name), serde_json::json!(value)],
                            );
                        }
                        label_seq += 1;
                        let tag = format!("{LABEL_TAG}{label_seq}");
                        // What it would take to call the branch after the
                        // work, decided here where the desk and its projects
                        // are in hand. The answer comes back on another turn
                        let project = desk.project_of(&at);
                        label_jobs.push(LabelJob {
                            tag: tag.clone(),
                            desk: desk.name.clone(),
                            folder: at.clone(),
                            asks,
                            started: now,
                            drawn: f.drawn.clone(),
                            prefix: project
                                .and_then(|p| p.branch_prefix.clone())
                                .unwrap_or_default(),
                            rename: desk.rename_branch != Some(false),
                        });
                        eng.start_snippet(&tag, crate::hooks::LABEL_LUA);
                    }
                }
            }
        }
        // Answers from the Lua that was left running (the commit message)
        if let Some(eng) = engine.as_mut() {
            for (tag, said) in eng.take_snippets() {
                // A folder's name and summary go into the settings, and only
                // into a folder that still wants them written
                if tag.starts_with(LABEL_TAG) {
                    let Some(i) = label_jobs.iter().position(|j| j.tag == tag) else { continue };
                    let job = label_jobs.remove(i);
                    let now = Instant::now();
                    // The branch name is asked for in the same breath as the
                    // folder's name, so naming the work costs one call and not
                    // two -- and the two can never disagree about what the
                    // work is
                    let mut slug = String::new();
                    let written = said.and_then(|text| {
                        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
                        let text_of = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
                        let name = crate::labels::fit(&text_of("name"), crate::labels::NAME_ROOM);
                        let summary = crate::labels::fit(&text_of("summary"), crate::labels::SUMMARY_ROOM);
                        slug = text_of("slug");
                        config::write_folder_label(&job.desk, &job.folder, &name, &summary).map_err(|e| format!("{e:#}"))
                    });
                    match written {
                        Ok(wrote) => {
                            if wrote {
                                rename_drawn_branch(&config::config_file_path(), &job, &slug);
                            }
                            crate::labels::note_outcome(&job.folder, None);
                            heard.finished(&job.folder, now, None);
                        }
                        Err(why) => {
                            append_hook_log(&format!("folder label for {}: {why}", job.folder.display()));
                            crate::labels::note_outcome(&job.folder, Some(&why));
                            heard.finished(&job.folder, now, Some(job.asks));
                        }
                    }
                    continue;
                }
                // A draft goes to the Issue tab under the act that asked for it,
                // everything else to git
                let issue = tag == DRAFT_ISSUE_TAG || tag == DRAFT_PR_TAG;
                let act = if tag == DRAFT_ISSUE_TAG { "draft" } else if tag == DRAFT_PR_TAG { "pr_draft" } else { tag.as_str() };
                let payload = match said {
                    Ok(text) => serde_json::json!({"act": act, "ok": true, "data": text}),
                    Err(why) => serde_json::json!({"act": act, "ok": false, "error": why}),
                };
                let js = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
                if issue {
                    shell.push_issues(&js);
                } else {
                    shell.push_git(&js);
                }
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"{}\":{js}}}", if issue { "issues" } else { "git" }));
                }
            }
        }
        // Answers from the ones that went to a thread
        while let Ok(js) = git_rx.try_recv() {
            append_hook_log(&format!("git: {}", log_excerpt(&js, 200)));
            shell.push_git(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"git\":{js}}}"));
            }
        }
        // The failed checks of a pull request's commit, read: handed to an AI tab
        // in the folder its branch is in, told what the desk's CI prompt says
        while let Ok(done) = ci_rx.try_recv() {
            let CiFix { project, number, seq, title, url, head, sha, dir, result } = done;
            let ai = cfg.as_ref().and_then(|c| c.ai_engine.clone()).unwrap_or_default();
            let chosen = ai_for_folder(desks.get(desk_index), &dir, &ai, &ai_choices);
            let mut js = match (result, desks.get(desk_index), chosen.as_ref().ok()) {
                (Err(e), ..) => serde_json::json!({"ok": false, "error": plain_error(&format!("{e:#}"))}),
                (Ok(failed), ..) if failed.as_array().is_none_or(|a| a.is_empty()) => {
                    serde_json::json!({"ok": false, "error": i18n::t("git.ci.none")})
                }
                (Ok(_), None, _) => serde_json::json!({"ok": false, "error": i18n::t("err.github.no_project")}),
                (Ok(_), _, None) => serde_json::json!({"ok": false, "error": chosen.as_ref().err().cloned().unwrap_or_default()}),
                (Ok(failed), Some(desk), Some(choice)) => {
                    let label = i18n::t("git.ci.tab");
                    let ci = ci_ran_on(number, &title, &url, &head, &sha);
                    let opened = hand_to_ai_tab(desk, &dir, &label, choice, &tabs, &mut pending_quicks, &mut reveal, || {
                        // What came from GitHub -- the title, the logs -- goes in last,
                        // so nothing in it is taken for a word to fill in
                        desk.git_of(&dir)
                            .ci_prompt()
                            .replace("{ci}", &ci)
                            // Empty rather than "0" for a branch with no pull
                            // request: a prompt somebody wrote their own way
                            // may still name it
                            .replace("{pr}", &match number {
                                0 => String::new(),
                                n => n.to_string(),
                            })
                            .replace("{branch}", &head)
                            .replace("{folder}", &dir.display().to_string())
                            .replace("{language}", &i18n::t("lang.self"))
                            .replace("{url}", &url)
                            .replace("{title}", &title)
                            .replace("{checks}", &serde_json::to_string_pretty(&failed).unwrap_or_default())
                    });
                    match opened {
                        Ok(tab) => {
                            if tab["already"] == false {
                                watcher.poke();
                            }
                            serde_json::json!({"ok": true, "data": tab})
                        }
                        Err(why) => serde_json::json!({"ok": false, "error": why}),
                    }
                }
            };
            js["act"] = serde_json::json!("ci_fix");
            js["project"] = serde_json::json!(project);
            js["number"] = serde_json::json!(number);
            js["seq"] = seq;
            let js = js.to_string();
            append_hook_log(&format!("issues: {}", log_excerpt(&js, 200)));
            shell.push_issues(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"issues\":{js}}}"));
            }
        }
        // A pull request's base, brought into its folder. A conflict goes to an
        // AI tab from here; anything else is said on the pull request's page
        while let Ok(done) = pr_rx.try_recv() {
            let PrCatchUp { project, number, seq, dir, base, result, far_branch } = done;
            let folder = dir.display().to_string();
            let answer = match result {
                Ok(taken) => serde_json::json!({"ok": true, "data": {"state": if taken > 0 { "taken" } else { "latest" }, "taken": taken, "base": base, "folder": folder}}),
                Err(e) => match e.downcast_ref::<crate::git::CatchUpStop>() {
                    Some(crate::git::CatchUpStop::Conflict { files, .. }) => {
                        let ai = cfg.as_ref().and_then(|c| c.ai_engine.clone()).unwrap_or_default();
                        let files = files.clone();
                        let chosen = ai_for_folder(desks.get(desk_index), &dir, &ai, &ai_choices);
                        match (desks.get(desk_index), chosen.as_ref()) {
                            (None, _) => serde_json::json!({"ok": false, "error": i18n::t("err.github.no_project")}),
                            (_, Err(why)) => serde_json::json!({"ok": false, "error": why}),
                            (Some(desk), Ok(choice)) => match resolve_in_tab_knowing(
                                desk,
                                &dir,
                                &base,
                                far_branch.as_ref().map(|b| (b.clone(), files.clone())),
                                choice,
                                &tabs,
                                &mut pending_quicks,
                                &mut reveal,
                            ) {
                                Ok(tab) => {
                                    if tab["already"] == false {
                                        watcher.poke();
                                    }
                                    serde_json::json!({"ok": true, "data": {"state": "tab", "title": tab["title"], "already": tab["already"],
                                        "base": base, "files": files, "folder": folder}})
                                }
                                Err(why) => serde_json::json!({"ok": false, "error": why}),
                            },
                        }
                    }
                    Some(stop) => serde_json::json!({"ok": false, "error": stop.to_string()}),
                    None => serde_json::json!({"ok": false, "error": plain_error(&e.to_string())}),
                },
            };
            let mut js = answer;
            js["act"] = serde_json::json!("pr_resolve");
            js["project"] = serde_json::json!(project);
            js["number"] = serde_json::json!(number);
            js["seq"] = seq;
            let js = js.to_string();
            append_hook_log(&format!("issues: {}", log_excerpt(&js, 200)));
            shell.push_issues(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"issues\":{js}}}"));
            }
        }

        // A file was pressed in the list. Which editor takes it is decided the
        // way everything else on screen is decided -- by what is in front:
        //
        //   an editor in front   -> that one
        //   an editor elsewhere  -> the first one of this folder
        //   no editor at all     -> a throwaway one, reused from then on
        //
        // so nothing has to be set up before pressing a file, and pressing ten
        // files leaves one editor rather than ten
        for (panel, path, diff) in shell.mail().take_edits() {
            let Some(place) = files_at(&panel, &surfaces, &tabs) else { continue };
            let (dir, machine) = match &place {
                FilesAt::Here(d) => (d.clone(), None),
                FilesAt::There { at, root } => (std::path::PathBuf::from(root), Some(at.clone())),
            };
            let focused = surfaces
                .get(pane_layout.focused_surface().wrapping_sub(1))
                .and_then(|s| match s {
                    Surface::Editor { key, .. } => Some(key.clone()),
                    _ => None,
                });
            let key = focused
                .or_else(|| {
                    surfaces.iter().find_map(|s| match s {
                        Surface::Editor { key, dir: Some(d), .. }
                            if crate::uistate::same_folder(d, &dir) =>
                        {
                            Some(key.clone())
                        }
                        _ => None,
                    })
                })
                .unwrap_or_else(|| EDITOR_SCRATCH.to_string());
            // An empty path is the file being put away rather than opened. The
            // throwaway editor has nothing left to be, so it goes with it
            let showing = if path.trim().is_empty() {
                None
            } else if !place.holds(&path) {
                continue;
            } else {
                Some(path.clone())
            };
            if showing.is_none() && key == EDITOR_SCRATCH {
                editors.retain(|e| e.key != key);
                active = pane_layout.focused_surface();
                continue;
            }
            let entry = crate::view::EditorOpen {
                key: key.clone(),
                dir: Some(dir.clone()),
                showing: showing.clone(),
                // Filled in when the state is built, from the disk itself
                stamp: None,
                scratch: key == EDITOR_SCRATCH,
                at: machine,
                // A change is only ever of a file that is being shown
                diff: showing.as_ref().and_then(|_| (!diff.trim().is_empty()).then(|| diff.trim().to_string())),
            };
            match editors.iter_mut().find(|e| e.key == key) {
                Some(e) => *e = entry,
                None => editors.push(entry),
            }
            open_editor = Some(key);
        }
        // The Issue row in the list: open the tab, and look at it
        if shell.mail().take_open_issues() {
            // Brought to the front at the top of the next pass, once the row
            // is on the list the rest of the loop measures against
            if !issues_open {
                issues_basis = (desk_index, settings_gen);
            }
            issues_open = true;
            issues_front = true;
        }
        // The settings changed, or another desk is in front, since the Issue tab
        // last read its projects: it asks again, whether or not it is in view --
        // the settings screen covers it while the account is being chosen
        if issues_open && issues_basis != (desk_index, settings_gen) {
            issues_basis = (desk_index, settings_gen);
            let js = serde_json::json!({"act": "reload", "ok": true}).to_string();
            shell.push_issues(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"issues\":{js}}}"));
            }
        }
        // What the Issue tab asked for. The projects are answered here; every
        // other request is allowed or refused by the same permission-table row
        // as its automation command, then waits for GitHub on a thread of its
        // own -- a window cannot wait on somebody's network
        for (act, args) in shell.mail().take_issues() {
            let Some(desk) = desks.get(desk_index) else {
                continue;
            };
            // A link in an issue, pressed in the window: handed to this PC's browser,
            // and only when it is an address on the web -- the same call opens a
            // program when given a path
            if act == "link" {
                let url = args.get("url").and_then(|u| u.as_str()).unwrap_or_default();
                if crate::github::openable_link(url) {
                    crate::webui::open_external(url);
                }
                continue;
            }
            // Something for the AI to write into a form: an issue from the notes
            // in its description, or a pull request from a branch's commits. The
            // desk's prompt with its words filled in, then the shape of the answer;
            // Lua the same way the commit message is, so the window keeps drawing
            if act == "draft" || act == "pr_draft" {
                let ai = cfg.as_ref().and_then(|c| c.ai_engine.clone()).filter(|s| !s.is_empty());
                let label = crate::webui::local_ai_label(ai.as_deref()).unwrap_or("an AI");
                let text = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
                let list = |k: &str| args.get(k).cloned().filter(|v| v.is_array()).unwrap_or(serde_json::json!([]));
                let (prompt, shape, tag) = if act == "draft" {
                    let notes = text("text");
                    if notes.trim().is_empty() {
                        let js = serde_json::json!({"act": "draft", "ok": false, "error": i18n::t("err.issue.nothing_to_draft")}).to_string();
                        shell.push_issues(&js);
                        continue;
                    }
                    let shape = i18n::tp("ai.issue.shape", &[
                        ("labels", &list("labels").to_string()),
                        ("assignees", &list("assignees").to_string()),
                    ]);
                    // The project's prompt, by the name the issue list knows it by
                    let prompt = fill_or_append(
                        &desk.git_of_project(&text("project")).issue_prompt(),
                        &[("ai", label), ("language", &i18n::t("lang.self"))],
                        "text",
                        &notes,
                    );
                    (prompt, shape, DRAFT_ISSUE_TAG)
                } else {
                    // The branch, what it goes into, its commits and its change,
                    // read from the folder the pull request is for
                    // Only a folder of the project it names: the page says which
                    // folder, and git is not run anywhere it could point at
                    let Some(dir) = crate::github::project_folder(&crate::github::desk_sources(desk), &text("project"), &text("folder")) else {
                        let js = serde_json::json!({"act": "pr_draft", "ok": false, "error": i18n::t("err.github.no_project")}).to_string();
                        shell.push_issues(&js);
                        continue;
                    };
                    let (head, into) = (text("head"), text("base"));
                    let template = desk.git_of(&dir).pr_prompt();
                    let label = label.to_string();
                    let read = move |dir: &std::path::Path| {
                        let against = format!("origin/{into}");
                        let commits = crate::git::run(dir, &["log", "--no-color", "--format=- %s", &format!("{against}..HEAD")])
                            .unwrap_or_default();
                        let mut change = crate::git::run(dir, &["diff", "--no-color", &format!("{against}...HEAD")]).unwrap_or_default();
                        // A change can be a megabyte. The shape of it is in the first pages
                        if change.len() > 12000 {
                            let cut = (0..=12000).rev().find(|&i| change.is_char_boundary(i)).unwrap_or(0);
                            change.truncate(cut);
                            change.push_str("\n...\n");
                        }
                        fill_or_append(
                            &template,
                            &[
                                ("ai", &label),
                                ("branch", &head),
                                ("base", &into),
                                ("commits", commits.trim()),
                                ("language", &i18n::t("lang.self")),
                            ],
                            "diff",
                            &change,
                        )
                    };
                    // On another machine git answers over the network, and the
                    // board does not wait on a network: read there on a
                    // thread, and the draft starts when it is back
                    if crate::git::is_far(&dir) {
                        let at = crate::github::desk_sources(desk)
                            .into_iter()
                            .flat_map(|s| s.far)
                            .find(|(f, _)| crate::uistate::same_folder(f, &dir))
                            .map(|(_, at)| at);
                        crate::git::there(&dir, None);
                        let tx = pr_draft_tx.clone();
                        std::thread::spawn(move || {
                            crate::git::there(&dir, at.as_ref());
                            let _ = tx.send((read(&dir), i18n::t("ai.pr.shape")));
                        });
                        continue;
                    }
                    (read(&dir), i18n::t("ai.pr.shape"), DRAFT_PR_TAG)
                };
                if engine.is_none() {
                    engine = crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)).ok();
                }
                let Some(eng) = engine.as_mut() else { continue };
                for (name, value) in [("draft_prompt", prompt), ("draft_shape", shape)] {
                    let _ = eng.call_primitive_as(
                        None,
                        grants::Subject::Human,
                        "set_var",
                        &[serde_json::json!(name), serde_json::json!(value)],
                    );
                }
                eng.start_snippet(tag, crate::hooks::DRAFT_LUA);
                continue;
            }
            let sources = crate::github::desk_sources(desk);
            // A pull request that cannot be merged for its conflicts: where its
            // branch is checked out on this PC and what bringing its base in
            // there would run (`pr_place`), and doing it (`pr_resolve`). What is
            // shown and what runs are the same steps
            if act == "pr_place" || act == "pr_resolve" {
                let text = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or_default().trim().to_string();
                let (project, head, into) = (text("project"), text("head"), text("base"));
                let number = args.get("number").and_then(|v| v.as_u64()).unwrap_or(0);
                let source = sources.iter().find(|s| s.name == project);
                // A project on another machine: its folders there are asked
                // which of them is on the branch, and everything after that
                // runs there too -- all of it on a thread, since each step is
                // a trip over the network the board must not wait on
                if let Some(far) = source.filter(|s| !s.far.is_empty()).cloned() {
                    let seq = args.get("seq").cloned().unwrap_or(serde_json::Value::Null);
                    let base = format!("origin/{into}");
                    let allowed = caps.allows("git_catch_up", grants::Subject::Human);
                    let who = far.git.to_git(true, &|k| caps.secret_value(k).ok());
                    let (act, issues, prs) = (act.clone(), issues_tx.clone(), pr_tx.clone());
                    std::thread::spawn(move || {
                        let say = |mut js: serde_json::Value| {
                            js["act"] = serde_json::json!(act);
                            js["project"] = serde_json::json!(project);
                            js["number"] = serde_json::json!(number);
                            js["seq"] = seq.clone();
                            let _ = issues.send(js.to_string());
                        };
                        if act == "pr_resolve" && !allowed {
                            return say(serde_json::json!({"ok": false, "error": i18n::tp(
                                "err.hooks.not_permitted",
                                &[("name", "git_catch_up"), ("who", &i18n::t("grant.who.human"))],
                            )}));
                        }
                        let Some(dir) = crate::github::far_head_folder(&far, &head) else {
                            return say(match act.as_str() {
                                "pr_place" => serde_json::json!({"ok": true, "data": {"folder": null}}),
                                _ => serde_json::json!({"ok": false, "error": i18n::tp("err.github.pr.no_folder", &[("branch", &head)])}),
                            });
                        };
                        let conflicted = crate::git::conflicts(&dir).unwrap_or_default();
                        let stopped = crate::git::merging_in(&dir, &base) && !conflicted.is_empty();
                        if act == "pr_place" {
                            return say(serde_json::json!({"ok": true, "data": {
                                "folder": dir.display().to_string(),
                                "runs": crate::git::said(&crate::git::catch_up_steps_for(&dir, Some(&head), &base)),
                                "merging": stopped,
                            }}));
                        }
                        if crate::git::recorded_base(&dir, &head).is_none() {
                            let _ = crate::git::record_base(&dir, &head, &base);
                        }
                        let result = match (stopped, who) {
                            // Stopped on this base already: straight to the AI
                            (true, _) => Err(anyhow::Error::new(crate::git::CatchUpStop::Conflict {
                                base: base.clone(),
                                files: conflicted,
                            })),
                            (false, Err(why)) => return say(serde_json::json!({"ok": false, "error": why})),
                            (false, Ok(who)) => crate::git::catch_up_for(&dir, Some(&head), &base, &who),
                        };
                        let _ = prs.send(PrCatchUp { project: project.clone(), number, seq: seq.clone(), dir, base, result, far_branch: Some(head.clone()) });
                    });
                    continue;
                }
                let dir = source.and_then(|s| crate::github::head_folder(&s.dir, &head));
                let base = format!("origin/{into}");
                let seq = args.get("seq").cloned().unwrap_or(serde_json::Value::Null);
                let say = |mut js: serde_json::Value| {
                    js["act"] = serde_json::json!(act);
                    js["project"] = serde_json::json!(project);
                    js["number"] = serde_json::json!(number);
                    js["seq"] = seq.clone();
                    js.to_string()
                };
                let answer = if act == "pr_place" {
                    Some(say(match &dir {
                        Some(d) => serde_json::json!({"ok": true, "data": {
                            "folder": d.display().to_string(),
                            "runs": crate::git::said(&crate::git::catch_up_steps_for(d, Some(&head), &base)),
                            "merging": crate::git::merging_in(d, &base) && !crate::git::conflicts(d).unwrap_or_default().is_empty(),
                        }}),
                        None => serde_json::json!({"ok": true, "data": {"folder": null}}),
                    }))
                } else {
                    match (caps.allows("git_catch_up", grants::Subject::Human), source, dir) {
                        (false, ..) => Some(say(serde_json::json!({"ok": false, "error": i18n::tp(
                            "err.hooks.not_permitted",
                            &[("name", "git_catch_up"), ("who", &i18n::t("grant.who.human"))],
                        )}))),
                        (true, None, _) => Some(say(serde_json::json!({"ok": false, "error": i18n::t("err.github.no_project")}))),
                        (true, Some(_), None) => Some(say(serde_json::json!({"ok": false,
                            "error": i18n::tp("err.github.pr.no_folder", &[("branch", &head)])}))),
                        (true, Some(source), Some(dir)) => {
                            // Written down for the branch when nothing is, so the git
                            // column beside that folder speaks of the same base
                            if !head.is_empty() && crate::git::recorded_base(&dir, &head).is_none() {
                                let _ = crate::git::record_base(&dir, &head, &base);
                            }
                            let stopped = crate::git::merging_in(&dir, &base)
                                && !crate::git::conflicts(&dir).unwrap_or_default().is_empty();
                            match (source.git.to_git(true, &|k| caps.secret_value(k).ok()), stopped) {
                                // A merge of this base that already stopped here goes
                                // straight to the AI: nothing to fetch or merge again
                                (_, true) => {
                                    let _ = pr_tx.send(PrCatchUp {
                                        project: project.clone(), number, seq: seq.clone(), dir: dir.clone(), base: base.clone(), far_branch: None,
                                        result: Err(anyhow::Error::new(crate::git::CatchUpStop::Conflict {
                                            base: base.clone(),
                                            files: crate::git::conflicts(&dir).unwrap_or_default(),
                                        })),
                                    });
                                    None
                                }
                                (Err(why), _) => Some(say(serde_json::json!({"ok": false, "error": why}))),
                                (Ok(who), false) => {
                                    let tx = pr_tx.clone();
                                    let (project, base, seq) = (project.clone(), base.clone(), seq.clone());
                                    std::thread::spawn(move || {
                                        let result = crate::git::catch_up_for(&dir, Some(&head), &base, &who);
                                        let _ = tx.send(PrCatchUp { project, number, seq, dir, base, result, far_branch: None });
                                    });
                                    None
                                }
                            }
                        }
                    }
                };
                if let Some(js) = answer {
                    shell.push_issues(&js);
                    if let Some(r) = remote_ui.as_ref() {
                        r.push_state(format!("{{\"issues\":{js}}}"));
                    }
                }
                continue;
            }
            if act == "projects" {
                let projects: Vec<serde_json::Value> = sources
                    .iter()
                    .map(|s| {
                        serde_json::json!({
                            "name": s.name,
                            "dir": s.dir.display().to_string(),
                            // The folder of this desk it was found through, for
                            // the places that point back at a line of the
                            // settings or ask which project a folder is in
                            "at": s.at.display().to_string(),
                            "repo": s.repo,
                            "account": s.git.written(),
                        })
                    })
                    .collect();
                // The accounts a project can be given, for the screens that offer
                // to choose one where reading GitHub needs it
                let accounts: Vec<serde_json::Value> = desk
                    .git_accounts
                    .iter()
                    .map(|a| serde_json::json!({"name": a.name, "gh": a.is_gh(), "host": a.host(), "label": a.label}))
                    .collect();
                // And the GitHub accounts git on this PC holds, each a choice,
                // with what the settings call them
                let pc = crate::pr::pc_accounts_known();
                let gh = crate::pr::gh_accounts_known();
                let labels = cfg.as_ref().map(|c| c.sign_in_labels.clone()).unwrap_or_default();
                let js = serde_json::json!({"act": "projects", "ok": true, "projects": projects, "accounts": accounts, "pc": pc, "gh": gh, "labels": labels}).to_string();
                shell.push_issues(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"issues\":{js}}}"));
                }
                continue;
            }
            let pulls = args.get("kind").and_then(|k| k.as_str()) == Some("pr");
            let refused = match crate::github::command_for(&act, pulls) {
                None => Some(format!("unknown request {act}")),
                Some(name) if !caps.allows(name, grants::Subject::Human) => Some(i18n::tp(
                    "err.hooks.not_permitted",
                    &[("name", name), ("who", &i18n::t("grant.who.human"))],
                )),
                Some(_) => None,
            };
            if let Some(why) = refused {
                let js = serde_json::json!({"act": act, "ok": false, "error": why, "kind": if pulls { "pr" } else { "issue" },
                    "seq": args.get("seq").cloned().unwrap_or(serde_json::Value::Null)}).to_string();
                shell.push_issues(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"issues\":{js}}}"));
                }
                continue;
            }
            // The tokens this request may need, copied out for the thread: the
            // store is this loop's, and what leaves it is only what is used
            let tokens: std::collections::HashMap<String, String> = sources
                .iter()
                .filter_map(|s| match &s.git {
                    config::GitUse::Account { spec } => {
                        let key = config::git_token_key(&spec.name);
                        caps.secret_value(&key).ok().map(|t| (key, t))
                    }
                    _ => None,
                })
                .collect();
            // CI that failed on a pull request's commit, handed to an AI tab in
            // the folder its branch is checked out in
            if act == "ci_fix" {
                let text = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or_default().trim().to_string();
                let (project, head, sha, title, url) = (text("project"), text("head"), text("sha"), text("title"), text("url"));
                let number = args.get("number").and_then(|v| v.as_u64()).unwrap_or(0);
                let seq = args.get("seq").cloned().unwrap_or(serde_json::Value::Null);
                // A project on another machine: which of its folders there is
                // on the branch is asked there, on the thread that reads GitHub
                if let Some(far) = sources.iter().find(|s| s.name == project && !s.far.is_empty()).cloned() {
                    let (tx, issues) = (ci_tx.clone(), issues_tx.clone());
                    std::thread::spawn(move || {
                        match crate::github::far_head_folder(&far, &head) {
                            None => {
                                let js = serde_json::json!({"act": "ci_fix", "ok": false, "seq": seq, "project": project, "number": number,
                                    "error": i18n::tp("err.github.pr.no_folder", &[("branch", &head)])});
                                let _ = issues.send(js.to_string());
                            }
                            Some(dir) => {
                                let result = crate::github::ci_failures(&sources, &project, &sha, &|k| tokens.get(k).cloned());
                                let _ = tx.send(CiFix { project, number, seq, title, url, head, sha, dir, result });
                            }
                        }
                    });
                    continue;
                }
                match sources.iter().find(|s| s.name == project).and_then(|s| crate::github::head_folder(&s.dir, &head)) {
                    None => {
                        let js = serde_json::json!({"act": act, "ok": false, "seq": seq, "project": project, "number": number,
                            "error": i18n::tp("err.github.pr.no_folder", &[("branch", &head)])}).to_string();
                        shell.push_issues(&js);
                        if let Some(r) = remote_ui.as_ref() {
                            r.push_state(format!("{{\"issues\":{js}}}"));
                        }
                    }
                    Some(dir) => {
                        let tx = ci_tx.clone();
                        std::thread::spawn(move || {
                            let result = crate::github::ci_failures(&sources, &project, &sha, &|k| tokens.get(k).cloned());
                            let _ = tx.send(CiFix { project, number, seq, title, url, head, sha, dir, result });
                        });
                    }
                }
                continue;
            }
            let tx = issues_tx.clone();
            std::thread::spawn(move || {
                let js = crate::github::answer(&act, &args, &sources, &|k| tokens.get(k).cloned());
                let _ = tx.send(js.to_string());
            });
        }
        // What the ideas window asked for. Answered here: it is one small file
        // on this machine, and every surface looking at the cards is told the
        // result, so the window and a phone never show two different lists
        for (act, args) in shell.mail().take_ideas() {
            let projects = crate::ideas::projects(&desks);
            let js = crate::ideas::answer(&crate::ideas::path(), &act, &args, &projects).to_string();
            shell.push_ideas(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"ideas\":{js}}}"));
            }
        }
        // A pull request's draft read on another machine, back: started the
        // way one read here is
        while let Ok((prompt, shape)) = pr_draft_rx.try_recv() {
            if engine.is_none() {
                engine = crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)).ok();
            }
            let Some(eng) = engine.as_mut() else { continue };
            for (name, value) in [("draft_prompt", prompt), ("draft_shape", shape)] {
                let _ = eng.call_primitive_as(
                    None,
                    grants::Subject::Human,
                    "set_var",
                    &[serde_json::json!(name), serde_json::json!(value)],
                );
            }
            eng.start_snippet(DRAFT_PR_TAG, crate::hooks::DRAFT_LUA);
        }
        while let Ok(js) = issues_rx.try_recv() {
            shell.push_issues(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"issues\":{js}}}"));
            }
        }
        // The column's file list, and the editor. Answered on the spot when it
        // is one folder of this machine or a search that stops itself; a
        // folder on another machine is asked on a thread and answers below
        for (panel, act, args) in shell.mail().take_files() {
            if let Some(js) = files_answer(&panel, &act, &args, &surfaces, &tabs, &caps, &far_tx) {
                shell.push_files(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"files\":{js}}}"));
                }
            }
        }
        while let Ok(far) = far_rx.try_recv() {
            if far.polled {
                far_polls.entry(far.key.clone()).and_modify(|p| p.busy = false);
            }
            if let Some(stamp) = far.stamp {
                // A look that set out before the last read or save came back
                // may have seen the file before that save, and would read as
                // somebody else's change to a file nobody else touched
                let stale = far.polled
                    && far_seen.get(&far.key).is_some_and(|seen| seen.at > far.asked);
                if !stale {
                    far_seen.insert(far.key.clone(), FarSeen { path: far.path.clone(), stamp, at: far.asked });
                }
            }
            if let Some(js) = far.js {
                shell.push_files(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"files\":{js}}}"));
                }
            }
        }
        // An editor put away takes what was heard about its file with it
        far_seen.retain(|k, _| editors.iter().any(|e| &e.key == k));
        far_polls.retain(|k, _| editors.iter().any(|e| &e.key == k));
        far_stamp_polls(&editors, &tabs, start.elapsed().as_millis() as u64, &mut far_polls, &far_tx);
        keep_machines_up(&tabs, start.elapsed().as_millis() as u64, &mut kept_up);
        // What the transfer panel asked for. Reading this machine is answered
        // on the spot; anything that touches the server goes to a thread,
        // because a folder listing over a network is a wait and this loop
        // draws the window
        for (panel, act, args) in shell.mail().take_sftps() {
            // A folder is not one job. It is a walk and a series of them, and
            // how to arrange that is a template's business rather than this
            // loop's -- so the ask is handed to Lua with what it is about, and
            // the loop goes back to drawing while it runs
            if act == "move_folder" {
                let refuse = |why: String| {
                    let js = serde_json::json!({
                        "act": "progress", "panel": panel, "ok": false, "error": why,
                    });
                    let _ = sftp_tx.send(js.to_string());
                };
                let Some(at) = surfaces
                    .iter()
                    .position(|s| matches!(s, Surface::Sftp { key, .. } if *key == panel))
                else {
                    refuse(i18n::t("err.sftp.no_panel"));
                    continue;
                };
                // A desk with no Lua of its own has no engine until something
                // asks for one, and this is something asking. Made here, the
                // way the outside door makes one -- without it the folder was
                // refused on every desk nobody had written automation for,
                // which is nearly all of them, and refused as a panel that was
                // no longer in the settings, which it was
                if engine.is_none() {
                    match crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)) {
                        Ok(eng) => engine = Some(eng),
                        Err(e) => {
                            refuse(format!("{e:#}"));
                            continue;
                        }
                    }
                }
                let Some(eng) = engine.as_mut() else { continue };
                eng.set_states(tab_states(&tabs));
                eng.set_places(places_by_surface(&surfaces, &tabs));
                let mut job = args.clone();
                job["tab"] = serde_json::json!(panel);
                eng.fire_template(crate::hooks::FOLDER_MOVE_LUA, &panel_ctx(at + 1, &panel), &job);
                continue;
            }
            let js = sftp_answer(&panel, &act, &args, &surfaces, &caps, &sftp_tx);
            if let Some(js) = js {
                shell.push_sftp(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"sftp\":{js}}}"));
                }
            }
        }
        while let Ok(js) = sftp_rx.try_recv() {
            append_hook_log(&format!("sftp: {}", log_excerpt(&js, 200)));
            shell.push_sftp(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"sftp\":{js}}}"));
            }
        }

        // "Why did it stop?" -- the notice about a run that ended badly.
        // Explaining it means reading a system log and asking an AI, neither
        // of which belongs on the loop that draws, so it happens on a thread
        // and the answer arrives through the same door as everything else
        for ask in shell.mail().take_why_stopped() {
            if !ask {
                crate::lastexit::dismiss();
                asking_why = false;
                continue;
            }
            let Some(told) = crate::lastexit::told() else { continue };
            if asking_why {
                continue;
            }
            asking_why = true;
            let engine = cfg.as_ref().and_then(|c| c.ai_engine.clone());
            let language = i18n::language_name();
            let said = why_tx.clone();
            std::thread::spawn(move || {
                let (job, records) = crate::lastexit::question(&told.ended, &told.mark, &language);
                // Asked with no tools, in a folder of its own. Asked the
                // ordinary way, an assistant AI treats the question as work
                // to be done: one of them went off and changed code, then
                // reported that it had fixed it
                let answer = match crate::webui::ask_local_ai_confined(
                    &records,
                    &job,
                    engine.as_deref(),
                    std::time::Duration::from_secs(180),
                ) {
                    Ok(text) => text,
                    // The reason it could not be explained is itself the
                    // answer to give: "nothing happened" is the one thing a
                    // button must never do
                    Err(e) => i18n::tp("lastexit.failed", &[("why", &format!("{e:#}"))]),
                };
                let _ = said.send(answer);
            });
        }
        while let Ok(said) = why_rx.try_recv() {
            crate::lastexit::explained(said);
            asking_why = false;
        }

        // 🗣 drive the shown page from a goal written in ordinary words. The
        // run is attached to the page's own pane: nobody is operating it from
        // another tab, and the strip the person watches belongs to that page
        for (on, goal, agree) in shell.mail().take_words() {
            let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) else {
                flash = Some(i18n::t("msg.words.browser_only"));
                continue;
            };
            let key = key.clone();
            if !on {
                if let (Some(eng), Some((pane, _))) = (engine.as_mut(), driving.take()) {
                    eng.stop_words(pane);
                }
                continue;
            }
            // Sending page contents to a company's service is the person's
            // decision to make, once, knowingly -- the same gate the picture
            // tools pass through, and refused here rather than half-started
            let mut gate = config::pages_gate(cfg.as_ref(), desks.get(desk_index), Some(&key));
            // "Agree and run", pressed beside that question: the agreement is
            // written in the settings, the way ticking it there writes it
            if agree && !gate.agree().is_empty() {
                let rows = gate.agree().to_vec();
                let saved = rows.iter().all(|row| config::save_agreed(row, config::CONSENT_PAGES));
                if let (true, Some(c)) = (saved, cfg.as_mut()) {
                    append_hook_log(&format!("words: agreed to send pages to {}", rows.join(" + ")));
                    for row in rows {
                        let kinds = c.agreed.entry(row).or_default();
                        if !kinds.iter().any(|k| k == config::CONSENT_PAGES) {
                            kinds.push(config::CONSENT_PAGES.to_string());
                        }
                    }
                    gate = config::pages_gate(cfg.as_ref(), desks.get(desk_index), Some(&key));
                } else {
                    flash = Some(i18n::t("msg.words.not_saved"));
                    continue;
                }
            }
            if !matches!(gate, config::PageGate::Ready { .. }) {
                append_hook_log(&format!("words: not started: {}", gate.why()));
                // Said on the 🗣 line, where the goal was typed, and with the
                // button that agrees when agreeing is what is missing. A flash
                // alone went by in a moment, and read as nothing happening
                let note = match gate.why_here() {
                    // A question with its answer beside it, not a fault
                    Some(text) => serde_json::json!({"text": text, "bad": false, "agree": goal}),
                    None => serde_json::json!({"text": gate.why(), "bad": true}),
                };
                let js = note.to_string();
                shell.push_words_note(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"words\":{js}}}"));
                }
                continue;
            }
            if engine.is_none() {
                engine = crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)).ok();
            }
            let Some(eng) = engine.as_mut() else { continue };
            let ctx = browser_ctx(active, &key);
            // A goal typed while a run is going is a correction to it, not a
            // second run: the same page, told something more
            if driving.as_ref().map(|(p, _)| *p) == Some(active) && eng.words_running() {
                eng.set_goal_in_words(&goal);
                continue;
            }
            let stops = desks
                .get(desk_index)
                .map(|w| config::stops_to_lua(&w.stops))
                .unwrap_or_else(|| "{}".to_string());
            // This page's own models, and the desk's where it has none
            let models = desks.get(desk_index).map(|w| w.words_models(cfg.as_ref(), Some(&key))).unwrap_or_default();
            match eng.start_words(active, &key, &stops, &goal, &models, &ctx) {
                Ok(()) => driving = Some((active, key)),
                Err(e) => {
                    append_hook_log(&format!("words start failed: {e:#}"));
                    flash = Some(format!("{e}"));
                }
            }
        }

        // One move per pass while a words-driven run is going. Written as a
        // step rather than a loop on purpose: each move waits on a page, and a
        // loop that waited here would hold everything else in the program
        // still for as long as the whole task took
        if let Some((pane, key)) = driving.clone() {
            // Carried over a reload: taken up by the engine built in its place,
            // with the page's models and stops as the settings now say. A desk
            // with no automation of its own has no engine until one is needed
            if words_carried.is_some() && engine.is_none() {
                engine = crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)).ok();
            }
            if let (Some(carried), Some(eng)) = (words_carried.take(), engine.as_mut()) {
                let stops = desks
                    .get(desk_index)
                    .map(|w| config::stops_to_lua(&w.stops))
                    .unwrap_or_else(|| "{}".to_string());
                let models = desks.get(desk_index).map(|w| w.words_models(cfg.as_ref(), Some(&key))).unwrap_or_default();
                if let Err(e) = eng.resume_words(pane, &key, &stops, &models, &carried) {
                    append_hook_log(&format!("words could not carry on: {e:#}"));
                }
            }
            let ctx = browser_ctx(pane, &key);
            let still = match engine.as_mut() {
                Some(eng) => eng.step_words(&ctx),
                None => false,
            };
            if !still {
                if let Some(eng) = engine.as_mut() {
                    eng.stop_words(pane);
                }
                driving = None;
            }
            // Whatever ran is on the journal in its durable spelling. Onto the
            // sheet it goes, so the run leaves a script behind and not only a
            // result: do it once, and the second time is a ▶ away
            if let Some(eng) = engine.as_mut() {
                for line in eng.take_replay_lines() {
                    let js = serde_json::to_string(&line).unwrap_or_default();
                    shell.push_recorded(&js);
                    if let Some(r) = remote_ui.as_ref() {
                        r.push_state(format!("{{\"recorded\":{js}}}"));
                    }
                }
            }
        }
        for (text, bad) in take_words_notes() {
            let js = serde_json::to_string(&serde_json::json!({"text": text, "bad": bad}))
                .unwrap_or_else(|_| "null".into());
            shell.push_words_note(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"words\":{js}}}"));
            }
        }

        // ▶ run mode: composer Lua against the shown browser, in the rally's
        // sandbox (browser functions on that one tab, nothing else). The verdict
        // returns as a toast on both surfaces.
        for code in shell.mail().take_run_luas() {
            let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) else {
                continue;
            };
            // Running needs an engine; make a bare one if this desk didn't
            // otherwise have any Lua (same gap-filler as 🎯 operate).
            if engine.is_none() {
                engine = crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)).ok();
            }
            let Some(eng) = engine.as_mut() else { continue };
            let err = eng.run_browser_lua(key, &code);
            let js = serde_json::to_string(&err).unwrap_or_else(|_| "null".into());
            shell.push_lua_done(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"luadone\":{js}}}"));
            }
        }

        // 🩺 environment survey: DRAFT the fixed read-only probe (syntax
        // picked from the tab's launch command / prompt shape) into the
        // composer — the person reviews and sends it themselves, exactly
        // like a ✨ suggestion. Nothing types itself into a terminal. The
        // watcher below waits for the marker-wrapped output to appear
        if shell.mail().take_surveys() > 0 {
            match session_at(&surfaces, active).and_then(|i| tabs.get(i)) {
                Some(t) if t.ai_kind().is_none() => {
                    let screen =
                        t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents();
                    let probe = survey_probe(&t.command_line(), &screen);
                    let tab_id = t.id.clone().unwrap_or_else(|| t.title.clone());
                    // A generous window: the person may take their time
                    // pressing Send — or decide not to (then this just lapses)
                    append_hook_log(&format!("survey drafted for tab {tab_id}"));
                    pending_survey = Some((
                        tab_id,
                        std::time::Instant::now() + std::time::Duration::from_secs(300),
                    ));
                    let js = serde_json::json!({"stage": "draft", "cmd": probe}).to_string();
                    shell.push_surveyed(&js);
                    if let Some(r) = remote_ui.as_ref() {
                        r.push_state(format!("{{\"surveyed\":{js}}}"));
                    }
                }
                _ => {
                    append_hook_log("survey refused: active pane is not a plain terminal");
                    let js = serde_json::json!({"ok": false, "error": i18n::t("msg.suggest.no_tab")})
                        .to_string();
                    shell.push_surveyed(&js);
                    if let Some(r) = remote_ui.as_ref() {
                        r.push_state(format!("{{\"surveyed\":{js}}}"));
                    }
                }
            }
        }
        // Watch for the survey's end marker (event-paced: the loop's normal
        // tick, no sleeps). Lapses silently if the person never sent it
        if let Some((tab_id, deadline)) = pending_survey.clone() {
            let block = tabs
                .iter()
                .find(|t| t.id.as_deref() == Some(tab_id.as_str()) || t.title == tab_id)
                .and_then(|t| {
                    let s =
                        t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents();
                    extract_env_block(&s)
                });
            if let Some(env) = block {
                append_hook_log(&format!("survey [{tab_id}]: {}", log_excerpt(&env, 160)));
                env_cards.insert(tab_id, env);
                pending_survey = None;
                let js = r#"{"ok":true}"#;
                shell.push_surveyed(js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"surveyed\":{js}}}"));
                }
            } else if std::time::Instant::now() > deadline {
                pending_survey = None;
            }
        }

        // ✨ NL → command suggestions for the active terminal tab. The
        // assistant AI reads the tab's launch command plus the recent screen
        // (prompt strings, login banners, recent I/O — the environment's own
        // fingerprint), and — when the person ran 🔍 — the captured
        // environment card; the call runs on a worker thread and the answer
        // is polled right below
        // The Vault: search past conversations, and reopen one as a resuming tab.
        //
        // A search is answered into `vault_view`, which the state carries while
        // the overlay is open. Reopening writes a tab into the active
        // desk's settings; the change-watcher then launches it, resumed,
        // through the ordinary reload -- the one place a tab is safely made
        // What was said in one tab's folder before. Asked by a tab that came
        // up on a conversation of nobody's, and answered from the CLI's own
        // records -- the app's memory of that tab is exactly what is missing
        for which in shell.mail().take_past_lists() {
            past_view = past_of(&surfaces, &tabs, which as usize, &past_tx);
            // Asking is the moment the loss has been seen, so the caption goes
            // back to its ordinary manners: the offer stands while nobody has
            // spoken here, and no longer holds itself open past that
            if let Some(t) = session_at(&surfaces, which as usize).and_then(|i| tabs.get_mut(i)) {
                t.lost = false;
            }
        }
        // A list asked of another machine, back: put on the list that asked,
        // if it is still the one open
        while let Ok((which, hits)) = past_rx.try_recv() {
            if let Some(v) = past_view.as_mut().filter(|v| v.tab == which && v.asking) {
                v.hits = hits;
                v.asking = false;
            }
        }
        // One of them chosen: that tab is relaunched into it, the way every
        // other resume relaunches a tab. By its screen number, like the list
        // it was chosen from -- counted along the tabs instead, a page in
        // front of it would relaunch its neighbour into this conversation
        for (which, id) in shell.mail().take_past_resumes() {
            if let Some(t) = session_at(&surfaces, which as usize).and_then(|i| tabs.get_mut(i)) {
                flash = Some(resume_tab_into(t, id, rows, cols));
            }
            past_view = None;
        }
        for (query, wake) in shell.mail().take_vault_queries() {
            // The present, then the past. What is on screen right now across
            // every open tab comes first -- a live match is more likely the
            // thing being looked for than an old conversation -- then the
            // records on disk. One box finds both
            let mut hits: Vec<crate::vault::Hit> = Vec::new();
            if !query.trim().is_empty() {
                for (i, t) in tabs.iter().enumerate() {
                    for (_, line) in t.search_lines(&query, 6) {
                        hits.push(crate::vault::Hit {
                            program: String::new(),
                            id: String::new(),
                            cwd: None,
                            title: t.title.clone(),
                            snippet: line,
                            when: 0,
                            // The display number, not the tabs index: INDEX is
                            // surface 0, so tab i sits at i + 1 -- the number
                            // Select expects and a person presses
                            tab: Some(i + 1),
                            host: None,
                        });
                    }
                }
            }
            let found = crate::vault::search(&query, 40);
            hits.extend(found.hits);
            // Then every other machine a folder of any desk is on, each on a
            // thread of its own, its hits joining as they come. A paused
            // MicroVM is left out unless asked for: searching it starts it
            vault_seq += 1;
            let mut asking = 0;
            let mut sleeping = 0;
            let mut seen: Vec<String> = Vec::new();
            for d in &desks {
                for f in &d.folders {
                    let Some(host) = f.host.as_ref() else { continue };
                    let Ok(at) = crate::elsewhere::Elsewhere::of(host) else { continue };
                    let key = format!("{}\u{1f}{}", at.address(), host.instance.as_deref().unwrap_or_default());
                    if seen.contains(&key) {
                        continue;
                    }
                    seen.push(key);
                    if let Some(id) = host.instance.as_deref().filter(|_| host.is_made())
                        && !wake
                        && !crate::e2b::awake(id)
                    {
                        sleeping += 1;
                        continue;
                    }
                    asking += 1;
                    let (tx, q, seq, name) = (vault_far_tx.clone(), query.clone(), vault_seq, host.name.clone());
                    std::thread::spawn(move || {
                        let _ = tx.send((seq, crate::vault::search_far(&at, &name, &q, 40)));
                    });
                }
            }
            vault_view = Some(crate::uistate::VaultState {
                query,
                hits,
                capped: found.capped,
                asking,
                sleeping,
                seq: vault_seq,
            });
        }
        // Hits from another machine, joining the search that asked for them.
        // A conversation a copied machine carries from the one it was copied
        // from is listed once
        while let Ok((seq, far)) = vault_far_rx.try_recv() {
            let Some(v) = vault_view.as_mut().filter(|v| v.seq == seq) else { continue };
            v.asking = v.asking.saturating_sub(1);
            for h in far {
                if !v.hits.iter().any(|x| x.tab.is_none() && x.program == h.program && x.id == h.id) {
                    v.hits.push(h);
                }
            }
            // What is on screen first, then every machine's together, newest
            // first: a conversation of today on a MicroVM above a year-old one
            // here, as one list would have it
            v.hits.sort_by(|a, b| match (a.tab.is_some(), b.tab.is_some()) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                (true, true) => std::cmp::Ordering::Equal,
                (false, false) => b.when.cmp(&a.when),
            });
            v.hits.truncate(120);
        }
        // A folder renamed in the list, or taken out of it. Both are changes
        // to the settings, so the reload that follows is what actually shows
        // A tab renamed where it stands. The settings match a running tab to
        // its entry by title, so the running tab takes the new title first:
        // the reload that follows the write then finds it under that title,
        // rather than ending it and starting another under the new one
        for (index, name) in shell.mail().take_tab_names() {
            let wanted = name.trim();
            let Some(i) = session_at(&surfaces, index) else {
                // No session: a page, a git or file panel, an editor. Found by
                // where it is written, the way a tab moved to a folder is
                let Some(desk) = desks.get(desk_index) else { continue };
                let titles: Vec<&str> = tabs.iter().map(|t| t.title.as_str()).collect();
                let rows = surfaces_written(Some(desk), &titles, &caps.hosted_names(), &editors, issues_open);
                let written = index.checked_sub(1).and_then(|i| rows.get(i)).and_then(|(_, w)| *w);
                let Some((written, ft)) = written.and_then(|w| desk.tabs.get(w).map(|ft| (w, ft))) else {
                    flash = Some(i18n::t("err.tab.not_in_settings"));
                    continue;
                };
                let mark = config::TabMark::of(desk, ft);
                if let Err(e) = config::rename_tab_written(&desk.name, written, &mark, wanted) {
                    flash = Some(format!("{e:#}"));
                }
                continue;
            };
            let Some(old) = tabs.get(i).map(|t| t.title.clone()) else { continue };
            if !wanted.is_empty() && wanted != old && tabs.iter().any(|t| t.title == wanted) {
                flash = Some(i18n::tp("err.tab.name_taken", &[("name", wanted)]));
                continue;
            }
            let desk = desks.get(desk_index).map(|w| w.name.clone()).unwrap_or_default();
            match config::rename_tab(&desk, &old, wanted) {
                Ok(Some(title)) => {
                    if let Some(t) = tabs.get_mut(i) {
                        t.title = title;
                    }
                }
                Ok(None) => {}
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        // A tab that was waiting for somewhere to work, given a folder from its
        // own screen. Written into the settings, which is what starts it: the
        // reload that follows sees a tab with a folder and launches it there
        for (index, folder) in shell.mail().take_tab_folders() {
            let at = folder.trim();
            if at.is_empty() {
                continue;
            }
            let Some(desk) = desks.get(desk_index) else { continue };
            let titles: Vec<&str> = tabs.iter().map(|t| t.title.as_str()).collect();
            let rows = surfaces_written(Some(desk), &titles, &caps.hosted_names(), &editors, issues_open);
            let written = index
                .checked_sub(1)
                .and_then(|i| rows.get(i))
                .and_then(|(_, w)| *w);
            let Some(written) = written else { continue };
            let Some(ft) = desk.tabs.get(written) else { continue };
            let mark = config::TabMark::of(desk, ft);
            match config::move_tab_to_folder(
                &desk.name,
                written,
                &mark,
                std::path::Path::new(at),
            ) {
                Ok(_) => flash = Some(i18n::tp("msg.folder.tab_moved", &[("path", at)])),
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        for (folder, name) in shell.mail().take_folder_names() {
            let desk = desks.get(desk_index).map(|w| w.name.clone()).unwrap_or_default();
            if let Err(e) = config::rename_folder(&desk, std::path::Path::new(&folder), &name) {
                flash = Some(format!("{e:#}"));
            }
        }
        // Thrown away for good. Refused first, while nothing has happened yet,
        // so a folder with work in it is never closed on the way to a no.
        // Then the tabs are ended by taking the folder out of the settings --
        // git will not remove a folder something is still standing in -- and
        // the removal itself waits for them to actually be gone
        // A MicroVM folder checked on its machine: nothing unsaved there, and
        // the deleting goes on as it would have; something there, and nothing
        // happens but being told what
        while let Ok((folder, said)) = far_discard_rx.try_recv() {
            match said {
                Ok(()) => {
                    far_discard_checked.insert(folder.clone());
                    shell.mail().folder_discards.push((folder, false));
                }
                Err(why) => flash = Some(i18n::tp("msg.folder.not_discarded", &[("path", &folder), ("why", &why)])),
            }
        }
        for (folder, unasked) in shell.mail().take_folder_discards() {
            // Written before the folder is tried: the person asked not to be
            // asked again, whatever becomes of this one
            if unasked {
                config::save_setting(&["confirm_worktree_delete"], serde_json::json!(false));
            }
            let at = std::path::PathBuf::from(&folder);
            // A folder on a MicroVM is its machine: the machine goes, and with
            // it everything in the folder. The project's checkout there going
            // is the project's checkout there going -- the next worktree makes
            // a new one -- while the worktrees copied from it are machines of
            // their own and stay
            let on_microvm = desks.get(desk_index).and_then(|d| {
                d.folders
                    .iter()
                    .find(|f| f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, &at)))
                    .and_then(|f| f.host.clone().filter(|h| h.is_made()).map(|h| (h, f.project.clone())))
            });
            if let Some((h, project)) = on_microvm {
                // Asked of the machine first, on a thread: what is not
                // committed or not pushed there is lost with the machine, and
                // nothing is closed on the way to finding that out
                if !far_discard_checked.remove(&folder) {
                    let (tx, folder, host) = (far_discard_tx.clone(), folder.clone(), h.clone());
                    flash = Some(i18n::tp("msg.folder.checking", &[("path", &folder)]));
                    std::thread::spawn(move || {
                        let said = crate::worktree::far_ready_to_discard(&host, &folder).map_err(|e| format!("{e:#}"));
                        let _ = tx.send((folder, said));
                    });
                    continue;
                }
                let Some(d) = desks.get(desk_index) else { continue };
                let checkout = project
                    .as_deref()
                    .and_then(|n| d.projects.iter().find(|p| p.name == n))
                    .and_then(|p| p.home_on(&h.name))
                    .filter(|home| home.sandbox.is_some() && home.sandbox == h.instance);
                let checkout = checkout.cloned();
                // The folder first: taking it can be refused (the desk's last
                // folder), and a project that let go of its checkout while the
                // folder stayed would make a second one for its next worktree
                match config::take_folder(&d.name, &at) {
                    Ok(taken) => {
                        let mut dropped = None;
                        if let (Some(home), Some(p)) = (checkout, project.as_deref()) {
                            match config::drop_project_home(&d.id, p, &h.name) {
                                Ok(()) => {
                                    if let Some(id) = home.sandbox.as_deref() {
                                        crate::worktree::forget_checkout(id);
                                    }
                                    dropped = Some((d.id.clone(), p.to_string(), home));
                                }
                                Err(e) => {
                                    // The folder goes back where it was, and
                                    // nothing is deleted
                                    if let Some(t) = &taken {
                                        let _ = config::put_folder_back(&d.name, t);
                                    }
                                    flash = Some(format!("{e:#}"));
                                    continue;
                                }
                            }
                        }
                        let removal = crate::worktree::Removal::start_on_microvm(at.clone(), h);
                        // The throwaway editor is in no list the settings
                        // keep, so the reload that ends the tabs leaves it
                        editors.retain(|e| !(e.scratch && e.dir.as_deref().is_some_and(|d| removal.takes(d, e.at.as_ref()))));
                        making_seq += 1;
                        leavings.push(Leaving {
                            id: making_seq,
                            family: String::new(),
                            name: at.to_string_lossy().rsplit('/').next().unwrap_or_default().to_string(),
                            removal,
                            desk: d.name.clone(),
                            main: None,
                            taken,
                            checkout: dropped,
                            error: None,
                            restored: None,
                            gone: false,
                        });
                    }
                    Err(e) => flash = Some(format!("{e:#}")),
                }
                continue;
            }
            if let Err(e) = crate::worktree::ready_to_discard(&at) {
                flash = Some(format!("{e:#}"));
                continue;
            }
            let desk = desks.get(desk_index).map(|w| w.name.clone()).unwrap_or_default();
            // Read while the folder is still a worktree: once it is going, git
            // can no longer say whose it is or what branch it was on
            let family = crate::repo::family_of(&at).map(|f| f.display().to_string()).unwrap_or_default();
            let main = crate::repo::main_checkout(&at);
            let name = crate::repo::branch_of(&at)
                .or_else(|| at.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default();
            match config::take_folder(&desk, &at) {
                // Said once the folder is really gone, not when it was asked
                // to go: the row says it is going until then
                Ok(taken) => {
                    let removal = crate::worktree::Removal::start(at);
                    editors.retain(|e| !(e.scratch && e.dir.as_deref().is_some_and(|d| removal.takes(d, e.at.as_ref()))));
                    making_seq += 1;
                    leavings.push(Leaving {
                        id: making_seq,
                        removal,
                        desk,
                        family,
                        name,
                        main,
                        taken,
                        checkout: None,
                        error: None,
                        restored: None,
                        gone: false,
                    });
                }
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        // The public addresses of a folder on a MicroVM: asked of the machine
        // on a thread, since asking starts one that was paused
        for folder in shell.mail().take_far_ports() {
            let at = std::path::PathBuf::from(&folder);
            let host = desks.get(desk_index).and_then(|d| {
                d.folders
                    .iter()
                    .find(|f| f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, &at)))
                    .and_then(|f| f.host.clone())
                    .filter(|h| h.is_made())
            });
            let Some(host) = host else { continue };
            far_ports_view = Some(crate::uistate::FarPortsState { folder: folder.clone(), busy: true, ..Default::default() });
            let tx = far_ports_tx.clone();
            std::thread::spawn(move || {
                let said = crate::microvm::ports_of(&host);
                let _ = tx.send(match said {
                    Ok(ports) => crate::uistate::FarPortsState { folder, busy: false, ports, error: String::new() },
                    Err(error) => crate::uistate::FarPortsState { folder, busy: false, ports: Vec::new(), error },
                });
            });
        }
        // One of a MicroVM's public addresses, opened in a browser tab here.
        // Only an address this app found for that folder: the page names the
        // port, and the address is looked up rather than taken from it
        for (folder, port) in shell.mail().take_far_pages() {
            let found = far_ports_view
                .as_ref()
                .filter(|v| crate::uistate::same_folder(std::path::Path::new(&v.folder), std::path::Path::new(&folder)))
                .and_then(|v| v.ports.iter().find(|p| p.port == port))
                .map(|p| p.url.clone());
            let Some(url) = found else {
                flash = Some(i18n::t("tui.urls.gone"));
                continue;
            };
            let short = folder.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string();
            let name = format!("{short}:{port}");
            match caps.browser_open(&name, &url, shikisha_shared::BrowserProfile::shared_default()) {
                Ok(()) => reveal = Some((name, Instant::now() + Duration::from_secs(10))),
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        while let Ok(answer) = far_ports_rx.try_recv() {
            // Only the folder last asked about
            if far_ports_view.as_ref().is_none_or(|v| v.folder == answer.folder) {
                far_ports_view = Some(answer);
            }
        }
        // The folders whose tabs are on their way out. Tried again each time
        // round until git can have it, and given up on out loud rather than
        // silently -- a folder that was asked to go and did not is a surprise
        // waiting in the settings
        for folder in shell.mail().take_folder_closes() {
            let desk = desks.get(desk_index).map(|w| w.name.clone()).unwrap_or_default();
            let at = std::path::Path::new(&folder);
            // The tabs standing in it go with it, and that is the point: a
            // folder whose tabs are started again on every launch can never be
            // emptied by hand, so asking for an empty folder first was asking
            // for the one thing the app undoes each time it opens. Only a tab
            // in the middle of something is in the way -- the same two states
            // a tab's own close stops to ask about, for the same reason
            if let Some(busy) = tabs.iter().find(|t| {
                t.cwd().is_some_and(|c| crate::uistate::same_folder(c, at))
                    && matches!(t.state, TabState::Busy | TabState::Question)
            }) {
                flash = Some(i18n::tp("msg.folder.in_use", &[("name", &busy.title)]));
                continue;
            }
            // On a MicroVM the folder is its machine, and taking it off the
            // list leaves the machine as it leaves files on a disk -- but a
            // machine is paid for. Said, with where it is deleted from
            let on_microvm = desks.get(desk_index).is_some_and(|d| {
                d.folders.iter().any(|f| {
                    f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, at))
                        && f.host.as_ref().is_some_and(|h| h.is_made())
                })
            });
            match config::remove_folder(&desk, at) {
                // Said out loud, because the folder is still on disk and this
                // is the only sign that it was left there on purpose. Said by
                // the reload that closes its tabs, too: "settings reloaded" is
                // not what happened, and it is the last word otherwise
                Ok(()) => {
                    let said = i18n::tp(
                        if on_microvm { "msg.folder.closed_microvm" } else { "msg.folder.closed" },
                        &[("path", &folder)],
                    );
                    said_before_reload = Some((Instant::now(), said.clone()));
                    flash = Some(said);
                }
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        // Somewhere new to work. Looking hands back what is inside; choosing
        // writes the folder into the settings, and the reload opens it
        for (path, open, make) in shell.mail().take_browses() {
            if !open {
                // The projects this desk already works in, as places to start
                // from. This desk's only: a desk is kept apart from the others,
                // and a picker that offered another desk's checkouts would be
                // the one place the wall between them had a door in it
                let projects: Vec<(String, String)> = desks
                    .get(desk_index)
                    .map(|w| {
                        w.folders
                            .iter()
                            .filter_map(|f| f.cwd.as_ref())
                            .filter(|c| !crate::repo::is_linked(c) && crate::repo::family_of(c).is_some())
                            .map(|c| {
                                let name = c
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                (name, c.display().to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // Made first, then looked at again, so the new folder is in the
                // list the answer points at
                let (made, made_error) = match make.trim().is_empty() {
                    true => (None, None),
                    false => match crate::uistate::make_folder(&path, &make) {
                        Ok(p) => (Some(p), None),
                        Err(e) => (None, Some(e)),
                    },
                };
                let mut view = crate::uistate::BrowseState::of(&path).with_places(&projects);
                view.made = made;
                view.made_error = made_error;
                browse_view = Some(view);
                continue;
            }
            browse_view = None;
            match add_to_desk(desks.get(desk_index), std::path::Path::new(&path), &config::default_shell_start(cfg.as_ref().and_then(|c| c.default_shell.as_deref()))) {
                // Said by the reload that brings it onto the list, instead of
                // "settings reloaded": what happened is that a project arrived
                Ok(Added::New(said)) => {
                    said_before_reload = Some((Instant::now(), said.clone()));
                    flash = Some(said);
                }
                Ok(Added::Already(said)) | Err(said) => flash = Some(said),
            }
        }
        // A project's worktrees that git knows and the desk does not list. Shown
        // means put on the desk, each as a folder with nothing started in it --
        // they were made somewhere else, and what runs in them is for the
        // person to say. Kept hidden is remembered on this machine, per project
        for (family, act) in shell.mail().take_found() {
            match act.as_str() {
                "show" => {
                    let Some(desk) = desks.get(desk_index) else { continue };
                    let here: Vec<std::path::PathBuf> = desk.folders.iter().filter_map(|f| f.cwd.clone()).collect();
                    let found = folders::watch().cuts(&here).remove(std::path::Path::new(&family)).unwrap_or_default();
                    let mut added = 0;
                    for (folder, branch) in found {
                        if here.iter().any(|h| crate::uistate::same_folder(h, &folder)) {
                            continue;
                        }
                        match config::append_folder_starting(&desk.name, None, &folder, branch.as_deref(), &config::Start::Nothing, None) {
                            Ok(()) => added += 1,
                            Err(e) => flash = Some(format!("{e:#}")),
                        }
                    }
                    if added > 0 {
                        let said = i18n::tp("msg.found.shown", &[("n", &added.to_string())]);
                        said_before_reload = Some((Instant::now(), said.clone()));
                        flash = Some(said);
                    }
                    worktrees_kept.remove(&family);
                    save_kept(&worktrees_kept);
                }
                "keep" => {
                    worktrees_kept.insert(family);
                    save_kept(&worktrees_kept);
                }
                "offer" => {
                    worktrees_kept.remove(&family);
                    save_kept(&worktrees_kept);
                }
                _ => {}
            }
        }
        // The rows of worktrees being made: stopped, tried again, put away
        for (id, act) in shell.mail().take_makings() {
            // A job on a machine: stopped after the command it is on (a clone
            // stopped throws its machine away; a preparation keeps what was
            // done), tried again from the start, or put away
            if let Some(j) = vm_jobs.iter_mut().find(|j| j.id == id) {
                match act.as_str() {
                    "stop" if j.error.is_none() => {
                        j.stopping = true;
                        match &j.work {
                            VmWork::Clone { job, .. } => job.stop(),
                            VmWork::Prepare { job, .. } => job.stop(),
                            VmWork::SshClone { job, .. } => job.stop(),
                        }
                    }
                    // Finished, and only writing it down failed: that is what
                    // is tried again. Running it again would make a second
                    // machine and leave the first one nobody's
                    "retry" if j.error.is_some() && j.finished().is_some() => j.error = None,
                    "retry" if j.error.is_some() => {
                        j.stopping = false;
                        let again = match &j.work {
                            VmWork::Clone { add, url, sign_in, preparing, .. } => Ok(VmWork::Clone {
                                job: crate::microvm::Checkout::start(j.host.clone(), url, &j.project, sign_in.clone(), preparing.clone()),
                                add: add.clone(),
                                url: url.clone(),
                                sign_in: sign_in.clone(),
                                preparing: preparing.clone(),
                            }),
                            VmWork::Prepare { home, preparing, follow, .. } => Ok(VmWork::Prepare {
                                job: crate::microvm::Prepare::start(j.host.clone(), &home.at, preparing.clone()),
                                home: home.clone(),
                                preparing: preparing.clone(),
                                follow: follow.clone(),
                            }),
                            VmWork::SshClone { spec, url, parent, .. } => crate::addproject::start_clone_on(spec.clone(), url, parent)
                                .map(|job| VmWork::SshClone { job, spec: spec.clone(), url: url.clone(), parent: parent.clone() }),
                        };
                        match again {
                            Ok(work) => {
                                j.work = work;
                                j.error = None;
                            }
                            Err(e) => j.error = Some(e),
                        }
                    }
                    "dismiss" if j.error.is_some() => {
                        // A clone that finished and was never written down is a
                        // machine nothing will point at: it goes with the row
                        if let (VmWork::Clone { .. }, Some(id)) = (&j.work, j.finished()) {
                            let_go_if_nobodys(id);
                        }
                        j.gone = true;
                    }
                    _ => {}
                }
                continue;
            }
            // A worktree on its way out answers through the same row. Its
            // folder would not go, and what is left of it is kept on disk, put
            // back in the list, or tried again
            if let Some(l) = leavings.iter_mut().find(|l| l.id == id && l.error.is_some()) {
                match act.as_str() {
                    "retry" => {
                        l.error = None;
                        l.removal = l.removal.again();
                    }
                    "restore" => match &l.taken {
                        Some(taken) => match config::put_folder_back(&l.desk, taken) {
                            Ok(()) => {
                                l.removal.put_back();
                                // A project's checkout is the project's again
                                if let Some((desk_id, project, home)) = &l.checkout
                                    && let Err(e) = config::set_project_home(desk_id, project, home, None) {
                                        flash = Some(format!("{e:#}"));
                                    }
                                l.restored = Some(Instant::now());
                            }
                            Err(e) => flash = Some(format!("{e:#}")),
                        },
                        None => l.gone = true,
                    },
                    "forget" => {
                        // Git may still list a worktree it refused to remove.
                        // Forgetting one whose folder is gone is all prune does
                        if let Some(main) = &l.main {
                            let mut prune = std::process::Command::new("git");
                            prune.arg("-C").arg(main).args(["worktree", "prune"]);
                            let _ = crate::detach_console(&mut prune).output();
                        }
                        // A machine that would not be deleted is still paid
                        // for, and the list in the settings is where it goes
                        let said = if l.removal.on_microvm() { "msg.folder.closed_microvm" } else { "msg.folder.left" };
                        flash = Some(i18n::tp(said, &[("path", &l.removal.folder.display().to_string())]));
                        l.gone = true;
                    }
                    _ => {}
                }
                continue;
            }
            let Some(p) = makings.iter_mut().find(|p| p.id == id) else { continue };
            match act.as_str() {
                "stop" if p.error.is_none() && !p.made && p.written.is_none() => p.making.stop(),
                "retry" if p.error.is_some() => {
                    p.error = None;
                    p.trust = None;
                    // Made already, and only writing it down failed: that
                    // is what is tried again, since the folder is there
                    if !p.made {
                        p.making = crate::worktree::Making::start(p.making.plan.clone(), p.carry.clone());
                    }
                }
                // The person said yes to the line git asked for. It goes into
                // their own git settings and nowhere else, and what stopped
                // for want of it goes on: a branch that was refused is cut
                // again, and one that was made is simply usable
                "trust" => {
                    let Some(value) = p.trust.clone() else { continue };
                    match crate::trust::add(&value) {
                        Ok(()) => {
                            p.trust = None;
                            flash = Some(i18n::tp("msg.trust.added", &[("what", &value)]));
                            if p.error.take().is_some() && !p.made {
                                p.making = crate::worktree::Making::start(
                                    p.making.plan.clone(),
                                    p.carry.clone(),
                                );
                            }
                        }
                        Err(e) => p.error = Some(format!("{e:#}")),
                    }
                }
                // The person said yes to copying in what could not be shared
                // with the project. The same carrying, asked for as a copy
                "copy_instead" if !p.unlinked.is_empty() => {
                    let names = std::mem::take(&mut p.unlinked);
                    let carry: Vec<crate::worktree::Carry> = p
                        .carry
                        .iter()
                        .filter(|c| names.contains(&c.name))
                        .map(|c| crate::worktree::Carry { how: "copy".into(), ..c.clone() })
                        .collect();
                    let job = crate::worktree::Making::carrying(p.making.plan.clone(), carry);
                    p.copying = Some((job, names));
                }
                "dismiss" if p.error.is_some() || p.trust.is_some() || !p.unlinked.is_empty() => {
                    // Left as it is, on purpose. The folder stays; git will
                    // say the same thing in it until somebody says otherwise,
                    // and what was to be shared is simply not there
                    p.trust = None;
                    p.unlinked.clear();
                    p.gone = p.error.is_some();
                    // A worktree copied onto a machine of its own and never
                    // written down: nothing will ever point at that machine
                    if p.gone && p.made && p.written.is_none()
                        && let Some(id) = p.making.machines().worktree {
                            let_go_if_nobodys(id);
                        }
                }
                _ => {}
            }
        }
        // How each is getting on. Made: written down, and the row waits for
        // the card that replaces it
        for p in makings.iter_mut().filter(|p| p.error.is_none() && p.written.is_none() && !p.gone) {
            // A checkout a MicroVM making made is the project's the moment it
            // is there, however the rest of the making goes
            if let Err(e) = p.note_checkout() {
                append_hook_log(&format!("could not write down the checkout of {}: {e:#}", p.making.plan.project));
                flash = Some(format!("{e:#}"));
            }
            if !p.made {
                match p.making.outcome() {
                    None => continue,
                    Some(Err(why)) if why.is_empty() => {
                        p.gone = true;
                        continue;
                    }
                    Some(Err(why)) => {
                        append_hook_log(&format!("could not make {}: {why}", p.making.plan.branch));
                        // Git refuses a project whose files belong to another
                        // account -- a share on another machine is exactly
                        // that -- and says which folder it wants written down
                        // as one to trust. Its own words are kept as the
                        // reason; the row turns them into the one press that
                        // answers them
                        p.trust = crate::trust::asked_for(&why).map(|v| crate::trust::spread(&v));
                        p.error = Some(why);
                        continue;
                    }
                    Some(Ok(brought)) => {
                        p.made = true;
                        // The branch's own folder is the second place git
                        // stops: the folder is here, and the git folder it
                        // belongs to is on the share. Asked of git rather than
                        // assumed, and only about a folder on this machine
                        if p.making.plan.host.is_none() {
                            p.trust = crate::trust::refused(&p.making.plan.folder)
                                .map(|v| crate::trust::spread(&v));
                        }
                        // What has no second name here is not said in passing:
                        // the row asks, and the answer is a press
                        p.unlinked = brought.unlinked.clone();
                        let said = brought_note(&p.making.plan.branch, &brought);
                        said_before_reload = Some((Instant::now(), said.clone()));
                        flash = Some(said);
                    }
                }
            }
            match p.write_down() {
                Ok(()) => {
                    p.written = Some(Instant::now());
                    remember_work_item(&p.desk, &p.making.plan.folder, &p.link, &mut pending_drafts);
                }
                Err(e) => p.error = Some(format!("{e:#}")),
            }
        }
        // Copying in what the branch could not share with the project, once
        // the person said to. Its row waits on it and says how it went
        for p in makings.iter_mut().filter(|p| p.copying.is_some()) {
            let Some(done) = p.copying.as_ref().and_then(|(job, _)| job.outcome()) else {
                continue;
            };
            let Some((_, names)) = p.copying.take() else { continue };
            let missed = match done {
                Ok(b) => b.missed,
                Err(why) => {
                    append_hook_log(&format!("could not copy into {}: {why}", p.making.plan.folder.display()));
                    names.clone()
                }
            };
            flash = Some(match missed.is_empty() {
                true => i18n::tp("msg.branch.copied_in", &[("names", &names.join(", "))]),
                false => i18n::tp("msg.branch.copy_failed", &[("names", &missed.join(", "))]),
            });
        }
        makings.retain(|p| {
            let listed = || {
                desks.iter().filter(|d| d.name == p.desk).any(|d| {
                    d.folders
                        .iter()
                        .any(|f| f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, &p.making.plan.folder)))
                })
            };
            // A row with a question on it stays until the question is
            // answered: the folder is on the desk already, and nothing else on
            // screen would say that git will not work in it, or that what the
            // branch was to share with the project is not there. A row doing
            // what an answer asked for stays until that is done
            !p.gone
                && (p.trust.is_some()
                    || !p.unlinked.is_empty()
                    || p.copying.is_some()
                    || !p.written.is_some_and(|at| at.elapsed() > MAKING_CARD_WAIT || listed()))
        });
        // The worktrees being deleted. Gone is said then and not before; a
        // folder that stayed keeps its row and asks
        for l in leavings.iter_mut().filter(|l| l.error.is_none() && l.restored.is_none() && !l.gone) {
            match l.removal.outcome() {
                None => {}
                Some(Ok(())) => {
                    flash = Some(i18n::tp("msg.folder.discarded", &[("path", &l.removal.folder.display().to_string())]));
                    // The project it was cut from has one worktree fewer. Said
                    // now rather than waited out, so the line offering the
                    // found ones stops offering a folder that is gone
                    if let Some(main) = &l.main {
                        crate::folders::watch().forget(main);
                    }
                    l.gone = true;
                }
                Some(Err(why)) => l.error = Some(why),
            }
        }
        leavings.retain(|l| {
            let listed = || {
                desks.iter().filter(|d| d.name == l.desk).any(|d| {
                    d.folders
                        .iter()
                        .any(|f| f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, &l.removal.folder)))
                })
            };
            !l.gone && !l.restored.is_some_and(|at| at.elapsed() > MAKING_CARD_WAIT || listed())
        });
        // A project from a URL, or made new. Cloning takes as long as the
        // network does, so it runs on its own and is looked at every turn; a
        // new project is a folder and two quick git commands, done here
        // The machine a project is asked for on, by name, of the kind asked
        // for: one reached over SSH has folders to add and clone into; a
        // MicroVM has the one a clone onto it makes
        let host_named = |name: &str, made: bool| {
            cfg.as_ref().and_then(|c| c.hosts.iter().find(|h| h.name == name && h.is_made() == made).cloned())
        };
        // A server's GitHub accounts, answered to the page that asked. A
        // server that could not be asked has none to offer: the page still
        // offers to name one, and to use what the server's git has in front
        if let Some((ask, said)) = ssh_accounts_answer.lock().unwrap_or_else(|e| e.into_inner()).take() {
            if let Err(e) = &said {
                append_hook_log(&format!("could not ask a server for its GitHub accounts: {e}"));
            }
            add_view = Some(crate::uistate::AddProjectState { ask, accounts: Some(said.unwrap_or_default()), ..Default::default() });
        }
        for a in shell.mail().take_add_projects() {
            let (how, text, parent, ask, host) = (a.how.clone(), a.text.clone(), a.parent.clone(), a.ask, a.host.clone());
            let failed = |e: String| Some(crate::uistate::AddProjectState { ask, error: Some(e), ..Default::default() });
            let made = matches!(how.as_str(), "microvm" | "microvm_look");
            let on = match host.is_empty() {
                true => None,
                false => match host_named(&host, made) {
                    Some(h) => Some(h),
                    None => {
                        add_view = failed(i18n::tp("err.addproj.no_host", &[("host", &host)]));
                        continue;
                    }
                },
            };
            match (how.as_str(), on) {
                ("stop", _) => {
                    if let Some((_, job)) = &add_job {
                        job.stop();
                    }
                }
                // What a clone onto a MicroVM would sign in as, asked while
                // the address is typed, so it is said before anything is made
                ("microvm_look", Some(_)) => {
                    let desk = desks.get(desk_index);
                    // The account the dialog chose; the one for the address's
                    // owner when it chose none (an older board)
                    let account = match a.account.as_str() {
                        "" => account_for_url(desk, &text),
                        config::THIS_PC => None,
                        chosen => Some(chosen.to_string()),
                    };
                    let git = desk.map(|d| d.git_use(account.as_deref())).unwrap_or_default();
                    let label = account.clone().unwrap_or_else(|| i18n::t("tui.branch.signin.pc"));
                    let far = git.far(&|k| crate::git::secret(k));
                    let note = crate::microvm::sign_in_note(&label, &far);
                    signin_waiting = note.is_none().then(|| (label.clone(), far.clone()));
                    add_view = Some(crate::uistate::AddProjectState { ask, sign_in: note, microvm: true, ..Default::default() });
                }
                ("microvm", Some(h)) => {
                    let desk = desks.get(desk_index);
                    // The account the dialog chose; the one for the address's
                    // owner when it chose none (an older board)
                    let account = match a.account.as_str() {
                        "" => account_for_url(desk, &text),
                        config::THIS_PC => None,
                        chosen => Some(chosen.to_string()),
                    };
                    let git = desk.map(|d| d.git_use(account.as_deref())).unwrap_or_default();
                    // Named for its address, and never onto a project of that
                    // name that already has a checkout on this machine
                    let base = crate::microvm::name_of_url(&text);
                    let taken = |n: &str| desk.is_some_and(|w| w.projects.iter().any(|p| p.name == n && p.home_on(&h.name).is_some()));
                    let project = match taken(&base) {
                        false => base.clone(),
                        true => (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).expect("endless"),
                    };
                    // Prepared with the AI the dialog chose, and -- for a
                    // project already written down -- its machine setup
                    let written = desk.and_then(|w| w.projects.iter().find(|p| p.name == project));
                    let preparing = crate::microvm::Preparing::of(Some(&a.ai), written.and_then(|p| p.machine_setup.as_deref()));
                    if let Err(e) = preparing.commands(crate::worktree::MICROVM_HOME) {
                        add_view = failed(e);
                        continue;
                    }
                    let sign_in = git.far(&|k| crate::git::secret(k)).unwrap_or_default();
                    let job = crate::microvm::Checkout::start(h.clone(), &text, &project, sign_in.clone(), preparing.clone());
                    making_seq += 1;
                    vm_jobs.push(VmJob {
                        id: making_seq,
                        desk: desk.map(|d| d.name.clone()).unwrap_or_default(),
                        desk_id: desk.map(|d| d.id.clone()).unwrap_or_default(),
                        project: project.clone(),
                        host: h.clone(),
                        at: crate::microvm::checkout_path(&project),
                        work: VmWork::Clone { job, add: MicrovmAdd { host: h.name.clone(), project, account, ai: preparing.ai.clone() }, url: text.clone(), sign_in, preparing },
                        error: None,
                        stopping: false,
                        gone: false,
                    });
                    // The dialog closes on this: the row says the rest
                    add_view = Some(crate::uistate::AddProjectState { ask, started: true, microvm: true, ..Default::default() });
                }
                // Which GitHub accounts GitHub CLI on the server holds, for
                // the clone page to offer
                ("ssh_accounts", Some(h)) => match config::host_spec(&h) {
                    Ok(spec) => {
                        let slot = ssh_accounts_answer.clone();
                        std::thread::spawn(move || {
                            let said = crate::addproject::server_github_accounts(&spec);
                            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((ask, said));
                        });
                    }
                    Err(e) => add_view = failed(format!("{e:#}")),
                },
                // Onto a server: a row on the board, as a MicroVM's clone is,
                // and the dialog closes on it. With a GitHub account chosen,
                // the address carries its login, so the clone -- and the
                // project after it -- signs in as that account whichever one
                // GitHub CLI there has in front
                ("clone", Some(h)) => {
                    let text = crate::addproject::address_as(&text, &a.account);
                    let started = config::host_spec(&h).map_err(|e| format!("{e:#}")).and_then(|spec| {
                        crate::addproject::start_clone_on(spec.clone(), &text, &parent).map(|job| (job, spec))
                    });
                    match started {
                        Ok((job, spec)) => {
                            let desk = desks.get(desk_index);
                            let project = crate::addproject::repo_name_of(&text).unwrap_or_default();
                            making_seq += 1;
                            vm_jobs.push(VmJob {
                                id: making_seq,
                                desk: desk.map(|d| d.name.clone()).unwrap_or_default(),
                                desk_id: desk.map(|d| d.id.clone()).unwrap_or_default(),
                                at: crate::addproject::remote_join(&parent, &project),
                                project,
                                host: h.clone(),
                                work: VmWork::SshClone { job, spec, url: text.clone(), parent: parent.clone() },
                                error: None,
                                stopping: false,
                                gone: false,
                            });
                            add_view = Some(crate::uistate::AddProjectState { ask, started: true, ..Default::default() });
                        }
                        Err(e) => add_view = failed(e),
                    }
                }
                ("clone", None) if add_job.is_none() => {
                    match crate::addproject::start_clone(&text, &parent) {
                        Ok(job) => {
                            add_view = Some(crate::uistate::AddProjectState { ask, running: true, ..Default::default() });
                            add_job = Some((ask, job));
                        }
                        Err(e) => add_view = failed(e),
                    }
                }
                // A folder over there, chosen in the dialog's own listing
                ("remote", Some(h)) => {
                    add_view = match add_remote_to_desk(desks.get(desk_index), &h.name, &text, Some(&a.project)) {
                        Ok(Added::New(said)) | Ok(Added::Already(said)) => {
                            said_before_reload = Some((Instant::now(), said.clone()));
                            flash = Some(said);
                            Some(crate::uistate::AddProjectState { ask, done: Some(text.clone()), host: h.name.clone(), ..Default::default() })
                        }
                        Err(e) => failed(e),
                    };
                }
                ("create", Some(_)) => add_view = failed(i18n::t("err.addproj.remote_create")),
                ("create", None) => {
                    add_view = Some(match crate::addproject::create(&text, &parent) {
                        Ok(at) => match add_to_desk(desks.get(desk_index), &at, &config::default_shell_start(cfg.as_ref().and_then(|c| c.default_shell.as_deref()))) {
                            Ok(Added::New(said)) | Ok(Added::Already(said)) => {
                                said_before_reload = Some((Instant::now(), said.clone()));
                                flash = Some(said);
                                crate::uistate::AddProjectState { ask, done: Some(at.display().to_string()), ..Default::default() }
                            }
                            Err(e) => crate::uistate::AddProjectState { ask, error: Some(e), ..Default::default() },
                        },
                        Err(e) => crate::uistate::AddProjectState { ask, error: Some(e), ..Default::default() },
                    });
                }
                _ => {}
            }
        }
        // A folder on another machine to list, and the listings that came back.
        // Only the newest listing the dialog asked for is kept
        for (host, path, ask) in shell.mail().take_remote_lists() {
            let Some(h) = host_named(&host, false) else {
                remote_view = Some(crate::uistate::RemoteListState {
                    ask,
                    host: host.clone(),
                    error: Some(i18n::tp("err.addproj.no_host", &[("host", &host)])),
                    ..Default::default()
                });
                continue;
            };
            remote_view = Some(crate::uistate::RemoteListState { ask, host: host.clone(), busy: true, ..Default::default() });
            let tx = listing_tx.clone();
            std::thread::spawn(move || {
                let listed = config::host_spec(&h)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|spec| crate::addproject::list_remote(&spec, &path));
                let _ = tx.send(match listed {
                    Ok(l) => crate::uistate::RemoteListState { ask, host, busy: false, at: l.at, dirs: l.dirs, git: l.git, error: None },
                    Err(e) => crate::uistate::RemoteListState { ask, host, error: Some(e), ..Default::default() },
                });
            });
        }
        while let Ok(answer) = listing_rx.try_recv() {
            if remote_view.as_ref().is_none_or(|v| v.ask <= answer.ask) {
                remote_view = Some(answer);
            }
        }
        // A machine written into the settings from the dialog. The dialog goes
        // on with it chosen once the settings are read back
        for h in shell.mail().take_add_hosts() {
            let ask = h.ask;
            let key = Some(h.key.trim().to_string()).filter(|k| !k.is_empty());
            let spec = config::HostSpec { name: h.name.trim().to_string(), at: h.at, key, ..Default::default() };
            // A password goes to the secret store, under the name the
            // connection reads it by, before the host is written: the settings
            // read in again after the host is written are read with it there
            let kept = match h.password.is_empty() {
                true => Ok(()),
                false => cfg
                    .as_ref()
                    .and_then(|c| c.secrets_path())
                    .ok_or_else(|| anyhow::anyhow!(i18n::t("err.host.no_store")))
                    .and_then(|path| {
                        let meta = config::SecretMeta {
                            human: true,
                            ai: false,
                            urls: Vec::new(),
                            desc: format!("SSH {}", spec.name),
                        };
                        config::upsert_secret(&path, password.as_deref(), &format!("ssh/host/{}/password", spec.name), &meta, &h.password)
                    }),
            };
            if let Err(e) = kept {
                add_view = Some(crate::uistate::AddProjectState { ask, error: Some(format!("{e:#}")), ..Default::default() });
                continue;
            }
            add_view = Some(match config::add_host(&spec) {
                Ok(()) => {
                    let said = i18n::tp("msg.host.added", &[("name", &spec.name)]);
                    said_before_reload = Some((Instant::now(), said.clone()));
                    flash = Some(said);
                    crate::uistate::AddProjectState { ask, done: Some(spec.name.clone()), host: spec.name.clone(), ..Default::default() }
                }
                Err(e) => crate::uistate::AddProjectState { ask, error: Some(format!("{e:#}")), ..Default::default() },
            });
        }
        if let Some((ask, job)) = add_job.clone() {
            match job.outcome() {
                crate::addproject::Outcome::Running(p) => {
                    add_view = Some(crate::uistate::AddProjectState {
                        ask,
                        running: true,
                        phase: p.phase,
                        percent: p.percent,
                        ..Default::default()
                    });
                }
                crate::addproject::Outcome::Done(at) => {
                    add_job = None;
                    let added = add_to_desk(desks.get(desk_index), &at, &config::default_shell_start(cfg.as_ref().and_then(|c| c.default_shell.as_deref())));
                    add_view = Some(match added {
                        Ok(Added::New(said)) | Ok(Added::Already(said)) => {
                            said_before_reload = Some((Instant::now(), said.clone()));
                            flash = Some(said);
                            crate::uistate::AddProjectState { ask, done: Some(at.display().to_string()), ..Default::default() }
                        }
                        Err(e) => crate::uistate::AddProjectState { ask, error: Some(e), ..Default::default() },
                    });
                }
                crate::addproject::Outcome::Failed(e) => {
                    add_job = None;
                    add_view = Some(crate::uistate::AddProjectState { ask, error: Some(e), ..Default::default() });
                }
            }
        }
        // A project's checkouts on MicroVMs, asked by the settings page to be
        // prepared as its saved settings say: one row per machine
        for ask in crate::webui::take_prepare_asks() {
            let targets = match crate::microvm::prepare_targets(&ask.desk_id, &ask.project) {
                Ok(t) => t,
                Err(e) => {
                    flash = Some(e);
                    continue;
                }
            };
            let desk_name = desks.iter().find(|d| d.id == ask.desk_id).map(|d| d.name.clone()).unwrap_or_default();
            for (host, home) in targets.homes {
                // One at a time per machine: a second ask while the first is
                // still on it would run the same install twice at once
                if vm_jobs.iter().any(|j| !j.gone && j.error.is_none() && j.host.instance == host.instance) {
                    continue;
                }
                making_seq += 1;
                vm_jobs.push(VmJob {
                    id: making_seq,
                    desk: desk_name.clone(),
                    desk_id: ask.desk_id.clone(),
                    project: ask.project.clone(),
                    at: home.at.clone(),
                    work: VmWork::Prepare {
                        job: crate::microvm::Prepare::start(host.clone(), &home.at, targets.preparing.clone()),
                        home,
                        preparing: targets.preparing.clone(),
                        follow: ask.follow.clone(),
                    },
                    host,
                    error: None,
                    stopping: false,
                    gone: false,
                });
            }
        }
        // How each job on a machine is getting on. Done, what it made is
        // written down and the board is told what comes next; failed, the
        // row says so and waits to be tried again or put away
        for j in vm_jobs.iter_mut().filter(|j| j.error.is_none() && !j.gone) {
            // A server's clone: the folder there written on the desk, and the
            // project on through its rules, as a MicroVM's goes. A stopped
            // one has taken back what it made and goes with its row
            if let VmWork::SshClone { job, spec, url, parent } = &j.work {
                match job.outcome() {
                    crate::addproject::Outcome::Running(_) => {}
                    crate::addproject::Outcome::Failed(_) if j.stopping => j.gone = true,
                    crate::addproject::Outcome::Failed(e) => {
                        append_hook_log(&format!("could not clone {} on {}: {e}", j.project, j.host.name));
                        // The server's git could not sign in to GitHub: the
                        // step that walks the person through giving it one
                        // opens, the server looked at first to draft for it.
                        // The row keeps what git said either way
                        if crate::git::refused_sign_in(&e).is_some()
                            && crate::addproject::is_github(url)
                            && git_signin.is_none()
                            && login_pending.is_none()
                        {
                            let probe = std::sync::Arc::new(std::sync::Mutex::new(None));
                            let (slot, spec2, parent2) = (probe.clone(), spec.clone(), parent.clone());
                            std::thread::spawn(move || {
                                let look = crate::addproject::look_at_server(&spec2, &parent2);
                                *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(look);
                            });
                            login_seq += 1;
                            git_signin = Some(GitSignIn {
                                seq: login_seq,
                                row: j.id,
                                desk: j.desk.clone(),
                                host: j.host.clone(),
                                spec: spec.clone(),
                                url: url.clone(),
                                probe,
                                look: None,
                                added: false,
                                access: Default::default(),
                            });
                        }
                        j.error = Some(e);
                    }
                    crate::addproject::Outcome::Done(at) => {
                        let at = at.to_string_lossy().to_string();
                        match add_remote_to_desk(desks.iter().find(|d| d.name == j.desk), &j.host.name, &at, None) {
                            Ok(Added::New(said)) | Ok(Added::Already(said)) => {
                                said_before_reload = Some((Instant::now(), said.clone()));
                                flash = Some(said);
                                crate::webui::ask_branch_next(&at, true);
                                j.gone = true;
                            }
                            Err(e) => j.error = Some(e),
                        }
                    }
                }
                continue;
            }
            let outcome = match &j.work {
                VmWork::Clone { job, .. } => job.outcome(),
                VmWork::Prepare { job, .. } => job.outcome(),
                VmWork::SshClone { .. } => continue,
            };
            match outcome {
                crate::microvm::Outcome::Running(_) => {}
                crate::microvm::Outcome::Failed(e) if e.is_empty() => j.gone = true,
                crate::microvm::Outcome::Failed(e) => {
                    append_hook_log(&format!("could not prepare {} on {}: {e}", j.project, j.host.name));
                    j.error = Some(e);
                }
                crate::microvm::Outcome::Done { sandbox, at, prepared } => {
                    let written = match &j.work {
                        // The checkout is the project's, with a folder of its own
                        // on the desk; the project goes on through its rules
                        VmWork::Clone { add, .. } => {
                            let home = config::ProjectHome {
                                host: add.host.clone(),
                                at: at.clone(),
                                placement: None,
                                sandbox: Some(sandbox.clone()),
                                prepared: Some(prepared),
                            };
                            config::set_project_home(&j.desk_id, &add.project, &home, None)
                                .and_then(|()| match &add.account {
                                    Some(a) => config::set_project_git_account(&j.desk_id, &add.project, a),
                                    None => Ok(()),
                                })
                                .and_then(|()| {
                                    let ai = add.ai.as_deref().unwrap_or(crate::microvm::NO_AI);
                                    config::set_project_value(&j.desk_id, &add.project, "machine_ai", Some(ai))
                                })
                                .and_then(|()| {
                                    config::append_folder_starting(
                                        &j.desk,
                                        None,
                                        std::path::Path::new(&at),
                                        None,
                                        &crate::microvm::start_with(add.ai.as_deref()),
                                        Some(&add.host),
                                    )
                                })
                                .and_then(|()| config::set_folder_far(&j.desk, std::path::Path::new(&at), &add.host, Some(&add.project), Some(&sandbox)))
                                .map(|()| {
                                    // On to the rules -- through the sign-in
                                    // step first when the machine was given an
                                    // AI, since every worktree is a copy of
                                    // this machine, sign-in and all
                                    match add.ai.as_deref().and_then(crate::profile::machine_ai) {
                                        Some(known) => {
                                            login_seq += 1;
                                            login_pending = Some(LoginPending {
                                                seq: login_seq,
                                                folder: at.clone(),
                                                host: j.host.clone(),
                                                home: home.clone(),
                                                ai: known.key,
                                                name: known.name,
                                                shown: false,
                                            });
                                        }
                                        None => {
                                            crate::webui::ask_branch_next(&at, true);
                                        }
                                    }
                                    i18n::tp("msg.project.remote_added", &[("name", &add.project), ("host", &add.host)])
                                })
                        }
                        // The checkout's machine has what the project says now,
                        // and the worktree dialog opens where the page was going
                        // Written down above, before a MicroVM's outcome is read
                        VmWork::SshClone { .. } => continue,
                        VmWork::Prepare { home, follow, .. } => {
                            let done = config::ProjectHome { prepared: Some(prepared), ..home.clone() };
                            config::set_project_home(&j.desk_id, &j.project, &done, None).map(|()| {
                                if let Some(f) = follow {
                                    crate::webui::ask_branch_next(f, false);
                                }
                                i18n::tp("msg.microvm.prepared", &[("name", &j.project), ("host", &j.host.name)])
                            })
                        }
                    };
                    match written {
                        Ok(said) => {
                            said_before_reload = Some((Instant::now(), said.clone()));
                            flash = Some(said);
                            j.gone = true;
                        }
                        Err(e) => j.error = Some(format!("{e:#}")),
                    }
                }
            }
        }
        vm_jobs.retain(|j| !j.gone);
        // A colour chosen for a project. Written against the folder git shares
        // between its branches, so all of them change at once
        for (folder, color) in shell.mail().take_folder_colors() {
            let at = std::path::PathBuf::from(&folder);
            if let Some(family) = crate::repo::family_of(&at)
                && let Err(e) = config::set_folder_color(&family, &color) {
                    flash = Some(format!("{e:#}"));
                }
        }
        // A working folder that is not on this machine. The same call answers
        // "what would it take" and does it, so the lines shown before it
        // happens are the lines that happen
        for (folder, choose, branch, take) in shell.mail().take_repairs() {
            let at = std::path::PathBuf::from(&folder);
            let desk = desks.get(desk_index);
            let desk_name = desk.map(|w| w.name.clone()).unwrap_or_default();
            // The answer to the one question that has to be asked, written into
            // the settings the moment it is given. Every machine after this one
            // reads it instead of asking
            let chosen = match choose.trim() {
                "" => None,
                "folder" => Some(config::SourceSpec::plain()),
                url => Some(config::SourceSpec::worktree(
                    url,
                    match branch.trim().is_empty() {
                        true => at
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        false => branch.trim().to_string(),
                    }
                    .as_str(),
                    "origin/main",
                )),
            };
            if let Some(spec) = chosen.as_ref()
                && let Err(e) = config::set_folder_source(&desk_name, &at, spec) {
                    flash = Some(format!("{e:#}"));
                }
            // What the settings say now: the answer just given, or what was
            // written down when the folder was made
            let source = match chosen.as_ref() {
                Some(spec) => spec.read(),
                None => desk
                    .and_then(|w| w.folders.iter().find(|f| f.cwd.as_deref() == Some(at.as_path())))
                    .map(|f| f.source.clone())
                    .unwrap_or_default(),
            };
            // The project the settings already name, when they do. Preferred
            // over working it out from the path: what somebody wrote down beats
            // what a folder's shape suggests
            let checkout = desk.and_then(|w| {
                w.folders.iter().find_map(|f| {
                    let cwd = f.cwd.as_deref()?;
                    let url = crate::repo::remote_url_of(cwd)?;
                    match &source {
                        config::Source::Worktree { origin, .. }
                            if crate::folders::scrub(&url) == crate::folders::scrub(origin) =>
                        {
                            crate::repo::main_checkout(cwd)
                        }
                        _ => None,
                    }
                })
            });
            let name = desk
                .and_then(|w| w.folders.iter().find(|f| f.cwd.as_deref() == Some(at.as_path())))
                .and_then(|f| f.name.clone())
                .unwrap_or_else(|| {
                    at.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
                });
            let planned = crate::folders::plan(&at, &source, checkout.as_deref());
            let mut view = crate::uistate::RepairPlan {
                folder: folder.clone(),
                name,
                trouble: trouble_of(&at),
                branch: at
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                ..Default::default()
            };
            match planned {
                Ok(steps) => {
                    view.done = steps.is_empty();
                    view.steps = steps.clone();
                    // Already working on this folder: the answer is how far it
                    // has got, not a second clone of the same project into the
                    // same place
                    if let Some(p) = putting.as_ref().filter(|p| p.at == at) {
                        view.running = true;
                        view.at_step = p.at_step();
                        view.steps = p.steps.clone();
                    } else if take && !steps.is_empty() {
                        // Cloning takes as long as the network does, so it runs
                        // on its own thread and the dialog says which step is
                        // running. Run here, a repository of any size would
                        // stop the window until git was finished with it
                        view.running = true;
                        view.at_step = 0;
                        putting = Some(crate::folders::Putting::start(at.clone(), steps, source.clone()));
                    }
                }
                Err(blocked) => {
                    // Nothing was ever written down about this folder. This is
                    // the only case that asks, and the answer ends the asking
                    // for every machine, not just this one
                    view.asking = matches!(blocked, crate::folders::Blocked::Unknown);
                    view.projects = projects_here(desk);
                    view.said = blocked_said(&blocked);
                    view.blocked = Some(blocked);
                }
            }
            repair_view = Some(view);
        }
        // The folder being put back, while it is being put back. The card is
        // kept up to date from here rather than by the page asking again and
        // again: what is running is known on this side, and a page that had to
        // poll would say nothing at all on the frames between
        if let Some(p) = putting.as_ref() {
            let at = p.at.clone();
            match p.outcome() {
                None => {
                    if let Some(v) = repair_view.as_mut().filter(|v| v.folder == at.display().to_string()) {
                        v.running = true;
                        v.at_step = p.at_step();
                    }
                }
                Some(said) => {
                    // What was true a moment ago is not true now. Said out loud
                    // rather than waited out, so the folder stops being called
                    // missing the instant it is made -- and the tabs held back
                    // for it start on the next beat
                    crate::folders::watch().forget(&at);
                    let here = at.is_dir();
                    if let Some(v) = repair_view.as_mut().filter(|v| v.folder == at.display().to_string()) {
                        v.running = false;
                        v.done = here;
                        v.error = said.as_ref().err().cloned();
                        if here {
                            flash = Some(i18n::tp("msg.folder.ready", &[("name", &v.name)]));
                        }
                    } else if here {
                        flash = Some(i18n::tp(
                            "msg.folder.ready",
                            &[("name", &at.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default())],
                        ));
                    }
                    // A folder that came back is no longer one anybody chose
                    // to look away from
                    folders_hidden.retain(|h| !crate::uistate::same_folder(h, &at));
                    putting = None;
                }
            }
        }
        // Folders put out of sight until the next launch, and the one line that
        // brings them all back. Nothing is written down: this is what somebody
        // does with a desk that belongs to another machine, and it must not
        // follow them to the machine the folders are really on
        for (folder, hide) in shell.mail().take_folder_hides() {
            match (hide, folder.trim().is_empty()) {
                (true, false) => {
                    let at = std::path::PathBuf::from(&folder);
                    folders_hidden.insert(at.clone());
                    // What was on screen is about to stop being drawn, so the
                    // view steps on to the nearest tab that is still on the
                    // list -- or, when there is none, to the board. Left where
                    // it was, the next pass would read it resting there as
                    // somebody asking for the folder back. Put, not asked for:
                    // the same question the pass before drawing asks, so the
                    // answer cannot come out differently here. Whatever is in
                    // front -- the board, the settings -- stays in front: the
                    // press was "stop drawing this", not "take me somewhere"
                    match crate::view::settle(true, active, &surfaces, &tabs, &folders_hidden) {
                        crate::view::Settled::Show(n) => {
                            active = n;
                            pane_layout.show(active);
                        }
                        crate::view::Settled::Board => {
                            active = 0;
                            board_open = true;
                        }
                        crate::view::Settled::Stay | crate::view::Settled::Bring(_) => {}
                    }
                }
                (false, true) => folders_hidden.clear(),
                (false, false) => {
                    folders_hidden.retain(|h| !crate::uistate::same_folder(h, std::path::Path::new(&folder)));
                }
                (true, true) => {}
            }
        }
        // A folder told to work somewhere else. The folder keeps its name, its
        // colour and its tabs: it is the same folder, standing in a different
        // place
        for (folder, to) in shell.mail().take_folder_moves() {
            let (at, to) = (std::path::PathBuf::from(&folder), std::path::PathBuf::from(&to));
            if to.as_os_str().is_empty() {
                continue;
            }
            let desk = desks.get(desk_index).map(|w| w.name.clone()).unwrap_or_default();
            match config::move_folder(&desk, &at, &to) {
                Ok(()) => {
                    folders_hidden.retain(|h| !crate::uistate::same_folder(h, &at));
                    crate::folders::watch().forget(&at);
                    let said = i18n::tp(
                        "msg.folder.moved",
                        &[
                            ("name", &at.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
                            ("path", &to.display().to_string()),
                        ],
                    );
                    said_before_reload = Some((Instant::now(), said.clone()));
                    flash = Some(said);
                }
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        // Another branch of a project already open. The same call answers "what
        // would this do" and does it, so the line shown before it happens is
        // the line that happens
        // An environment file the project was offered, and somebody said yes
        // to. Worked out again here rather than trusted from the page: what is
        // written is what this side would have proposed, whatever a page said
        for from in shell.mail().take_keep_envs() {
            let said = crate::repo::main_checkout(std::path::Path::new(&from))
                .as_deref()
                .and_then(crate::devcontainer::propose)
                .ok_or_else(|| crate::i18n::t("err.devcontainer.nothing"))
                .and_then(|d| {
                    crate::devcontainer::save(&d)
                        .map(|()| crate::i18n::tp("msg.devcontainer.kept", &[("path", &d.at)]))
                        .map_err(|e| format!("{e:#}"))
                });
            flash = Some(match said {
                Ok(m) => m,
                Err(e) => e,
            });
        }
        // How each ignore line comes along, chosen by line in the worktree
        // dialog and applied: kept as the project's own, for this worktree and
        // the ones after it. The dialog has already put the choices on its own
        // list; what is left is to write them down, and to say so if that fails
        for (from, lines) in shell.mail().take_bring_lines() {
            let Some(desk) = desks.get(desk_index) else { continue };
            let choices: Vec<config::BringChoice> = lines
                .into_iter()
                .map(|(source, pattern, how)| config::BringChoice {
                    source: match source.trim() {
                        "" => ".gitignore".into(),
                        s => s.to_string(),
                    },
                    pattern,
                    how,
                })
                .collect();
            if !config::save_bring_choices(&desk.id, std::path::Path::new(&from), &choices) {
                flash = Some(i18n::t("msg.bring.not_saved"));
            }
        }
        for ask in shell.mail().take_branches() {
            let from = std::path::PathBuf::from(&ask.from);
            let name = ask.branch.clone();
            // The machines this could be made on besides this one
            let machines: Vec<crate::config::HostSpec> =
                cfg.as_ref().map(|c| c.hosts.clone()).unwrap_or_default();
            // The folder asked from, when it is on another machine: a worktree
            // of it is cut on that machine unless another place is chosen.
            // Nothing chosen means "where the folder is", and this PC chosen
            // from a folder elsewhere is said in so many words
            let from_far = desks.get(desk_index).and_then(|d| {
                d.folders
                    .iter()
                    .find(|f| f.host.is_some() && f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, &from)))
                    .and_then(|f| f.host.clone())
            });
            let wanted_host = match (ask.host.trim(), &from_far) {
                (HERE, _) => "",
                ("", Some(f)) => f.name.as_str(),
                (h, _) => h,
            };
            let on = machines.iter().find(|h| h.name == wanted_host && !h.name.is_empty());
            // What this project can offer -- the branches to grow from, and
            // the things git will not carry -- is a fact about the folder, not
            // about what has been typed so far. Answered even when the name is
            // still empty, so the pickers are filled the moment the dialog opens
            let repo = crate::repo::main_checkout(&from);
            // Which project this folder is a piece of, and what it says about
            // itself. One lookup, used for the place the folder goes, for what
            // is run in it, and for whether anything is offered
            // Asked of the desk on screen: the same repository may be a project
            // with another setup in another desk
            let project = desks.get(desk_index).and_then(|d| d.project_of(&from));
            let told = match repo.as_deref() {
                Some(r) => crate::devcontainer::told(r, project.and_then(|p| p.setup.as_deref())),
                // A folder on another machine: what the project's settings say
                None => crate::devcontainer::told_plainly(project.and_then(|p| p.setup.as_deref())),
            };
            // What comes along is the project's answer for each ignore line and
            // for each file it brings from elsewhere, with whatever the dialog
            // changed for this one folder laid over it
            let rules = project.map(|p| p.bring.clone()).unwrap_or_default();
            let offers = repo.as_deref().map(|main| {
                let mut carry = crate::worktree::carryables(main, &rules);
                // Before this folder's own changes: a line says what the project says
                let lines = crate::worktree::carry_lines(&carry);
                for c in carry.iter_mut() {
                    if let Some((_, how)) = ask.carry.iter().find(|(n, _)| *n == c.name)
                        && crate::worktree::HOWS.contains(&how.as_str())
                    {
                        c.how = how.clone();
                    }
                }
                (crate::worktree::bases(main), carry, lines)
            });
            let (mut bases, carryable, carry_lines) = offers.unwrap_or_default();
            // A folder on another machine: its branches are git's there, asked
            // on a thread and put on the dialog when they come
            let far_bases = match (&repo, &from_far) {
                (None, Some(h)) => {
                    let opening = branch_view.as_ref().is_none_or(|v| v.seq != ask.seq);
                    let at = from.to_string_lossy().replace('\\', "/");
                    bases_watch = Some((h.clone(), at.clone()));
                    crate::worktree::far_bases(h, &at, opening && ask.branch.trim().is_empty() && ask.base.trim().is_empty())
                }
                _ => None,
            };
            if let Some((found, _)) = &far_bases {
                bases = found.clone();
            }
            // What it would grow from, even when there is no name yet to grow.
            // Echoing back the empty answer would leave the picker with nothing
            // to show until somebody typed
            let chosen = match ask.base.trim().is_empty() {
                true => repo
                    .as_deref()
                    .map(crate::worktree::default_base)
                    .or_else(|| far_bases.as_ref().map(|(_, c)| c.clone()))
                    .unwrap_or_default(),
                false => ask.base.clone(),
            };
            // What was typed, in the letters a branch and a folder can both
            // hold on any machine. A name written in Japanese leaves nothing
            // to keep, and so does an empty box -- both draw a name here.
            //
            // The same one every time this dialog asks, while it is still free.
            // Drawn afresh on each ask, the name changed with every keystroke
            // in another field -- and on the press itself, so the folder that
            // was made was not the one on screen when the button was pressed
            //
            // What the project puts in front of every branch of it goes in
            // front of this one, typed or drawn. A person who typed the prefix
            // themselves is not given it twice
            let prefix = project.and_then(|p| p.branch_prefix.clone()).unwrap_or_default();
            // Where this project's folders go, and so what "free" means for a
            // name: a branch nobody has, and a folder nothing stands in there
            let placement = crate::worktree::Placement::of(project);
            let (wanted, drawn) = match (crate::worktree::tidy(&name), repo.as_deref()) {
                (Some(kept), _) => (crate::worktree::with_prefix(&prefix, &kept), None),
                (None, Some(main)) => {
                    let kept = match drawn_names.get(&ask.from) {
                        Some(kept) if crate::worktree::is_free(main, &placement, kept) => kept.clone(),
                        _ => {
                            let fresh = crate::worktree::suggest(main, &placement);
                            drawn_names.insert(ask.from.clone(), fresh.clone());
                            fresh
                        }
                    };
                    // Drawn, so the work may write over it once it has a title
                    (kept.clone(), Some(kept))
                }
                // A project with no checkout here: drawn all the same, and the
                // machine it is cut on is the one that says a name is taken
                (None, None) if on.is_some() => {
                    let kept = drawn_names
                        .entry(ask.from.clone())
                        .or_insert_with(|| crate::worktree::suggest_anywhere(&prefix))
                        .clone();
                    (kept.clone(), Some(kept))
                }
                (None, None) => (String::new(), None),
            };
            // What the folder's card will be called. This app's own label: it
            // goes in the settings and nowhere near git or the disk, so it
            // keeps what the person wrote, whatever letters they wrote it in.
            // Nothing written means there is nothing of theirs to keep, and
            // the drawn name is what they saw in the empty box
            let label = match name.trim() {
                "" => wanted.clone(),
                typed => typed.to_string(),
            };
            let desk = desks
                .get(desk_index)
                .map(|w| w.name.clone())
                .unwrap_or_default();
            // What the new folder runs, chosen from what this machine has
            let start = start_of(&ask.start, &ai_choices);
            // Which project this is cut from. Worked out the same way the
            // making itself works it out, so the row cannot say one thing while
            // git is handed another
            let checkout = crate::repo::main_checkout(&from);
            // On another machine the project's checkout there is what the
            // project says it is
            let far_checkout = checkout.is_none().then(|| {
                from_far.as_ref().and_then(|h| project.and_then(|p| p.home_on(&h.name)).map(|home| home.at.clone()))
            }).flatten();
            let mut view = crate::uistate::BranchPlan {
                seq: ask.seq,
                from: from.display().to_string(),
                branch: name.clone(),
                asked: name.clone(),
                base: chosen.clone(),
                bases,
                carry: carryable.clone(),
                carry_lines,
                project: checkout
                    .as_deref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .or_else(|| project.map(|p| p.name.clone()))
                    .unwrap_or_default(),
                project_at: checkout
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .or(far_checkout)
                    .unwrap_or_default(),
                hosts: machines
                    .iter()
                    .filter(|h| !h.name.trim().is_empty())
                    .map(|h| crate::uistate::HostOffer {
                        name: h.name.clone(),
                        kind: if h.is_made() { "microvm" } else { "ssh" }.into(),
                        at: project.and_then(|p| p.home_on(&h.name)).map(|home| home.at.clone()).unwrap_or_default(),
                    })
                    .collect(),
                // This PC is a place when the project is checked out here
                here: repo.is_some(),
                host: on.map(|h| h.name.clone()).unwrap_or_default(),
                // Said whether or not it is switched on, so the switch is not
                // one nobody can tell the meaning of
                setup_from: told.as_ref().map(|e| e.from.clone()).unwrap_or_default(),
                setup_unresolved: told.as_ref().map(|e| e.unresolved.clone()).unwrap_or_default(),
                // Only where it says nothing. A project that says something has
                // already answered, and being offered a guess beside its own
                // answer would be this app talking over it
                offer: told
                    .is_none()
                    .then(|| repo.as_deref().and_then(crate::devcontainer::propose))
                    .flatten(),
                project_name: project.map(|p| p.name.clone()).unwrap_or_default(),
                machine_ai: project.and_then(|p| p.machine_ai.clone()).unwrap_or_default(),
                ..Default::default()
            };
            if ask.ais.is_empty() {
                let at = std::path::PathBuf::from(ask.at.trim());
                // What the project says it needs, unless somebody said not to.
                // Read from the checkout here: it is the same repository
                // wherever the folder ends up
                let env = ask.setup.then(|| told.clone()).flatten();
                // On this machine or another. Two planners rather than one
                // with a switch inside it: almost nothing the local one does
                // can be done about a machine we would have to ask
                let planned = match on {
                    Some(h) => {
                        let far = far_of(desks.get(desk_index), project, h, &from, &checkout, env.clone(), &ask.machine_ai);
                        if h.is_made() {
                            view.sign_in = crate::microvm::sign_in_note(&far.1, &far.2);
                            signin_waiting = view.sign_in.is_none().then(|| (far.1.clone(), far.2.clone()));
                            view.ai_sign_in = crate::microvm::ai_sign_in_note(h, far.0.home.as_ref(), far.0.preparing.ai.as_deref(), crate::microvm::FRESH);
                            ai_signin_watch = Some((h.clone(), far.0.home.clone(), far.0.preparing.ai.clone()));
                        }
                        crate::worktree::plan_on(&far.0.of(), &wanted, &prefix, Some(&ask.base), Some(ask.at.trim()))
                    }
                    None => crate::worktree::plan_for(
                        &from,
                        &placement,
                        &wanted,
                        Some(&ask.base),
                        (!ask.at.trim().is_empty()).then_some(at.as_path()),
                        env,
                    ),
                };
                match planned {
                    // Open in another folder already: not a dead end but a
                    // question -- that folder, or this one under another name
                    Err(e) if e.downcast_ref::<crate::worktree::InUse>().is_some() => {
                        let taken = e.downcast_ref::<crate::worktree::InUse>().cloned().unwrap_or_else(|| {
                            crate::worktree::InUse { branch: wanted.clone(), folder: Default::default() }
                        });
                        let listed = desks.get(desk_index).is_some_and(|d| {
                            d.folders
                                .iter()
                                .any(|f| f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, &taken.folder)))
                        });
                        if ask.adopt && ask.make {
                            // The answer was "that folder": taken in as it is,
                            // running what a new one would, and tied to the issue
                            let taken_in = match listed {
                                true => Ok(()),
                                false => config::append_folder_starting(
                                    &desk,
                                    Some(&from),
                                    &taken.folder,
                                    Some(&taken.branch),
                                    &start,
                                    None,
                                ),
                            };
                            // Taken in by this answer, it names itself as a
                            // new one would. One already on the desk keeps
                            // whatever it was called there
                            let taken_in = taken_in.and_then(|()| match (listed, ask.auto) {
                                (false, true) => config::set_folder_auto_label(&desk, &taken.folder, true),
                                _ => Ok(()),
                            });
                            match taken_in {
                                Ok(()) => {
                                    view.done = true;
                                    remember_work_item(&desk, &taken.folder, &ask.link, &mut pending_drafts);
                                    shell.mail().folder_views.push(taken.folder.display().to_string());
                                    flash = Some(i18n::tp(
                                        "msg.branch.adopted",
                                        &[("path", &taken.folder.display().to_string())],
                                    ));
                                }
                                Err(e) => view.error = Some(format!("{e:#}")),
                            }
                        } else {
                            let instead = repo
                                .as_deref()
                                .map(|main| crate::worktree::next_free(main, &placement, &taken.branch))
                                .unwrap_or_default();
                            view.in_use = Some(crate::uistate::BranchInUse {
                                branch: taken.branch.clone(),
                                folder: taken.folder.display().to_string(),
                                listed,
                                instead,
                            });
                        }
                    }
                    Err(e) => view.error = Some(format!("{e:#}")),
                    Ok(plan) => {
                        view.branch = plan.branch.clone();
                        view.folder = plan.folder.display().to_string();
                        view.line = plan.line();
                        view.base = plan.base.clone();
                        if ask.make {
                            // Made on a thread, a row under the project saying how
                            // far it has got; written down once it is there, since
                            // settings naming a folder that does not exist would
                            // launch tabs into nowhere on the next reload. What
                            // could not be brought along is said when it is made
                            // A second press on the same folder is the first one
                            // still going, not another worktree. One that failed
                            // there is what this press tries again
                            let same = |p: &Pending| crate::uistate::same_folder(&p.making.plan.folder, &plan.folder);
                            makings.retain(|p| !(same(p) && p.error.is_some() && !p.made));
                            let already = makings.iter().any(|p| !p.gone && same(p));
                            making_seq += 1;
                            if !already { makings.push(Pending {
                                id: making_seq,
                                family: making_family(&plan, &from),
                                making: crate::worktree::Making::start(plan, carryable.clone()),
                                desk: desk.clone(),
                                desk_id: desks.get(desk_index).map(|d| d.id.clone()).unwrap_or_default(),
                                checkout_noted: false,
                                checkout_tried: None,
                                from: from.clone(),
                                label: label.clone(),
                                start: start.clone(),
                                link: ask.link.clone(),
                                auto: ask.auto,
                                drawn: drawn.clone(),
                                carry: carryable.clone(),
                                made: false,
                                error: None,
                                trust: None,
                                unlinked: Vec::new(),
                                copying: None,
                                written: None,
                                gone: false,
                            }); }
                            view.done = true;
                        }
                    }
                }
            } else {
                // One folder per AI, each branch named for its AI. Every line
                // is shown before any of them runs; one that cannot be made
                // stops the whole ask, because "three of the four were made"
                // is a state nobody asked for
                // On the machine that was chosen, the same as a single one
                let fanned = match on {
                    Some(h) => {
                        let env = ask.setup.then(|| told.clone()).flatten();
                        let far = far_of(desks.get(desk_index), project, h, &from, &checkout, env, &ask.machine_ai);
                        if h.is_made() {
                            view.sign_in = crate::microvm::sign_in_note(&far.1, &far.2);
                            signin_waiting = view.sign_in.is_none().then(|| (far.1.clone(), far.2.clone()));
                            view.ai_sign_in = crate::microvm::ai_sign_in_note(h, far.0.home.as_ref(), far.0.preparing.ai.as_deref(), crate::microvm::FRESH);
                            ai_signin_watch = Some((h.clone(), far.0.home.clone(), far.0.preparing.ai.clone()));
                        }
                        crate::worktree::fan_on(&far.0.of(), &wanted, &prefix, Some(&ask.base), &ask.ais)
                    }
                    None => crate::worktree::fan(&from, &placement, &wanted, Some(&ask.base), &ask.ais),
                };
                view.branch = wanted.clone();
                view.lines = fanned.iter().filter_map(|(_, p)| p.as_ref().ok().map(|p| p.line())).collect();
                view.folder = fanned
                    .iter()
                    .filter_map(|(_, p)| p.as_ref().ok().map(|p| p.folder.display().to_string()))
                    .collect::<Vec<_>>()
                    .join("\n");
                if let Some((_, Err(e))) = fanned.iter().find(|(_, p)| p.is_err()) {
                    view.error = Some(format!("{e:#}"));
                } else if ask.make {
                    // One row each, made side by side: each says for itself how
                    // far it has got, and one that fails says so on its own row
                    // while the others become cards
                    for (ai, plan) in fanned {
                        let Ok(plan) = plan else { continue };
                        // Each folder's own branch is the drawn name with its
                        // AI on the end, so each is drawn in its own right
                        let branch = plan.branch.clone();
                        if makings.iter().any(|p| !p.gone && crate::uistate::same_folder(&p.making.plan.folder, &plan.folder)) {
                            continue;
                        }
                        making_seq += 1;
                        makings.push(Pending {
                            id: making_seq,
                            family: making_family(&plan, &from),
                            making: crate::worktree::Making::start(plan, carryable.clone()),
                            desk: desk.clone(),
                            desk_id: desks.get(desk_index).map(|d| d.id.clone()).unwrap_or_default(),
                            checkout_noted: false,
                            checkout_tried: None,
                            from: from.clone(),
                            // The AI on the end, the same way its branch has
                            // it, so a card and its branch read as one pair
                            label: format!("{label}-{ai}"),
                            start: start_of(&ai, &ai_choices),
                            link: ask.link.clone(),
                            auto: ask.auto,
                            drawn: drawn.as_ref().map(|_| branch.clone()),
                            carry: carryable.clone(),
                            made: false,
                            error: None,
                            trust: None,
                            unlinked: Vec::new(),
                            copying: None,
                            written: None,
                            gone: false,
                        });
                    }
                    view.done = true;
                }
            }
            // Made: the next dialog on this folder is new work and gets a
            // name of its own
            if view.done {
                drawn_names.remove(&ask.from);
                if let Some(f) = flash.clone() {
                    said_before_reload = Some((std::time::Instant::now(), f));
                }
            }
            branch_view = Some(view);
        }
        // What a MicroVM would sign in as, once it has been found out: put on
        // whichever dialog is waiting for it
        if let Some((account, far)) = &signin_waiting
            && let Some(note) = crate::microvm::sign_in_note(account, far)
        {
            if let Some(v) = branch_view.as_mut().filter(|v| v.sign_in.is_none()) {
                v.sign_in = Some(note.clone());
            }
            if let Some(v) = add_view.as_mut().filter(|v| v.microvm && !v.running && v.sign_in.is_none()) {
                v.sign_in = Some(note);
            }
            signin_waiting = None;
        }
        // The branches of a folder on another machine, once git there has
        // answered: put on the dialog that asked, with the one to start from
        if let Some((h, at)) = bases_watch.as_ref() {
            match branch_view.as_mut() {
                None => bases_watch = None,
                Some(v) => {
                    if let Some((found, chosen)) = crate::worktree::far_bases(h, at, false) {
                        if v.bases != found {
                            if v.base.trim().is_empty() {
                                v.base = chosen;
                            }
                            v.bases = found;
                        }
                        bases_watch = None;
                    }
                }
            }
        }
        // Whether the checkout's AI is signed in, kept current while the
        // dialog is open: the answer arrives from a thread, and a sign-in
        // done in the checkout's tab meanwhile changes it
        if let Some((h, home, ai)) = ai_signin_watch.as_ref() {
            match branch_view.as_mut() {
                None => ai_signin_watch = None,
                Some(v) => {
                    let note = crate::microvm::ai_sign_in_note(h, home.as_ref(), ai.as_deref(), crate::microvm::FRESH);
                    if note != v.ai_sign_in {
                        v.ai_sign_in = note;
                    }
                }
            }
        }
        // The sign-in step of a project just cloned onto a MicroVM. The
        // machine is asked whether its AI is signed in; one that is (a key
        // given by the machine setup) goes straight on to the rules, and one
        // that is not is shown the step, looked at again every few seconds
        // while it is open so a sign-in done in it is seen
        if let Some(p) = login_pending.as_mut() {
            let note = crate::microvm::ai_sign_in_note(&p.host, Some(&p.home), Some(&p.ai), LOGIN_FRESH);
            let go_on = match note.as_ref().map(|n| n.state.as_str()) {
                // Nothing to ask about (no such AI), or signed in before
                // the step was ever shown: nothing to do here
                None => true,
                Some("yes") if !p.shown => true,
                Some("asking") if !p.shown => false,
                Some(state) => {
                    p.shown = true;
                    // The checkout's AI terminal, as the tab standing in that
                    // folder on that machine draws it now
                    let (screen, url) = tabs
                        .iter()
                        .find(|t| t.remote_cwd() == Some(p.folder.as_str()) && t.title == p.ai)
                        .map(|t| {
                            let parser = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                            let s = parser.screen();
                            let (_, cols) = s.size();
                            // The text with rows joined where a line went on
                            // into the next row, which is where a long
                            // address is whole again
                            let rows: Vec<(String, bool)> =
                                s.rows(0, cols).enumerate().map(|(i, r)| (r, s.row_wrapped(i as u16))).collect();
                            let url = web_address_in(&join_rows(&rows, cols));
                            (crate::shell::screen_html(s), url)
                        })
                        .unwrap_or_default();
                    let v = crate::uistate::LoginStepState {
                        seq: p.seq,
                        folder: p.folder.clone(),
                        host: p.host.name.clone(),
                        ai: p.ai.clone(),
                        name: p.name.clone(),
                        state: state.to_string(),
                        error: note.as_ref().map(|n| n.error.clone()).unwrap_or_default(),
                        screen,
                        url,
                        ..Default::default()
                    };
                    if login_view.as_ref() != Some(&v) {
                        login_view = Some(v);
                    }
                    false
                }
            };
            if go_on {
                let folder = p.folder.clone();
                login_pending = None;
                login_view = None;
                crate::webui::ask_branch_next(&folder, true);
            }
        }
        // The sign-in step for a server's git. Drawn once the server has been
        // looked at: the commands drafted for it, its terminal, and whether
        // the server's git can read the repository yet
        let mut git_signin_gone = false;
        if let Some(g) = git_signin.as_mut() {
            if g.look.is_none() {
                let arrived = g.probe.lock().unwrap_or_else(|e| e.into_inner()).take();
                match arrived {
                    None => {}
                    // Not looked at: the row says what git said, as before
                    Some(Err(e)) => {
                        append_hook_log(&format!("could not look at {} for its git sign-in: {e}", g.host.name));
                        git_signin_gone = true;
                    }
                    Some(Ok(look)) => {
                        // A terminal there, standing in the clone's folder:
                        // put on the desk for the step unless it is there
                        let here = desks.iter().find(|d| d.name == g.desk).is_some_and(|d| {
                            d.folders.iter().any(|f| {
                                f.host.as_ref().is_some_and(|h| h.name == g.host.name)
                                    && f.cwd.as_ref().is_some_and(|c| c.to_string_lossy().trim_end_matches('/') == look.parent)
                            })
                        });
                        if !here {
                            match config::append_folder_starting(&g.desk, None, std::path::Path::new(&look.parent), None, &config::Start::Same, Some(&g.host.name)) {
                                // Read in again now, so its terminal starts
                                // while the step is up rather than whenever
                                // the settings are next looked at
                                Ok(()) => {
                                    g.added = true;
                                    watcher.poke();
                                }
                                Err(e) => append_hook_log(&format!("could not put a terminal on {} for its git sign-in: {e:#}", g.host.name)),
                            }
                        }
                        g.look = Some(look);
                    }
                }
            }
            if let Some(look) = g.look.clone() {
                // Asked again every few seconds while the step is open
                {
                    let mut a = g.access.lock().unwrap_or_else(|e| e.into_inner());
                    if !a.2 && a.0.is_none_or(|at| at.elapsed() >= LOGIN_FRESH) {
                        a.2 = true;
                        let (slot, spec, url) = (g.access.clone(), g.spec.clone(), g.url.clone());
                        std::thread::spawn(move || {
                            let can = crate::addproject::can_read(&spec, &url);
                            let mut a = slot.lock().unwrap_or_else(|e| e.into_inner());
                            *a = (Some(Instant::now()), Some(can), false);
                        });
                    }
                }
                let can = g.access.lock().unwrap_or_else(|e| e.into_inner()).1;
                let screen = tabs
                    .iter()
                    .find(|t| t.remote_cwd() == Some(look.parent.as_str()) && t.title == g.host.name)
                    .map(|t| crate::shell::screen_html(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen()))
                    .unwrap_or_default();
                let v = crate::uistate::LoginStepState {
                    seq: g.seq,
                    folder: look.parent.clone(),
                    host: g.host.name.clone(),
                    name: g.host.name.clone(),
                    state: match can {
                        None => "asking",
                        Some(true) => "yes",
                        Some(false) => "no",
                    }
                    .into(),
                    screen,
                    url: g.url.clone(),
                    kind: "git".into(),
                    commands: crate::addproject::github_sign_in_commands(&look),
                    account: crate::addproject::login_of(&g.url),
                    ..Default::default()
                };
                if login_view.as_ref() != Some(&v) {
                    login_view = Some(v);
                }
            }
        }
        if git_signin_gone {
            git_signin = None;
        }
        for (folder, act) in shell.mail().take_logins() {
            // The server's git step: "next" tries the clone again, and either
            // way the terminal put there for it comes off the desk
            if git_signin.as_ref().is_some_and(|g| g.look.as_ref().is_some_and(|l| l.parent == folder)) {
                if let Some(g) = git_signin.take() {
                    if g.added && config::remove_folder(&g.desk, std::path::Path::new(&folder)).is_ok() {
                        watcher.poke();
                    }
                    if act == "next" {
                        shell.mail().makings.push((g.row, "retry".to_string()));
                    }
                }
                login_view = None;
                continue;
            }
            if login_pending.as_ref().is_some_and(|p| p.folder == folder) {
                login_pending = None;
                login_view = None;
                if act == "next" {
                    crate::webui::ask_branch_next(&folder, true);
                }
            }
        }
        for ev in shell.mail().take_vault_opens() {
            if let shikisha_shared::Ev::VaultOpen { program, id, cwd, title, host } = ev {
                // The command is the program alone; the resume id rides in its
                // own field, where the launch path turns it into the CLI's
                // resume flags. Writing the flags into the command here would
                // fight the auto-resume that also reads the profile
                let tab = serde_json::json!({
                    "name": title,
                    "command": program,
                    "resume": id,
                });
                // The folder the conversation was had in decides which group it
                // comes back into -- one already working there, or a new one
                let folder = cwd.as_deref().map(std::path::Path::new);
                let desk = desks.get(desk_index).map(|w| w.name.clone()).unwrap_or_default();
                // A conversation had on another machine opens in its folder
                // there, which has to be on this desk: written down as a
                // folder here, a path of that machine would be a folder that
                // exists nowhere
                let far_path = cwd.as_deref().is_some_and(|c| cfg!(windows) && c.starts_with('/'));
                let on_desk = folder.is_some_and(|p| {
                    desks.get(desk_index).is_some_and(|d| {
                        d.folders.iter().any(|f| {
                            f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, p))
                                && (host.is_none() || f.host.as_ref().map(|h| h.name.as_str()) == host.as_deref())
                        })
                    })
                });
                if far_path && !on_desk {
                    flash = Some(i18n::tp("msg.vault.far_not_here", &[("title", &title)]));
                    continue;
                }
                if config::append_tab_on(&desk, tab, folder, host.as_deref()) {
                    flash = Some(i18n::tp("msg.vault.reopened", &[("title", &title)]));
                } else {
                    flash = Some(i18n::t("msg.vault.reopen_failed"));
                }
            }
        }

        for want in shell.mail().take_suggests() {
            let target = session_at(&surfaces, active).and_then(|i| tabs.get(i));
            let Some(t) = target else {
                shell.push_suggested(
                    &serde_json::json!({"ok": false, "error": i18n::t("msg.suggest.no_tab")})
                        .to_string(),
                );
                continue;
            };
            let shell = t.command_line();
            let screen = {
                let s = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents();
                let tail: Vec<&str> = s.lines().rev().take(40).collect();
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            };
            let tab_id = t.id.clone().unwrap_or_else(|| t.title.clone());
            let env = env_cards.get(&tab_id).cloned().unwrap_or_default();
            let engine = cfg
                .as_ref()
                .and_then(|c| c.ai_engine.clone())
                .filter(|s| !s.is_empty());
            let tx = suggest_tx.clone();
            std::thread::spawn(move || {
                let out = match webui::suggest_with_local_ai(
                    &want,
                    &shell,
                    &screen,
                    &env,
                    engine.as_deref(),
                ) {
                    Ok(cmd) => serde_json::json!({"ok": true, "cmd": cmd}),
                    Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
                };
                let _ = tx.send(out.to_string());
            });
        }
        while let Ok(js) = suggest_rx.try_recv() {
            append_hook_log(&format!("suggest: {}", log_excerpt(&js, 200)));
            shell.push_suggested(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"suggested\":{js}}}"));
            }
        }

        // Recorded steps → one Lua line each, appended to the composer on both
        // surfaces. Each line calls the same primitives the automation uses,
        // addressed by the browser's Lua name, so record → paste → run round-trips.
        for step in shell.mail().take_recorded() {
            let Some(name) = caps.name_of_child(&step.child) else {
                continue;
            };
            let Some(line) = recorded_lua(&name, &step) else {
                continue;
            };
            let js = serde_json::to_string(&line).unwrap_or_default();
            shell.push_recorded(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"recorded\":{js}}}"));
            }
        }

        // Composer text/keys typed while viewing a browser tab go straight into that
        // browser — the same caps.browser_inject the phone's relay drives, so the two
        // share one injection path rather than each growing its own.
        let injects = shell.mail().take_injects();
        if !injects.is_empty()
            && let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) {
                for input in injects {
                    let _ = caps.browser_inject(key, input);
                }
            }

        // The 🎯 panel's replay button: put the newest run's durable script
        // where the user can grab it (the board itself can't download files)
        if std::mem::take(&mut shell.mail().replay_saves) {
            flash = Some(match save_replay_to_downloads() {
                Ok(Some(path)) => {
                    i18n::tp("msg.replay.saved", &[("path", &path.display().to_string())])
                }
                Ok(None) => i18n::t("msg.replay.none"),
                Err(e) => i18n::tp("msg.replay.failed", &[("e", &e.to_string())]),
            });
        }

        // "Operate a target tab" (🎯): aim the active AI at another tab and, if a
        // goal was given, hand it over. Browser targets reuse the built-in
        // browser-operate loop; the AI then writes Lua to drive the target.
        for (target, goal) in shell.mail().take_operates() {
            let src_pane = active;
            // The tab doing the driving, under the name it is written down by.
            // The aim is remembered against it, so picking one on screen is the
            // whole of the setting — there is no second place to look
            let operator_name = session_at(&surfaces, active)
                .and_then(|i| tabs.get(i))
                .map(|t| t.id.clone().unwrap_or_else(|| t.title.clone()));
            if target == 0 {
                if let Some(eng) = engine.as_mut() {
                    eng.stop_operate(src_pane);
                }
                operating = None;
                if remember_aim(desks.get_mut(desk_index), operator_name.as_deref(), None) {
                    // Our own write is not news to the watcher (see the font size)
                    watcher.retarget(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
                }
                if let Some(t) = session_at(&surfaces, active).and_then(|i| tabs.get_mut(i)) {
                    t.set_brain(None);
                }
                continue;
            }
            // A discussion participant already has a job: the script that keeps
            // its turn in the ring lives on this pane, and aiming would replace
            // it. The settings screen used to be the only place that could be
            // asked for, and it refused there; now that the aim is picked on
            // screen, the refusal belongs on screen too.
            let in_discuss = desks
                .get(desk_index)
                .and_then(|w| w.discuss.as_ref())
                .is_some_and(|d| {
                    let me = operator_name.as_deref().unwrap_or_default();
                    !me.is_empty()
                        && d.agents
                            .iter()
                            .chain(d.judge.iter())
                            .chain(d.moderator.iter())
                            .any(|x| x.trim() == me)
                });
            if in_discuss {
                flash = Some(i18n::t("msg.operate.in_discuss"));
                continue;
            }
            // First slice: browser targets only. Its id comes from the layout.
            // Resolve the target: a browser (driven with browser_* Lua) or another
            // AI tab (driven by relaying prompts). INDEX / settings / unknown surfaces
            // can't be operated.
            let (is_browser, target_id) = match surfaces.get(target.wrapping_sub(1)) {
                // Drive by the browser's KEY, not its display name: the display name
                // may be localized (a translated "Browser") while browser_* resolves by key, so
                // passing the name yields "that browser isn't open".
                Some(Surface::Browser { key, .. }) => (true, key.clone()),
                Some(Surface::Session(s)) if Some(*s) != session_at(&surfaces, active) => {
                    match tabs.get(*s) {
                        // Only an AI can be operated by relaying instructions.
                        // Typed into a plain shell/SSH/WSL they would execute
                        // as commands — refuse, don't relay
                        Some(t) if t.ai_kind().is_some() => {
                            (false, t.id.clone().unwrap_or_else(|| t.title.clone()))
                        }
                        Some(_) => {
                            flash = Some(i18n::t("msg.operate.bad_target"));
                            continue;
                        }
                        None => continue,
                    }
                }
                _ => {
                    flash = Some(i18n::t("msg.operate.bad_target"));
                    continue;
                }
            };
            // Remember it, whether or not there is work yet: what is picked on
            // screen IS the setting, and it has to survive the next start
            if remember_aim(
                desks.get_mut(desk_index),
                operator_name.as_deref(),
                Some(&target_id),
            ) {
                watcher.retarget(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
            }
            // A model operator is a browser brain exactly while it is aimed at
            // one: it changes the system prompt it gets and whether its turn
            // reaches the orchestrator, and both must follow the live aim
            if let Some(t) = session_at(&surfaces, active).and_then(|i| tabs.get_mut(i)) {
                t.set_brain(is_browser.then(|| target_id.clone()));
            }
            // Aiming is not yet working. The operator is briefed when there is
            // something to do — otherwise touching the picker would fire a turn
            // at an AI that has not been asked for anything
            if goal.is_empty() {
                continue;
            }
            // The operator (the active tab) must act without confirmation, or every
            // step would stall waiting for a human. The shell already greys the
            // picker out; this backs it up for anything that posts operate directly.
            let operator_ready = session_at(&surfaces, active)
                .and_then(|i| tabs.get(i))
                .map(|t| t.auto_runs())
                .unwrap_or(false);
            if !operator_ready {
                flash = Some(i18n::t("msg.operate.needs_autoapprove"));
                continue;
            }
            // Operating needs an engine to run in; make a bare one if this
            // desk didn't otherwise have any Lua (same gap as Lua actions).
            if engine.is_none() {
                engine = crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)).ok();
            }
            // Attach the active AI as the operator once per (source, target).
            if operating != Some((src_pane, target)) {
                let tab_idx = session_at(&surfaces, active);
                let started = tab_idx
                    .and_then(|i| tabs.get(i))
                    .map(|t| tab_ctx(t, active))
                    .zip(engine.as_mut())
                    .map(|(ctx, eng)| {
                        if is_browser {
                            // The referee is the desk's, as it always was
                            // for a browser driven from the settings file. The
                            // ad-hoc path used to hand over an empty one, so
                            // whoever aimed on screen quietly had no stops
                            let stops = desks
                                .get(desk_index)
                                .map(|w| config::stops_to_lua(&w.stops))
                                .unwrap_or_else(|| "{}".to_string());
                            eng.start_operate(src_pane, &target_id, &stops, &ctx)
                        } else {
                            eng.start_operate_ai(src_pane, &target_id, &ctx)
                        }
                    });
                match started {
                    Some(Ok(())) => {
                        operating = Some((src_pane, target));
                        // start_operate briefs the operator itself (it fires on_start
                        // with the browser protocol). Mark this tab's startup hook as
                        // already fired so the generic on_start machinery above doesn't
                        // brief it a SECOND time now that the agent is attached.
                        if let Some(f) = tab_idx.and_then(|i| started_fired.get_mut(i)) {
                            *f = true;
                        }
                    }
                    Some(Err(e)) => {
                        append_hook_log(&format!("operate start failed: {e:#}"));
                        continue;
                    }
                    None => continue,
                }
            }
            // Deliver the goal to the operator. Queued as a command (like the
            // on_start brief) so it lands after the protocol, not before it.
            if !goal.is_empty()
                && let Some(eng) = engine.as_mut() {
                    eng.deliver_goal(active, &goal);
                }
        }

        // The settings page's "close settings" button. Collapses the settings tab
        // and returns to the operating board (INDEX). Settings disappears from the
        // left-hand list because it drops out of `hosted`, and the layout gets
        // rebuilt on the next draw.
        if shell.mail().take_close_settings() {
            let _ = caps.browser_close(SETTINGS_TAB);
            settings_open = false;
        }
        // The add-a-tab dialog's "More settings": the same page, the whole window
        if shell.mail().take_settings_full() {
            settings_place = SettingsPlace::Full;
        }

        // The sidebar gear. Opens settings from any tab (the menu "e" key only
        // fires while INDEX is in view, so the gear needs its own path).
        // The desk being viewed rides along so its group opens expanded.
        if let Some(want) = shell.take_open_settings() {
            // The gear passes the desk being viewed, and the tab in view so
            // the page opens on its card; a deep-link shortcut may instead name
            // a section to land on and ask to return once saved.
            let mut query = format!("&desk={desk_index}");
            if let Some(f) = want.folder {
                query += &format!("&folder={}", urlish(&f));
            }
            if let Some(n) = want.tabpos {
                query += &format!("&tabpos={n}");
            }
            if let Some(t) = want.tabname {
                query += &format!("&tabname={}", urlish(&t));
            }
            if let Some(k) = want.tabkey {
                query += &format!("&tabkey={}", urlish(&k));
            }
            if let Some(s) = want.section {
                query += &format!("&section={s}");
            }
            if want.ret {
                query += "&ret=1";
            }
            // Standing over the board makes it a dialog, and Escape is a
            // dialog's way out (style guide 5.2). The page hears that key
            // itself -- the board behind cannot, the page has the keyboard
            if want.sheet {
                query += "&sheet=1";
            }
            flash = Some(
                match open_settings(&mut web, &config_file, &remote_info, &web_password, &caps, &query) {
                    Ok(()) => {
                        settings_open = true;
                        // A link that named one thing stands over the board it
                        // was pressed on; the settings themselves get the window
                        settings_place =
                            if want.sheet { SettingsPlace::Sheet } else { SettingsPlace::Full };
                        // A page built now sits above every page built before
                        // it, and the panel was built before. Put it back on
                        // top -- it is what sent them here, and being covered
                        // by the screen it opened would be the worst of both
                        if guide_open {
                            caps.raise_page(GUIDE_TAB);
                        }
                        i18n::t("msg.settings_here")
                    }
                    Err(e) => i18n::tp("msg.settings_failed", &[("error", &e.to_string())]),
                },
            );
        }

        // The status bar's "remote connected" control. Cut every remote session
        // honestly: rotate the token so a phone that already loaded the old URL
        // fails auth on its next request, and drop the connections it holds open.
        // The window reclaims its own terminal width on the page side (its click
        // also fires a fresh resize report), so nothing to do for width here.
        // With a sticky pairing (remote.sticky_token) the token is the string the
        // person wrote into settings, so the cut only drops connections and
        // password sessions; revoking a phone means changing that string.
        let sticky = cfg.as_ref().is_some_and(|c| c.remote.sticky_token);
        if let Some(step) = shell.mail().take_coach_done()
            && step > coach_seen {
                coach_seen = step;
                let _ = crate::crypto::write_atomic(&config::state_path("coach"), &step.to_string());
            }
        if let Some(open) = shell.mail().take_thanks() {
            if open {
                crate::webui::open_external(match thanks_kind {
                    "store" => STORE_REVIEW_URL,
                    _ => REPO_URL,
                });
            }
            // Asked once. Pressed either way, it is over
            thanks_show = false;
            thanks_asked = true;
            let _ = crate::crypto::write_atomic(&config::state_path("thanks-asked"), "1");
        }
        // The ? beside the gear. It used to open the manual on the site;
        // now it opens something that answers, and the manual is a line
        // inside it. Pressing it again puts it away
        if shell.mail().take_help_site() {
            guide_open = !guide_open;
            match guide_open {
                true => {
                    if let Err(e) =
                        open_guide(&mut web, &config_file, &remote_info, &web_password, &caps)
                    {
                        append_hook_log(&format!("the guide would not open: {e:#}"));
                        guide_open = false;
                    }
                }
                false => shut_guide(&caps),
            }
        }
        // What the panel asked the window for while it was open. Nothing here
        // happens on the panel's own thread: opening a screen, moving and
        // closing are all the window's, and this is where the window is
        if guide_open {
            let wants = crate::guide::take_wants();
            if wants.moved != (0, 0) {
                guide_at = guide_at.moved(wants.moved, shell.geom_full());
            }
            if let Some(handle) = wants.open {
                // A desk's screens are named `desk:<entry>` and the program's
                // by the entry alone, which is exactly what the page's
                // ?section= understands
                shell.mail().open_settings = Some(crate::mailbox::SettingsWanted {
                    section: Some(handle),
                    // Over the board, so the panel that sent them there is
                    // still beside the screen it sent them to
                    sheet: true,
                    ..Default::default()
                });
            }
            if wants.shut {
                guide_open = false;
                shut_guide(&caps);
            }
        }
        // The maker's install page for the tab in front, when it is one that
        // could not start. The address comes from the profile, never the page
        if shell.mail().take_install_help()
            && let Some(Surface::Failed { install_url: Some(url), .. }) = surfaces.get(active.wrapping_sub(1))
        {
            crate::webui::open_external(url);
        }
        // The same kind of page for a program named beside its button. Named by
        // its command; the address is still the app's own
        for prog in shell.mail().take_install_pages() {
            if let Some(url) = crate::webui::install_page(&prog) {
                crate::webui::open_external(&url);
            }
        }
        // "Refresh" on the setup: the PC is asked again, because the person
        // has just installed something. The list for new folders is asked with
        // it -- it answers the same question, and would otherwise go on saying
        // the AI is not there until the next start
        if let Some(step) = shell.mail().take_setup_refresh()
            && setup_view.is_some()
        {
            let now = crate::webui::setup_state();
            // Said either way, and about what the page it was pressed on asks:
            // a press that finds nothing new changes nothing on the setup, and
            // would otherwise look like a press that missed
            flash = Some(setup_found(&now, step));
            setup_view = Some(now);
            ai_choices = startable_ais();
        }
        // The first-start setup was answered. The AI is kept only if it is one
        // the setup offered as installed: the page says which card was
        // pressed, it does not get to write any word it likes into the
        // settings. Written down as answered either way, so it is asked once
        if let Some((ai, yolo)) = shell.mail().take_setup()
            && let Some(offered) = setup_view.take()
        {
            if let Some(ai) = ai.filter(|a| offered.installed.iter().any(|x| &x.id == a)) {
                config::save_setting(&["ai_engine"], serde_json::json!(ai));
                assistant_ai = ai;
            }
            config::save_setting(&["yolo"], serde_json::json!(yolo));
            // A desk for the work to go into, signing in to GitHub the way
            // GitHub CLI already does when it is here. Looked for again rather
            // than taken from the page: it may have been installed since the
            // page last asked. The settings file changing is what brings the
            // desk onto the screen
            let accounts = match crate::tab::resolve_command("gh").is_some() {
                true => vec![config::GitAccountSpec {
                    name: FIRST_DESK_GH.into(),
                    method: Some(config::GH_METHOD.into()),
                    ..Default::default()
                }],
                false => Vec::new(),
            };
            if config::make_first_desk(FIRST_DESK, &accounts) {
                setup_reload = Some(std::time::Instant::now());
            }
            let _ = crate::crypto::write_atomic(&config::state_path(SETUP_ANSWERED), "1");
        }
        // The update card was answered. Either answer puts it away for this
        // version; "open" leads to the settings' Update card, where the one
        // button that fetches and installs is -- the card itself installs
        // nothing, so a press by mistake costs nothing
        if let Some(open) = shell.mail().take_update_card() {
            update::card_answered();
            if open {
                // The card that says a newer version is out: its one card, over
                // the board the card was drawn on
                shell.mail().open_settings = Some(crate::mailbox::SettingsWanted {
                    section: Some("update".into()),
                    sheet: true,
                    ..Default::default()
                });
            }
        }
        // A tab asked for from the screen: a row in the list or the bar, a
        // notification clicked. What it does is `look_at`'s to say, the same
        // as the number pressed on the keyboard
        for n in shell.mail().take_selects() {
            // An empty pane is a pane waiting to be filled, and the list is
            // where the things to fill it with are. Pressing one while the
            // keyboard stands in an empty pane puts it THERE rather than
            // leaving the split for it -- which is the only way to show a row
            // of another folder beside this one, and was otherwise missing
            // entirely: the pane's own + makes a NEW tab, and nothing put an
            // existing one anywhere.
            //
            // Only while the pane is empty, so a press is never ambiguous: a
            // pane with something in it is not waiting for anything, and there
            // a press means what it means everywhere else
            if open_split.is_some()
                && pane_layout.focused_surface() == 0
                && (1..=surface_count).contains(&n)
                && !matches!(ui_surface_at(&surfaces, n), Some(Surface::Split { .. }))
            {
                let into = pane_layout.focus();
                pane_layout.put(into, n);
                pane_layout.focus_pane(into);
                active = pane_layout.focused_surface();
                board_open = false;
                view_drifted = false;
                view_touched_ms = start.elapsed().as_millis() as u64;
                continue;
            }
            if let Some(v) = look_at(n, surface_count, active, settings_open) {
                (active, board_open, settings_open) = (v.active, v.board_open, v.settings_open);
                // Pressing a row means "show me this row", and a row shown is
                // shown on its own. Inside a split, some of the rows in the
                // list are also in one of its panes, and without this a press
                // on one of those only moved the keyboard from pane to pane:
                // the split stayed in front, its mark stayed on the split, and
                // there was no row left to press that would get out of it.
                //
                // Moving between the panes is done in the panes -- that is what
                // they are, rectangles to point at
                left_split = true;
                view_touched_ms = start.elapsed().as_millis() as u64;
            }
        }
        for idx in shell.mail().take_limit_acks() {
            if let Some(i) = session_at(&surfaces, idx)
                && let Some(t) = tabs.get_mut(i) {
                    t.dismiss_limit_note();
                }
        }
        if shell.mail().take_remote_cut() && remote_ui.is_some() {
            if let Some(r) = remote_ui.as_mut() {
                if sticky {
                    r.cut_sessions();
                } else {
                    let new = random_hex(24);
                    // Persisted, or the old token would come back with the next
                    // launch and a cut phone with it. (A token pinned in
                    // secrets.json still wins at launch — that pin is the
                    // person's explicit choice; the rotation holds until then)
                    let _ = crypto::write_atomic(&config::state_path("remote-token"), &new);
                    r.rotate_token(new);
                }
            }
            publish_remote(&remote_info, &remote_ui);
            last_remote_ui = None;
            flash = Some(i18n::t(if sticky { "msg.remote_cut_sticky" } else { "msg.remote_cut" }));
        }

        // A built-in orchestrator (discussion / code review / browser rally)
        // just finished: show its transcript as a chat-style result tab and
        // switch to it. Don't steal the screen while the human is in settings.
        if let Some(run_id) = caps.take_open_result() {
            if settings_open {
                append_hook_log(&format!(
                    "open_result {run_id} deferred: settings overlay is open"
                ));
            } else {
                match open_result(&mut web, &config_file, &remote_info, &web_password, &caps, &run_id) {
                    Ok(()) => active = placed_active(&surfaces, RESULT_TAB),
                    Err(e) => append_hook_log(&format!("open_result failed: {e}")),
                }
            }
        }

        // The top bar was pressed. The destination is whatever page is currently
        // viewed (only one bar is ever shown). Don't touch chain depth — that's
        // only counted when work is passed to another tab.
        for go in shell.mail().take_gos() {
            let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) else {
                continue;
            };
            // Reject operations that aren't shown. It would be strange for
            // something not on screen to still work.
            let Some(spec) = caps.nav_of(key) else {
                continue;
            };
            use shikisha_shared::Go;
            let allowed = match &go {
                Go::Back => spec.back,
                Go::Forward => spec.forward,
                Go::Reload => spec.reload,
                // Its own switch. Shift on the plain button is a shortcut for
                // it, so that is allowed wherever either is shown
                Go::Hard => spec.reload_hard || spec.reload,
                Go::To(_) => spec.url,
            };
            if !allowed {
                continue;
            }
            // Check that text a human typed is an allowed destination before passing it along
            let go = match go {
                Go::To(raw) => match crate::view::openable(&raw) {
                    Some(u) => Go::To(u),
                    None => {
                        flash = Some(i18n::tp("msg.nav.bad_url", &[("url", raw.trim())]));
                        continue;
                    }
                },
                other => other,
            };
            append_hook_log(&format!("Navigate {key}: {go:?}"));
            let _ = caps.browser_go(key, go);
            // The location changes right after navigating. Make the next draw ask again.
            asked_where_ms = 0;
        }
        // The answer comes back using the name inside the window. Convert it back
        // to the human-facing id before caching it.
        for (child, url, can_back, can_forward) in shell.mail().take_wheres() {
            if let Some(name) = caps.name_of_child(&child) {
                where_now = Some((name, url, can_back, can_forward));
            }
        }
        // Load start/end likewise gets converted to the id before caching.
        // Update the start time, used for the top bar's "in progress" indicator.
        for (child, busy) in shell.mail().take_loading() {
            if let Some(name) = caps.name_of_child(&child) {
                let now = std::time::Instant::now();
                let e = loading_now.entry(name).or_insert((false, now));
                if busy {
                    e.1 = now;
                }
                e.0 = busy;
            }
        }

        // A page that finished loading. Fires on every navigation.
        for (child, url, complete) in shell.mail().take_loads() {
            let Some(name) = caps.name_of_child(&child) else {
                continue;
            };
            append_hook_log(&format!(
                "Loaded {name}: {url} ({})",
                if complete { "fully" } else { "DOM only" }
            ));
            if auto_enabled
                && let (Some(eng), Some(page)) =
                    (engine.as_mut(), page_ctx(&surfaces, &name, url, complete))
                {
                    eng.fire_page("on_load", &page);
                }
        }

        let polled = shell.poll(
            Duration::from_millis(16),
            session_at(&surfaces, active).and_then(|i| tabs.get(i)),
        )?;
        // Once the window is gone, fall through to the same place as Ctrl+B q.
        // We want cleanup to live in exactly one place.
        if shell.mail().closed {
            break;
        }
        if std::mem::take(&mut shell.mail().tray_open) {
            shell.show();
        }
        // The ✕ puts the window away by default: the AIs in the tabs go on
        // working and the phone stays connected, which is the point of a
        // program that conducts things. Quitting is the icon's menu, Ctrl+B q,
        // or the ✕ for those who set it so -- and every one of those asks
        // first when an AI is at work
        // The settings' Update button was pressed on a version that is ready.
        // Putting it in place ends this program, so the same question quitting
        // asks is asked first; the swap itself is update::apply, and the new
        // copy is started from there. The Store copy hands the job to the
        // Store instead, which ends the program itself when it is done
        if let Some(what) = update::take_apply() {
            if shell.confirm_quit(quit_busy(&tabs, &desk_tabs)) {
                match what {
                    update::Apply::Store => {
                        let _ = shell.install_store_update();
                    }
                    other => {
                        if update::apply(&other).is_ok() {
                            break;
                        }
                    }
                }
            } else {
                update::apply_declined();
            }
        }
        let close_pressed = std::mem::take(&mut shell.mail().close_requested);
        let quit_chosen = std::mem::take(&mut shell.mail().tray_quit);
        if close_pressed && resident {
            shell.hide();
            shell.say_where_it_went();
        } else if (close_pressed || quit_chosen) && shell.confirm_quit(quit_busy(&tabs, &desk_tabs)) {
            break;
        }
        let Some(ev) = polled else {
            continue;
        };

        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                flash = None;
                // Overlays (help / QR / desk list) take top priority
                if help_open {
                    help_open = false;
                    continue;
                }
                if qr_open {
                    qr_open = false;
                    continue;
                }
                if desk_open {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => desk_open = false,
                        KeyCode::Char(c @ '1'..='9') => {
                            let n = c as usize - '1' as usize;
                            if n < desks.len() {
                                switch_desk(
                                    n,
                                    &mut desk_index,
                                    &mut tabs,
                                    &mut desk_tabs,
                                    &desks,
                                    &mut active,
                                    &mut pane_layout,
                                    rows,
                                    cols,
                                    &mut startup_errors,
                                    &mut started_fired,
                                    cfg.as_ref(),
                                    &mut engine,
                                    &mut engines,
                                    &caps,
                                    &notifier,
                                    &prs,
                                    &last_session,
                                );
                            }
                            desk_open = false;
                            // Switching desk drops the settings overlay (it's
                            // hosted per-desk); don't leave the flag stuck on.
                            settings_open = false;
                        }
                        _ => {}
                    }
                    continue;
                }
                // What this press means, if anything. One place decides, and
                // what comes out is the action's own character -- so the arms
                // below never learn that keys can be moved, and neither do the
                // page's buttons, which press those characters themselves
                // A press that belonged to the prefix is used up whatever it
                // turned out to mean. Otherwise an unbound key after the
                // prefix would fall through and be typed into the tab, which
                // is how a stray "w" once ended up as "wwww" in a session
                let mut used = true;
                let meant = if prefix_active {
                    prefix_active = false;
                    keymap.after_prefix(key.code)
                } else if keymap.is_prefix(&key) {
                    prefix_active = true;
                    None
                } else {
                    used = false;
                    keymap.direct(&key)
                };
                if let Some(code) = meant {
                    match code {
                        KeyCode::Char('q') => {
                            if shell.confirm_quit(quit_busy(&tabs, &desk_tabs)) {
                                break;
                            }
                        }
                        // Open the command palette from any tab. It is drawn by
                        // the page, so this only nudges it open
                        KeyCode::Char(':') => shell.open_palette(),
                        // The quick commands, over everything. Drawn by the
                        // page like the palette, so this only nudges it open
                        KeyCode::Char('k') => shell.open_quick(),
                        // The ideas, the same way: drawn by the page
                        KeyCode::Char('m') => shell.open_ideas(),
                        // 0 is the board, which is a screen over everything;
                        // 1.. are the running things, which live in panes. One
                        // key row, two different kinds of destination
                        // Straight away rather than through the mailbox: the
                        // keys after it in this same batch belong to the tab
                        // it picked
                        KeyCode::Char(c @ '0'..='9') => {
                            if let Some(v) = look_at(c as usize - '0' as usize, surface_count, active, settings_open) {
                                (active, board_open, settings_open) = (v.active, v.board_open, v.settings_open);
                                view_touched_ms = start.elapsed().as_millis() as u64;
                            }
                        }
                        // Cycling walks the running things only. The board is
                        // not one of them, and stopping on it on the way past
                        // would be stopping on a different kind of thing
                        KeyCode::Char('n') | KeyCode::Char('p') => {
                            if surface_count > 0 {
                                let fwd = key.code == KeyCode::Char('n');
                                active = match (active, fwd) {
                                    (0, _) => 1,
                                    (a, true) if a >= surface_count => 1,
                                    (a, true) => a + 1,
                                    (1, false) => surface_count,
                                    (a, false) => a - 1,
                                };
                                board_open = false;
                                view_touched_ms = start.elapsed().as_millis() as u64;
                            }
                        }
                        // Ctrl+B b sends a literal Ctrl+B through to the child process
                        KeyCode::Char('b') => {
                            if let Some(t) = session_mut(&mut tabs, &surfaces, active) {
                                t.write_bytes(&[0x02])?;
                            }
                        }
                        // Ctrl+B r restarts this tab (recovers from exit/disconnect)
                        // and carries the conversation over; Ctrl+B R starts a
                        // new one. The default is the way round it is because
                        // the cases where this key is the ONLY way out — the CLI
                        // died, hung, or updated itself — all want the
                        // conversation back, while wanting a clean slate has an
                        // answer inside the CLI already (/clear)
                        KeyCode::Char('r') | KeyCode::Char('R') => {
                            flash = retry_failed(active, &surfaces, &mut tabs, desks.get(desk_index), rows, cols, Some(&last_session))
                                .or_else(|| {
                                    restart_surface(
                                        active,
                                        key.code == KeyCode::Char('r'),
                                        &mut tabs,
                                        &surfaces,
                                        &mut engine,
                                        &caps,
                                        rows,
                                        cols,
                                    )
                                });
                        }
                        // Ctrl+B l toggles the input lock / w desk list / ? help
                        KeyCode::Char('l') => {
                            if let Some(t) = session_mut(&mut tabs, &surfaces, active) {
                                t.locked = !t.locked;
                                flash = Some(i18n::t(if t.locked {
                                    "msg.lock_on"
                                } else {
                                    "msg.lock_off"
                                }));
                            }
                        }
                        KeyCode::Char('w') => {
                            // With nowhere to switch to, opening a list of one
                            // is not an answer -- and saying nothing at all is
                            // indistinguishable from a menu item that is broken
                            if desks.len() > 1 {
                                desk_open = true;
                            } else {
                                flash = Some(i18n::t("msg.desk.only_one"));
                            }
                        }
                        KeyCode::Char('W') => {
                            if desks.len() > 1 {
                                let next = (desk_index + 1) % desks.len();
                                switch_desk(
                                    next,
                                    &mut desk_index,
                                    &mut tabs,
                                    &mut desk_tabs,
                                    &desks,
                                    &mut active,
                                    &mut pane_layout,
                                    rows,
                                    cols,
                                    &mut startup_errors,
                                    &mut started_fired,
                                    cfg.as_ref(),
                                    &mut engine,
                                    &mut engines,
                                    &caps,
                                    &notifier,
                                    &prs,
                                    &last_session,
                                );
                                settings_open = false;
                            }
                        }
                        KeyCode::Char('?') => help_open = true,
                        // Ctrl+B t opens the settings screen in "add tab" state
                        // (this is what the tab bar's + button sends).
                        // Without changing the nonce, a second press returns to the
                        // same URL and nothing happens.
                        KeyCode::Char('t') => {
                            // Which folder it was asked for from, if it was
                            let at = shell
                                .mail()
                                .add_tab_folder
                                .take()
                                .map(|f| format!("&folder={}", percent_encode(&f)));
                            // Asked as a dialog over the board: what the new
                            // tab runs is the one question, and the board the
                            // + was pressed on stays in sight around it
                            let query = format!(
                                "&addtab={desk_index}{}&float=1&nonce={}",
                                at.unwrap_or_default(),
                                start.elapsed().as_millis()
                            );
                            flash = Some(
                                match open_settings(
                                    &mut web,
                                    &config_file,
                                    &remote_info,
                                    &web_password,
                                    &caps,
                                    &query,
                                ) {
                                    Ok(()) => {
                                        settings_open = true;
                                        settings_place = SettingsPlace::Dialog;
                                        i18n::t("msg.settings_here")
                                    }
                                    Err(e) => i18n::tp(
                                        "msg.settings_failed",
                                        &[("error", &e.to_string())],
                                    ),
                                },
                            );
                        }
                        // Ctrl+B a toggles automation on/off, Ctrl+B x is emergency stop
                        KeyCode::Char('a') => {
                            auto_enabled = !auto_enabled;
                            flash = Some(i18n::t(if auto_enabled {
                                "msg.auto_on"
                            } else {
                                "msg.auto_off"
                            }));
                        }
                        KeyCode::Char('x') => {
                            auto_enabled = false;
                            // A paste on its way out would otherwise keep
                            // trickling in, and its Enter land, after the stop.
                            // Whatever has already gone over stays in the input
                            // box, unsent — which is what stopping means here.
                            pending_send.clear();
                            // Discard every waiting loop too (don't let them revive on resume)
                            if let Some(eng) = engine.as_mut() {
                                eng.cancel_all();
                            }
                            // And the AIs themselves. Stopping the hand-overs
                            // leaves whoever is mid-turn working, and the one
                            // still working is the one the stop was for
                            let halted: Vec<&str> = tabs
                                .iter()
                                .filter(|t| t.interrupt())
                                .map(|t| t.title.as_str())
                                .collect();
                            append_hook_log(&format!(
                                "Emergency stop: automation off, interrupted [{}]",
                                halted.join(", ")
                            ));
                            flash = Some(if halted.is_empty() {
                                i18n::t("msg.emergency_stop")
                            } else {
                                i18n::tp(
                                    "msg.emergency_stop_ai",
                                    &[("tabs", &halted.join(", "))],
                                )
                            });
                        }
                        // Ctrl+B c copies the latest captured response to the clipboard
                        KeyCode::Char('c') => {
                            if let Some(t) = session_mut(&mut tabs, &surfaces, active) {
                                flash = Some(match &t.last_response {
                                    Some(r) if !r.trim().is_empty() => copy_text(r),
                                    _ => i18n::t("msg.no_response"),
                                });
                            }
                        }
                        // Ctrl+B % / | splits side by side, Ctrl+B " / - stacks.
                        // The tmux characters, because the prefix is tmux's; the
                        // second pair because nobody remembers which quote is which.
                        KeyCode::Char('%') | KeyCode::Char('|') | KeyCode::Char('"')
                        | KeyCode::Char('-') => {
                            let dir = match key.code {
                                KeyCode::Char('%') | KeyCode::Char('|') => layout::Dir::Row,
                                _ => layout::Dir::Col,
                            };
                            // Asked for on the pane the keyboard is in, and
                            // answered where every other ask to divide is
                            // answered: dividing a split is one thing and
                            // making one is another, and neither is written
                            // twice
                            let id = pane_layout.focus();
                            shell.mail().pane_splits.push((id, matches!(dir, layout::Dir::Col)));
                            view_touched_ms = start.elapsed().as_millis() as u64;
                        }
                        // Ctrl+B s puts the tab bar away, and brings it back
                        // the width it was. The whole window is worth having
                        // for one screen, and the list of tabs is the part you
                        // are not reading while you read the other
                        KeyCode::Char('s') => shell.toggle_tab_bar(),
                        // Ctrl+B g brings the changed files out on the right,
                        // and puts them away again. The same one width says
                        // which, so there is no second flag to fall out of
                        // step with it
                        KeyCode::Char('g') => shell.toggle_side_bar(),
                        // Ctrl+B = puts the dividers back to even halves. The
                        // mouse can do it by double-clicking one; this does the
                        // whole screen at once
                        KeyCode::Char('=') => pane_layout.equalize(),
                        // Ctrl+B < / > move the divider the focused pane sits
                        // against. There is no drag yet, and a split you cannot
                        // adjust is only half of one — a browser and a terminal
                        // rarely want the same half of the window.
                        KeyCode::Char('<') | KeyCode::Char('>') => {
                            let by = if key.code == KeyCode::Char('>') { 0.05 } else { -0.05 };
                            pane_layout.grow(pane_layout.focus(), by);
                        }
                        // Ctrl+B o cycles panes; the arrows go where you point
                        KeyCode::Char('o') => {
                            let order = pane_layout.leaves();
                            let at = order.iter().position(|(p, _)| *p == pane_layout.focus());
                            if let Some((id, _)) = at.and_then(|i| order.get((i + 1) % order.len()))
                            {
                                pane_layout.focus_pane(*id);
                                active = pane_layout.focused_surface();
                                view_touched_ms = start.elapsed().as_millis() as u64;
                            }
                        }
                        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down => {
                            let dir = match key.code {
                                KeyCode::Left => layout::Move::Left,
                                KeyCode::Right => layout::Move::Right,
                                KeyCode::Up => layout::Move::Up,
                                _ => layout::Move::Down,
                            };
                            if pane_layout.focus_move(dir) {
                                active = pane_layout.focused_surface();
                                view_touched_ms = start.elapsed().as_millis() as u64;
                            }
                        }
                        // Ctrl+B X closes the pane (capital, because lowercase x
                        // is the emergency stop and the two must never be a slip
                        // of the finger apart). The tab itself keeps running —
                        // this closes the view, not the work.
                        KeyCode::Char('X') => {
                            if pane_layout.close(pane_layout.focus()) {
                                active = pane_layout.focused_surface();
                                view_touched_ms = start.elapsed().as_millis() as u64;
                            } else {
                                flash = Some(i18n::t("msg.pane_last"));
                            }
                        }
                        // Ctrl+B & closes the tab in view -- the tab, not the
                        // pane, and so not a slip of the finger away from X.
                        // Through the same queue as its ✕, so it asks the same
                        // question when its AI is at work
                        KeyCode::Char('&') => {
                            if let Some(s) = (!board_open && !settings_open)
                                .then(|| active.checked_sub(1).and_then(|i| surfaces.get(i)))
                                .flatten()
                            {
                                let key = surface_key(s, &tabs);
                                shell.mail().close_tabs.push((active, key, false));
                            }
                        }
                        // Ctrl+B T opens the tab closed last, the way a browser's
                        // Ctrl+Shift+T does
                        KeyCode::Char('T') => shell.mail().reopen_tabs.push(None),
                        // Ctrl+B [ enters copy mode (tmux copy-mode style)
                        KeyCode::Char('[') => {
                            let rows = pty_dims(shell.size()?).0;
                            if let Some(t) = session_mut(&mut tabs, &surfaces, active) {
                                t.copy = Some(CopyState {
                                    cursor_row: rows.saturating_sub(1),
                                    anchor: None,
                                    find: None,
                                    last: String::new(),
                                });
                                // Copy mode looks exactly like not being in
                                // copy mode until you press something. Say
                                // what it is and what it can do, once
                                flash = Some(i18n::t("msg.copy_mode"));
                            }
                        }
                        _ => {}
                    }
                } else if used {
                    // Either the prefix was just pressed and the next key is
                    // the one that says what to do, or the key after it meant
                    // nothing. Neither is the tab's to receive
                } else if board_open {
                    // INDEX = home screen: digit keys switch tabs, letter keys run menu items.
                    // Characters received here must line up with MENU_KEYS
                    // (prevents a case where the board shows something that does nothing when pressed)
                    match key.code {
                        KeyCode::Char(c @ '0'..='9') => {
                            let n = c as usize - '0' as usize;
                            if n == 0 {
                                // Already here
                            } else if n <= surface_count {
                                active = n;
                                board_open = false;
                            }
                        }
                        KeyCode::Char('?') | KeyCode::Char('h') => help_open = true,
                        // Show the QR code for connecting from a phone
                        KeyCode::Char('i') => {
                            if remote_ui.is_some() || netaddr::demo_link().is_some() {
                                qr_open = true;
                            } else {
                                flash = Some(
                                    i18n::t("msg.remote_disabled"),
                                );
                            }
                        }
                        KeyCode::Char('w') => {
                            // With nowhere to switch to, opening a list of one
                            // is not an answer -- and saying nothing at all is
                            // indistinguishable from a menu item that is broken
                            if desks.len() > 1 {
                                desk_open = true;
                            } else {
                                flash = Some(i18n::t("msg.desk.only_one"));
                            }
                        }
                        KeyCode::Char('r') => {
                            let mut msgs = Vec::new();
                            let alone: Vec<bool> =
                                (0..tabs.len()).map(|i| only_one_here(&tabs, i)).collect();
                            for (i, t) in tabs.iter_mut().enumerate() {
                                if t.state != TabState::Exited {
                                    continue;
                                }
                                let (plan, _) =
                                    resume_plan(t, alone.get(i).copied().unwrap_or(false), true);
                                match t.restart_as(rows, cols, plan) {
                                    Ok(()) => msgs.push(t.title.clone()),
                                    Err(e) => msgs.push(format!("{}(failed:{e})", t.title)),
                                }
                            }
                            flash = Some(if msgs.is_empty() {
                                i18n::t("msg.restart_none")
                            } else {
                                i18n::tp("msg.restarted_list", &[("names", &msgs.join(", "))])
                            });
                        }
                        // Connectivity test for notification destinations (lets you
                        // verify settings without waiting for a hook)
                        KeyCode::Char('t') => {
                            flash = Some(if notifier.is_empty() {
                                i18n::t("msg.notify_none")
                            } else {
                                notifier.send_all(&crate::i18n::t("err.main.test_notify_body"))
                            });
                        }
                        // Set, change, or remove the master password (all within the TUI)
                        KeyCode::Char('k') => {
                            flash = Some(manage_master_password(shell, cfg.as_ref(), &mut password)?);
                            // Reflect the change into the settings GUI's encryption too
                            *web_password.lock().unwrap() = password.clone();
                        }
                        // Settings: open inside our own window.
                        // Throwing it at an external browser would leave no way to
                        // tell which window belongs to whom.
                        // "Edit settings" wants the General group open (gen=1); the
                        // desk being viewed rides along too, so its group expands.
                        KeyCode::Char('e') => {
                            let query = format!("&desk={desk_index}&gen=1");
                            flash = Some(
                                match open_settings(&mut web, &config_file, &remote_info, &web_password, &caps, &query)
                                {
                                    Ok(()) => {
                                        // Once opened, switch to that tab.
                                        // Don't leave it opened but invisible.
                                        // If already open, switch to its existing location.
                                        settings_open = true;
                                        settings_place = SettingsPlace::Full;
                                        i18n::t("msg.settings_here")
                                    }
                                    Err(e) => i18n::tp(
                                        "msg.settings_failed",
                                        &[("error", &e.to_string())],
                                    ),
                                },
                            );
                        }
                        // Open the Vault overlay on the window's own page. A
                        // page-side action, so this only nudges it open; the
                        // phone reaches the same overlay by tapping the entry
                        KeyCode::Char('f') => shell.open_vault(),
                        KeyCode::Char('p') => shell.open_palette(),
                        KeyCode::Char('q')
                            if shell.confirm_quit(quit_busy(&tabs, &desk_tabs)) => {
                                break;
                            }
                        _ => {}
                    }
                    // INDEX-END (a test checks whether keys the board offers are received here)
                } else {
                    let size = shell.size()?;
                    let now_ms = start.elapsed().as_millis() as u64;
                    let mut locked_hit = false;
                    if let Some(t) = session_mut(&mut tabs, &surfaces, active) {
                        if t.copy.is_some() {
                            handle_copy_key(t, &key, size, &mut flash)?;
                        } else if t.locked {
                            // Soft lock: viewing and copying still work, but input is ignored
                            locked_hit = true;
                        } else if let Some(bytes) =
                            key_to_bytes_with(&key, crate::tab::keyboard_flags(&t.keyboard))
                        {
                            // Manual input breaks the chain. Except input to a tab that
                            // received a draft doesn't break it — that's not a takeover,
                            // it's joining in; writing more and sending it is all part
                            // of the same flow.
                            if ball.awaiting_human && ball.holder == active {
                                ball.awaiting_human = false;
                            } else {
                                t.chain_depth = 0;
                            }
                            t.last_manual_ms = Some(now_ms);
                            view_touched_ms = now_ms;
                            // Typed characters show up at the very bottom. Scrolled back, they're invisible.
                            to_live(t);
                            finish_paste(&mut pending_send, t, active, now_ms);
                            t.write_bytes(&bytes)?;
                        }
                    }
                    if locked_hit {
                        flash = Some(
                            i18n::t("msg.locked"),
                        );
                    }
                }
            }
            Event::Paste(text) => {
                let now_ms = start.elapsed().as_millis() as u64;
                if let Some(t) = session_mut(&mut tabs, &surfaces, active)
                    && !t.locked {
                        t.chain_depth = 0;
                        t.last_manual_ms = Some(now_ms);
                        to_live(t);
                        finish_paste(&mut pending_send, t, active, now_ms);
                        t.write_bytes(text.as_bytes())?;
                    }
            }
            // A viewer remeasured itself. Nothing to carry out here: it has
            // already written its numbers down on the surface, and the top of
            // the loop cuts the terminals to whichever viewer is looking.
            // Arriving as an event is what wakes the loop to do that promptly.
            Event::Resize(..) => {}
            _ => {}
        }
    }

    if let Some(w) = &web {
        w.shutdown();
    }
    if let Some(r) = &remote_ui {
        r.shutdown();
    }
    if let Some(a) = api_server.as_mut() {
        a.shutdown();
    }
    // The last word on what was on screen. The periodic write above may be up
    // to a few seconds stale, and quitting is exactly when that matters
    if let Some(desk) = desks.get(desk_index) {
        last_session.remember(desk, &tabs, Some(&pane_layout));
        last_session.write();
    }
    for t in tabs.iter_mut() {
        t.kill();
    }
    // A shell on a MicroVM outlives this program unless it is told to end, and
    // the telling is on its way (see `e2b::settle`)
    crate::e2b::settle(Duration::from_secs(5));
    Ok(())
}
/// A value made safe to put in a URL's query.
///
/// Only what would otherwise end the value or start another one. A Windows
/// path is mostly letters, a colon and backslashes, and leaving those legible
/// means the address bar still says where it is going
pub fn urlish(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' | '/' | ':' => out.push(c),
            other => {
                let mut buf = [0u8; 4];
                for b in other.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    out
}
/// Work out the model connections again, and hand them to the tabs that are
/// using them.
///
/// A tab keeps the connection it was launched with, so the second half is not
/// optional: without it, a provider edited while its tab is open changes
/// nothing that tab can see. The new endpoint, the new key and the new wait sit
/// in the settings file being ignored, the tab fails in exactly the way it
/// failed before, and nothing on screen connects the two — the person is left
/// to conclude that the setting does not work. Reported as just that: the wait
/// was set to "as long as it takes" and the tab still gave up at 180 seconds,
/// because 180 was what it had been holding since it opened.
///
/// The model connections, read again from the settings and handed to every
/// model tab -- the ones on screen and the ones parked in the other desks,
/// which are just as open
pub fn reload_providers(
    cfg: &config::Config,
    look: &dyn Fn(&str) -> Option<String>,
    tabs: &mut [Tab],
    parked: &mut [Vec<Tab>],
) {
    let conns = config::app_providers(cfg, look);
    for t in tabs.iter_mut().chain(parked.iter_mut().flatten()) {
        t.refresh_model_conn(&conns);
    }
    bridge::use_connections(conns);
}
/// Give this tab the rest of whatever is being pasted into it, now.
///
/// Called just before anything else writes to that tab. A paste that goes over
/// in pieces holds the tab until it is finished; letting a keystroke into the
/// gaps would type it into the middle of the person's own sentence. The Enter
/// is left where it was — the person may still be adding to what was pasted.
pub fn finish_paste(pending: &mut [PendingSend], t: &Tab, tab: usize, now_ms: u64) {
    for p in pending.iter_mut().filter(|p| p.tab == tab) {
        let rest = p.rest(now_ms);
        if !rest.is_empty() {
            let _ = t.write_passthrough(&rest);
        }
    }
}
/// The name used when placing the settings page inside the window.
/// If the spelling drifts, it gets treated as a different browser and a second copy opens.
pub const SETTINGS_TAB: &str = "settings";
/// The style guide's dialog, in numbers (5.2): how wide it grows to, how tall
/// a page placed in a rectangle is given, how far down it starts, and the edge
/// left around it. One set of numbers for both surfaces -- the window places a
/// page here, and the board on a phone or tablet frames the same page with the
/// same measurements (shell::PAGE reads them as `{{DLG_*}}`)
pub const DLG_WIDE: i32 = 560;
pub const DLG_TALL: i32 = 640;
pub const DLG_TOP: i32 = 56;
pub const DLG_EDGE: i32 = 16;
/// The smallest area a dialog still floats over. Under either measurement there
/// is no board left around it, so it takes the whole area instead
pub const DLG_MIN_W: i32 = DLG_WIDE / 2 + DLG_EDGE * 2;
pub const DLG_MIN_H: i32 = DLG_TALL / 2 + DLG_TOP + DLG_EDGE;
/// And the sheet (5.2): a whole page of settings stood over the board, for a
/// link that named one thing. Wide enough for the page's own two columns --
/// its list beside the card -- where a dialog's width would fold them into the
/// narrow arrangement meant for a phone
pub const SHEET_WIDE: i32 = 1040;
pub const SHEET_TALL: i32 = 760;
/// The smallest area a sheet still stands over, read the same way as a
/// dialog's: under it, the page is given the whole of the area
pub const SHEET_MIN_W: i32 = SHEET_WIDE / 2 + DLG_EDGE * 2;
pub const SHEET_MIN_H: i32 = SHEET_TALL / 2 + DLG_TOP + DLG_EDGE;

/// The name used when placing the guide's panel inside the window.
pub const GUIDE_TAB: &str = "guide";
/// The style guide's floating panel (5.2): how wide, how tall it grows to,
/// and how much of its head must stay inside the window
pub const PANEL_WIDE: i32 = 380;
pub const PANEL_TALL: i32 = 560;
/// The head is 40px; leaving less than this of it inside would leave nothing
/// to take hold of to bring it back
pub const PANEL_HELD: i32 = 24;

/// Where the panel sits, in the content area, and how it is moved and kept
/// inside it.
///
/// The app holds this rather than the page, because the page is the thing
/// being placed: a page cannot put itself somewhere, and a page that thought
/// it knew where it was would be wrong the moment the window was resized.
#[derive(Clone, Copy, Debug)]
pub struct PanelAt {
    /// From the left and the top of the content area, before it is held inside
    pub x: i32,
    pub y: i32,
}

impl Default for PanelAt {
    /// Where it stands the first time: in from the bottom right, which is the
    /// corner the ? that opens it is in
    fn default() -> Self {
        PanelAt { x: i32::MAX, y: i32::MAX }
    }
}

impl PanelAt {
    /// The rectangle it gets, out of the whole content area, kept inside it.
    pub fn rect(self, full: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
        let (ax, ay, aw, ah) = full;
        let w = PANEL_WIDE.min(aw);
        let h = PANEL_TALL.min(ah);
        // The first time, and whenever the window has grown past where it was
        // left, it stands in from the bottom right
        let want = |v: i32, corner: i32| if v == i32::MAX { corner } else { v };
        let x = want(self.x, aw - w - DLG_EDGE).clamp(PANEL_HELD - w, aw - PANEL_HELD);
        let y = want(self.y, ah - h - DLG_EDGE).clamp(0, (ah - PANEL_HELD).max(0));
        (ax + x, ay + y, w, h)
    }

    /// The same, moved by a drag, so that where it is put is where it was
    /// dragged to rather than where the drag started
    pub fn moved(self, by: (i32, i32), full: (i32, i32, i32, i32)) -> Self {
        let (ax, ay, _, _) = full;
        let (x, y, _, _) = self.rect(full);
        PanelAt { x: x - ax + by.0, y: y - ay + by.1 }
    }
}

/// Where an open settings page stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsPlace {
    /// The settings themselves: the whole window, the way INDEX's own gear
    /// opens them
    Full,
    /// One thing's settings, stood over the board it was asked from
    Sheet,
    /// The one question the board's + asks
    Dialog,
}

impl SettingsPlace {
    /// The rectangle this place gives a page, out of the whole content area.
    pub fn rect(self, full: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
        match self {
            SettingsPlace::Full => full,
            SettingsPlace::Sheet => sheet_rect(full),
            SettingsPlace::Dialog => dialog_rect(full),
        }
    }
    /// Whether the board stays drawn behind it.
    pub fn floats(self) -> bool {
        self != SettingsPlace::Full
    }
}

/// Where a sheet-sized page goes over the board, given the whole content area.
/// The dialog's rule, at the sheet's size (see `dialog_rect`).
pub fn sheet_rect(full: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = full;
    if w < SHEET_MIN_W || h < SHEET_MIN_H {
        return full;
    }
    let dw = SHEET_WIDE.min(w - DLG_EDGE * 2);
    let dh = SHEET_TALL.min(h - DLG_TOP - DLG_EDGE);
    (x + (w - dw) / 2, y + DLG_TOP, dw, dh)
}
/// Where a dialog-sized page goes over the board, given the whole content area.
///
/// The style guide's dialog: at most 560 wide, 56 down from the top, centred
/// across. A page placed in the window cannot grow to fit what is in it, so its
/// height is chosen here and the dialog scrolls inside itself. An area too
/// small to leave any board around it gets the whole area -- a dialog squeezed
/// into a corner of a window that small would only be harder to use
pub fn dialog_rect(full: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = full;
    if w < DLG_MIN_W || h < DLG_MIN_H {
        return full;
    }
    let dw = DLG_WIDE.min(w - DLG_EDGE * 2);
    let dh = DLG_TALL.min(h - DLG_TOP - DLG_EDGE);
    (x + (w - dw) / 2, y + DLG_TOP, dw, dh)
}
/// The page in view, when putting it back the way it started is a thing that
/// makes sense — otherwise None.
///
/// The settings screen and the result view ride in the pane list like any other
/// page, but they are the app's own furniture: they are opened and closed by the
/// app, and restarting them means nothing. Anything else placed in the window is
/// the user's, and `browser_spec` is what says it can be opened again.
///
/// One rule, read by both the keystroke and the button the screen draws, so the
/// button can never appear where the key does nothing.
pub fn restartable_page(surfaces: &[Surface], active: usize, caps: &hooks::Caps) -> Option<String> {
    let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) else {
        return None;
    };
    if key == SETTINGS_TAB || key == RESULT_TAB {
        return None;
    }
    caps.browser_spec(key).map(|_| key.clone())
}
/// The screen number (1-based) to switch to for a placed local page (settings
/// or result). If already open, its own slot; otherwise the slot right after
/// the end (`surfaces.len() + 1`). Using `len()+1` while it is already in the
/// layout would point one slot too far and paint the screen solid black.
pub fn placed_active(surfaces: &[Surface], key_want: &str) -> usize {
    surfaces
        .iter()
        .position(|p| matches!(p, Surface::Browser { key, .. } if key == key_want))
        .map(|i| i + 1)
        .unwrap_or(surfaces.len() + 1)
}
pub fn settings_active(surfaces: &[Surface]) -> usize {
    placed_active(surfaces, SETTINGS_TAB)
}
/// Writes out the signal for one wheel tick, in terminal convention.
///
/// A full-screen program rewinds its own contents itself, so any history we
/// hold means nothing to it. Reporting the scroll itself is the correct thing
/// to do. Button numbers are fixed by convention: 64 is up, 65 is down.
pub fn wheel_bytes(up: bool, row: u16, col: u16, enc: vt100::MouseProtocolEncoding) -> Vec<u8> {
    let button = if up { 64 } else { 65 };
    // The top-left of the screen is 1,1 (not 0-based)
    let (x, y) = (col.saturating_add(1), row.saturating_add(1));
    match enc {
        vt100::MouseProtocolEncoding::Sgr => {
            format!("\x1b[<{button};{x};{y}M").into_bytes()
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            let mut out = b"\x1b[M".to_vec();
            for v in [button + 32, x + 32, y + 32] {
                let mut buf = [0u8; 4];
                out.extend_from_slice(
                    char::from_u32(v as u32).unwrap_or(' ').encode_utf8(&mut buf).as_bytes(),
                );
            }
            out
        }
        // The legacy encoding is one byte per value; it can't represent anything past 223
        _ => {
            let b = |v: u16| (v.min(223) as u8).saturating_add(32);
            vec![0x1b, b'[', b'M', b(button), b(x), b(y)]
        }
    }
}
/// The position after scrolling back. Positive is into the past. There's nothing before 0 (the future).
pub fn scrolled_to(cur: usize, by: i32) -> usize {
    if by > 0 {
        cur.saturating_add(by as usize)
    } else {
        cur.saturating_sub(by.unsigned_abs() as usize)
    }
}
/// The wheel was scrolled.
///
/// If the recipient is watching the mouse, pass the scroll straight through.
/// A full-screen program rewinds its own contents itself, so our history holds
/// nothing useful. If it's not watching (a plain shell, etc.), scroll back
/// through the history we keep instead. `by` is the tick count; positive is into the past.
pub fn scroll_by(t: &Tab, by: i32, row: u16, col: u16) {
    let (wants_mouse, enc) = {
        let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
        let s = p.screen();
        (
            s.mouse_protocol_mode() != vt100::MouseProtocolMode::None,
            s.mouse_protocol_encoding(),
        )
    };
    if wants_mouse {
        // The cap used to be 16 — plenty for a wheel notch or two from the
        // window. The phone's page buttons ask for a whole screenful at once
        // (and a full-screen TUI may only move a fraction of a row per tick),
        // so allow a larger burst; parse_intent still clamps `by` to 250.
        let mut bytes = Vec::new();
        for _ in 0..by.unsigned_abs().min(250) {
            bytes.extend_from_slice(&wheel_bytes(by > 0, row, col, enc));
        }
        let _ = t.write_bytes(&bytes);
        return;
    }
    // 3 lines per tick, matching terminal convention
    let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
    let next = scrolled_to(p.screen().scrollback(), by.saturating_mul(3));
    p.screen_mut().set_scrollback(next);
}
/// Returns to the current, live screen.
///
/// Typed characters show up at the very bottom of the screen. If you type
/// while still scrolled back, you can't see what you're typing.
pub fn to_live(t: &Tab) {
    let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
    if p.screen().scrollback() != 0 {
        p.screen_mut().set_scrollback(0);
    }
}
/// The interval for asking the window "where are you right now".
///
/// The pressable/unpressable appearance lags by this much. It's not worth
/// asking every frame, but it shouldn't lag enough for a human to notice either.
pub const WHERE_EVERY_MS: u64 = 400;
/// How long a division has to stand still before it is written down. Long
/// enough that a divider being dragged is one write and not one per frame
pub const SPLIT_SAVE_AFTER: Duration = Duration::from_millis(600);
/// The page placed in the focused pane, if that is what is there.
///
/// Two things need this and must agree: the pen (which that page draws for
/// itself) and a message raised while it is in front (same reason -- a window
/// of its own cannot be drawn over).
pub fn focused_page(layout: &crate::layout::Layout, surfaces: &[Surface]) -> Option<String> {
    match surfaces.get(layout.surface_of(layout.focus())?.checked_sub(1)?)? {
        Surface::Browser { key, .. } => Some(key.clone()),
        // The panel is drawn by the board, so there is no page in front of
        // anything -- a message can be raised over it like any other pane
        Surface::Session(_)
        | Surface::Git { .. }
        | Surface::Sftp { .. }
        | Surface::Editor { .. }
        | Surface::Failed { .. }
        | Surface::Split { .. }
        | Surface::Issues { .. } => None,
    }
}
/// Write down what a tab is aimed at: in the settings file, and in the copy of
/// it this run is holding.
///
/// Both, or the answer disagrees with itself. The file is what the next start
/// reads; the copy in memory is what the screen is drawn from, and it is not
/// re-read from disk (our own write is deliberately not treated as news, or
/// every pick would announce a settings reload). Returns whether the file was
/// written, which is the caller's cue to leave the watcher unbothered.
pub fn remember_aim(
    desk: Option<&mut config::Desk>,
    operator: Option<&str>,
    aim: Option<&str>,
) -> bool {
    let Some(name) = operator else { return false };
    if let Some(desk) = desk {
        for t in desk.tabs.iter_mut() {
            if t.cfg.id.as_deref() == Some(name) {
                t.cfg.drives = aim.map(str::to_string);
            }
        }
    }
    if config::save_tab_aim(name, aim) {
        append_hook_log(&match aim {
            Some(a) => format!("{name} is aimed at {a}"),
            None => format!("{name} is aimed at nothing"),
        });
        return true;
    }
    // A tab with no name of its own in the file has nowhere to keep this. It
    // still works for this run; it just won't be there next time, and saying so
    // beats a silent forgetting
    append_hook_log(&format!("could not record the aim for {name}"));
    false
}
/// What the tab in `surface` is aimed at (🎯), as a surface number.
///
/// The aim is picked on screen and written into the settings file, so this is
/// how a restart gets it back: read what was written for that tab, and turn the
/// id back into the number the screen speaks in. There is no separate "default
/// target" setting to reconcile with — one place holds the answer.
pub fn aim_of(
    desk: Option<&config::Desk>,
    surfaces: &[Surface],
    tabs: &[Tab],
    surface: usize,
) -> Option<usize> {
    let t = session_at(surfaces, surface).and_then(|i| tabs.get(i))?;
    let me = t.id.clone()?;
    let aim = desk?
        .tabs
        .iter()
        .find(|x| x.cfg.id.as_deref() == Some(me.as_str()))?
        .cfg
        .drives
        .clone()
        .filter(|d| !d.trim().is_empty())?;
    hooks::TabRef::Name(aim).resolve(&surface_keys(surfaces, tabs))
}
/// How long the panel waits on the far end before saying it did not answer.
///
/// Shorter than a script's own wait: a person is looking at the screen, and a
/// list that takes a minute to arrive is a broken screen whatever it says
pub const SFTP_WAIT_MS: u64 = 45_000;

/// How large a file the panel will read to compare it with another.
///
/// Both sides are read whole and held in memory to be compared, and a person
/// cannot read a diff of a file this size anyway. A build artefact dropped
/// into the wrong folder is the usual way somebody arrives here, and being
/// told so beats waiting for a megabyte of minified JavaScript to arrive
pub const SFTP_DIFF_MAX: usize = 1 << 20;

/// A file's text, or why it is not going to be compared.
///
/// Size first: a refusal that arrives before the reading is a refusal that
/// cost nothing. Then whether it is text at all, because a diff of two
/// pictures is a wall of replacement characters, not an answer.
///
/// Text in whatever encoding it is saved in: a Shift_JIS CSV on the server is
/// as much a file to compare as a UTF-8 one here. `encoding` is somebody's
/// choice when the guess was wrong, and is shown however well it reads -- the
/// marks are what tell them it is still the wrong one
pub fn diff_text(
    name: &str,
    bytes: &[u8],
    encoding: Option<&'static encoding_rs::Encoding>,
) -> std::result::Result<crate::charset::Reading, String> {
    if bytes.len() > SFTP_DIFF_MAX {
        return Err(i18n::tp(
            "err.sftp.diff_big",
            &[("name", name), ("max", &format!("{} MB", SFTP_DIFF_MAX / (1 << 20)))],
        ));
    }
    // A zero byte is what every tool uses to tell a picture from a page, and
    // it agrees with "not valid text" on everything either of them can see
    if bytes.contains(&0) {
        return Err(i18n::tp("err.sftp.diff_not_text", &[("name", name)]));
    }
    match encoding {
        Some(e) => Ok(crate::charset::read_as(bytes, e)),
        None => Some(crate::charset::read(bytes))
            .filter(|r| r.exact)
            .ok_or_else(|| i18n::tp("err.sftp.diff_not_text", &[("name", name)])),
    }
}

/// What the two sides of a comparison were read as, in the order the window
/// lists them: the server's, then this machine's. Said once when they agree,
/// and a side that is plain ASCII agrees with anything -- it reads the same in
/// all of them, and naming it UTF-8 beside a Shift_JIS file would be a
/// difference that is not there
pub fn diff_encodings(there: (&crate::charset::Reading, &[u8]), here: (&crate::charset::Reading, &[u8])) -> String {
    let named: Vec<&str> = [there, here]
        .iter()
        .filter(|(_, bytes)| !bytes.is_ascii())
        .map(|(r, _)| r.encoding.name())
        .collect();
    match named.as_slice() {
        [] => encoding_rs::UTF_8.name().to_string(),
        [one] => one.to_string(),
        [a, b] if a == b => a.to_string(),
        [a, b] => format!("{a} / {b}"),
        _ => unreachable!(),
    }
}
/// What the column's file list, or the editor, asked for.
///
/// Returns the answer when there is one to give at once, and `None` when the
/// folder is on another machine and a thread will send it along (see
/// [`files_at`] for which folder that is).
pub fn files_answer(
    panel: &str,
    act: &str,
    args: &serde_json::Value,
    surfaces: &[Surface],
    tabs: &[Tab],
    caps: &crate::caps::Capabilities,
    far: &std::sync::mpsc::Sender<FarFiles>,
) -> Option<String> {
    match files_at(panel, surfaces, tabs) {
        Some(FilesAt::Here(root)) => Some(files_here(panel, act, args, &root)),
        Some(FilesAt::There { at, root }) => files_there(panel, act, args, at, root, caps, far),
        None => Some(
            serde_json::json!({"act": act, "panel": panel, "ok": false, "error": i18n::t("err.files.no_folder")})
                .to_string(),
        ),
    }
}

/// Where the files a panel asks about are.
///
/// The folder is named the way the git panel names it -- by the tab standing
/// in it -- so the column follows whatever is being looked at without anything
/// being registered anywhere. A panel of its own is looked at first, so a
/// transfer panel keeps answering for its own folder here.
#[derive(Debug, Clone, PartialEq)]
pub enum FilesAt {
    /// A folder on this machine
    Here(std::path::PathBuf),
    /// A folder on another machine, by its path there
    There { at: crate::elsewhere::Elsewhere, root: String },
}

impl FilesAt {
    /// Whether a path the page gave is inside the folder -- the fence every
    /// read and save goes through, on either machine
    pub fn holds(&self, rel: &str) -> bool {
        match self {
            Self::Here(root) => local_under(root, rel).is_some(),
            Self::There { root, .. } => crate::transfer::under_remote(root, rel).is_some(),
        }
    }
}

/// The folder `panel` works in, and which machine it is on.
///
/// An editor, a git panel and a tab in a folder on another machine work in
/// that folder over there. A terminal given only an address was given no
/// folder over there, so what it has is its working folder here
pub fn files_at(panel: &str, surfaces: &[Surface], tabs: &[Tab]) -> Option<FilesAt> {
    for s in surfaces {
        let Some(p) = crate::desk::panel_place(s) else { continue };
        if !p.key.matches(panel) {
            continue;
        }
        return Some(match (s, p.remote) {
            // A file panel's column is its folder here: the far side is the
            // panel's own second list
            (Surface::Sftp { .. }, _) | (_, None) => FilesAt::Here(p.dir),
            (_, Some(at)) if !p.remote_dir.is_empty() => FilesAt::There { at, root: p.remote_dir },
            _ => FilesAt::Here(p.dir),
        });
    }
    let p = tab_place(tabs.iter().find(|t| t.key().matches(panel))?);
    match p.remote {
        Some(at) if !p.remote_dir.is_empty() => Some(FilesAt::There { at, root: p.remote_dir }),
        _ => Some(p.dir).filter(|d| !d.as_os_str().is_empty()).map(FilesAt::Here),
    }
}

/// What a thread working on a folder over there brings back: the answer for
/// the page, and what an open file is now, when the work found out.
pub struct FarFiles {
    /// The panel (for an editor, the editor) the work was for
    pub key: String,
    /// The file the stamp is of, as the page names it
    pub path: String,
    pub js: Option<String>,
    /// The file's mark ([`crate::files::stamp_from`]), when it was looked at
    pub stamp: Option<String>,
    /// When the work set out
    pub asked: Instant,
    /// Whether this was the clock's own look rather than something asked for
    pub polled: bool,
}

/// What an editor on another machine last heard about its file.
pub struct FarSeen {
    pub path: String,
    pub stamp: String,
    /// When the work that heard it set out
    pub at: Instant,
}

/// When an editor on another machine last looked at its file, and whether a
/// look is still out.
pub struct FarPoll {
    pub at: Instant,
    pub busy: bool,
}

/// How recently a terminal on a MicroVM has to have changed for its machine to
/// count as in use
const MACHINE_IN_USE_MS: u64 = 60_000;
/// How often a MicroVM in use is given its minutes again. Well inside a minute,
/// the shortest time a machine can be set to, so it never runs out while in use
const MACHINE_KEPT_EVERY: Duration = Duration::from_secs(30);

/// Give every MicroVM something is at work on its minutes again.
///
/// "At work" is a terminal of it whose screen changed in the last minute: an
/// AI working, a build running, a person typing. A machine where everything is
/// still is left alone, and pauses its minutes after it was last in use --
/// which is what the setting says. Each ask is a thread of its own: it is a
/// round trip, and this loop draws the window
fn keep_machines_up(tabs: &[Tab], now_ms: u64, kept: &mut std::collections::HashMap<String, Instant>) {
    for t in tabs {
        let Some(host) = t.cloud() else { continue };
        let Some(id) = host.instance.clone() else { continue };
        // Paused under its terminal: more minutes would start it again
        if crate::e2b::asleep(&id) {
            continue;
        }
        if t.ms_since_change(now_ms) >= MACHINE_IN_USE_MS
            || kept.get(&id).is_some_and(|at| at.elapsed() < MACHINE_KEPT_EVERY)
        {
            continue;
        }
        kept.insert(id, Instant::now());
        let host = host.clone();
        std::thread::spawn(move || {
            if let Err(e) = crate::e2b::keep_up(&host) {
                append_hook_log(&format!("e2b: keeping {} up failed: {e:#}", host.name));
            }
        });
    }
}

/// How often an editor showing a file on another machine asks whether it
/// changed
const FAR_STAMP_EVERY: Duration = Duration::from_secs(2);
/// How recently a terminal on that machine has to have changed for the asking
/// to happen at all
const FAR_AWAKE_MS: u64 = 20_000;

/// Ask, for each editor showing a file on another machine, whether that file
/// has changed -- only while a terminal on the same machine is moving.
///
/// Asking starts a paused MicroVM, and one that is kept asked never pauses and
/// never stops costing. A terminal there that is changing says the machine is
/// awake and somebody -- the AI in it, as often as not -- is working in it,
/// which is when a file changes under an open editor. When everything there is
/// still, nothing is asked, and a save still refuses to land on bytes that
/// changed since they were read
fn far_stamp_polls(
    editors: &[crate::view::EditorOpen],
    tabs: &[Tab],
    now_ms: u64,
    polls: &mut std::collections::HashMap<String, FarPoll>,
    tx: &std::sync::mpsc::Sender<FarFiles>,
) {
    for e in editors {
        let (Some(at), Some(dir), Some(rel), None) = (&e.at, &e.dir, &e.showing, &e.diff) else { continue };
        if polls.get(&e.key).is_some_and(|p| p.busy || p.at.elapsed() < FAR_STAMP_EVERY) {
            continue;
        }
        let awake = tabs
            .iter()
            .any(|t| {
                tab_machine(t).as_ref() == Some(at)
                    && t.ms_since_change(now_ms) < FAR_AWAKE_MS
                    // The change may be the line that says it paused
                    && !t.cloud().and_then(|h| h.instance.as_deref()).is_some_and(crate::e2b::asleep)
            });
        if !awake {
            continue;
        }
        let Some(path) = crate::transfer::under_remote(&crate::desk::far_path(dir), rel) else { continue };
        let asked = Instant::now();
        polls.insert(e.key.clone(), FarPoll { at: asked, busy: true });
        let (at, key, rel, tx) = (at.clone(), e.key.clone(), rel.clone(), tx.clone());
        std::thread::spawn(move || {
            let stamp = match crate::elsewhere::files(&at, ssh::FileJob::Stat { path }, SFTP_WAIT_MS) {
                Ok(ssh::FileAnswer::One(e)) => Some(crate::files::stamp_from(e.modified, e.size)),
                // Gone, or not answering: nothing to follow, and nothing said.
                // The save is what finds out, and says so
                _ => None,
            };
            let _ = tx.send(FarFiles { key, path: rel, js: None, stamp, asked, polled: true });
        });
    }
}

/// What the column's file list and the editor asked of a folder here.
///
/// Nothing here reaches outside that folder: every path is put back through
/// `local_under`, which is the same fence the transfer panel keeps.
fn files_here(panel: &str, act: &str, args: &serde_json::Value, root: &std::path::Path) -> String {
    let fail = |e: String| {
        serde_json::json!({"act": act, "panel": panel, "ok": false, "error": e}).to_string()
    };
    let str_of = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();

    match act {
        // One folder, as its rows. The root itself when nothing is named
        "ls" => {
            let at = match str_of("at").trim() {
                "" => root.to_path_buf(),
                given => match local_under(root, given) {
                    Some(p) => p,
                    None => return fail(i18n::t("err.sftp.outside")),
                },
            };
            match local_rows(&at) {
                // git's own folder is not content, and it is the one folder
                // nobody opens on purpose. Everything else is shown, ignored
                // or not: a file somebody put there is a file they may want
                Ok(rows) => serde_json::json!({
                    "act": "ls",
                    "panel": panel,
                    "ok": true,
                    "at": rel_of(root, &at),
                    "rows": rows
                        .into_iter()
                        .filter(|r| r.name != ".git")
                        .collect::<Vec<_>>(),
                })
                .to_string(),
                Err(e) => fail(format!("{e:#}")),
            }
        }
        // One file, as text, for the editor to show. A file too big to read
        // in one piece is not one somebody is reading here, and saying so is
        // better than a window that stops answering while it loads
        "read" => {
            let Some(at) = local_under(root, &str_of("path")) else {
                return fail(i18n::t("err.sftp.outside"));
            };
            let size = std::fs::metadata(&at).map(|m| m.len()).unwrap_or(0);
            if size > crate::files::READ_LIMIT {
                return fail(i18n::tp(
                    "err.files.too_big",
                    &[("mb", &(crate::files::READ_LIMIT / (1024 * 1024)).to_string())],
                ));
            }
            match std::fs::read(&at) {
                Ok(bytes) => read_reply(panel, &str_of("path"), &bytes, args, crate::files::stamp_of(&at)),
                Err(e) => fail(format!("{e}")),
            }
        }
        // ...and back again. `mark` is what the page was given when it read:
        // if the file no longer matches it, somebody else wrote in the
        // meantime and this save would throw their work away, so it refuses
        // and says so rather than winning the race
        "write" => {
            let Some(at) = local_under(root, &str_of("path")) else {
                return fail(i18n::t("err.sftp.outside"));
            };
            let had = str_of("mark");
            let now = std::fs::read(&at).map(|b| crate::files::mark_of(&b)).unwrap_or_default();
            if !had.is_empty() && had != now {
                return fail(i18n::t("err.files.moved_on"));
            }
            let bytes = match save_bytes_of(panel, args) {
                Ok(b) => b,
                Err(refused) => return refused,
            };
            match std::fs::write(&at, &bytes) {
                Ok(()) => write_reply(panel, &str_of("path"), &bytes, args, crate::files::stamp_of(&at)),
                Err(e) => fail(format!("{e}")),
            }
        }
        // By name, or by what is inside. Both stop themselves and say so
        "find" | "grep" => {
            let q = str_of("q");
            let found = if act == "find" {
                crate::files::by_name(root, &q, 300)
            } else {
                crate::files::by_text(root, &q, 300)
            };
            serde_json::json!({
                "act": act,
                "panel": panel,
                "ok": true,
                "q": q,
                "hits": found.hits,
                "capped": found.capped,
            })
            .to_string()
        }
        _ => fail(format!("unknown act: {act}")),
    }
}

/// The page's answer to a file that was read, from its bytes -- the same
/// answer whichever machine the bytes came from.
fn read_reply(panel: &str, path: &str, bytes: &[u8], args: &serde_json::Value, stamp: String) -> String {
    let fail = |e: String| serde_json::json!({"act": "read", "panel": panel, "ok": false, "error": e}).to_string();
    // What is not text has no lines to put in an editor, and guessing at its
    // encoding would write the guess back
    if bytes.contains(&0) {
        return fail(i18n::t("err.files.binary"));
    }
    // In the encoding asked for, else the one it most likely is
    let asked = args.get("encoding").and_then(|v| v.as_str()).unwrap_or_default().trim();
    let file = match asked {
        "" => crate::charset::read(bytes),
        name => match crate::charset::named(name) {
            Some(e) => crate::charset::read_as(bytes, e),
            None => return fail(i18n::tp("err.git.unknown_encoding", &[("enc", name)])),
        },
    };
    serde_json::json!({
        "act": "read",
        "panel": panel,
        "ok": true,
        "path": path,
        "text": file.text,
        // What it is saved back as, and whether that gives the same bytes: a
        // reading that lost characters says so
        "encoding": file.encoding.name(),
        "exact": file.exact,
        "stamp": stamp,
        // What the file was when it was read. A save compares this with what
        // is there, so a save can tell "nobody touched it" from "somebody did"
        "mark": crate::files::mark_of(bytes),
    })
    .to_string()
}

/// The bytes a save from the editor writes, or the page's answer saying why
/// there are none -- the same whichever machine they are going to.
fn save_bytes_of(panel: &str, args: &serde_json::Value) -> std::result::Result<Vec<u8>, String> {
    let str_of = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let encoding = match str_of("encoding").trim() {
        "" => encoding_rs::UTF_8,
        name => match crate::charset::named(name) {
            Some(e) => e,
            None => {
                return Err(serde_json::json!({"act": "write", "panel": panel, "ok": false,
                    "error": i18n::tp("err.git.unknown_encoding", &[("enc", name)])})
                .to_string());
            }
        },
    };
    let replacing = args.get("replace").and_then(|v| v.as_bool()).unwrap_or(false);
    let how = if replacing { crate::files::Save::Replacing } else { crate::files::Save::Exact };
    crate::files::save_bytes(&str_of("text"), encoding, how).map_err(|chars| {
        // Nothing is written. The characters are named, so the page can ask
        // which way to go rather than choose for the person
        serde_json::json!({
            "act": "write",
            "panel": panel,
            "ok": false,
            "why": "unwritable",
            "encoding": encoding.name(),
            "chars": chars.iter().take(12).map(|c| c.to_string()).collect::<Vec<_>>(),
            "more": chars.len().saturating_sub(12),
            "error": i18n::tp("err.files.unwritable", &[("enc", encoding.name())]),
        })
        .to_string()
    })
}

/// The page's answer to a save that was written.
fn write_reply(panel: &str, path: &str, bytes: &[u8], args: &serde_json::Value, stamp: String) -> String {
    let encoding = args
        .get("encoding")
        .and_then(|v| v.as_str())
        .and_then(|n| crate::charset::named(n.trim()))
        .unwrap_or(encoding_rs::UTF_8);
    let replacing = args.get("replace").and_then(|v| v.as_bool()).unwrap_or(false);
    serde_json::json!({
        "act": "write",
        "panel": panel,
        "ok": true,
        "path": path,
        "encoding": encoding.name(),
        // What is in the file now, when that is not what was typed
        "text": replacing.then(|| crate::charset::read_as(bytes, encoding).text),
        "mark": crate::files::mark_of(bytes),
        // Our own write moved the stamp on; hand back the new one so the
        // editor does not read its own save as somebody else's change
        "stamp": stamp,
    })
    .to_string()
}

/// How long a search on another machine may take. It is that machine walking
/// its own folder, and a big one takes a while
const FAR_SEARCH_WAIT_MS: u64 = 30_000;

/// What the column's file list and the editor asked of a folder on another
/// machine.
///
/// Everything is done on a thread, since every one of these is a round trip,
/// and the answer arrives through `far`. What can be refused without asking
/// the machine -- a path outside the folder, a permission not given -- is
/// refused here, at once. Every path goes through the same fence and the same
/// permissions the transfer panel and a script's `sftp_*` go through: the
/// folder is the fence, and a person reaches exactly what their own
/// automation would
fn files_there(
    panel: &str,
    act: &str,
    args: &serde_json::Value,
    at: crate::elsewhere::Elsewhere,
    root: String,
    caps: &crate::caps::Capabilities,
    far: &std::sync::mpsc::Sender<FarFiles>,
) -> Option<String> {
    let fail = |e: String| {
        Some(serde_json::json!({"act": act, "panel": panel, "ok": false, "error": e}).to_string())
    };
    let str_of = |k: &str| args.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let fences = crate::transfer::Fences { here: None, there: root.clone() };
    let ready = |job: ssh::FileJob| crate::transfer::ready(job, &fences, caps, grants::Subject::Human);
    let path = str_of("path");
    let asked = Instant::now();
    let (key, panel, act) = (panel.to_string(), panel.to_string(), act.to_string());
    let tx = far.clone();
    // What every answer is sent back in: the page's words, and the file's
    // stamp when this work learned it
    let answer = move |js: serde_json::Value, path: String, stamp: Option<String>| {
        let _ = tx.send(FarFiles { key: key.clone(), path, js: Some(js.to_string()), stamp, asked, polled: false });
    };
    let failed = |act: &str, panel: &str, e: String| serde_json::json!({"act": act, "panel": panel, "ok": false, "error": e});
    let named = |rel: &str| if rel.is_empty() { root.clone() } else { format!("{}/{rel}", root.trim_end_matches('/')) };

    match act.as_str() {
        // One folder, as its rows. The root itself when nothing is named
        "ls" => {
            let rel = str_of("at").replace('\\', "/").trim_matches('/').to_string();
            let job = match ready(ssh::FileJob::List { path: named(&rel) }) {
                Ok(j) => j,
                Err(e) => return fail(format!("{e}")),
            };
            let panel = panel.clone();
            std::thread::spawn(move || {
                let js = match crate::elsewhere::files(&at, job, SFTP_WAIT_MS) {
                    Ok(ssh::FileAnswer::Listing(rows)) => serde_json::json!({
                        "act": "ls",
                        "panel": panel,
                        "ok": true,
                        "at": rel,
                        "rows": rows.into_iter().filter(|r| r.name != ".git").collect::<Vec<_>>(),
                    }),
                    Ok(_) => serde_json::json!({"act": "ls", "panel": panel, "ok": true, "at": rel, "rows": []}),
                    Err(e) => failed("ls", &panel, format!("{e:#}")),
                };
                answer(js, String::new(), None);
            });
            None
        }
        // One file, as text. Looked at before it is read, so a file too big to
        // read in one piece is said as that without it crossing the network
        "read" => {
            let (look, read) = match (
                ready(ssh::FileJob::Stat { path: path.clone() }),
                ready(ssh::FileJob::Read { path: path.clone() }),
            ) {
                (Ok(l), Ok(r)) => (l, r),
                (Err(e), _) | (_, Err(e)) => return fail(format!("{e}")),
            };
            let (panel, args) = (panel.clone(), args.clone());
            std::thread::spawn(move || {
                let entry = match crate::elsewhere::files(&at, look, SFTP_WAIT_MS) {
                    Ok(ssh::FileAnswer::One(e)) => e,
                    Ok(_) => return answer(failed("read", &panel, i18n::t("err.files.binary")), path, None),
                    Err(e) => return answer(failed("read", &panel, format!("{e:#}")), path, None),
                };
                if entry.dir {
                    return answer(failed("read", &panel, i18n::t("err.files.binary")), path, None);
                }
                if entry.size > crate::files::READ_LIMIT {
                    let mb = (crate::files::READ_LIMIT / (1024 * 1024)).to_string();
                    return answer(failed("read", &panel, i18n::tp("err.files.too_big", &[("mb", &mb)])), path, None);
                }
                let stamp = crate::files::stamp_from(entry.modified, entry.size);
                match crate::elsewhere::files(&at, read, SFTP_WAIT_MS) {
                    Ok(ssh::FileAnswer::Bytes(bytes)) => {
                        let js = read_reply(&panel, &path, &bytes, &args, stamp.clone());
                        answer(serde_json::from_str(&js).unwrap_or_default(), path, Some(stamp));
                    }
                    Ok(_) => answer(failed("read", &panel, i18n::t("err.files.binary")), path, None),
                    Err(e) => answer(failed("read", &panel, format!("{e:#}")), path, None),
                }
            });
            None
        }
        // ...and back again. `mark` is what the page was given when it read:
        // what is there is read first, and if it no longer matches, somebody
        // else wrote in the meantime and this save would throw their work
        // away -- so it refuses and says so rather than winning the race
        "write" => {
            let bytes = match save_bytes_of(&panel, args) {
                Ok(b) => b,
                Err(refused) => return Some(refused),
            };
            let (check, write, look) = match (
                ready(ssh::FileJob::Read { path: path.clone() }),
                ready(ssh::FileJob::Write { to: path.clone(), bytes: bytes.clone() }),
                ready(ssh::FileJob::Stat { path: path.clone() }),
            ) {
                (Ok(c), Ok(w), Ok(l)) => (c, w, l),
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return fail(format!("{e}")),
            };
            let had = str_of("mark");
            let (panel, args) = (panel.clone(), args.clone());
            std::thread::spawn(move || {
                if !had.is_empty() {
                    let now = match crate::elsewhere::files(&at, check, SFTP_WAIT_MS) {
                        Ok(ssh::FileAnswer::Bytes(b)) => crate::files::mark_of(&b),
                        // Not reached at all is not "changed": said as it is,
                        // or the only way on offered is to save over a file
                        // that cannot be written either
                        Err(e) => return answer(failed("write", &panel, format!("{e:#}")), path, None),
                        _ => String::new(),
                    };
                    if now != had {
                        return answer(failed("write", &panel, i18n::t("err.files.moved_on")), path, None);
                    }
                }
                if let Err(e) = crate::elsewhere::files(&at, write, SFTP_WAIT_MS) {
                    return answer(failed("write", &panel, format!("{e:#}")), path, None);
                }
                let stamp = match crate::elsewhere::files(&at, look, SFTP_WAIT_MS) {
                    Ok(ssh::FileAnswer::One(e)) => crate::files::stamp_from(e.modified, e.size),
                    _ => String::new(),
                };
                let js = write_reply(&panel, &path, &bytes, &args, stamp.clone());
                answer(serde_json::from_str(&js).unwrap_or_default(), path, Some(stamp).filter(|s| !s.is_empty()));
            });
            None
        }
        // By name, or by what is inside: the machine walks its own folder
        "find" | "grep" => {
            let q = str_of("q");
            // Reading every file's name and contents is reading, and asked of
            // the same table a listing and a read are
            for name in ["sftp_ls", "sftp_read"] {
                if !caps.allows(name, grants::Subject::Human) {
                    return fail(i18n::tp(
                        "err.hooks.not_permitted",
                        &[("name", name), ("who", &i18n::t("grant.who.human"))],
                    ));
                }
            }
            if q.trim().is_empty() {
                return Some(
                    serde_json::json!({"act": act, "panel": panel, "ok": true, "q": q, "hits": [], "capped": false})
                        .to_string(),
                );
            }
            const LIMIT: usize = 300;
            let command = match act.as_str() {
                "find" => crate::files::far::names_command(&root),
                _ => crate::files::far::text_command(&root, &q, LIMIT),
            };
            let panel = panel.clone();
            std::thread::spawn(move || {
                let js = match crate::elsewhere::exec(&at, &command, FAR_SEARCH_WAIT_MS) {
                    Ok(ran) if ran.ok() => {
                        let found = match act.as_str() {
                            "find" => crate::files::far::names(&ran.out, &q, LIMIT),
                            _ => crate::files::far::text(&ran.out, &q, LIMIT),
                        };
                        serde_json::json!({"act": act, "panel": panel, "ok": true, "q": q,
                            "hits": found.hits, "capped": found.capped})
                    }
                    Ok(ran) => failed(&act, &panel, ran.said()),
                    Err(e) => failed(&act, &panel, format!("{e:#}")),
                };
                answer(js, String::new(), None);
            });
            None
        }
        _ => fail(format!("unknown act: {act}")),
    }
}

/// A folder under the root, written the one way the page uses: relative,
/// forward slashes, empty for the root itself.
fn rel_of(root: &std::path::Path, at: &std::path::Path) -> String {
    at.strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

/// One thing the file panel asked for.
///
/// Returns the answer when there is one to give at once, and `None` when the
/// far end has been asked and a thread will send it along. Everything that
/// touches a server asks the same permission table a script does -- the panel
/// is a screen a person opened, so it reaches exactly what their own
/// automation would, and not one thing more.
pub fn sftp_answer(
    panel: &str,
    act: &str,
    args: &serde_json::Value,
    surfaces: &[Surface],
    caps: &std::rc::Rc<crate::caps::Capabilities>,
    tx: &std::sync::mpsc::Sender<String>,
) -> Option<String> {
    let fail = |e: String| {
        Some(
            serde_json::json!({"act": act, "panel": panel, "ok": false, "error": e})
                .to_string(),
        )
    };
    let Some((local_root, machine, remote_root)) = surfaces.iter().find_map(|s| match s {
        Surface::Sftp { key, dir, at, remote_dir, .. } if key == panel => {
            Some((dir.clone(), at.clone(), remote_dir.clone()))
        }
        _ => None,
    }) else {
        return fail(i18n::t("err.sftp.no_panel"));
    };
    let str_of = |k: &str| {
        args.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string()
    };

    // Where this panel stands, on both sides, and whether it has been told
    // enough to reach the far one
    if act == "hello" {
        return Some(
            serde_json::json!({
                "act": "hello",
                "panel": panel,
                "ok": true,
                "data": {
                    // Who, where -- or just where, for a machine that
                    // hands out one account nobody chose
                    "server": machine.as_ref().map(|m| match m.user() {
                        Some(who) => format!("{who}@{}", m.address()),
                        None => m.address(),
                    }),
                    "local_root": local_root.as_ref().map(|p| display_path_of(p)),
                    "remote_root": remote_root,
                },
            })
            .to_string(),
        );
    }

    // This machine's side. No connection is involved, so it is answered here
    if act == "local" {
        let Some(root) = local_root.clone() else {
            return fail(i18n::t("err.sftp.no_folder"));
        };
        let at = match str_of("at").trim() {
            "" => root.clone(),
            given => match local_under(&root, given) {
                Some(p) => p,
                None => return fail(i18n::t("err.sftp.outside")),
            },
        };
        return match local_rows(&at) {
            Ok(rows) => Some(
                serde_json::json!({
                    "act": "local",
                    "panel": panel,
                    "ok": true,
                    "at": display_path_of(&at),
                    "root": display_path_of(&root),
                    "rows": rows,
                })
                .to_string(),
            ),
            Err(e) => fail(format!("{e:#}")),
        };
    }

    // A folder on this machine, made from the panel so that a place to put
    // what is coming back can be made without leaving the screen. Only making
    // one: deleting and renaming here are what this machine's own file manager
    // is for, and there is no primitive behind them to ask permission of
    if act == "local_mkdir" {
        let Some(root) = local_root.clone() else {
            return fail(i18n::t("err.sftp.no_folder"));
        };
        let Some(at) = local_under(&root, &str_of("path")) else {
            return fail(i18n::t("err.sftp.outside"));
        };
        return match std::fs::create_dir(&at) {
            Ok(()) => Some(
                serde_json::json!({"act": act, "panel": panel, "ok": true}).to_string(),
            ),
            Err(e) => fail(format!("{e}")),
        };
    }

    // Everything left goes to the far end, which has to be known by now
    let Some(machine) = machine else {
        return fail(i18n::t("err.sftp.no_address"));
    };

    // The job a pressed button becomes. Only the translation is here: what the
    // job then means -- how far it may reach and who may ask for it -- is
    // `transfer`, which is where a script's commands go through too
    let at = |given: &str| match given.trim() {
        "" => match remote_root.trim() {
            "" => ".".to_string(),
            r => r.to_string(),
        },
        g => g.to_string(),
    };
    let job: ssh::FileJob = match act {
        "remote" => ssh::FileJob::List { path: at(&str_of("at")) },
        "mkdir" => ssh::FileJob::MakeDir { path: str_of("path") },
        "rename" => ssh::FileJob::Rename { from: str_of("from"), to: str_of("to") },
        "rm" => ssh::FileJob::Remove { path: str_of("path") },
        "put" => ssh::FileJob::Put {
            from: std::path::PathBuf::from(str_of("from")),
            to: str_of("to"),
            overwrite: args.get("overwrite").and_then(|v| v.as_bool()).unwrap_or(false),
        },
        "get" => ssh::FileJob::Get {
            from: str_of("from"),
            to: std::path::PathBuf::from(str_of("to")),
            overwrite: args.get("overwrite").and_then(|v| v.as_bool()).unwrap_or(false),
        },
        // Reaching the far end at all, to say so before anything is saved
        "test" => ssh::FileJob::List { path: at("") },
        // Not a job of its own: the far side is read the way any read is read,
        // and what comes back is put beside the copy on this machine instead
        // of being written down
        "diff" => ssh::FileJob::Read { path: str_of("there") },
        _ => return None,
    };
    // Read here rather than in the thread: it is this machine's own disk, and
    // a file that is missing or too big should say so before a connection is
    // spent on the other half of the comparison
    let mut here: Option<(String, crate::charset::Reading, Vec<u8>)> = None;
    let chosen = match str_of("encoding").trim() {
        "" => None,
        name => match crate::charset::named(name) {
            Some(e) => Some(e),
            None => return fail(i18n::tp("err.git.unknown_encoding", &[("enc", name)])),
        },
    };
    if act == "diff" {
        let Some(root) = local_root.clone() else {
            return fail(i18n::t("err.sftp.no_folder"));
        };
        let Some(path) = local_under(&root, &str_of("here")) else {
            return fail(i18n::t("err.sftp.outside"));
        };
        let file = str_of("name");
        match std::fs::read(&path) {
            Err(e) => return fail(format!("{e}")),
            Ok(bytes) => match diff_text(&file, &bytes, chosen) {
                Err(why) => return fail(why),
                Ok(text) => here = Some((file, text, bytes)),
            },
        }
    }
    // Inside the fences and allowed, settled here where the answer can still be
    // handed straight back. What crosses to the thread is the settled job, so
    // nothing reads a path a second time between the checking and the doing
    let fences = crate::transfer::Fences {
        here: local_root.clone(),
        there: remote_root.clone(),
    };
    let job = match crate::transfer::ready(job, &fences, caps, grants::Subject::Human) {
        Ok(job) => job,
        Err(e) => return fail(format!("{e}")),
    };
    // The folder that was asked about, sent back with the answer: by the time
    // it arrives the person may have moved on, and a listing that lands in the
    // wrong folder is worse than one that never lands
    let asked = match &job {
        ssh::FileJob::List { path } => path.clone(),
        _ => String::new(),
    };
    let (act, panel) = (act.to_string(), panel.to_string());
    // Sent back as it was asked, so an answer for an encoding somebody has
    // since changed away from is known for what it is
    let asked_encoding = str_of("encoding");
    let tx = tx.clone();
    std::thread::spawn(move || {
        let said = crate::elsewhere::files(&machine, job, SFTP_WAIT_MS);
        let payload = match said {
            Ok(ssh::FileAnswer::Listing(rows)) => serde_json::json!({
                "act": act,
                "panel": panel,
                "ok": true,
                "at": asked,
                "rows": rows,
            }),
            // The far side, put beside the one here. `-` is the server's and
            // `+` is this machine's, whichever way the person was going to
            // move the file -- one reading, so the signs never swap meaning
            Ok(ssh::FileAnswer::Bytes(bytes)) => match here {
                None => serde_json::json!({"act": act, "panel": panel, "ok": true}),
                Some((file, mine, mine_bytes)) => match diff_text(&file, &bytes, chosen) {
                    Err(why) => serde_json::json!({"act": act, "panel": panel, "ok": false,
                        "name": file, "asked": asked_encoding, "error": why}),
                    Ok(theirs) => serde_json::json!({
                        "act": act,
                        "panel": panel,
                        "ok": true,
                        "name": file,
                        "asked": asked_encoding,
                        "encoding": diff_encodings((&theirs, &bytes), (&mine, &mine_bytes)),
                        "text": crate::diff::unified(
                            &theirs.text, &mine.text, &file, crate::diff::CONTEXT),
                    }),
                },
            },
            Ok(_) => serde_json::json!({"act": act, "panel": panel, "ok": true}),
            Err(e) => {
                serde_json::json!({"act": act, "panel": panel, "ok": false, "error": format!("{e:#}")})
            }
        };
        let _ = tx.send(payload.to_string());
    });
    None
}
/// What is in a folder on this machine, in the same shape the far end answers
/// in -- folders first and then by name, so the two lists read alike
pub fn local_rows(at: &std::path::Path) -> Result<Vec<ssh::Entry>> {
    let mut rows: Vec<ssh::Entry> = Vec::new();
    for e in std::fs::read_dir(at)? {
        let Ok(e) = e else { continue };
        let Ok(m) = e.metadata() else { continue };
        rows.push(ssh::Entry {
            name: e.file_name().to_string_lossy().to_string(),
            dir: m.is_dir(),
            size: m.len(),
            modified: m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0),
        });
    }
    rows.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
    Ok(rows)
}
/// The same path, refused if it is not inside the folder this panel works in.
///
/// The folder is the fence. A panel opened on one project cannot be walked up
/// into another, and `..` is not a way around it -- which is the promise the
/// far side already keeps, said once more for this machine
pub fn local_under(root: &std::path::Path, at: &str) -> Option<std::path::PathBuf> {
    let want = std::path::PathBuf::from(at.replace('\\', "/"));
    let want = if want.is_absolute() { want } else { root.join(want) };
    // Worked out without touching the disk, so that a folder that is not there
    // is a "not found" from the listing rather than a refusal from here
    let mut out = std::path::PathBuf::new();
    for part in want.components() {
        match part {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    let root = root.components().fold(std::path::PathBuf::new(), |mut acc, c| {
        acc.push(c.as_os_str());
        acc
    });
    out.starts_with(&root).then_some(out)
}
/// A path as a person reads it: one kind of slash, whatever the disk uses
pub fn display_path_of(p: &std::path::Path) -> String {
    p.display().to_string().replace('\\', "/")
}
/// What is wrong with a working folder, in one line.
///
/// Asked of the same table the sidebar reads, so the dialog and the row above
/// it cannot say two different things about one folder.
pub fn trouble_of(at: &std::path::Path) -> String {
    match folders::watch().settled(at, folders::BEFORE_LAUNCH) {
        folders::Health::NoDrive { drive } => i18n::tp(
            "msg.folder.no_drive",
            &[("drive", &drive), ("path", &at.display().to_string())],
        ),
        folders::Health::Missing => {
            i18n::tp("msg.folder.missing", &[("path", &at.display().to_string())])
        }
        _ => String::new(),
    }
}
/// Why the folder cannot simply be put back, in the person's language.
pub fn blocked_said(why: &folders::Blocked) -> String {
    use folders::Blocked;
    match why {
        Blocked::OtherProject { at, found, wanted } => i18n::tp(
            "msg.folder.other_project",
            &[("path", at), ("found", found), ("wanted", wanted)],
        ),
        Blocked::NotEmpty { at, holds } => i18n::tp(
            "msg.folder.not_empty",
            &[("path", at), ("holds", &holds.join(", "))],
        ),
        Blocked::BranchTaken { branch, at } => {
            i18n::tp("msg.folder.branch_taken", &[("branch", branch), ("path", at)])
        }
        Blocked::Unknown => i18n::t("msg.folder.unknown"),
        Blocked::NoDrive { drive } => {
            i18n::tp("msg.folder.no_drive_short", &[("drive", drive)])
        }
    }
}
/// The projects already on this machine, for the one question that has to be
/// asked.
///
/// Taken from the folders that are open, because those are the projects this
/// person actually works on -- and each is named by its remote, which is the
/// same string on every machine and therefore the thing worth writing down.
pub fn projects_here(desk: Option<&config::Desk>) -> Vec<crate::uistate::Project> {
    let mut out: Vec<crate::uistate::Project> = Vec::new();
    for f in desk.map(|w| w.folders.as_slice()).unwrap_or_default() {
        let Some(cwd) = f.cwd.as_deref() else { continue };
        let Some(url) = crate::repo::remote_url_of(cwd) else { continue };
        let origin = folders::scrub(&url);
        if out.iter().any(|p| p.origin == origin) {
            continue;
        }
        let at = crate::repo::main_checkout(cwd).unwrap_or_else(|| cwd.to_path_buf());
        out.push(crate::uistate::Project {
            name: at
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| origin.clone()),
            at: at.display().to_string(),
            origin,
        });
    }
    out
}
/// What a viewer from afar needs to lay the board out in panes.
///
/// The window is handed the division of the content area and a picture of
/// every pane that is not in front, by being called directly (see the
/// window's `draw`). A browser on a laptop was handed neither, so it drew one
/// terminal where the window drew four, and a split made from it split a
/// screen it could not see. This keeps what was last said, so only what
/// changed goes out, and can say all of it again to a viewer who just arrived.
#[derive(Default)]
pub struct PaneRelay {
    layout: String,
    /// Per pane: what its picture was built from, and the picture
    screens: std::collections::HashMap<crate::layout::PaneId, (PictureKey, String)>,
}

/// What a pane's picture was built from: the session, how much it had
/// written, its size and how far it was scrolled back
type PictureKey = (usize, u64, u16, u16, usize);

impl PaneRelay {
    /// The messages that bring a viewer up to date with how things are now.
    /// The read-only copies of the panes nobody is looking at.
    ///
    /// The division itself is not here: it travels with the state, because it
    /// is part of the same moment (`view::PanesState`). What this still does
    /// with it is notice when it changed, so the copies of panes that are gone
    /// -- or that have just taken the focus -- stop being remembered as sent
    pub fn changes(&mut self, layout: &crate::layout::Layout, surfaces: &[Surface], tabs: &[Tab]) -> Vec<String> {
        let mut out = Vec::new();
        let lay = crate::view::panes_json(layout);
        if lay != self.layout {
            let live: std::collections::HashSet<_> = layout.leaves().into_iter().map(|(id, _)| id).collect();
            self.screens.retain(|id, _| live.contains(id));
            // The pane in front is drawn by the full renderer, so its copy is
            // emptied on the page; forget it, or it would not be sent again
            // once focus moves on
            self.screens.remove(&layout.focus());
            self.layout = lay;
        }
        for (id, surface) in layout.leaves() {
            if id == layout.focus() {
                continue;
            }
            let Some((i, t)) = session_at(surfaces, surface).and_then(|i| tabs.get(i).map(|t| (i, t))) else {
                continue;
            };
            let (key, html) = {
                let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                let s = p.screen();
                let (rows, cols) = s.size();
                let key = (i, t.output_count(), rows, cols, s.scrollback());
                if self.screens.get(&id).is_some_and(|(k, _)| *k == key) {
                    continue;
                }
                (key, crate::shell::screen_html(s))
            };
            if self.screens.get(&id).map(|(_, h)| h.as_str()) != Some(html.as_str()) {
                out.push(pane_screen_message(id, &html));
            }
            self.screens.insert(id, (key, html));
        }
        out
    }

    /// Everything, for a viewer who has only just arrived
    /// What a viewer that has just arrived is told: a picture of every pane
    /// it is not looking at.
    ///
    /// Not the division. That arrives with the state, which a viewer is always
    /// sent on its first frame -- told here as well it would be two messages
    /// for one moment, which is the thing `view::PanesState` exists to end
    pub fn seed(&self) -> Vec<String> {
        self.screens.iter().map(|(id, (_, html))| pane_screen_message(*id, html)).collect()
    }
}

fn pane_screen_message(id: crate::layout::PaneId, html: &str) -> String {
    serde_json::json!({ "panescreen": { "id": id, "html": html } }).to_string()
}

/// Looks up a session's location from its screen number (1-based)
/// The surface a screen number stands for. Numbers are 1-based; 0 is no
/// surface at all -- a pane with nothing in it yet
pub fn ui_surface_at(surfaces: &[Surface], n: usize) -> Option<&Surface> {
    surfaces.get(n.checked_sub(1)?)
}
/// The row the settings just made, for the pane that asked for it: the one
/// that was not on the list when it asked.
///
/// `was` is the list as it stood then and `now` is the list as it stands,
/// each row carrying its number and what it is (`view::surface_key`).
///
/// Told apart by number instead, the arrival can only be guessed at, and the
/// guess this replaces -- "the last row nobody is looking at" -- was wrong
/// whenever the new tab was not last. A tab is written into its own folder's
/// list, so one added to any folder but the final one lands in the middle of
/// the rows: the pane was handed an unrelated tab in an unrelated folder, and
/// a folder somebody had put out of sight came straight back with it, because
/// something had brought one of its tabs to the front
pub fn arrived_row(was: &[String], now: &[(usize, String)]) -> Option<usize> {
    // Only once there is more one more row than there was. A row renamed while
    // the form was open is a key that was not there either, and nothing has
    // arrived for the pane to be given
    (now.len() > was.len())
        .then(|| now.iter().find(|(_, k)| !was.contains(k)).map(|(n, _)| *n))
        .flatten()
}
/// The name of the split row at this number, when that is what is there.
///
/// A split row shows other rows, so it is the one row whose number does not
/// say what is on screen. This is how the loop tells the two apart
pub fn split_at(surfaces: &[Surface], at: usize) -> Option<String> {
    match surfaces.get(at.checked_sub(1)?)? {
        Surface::Split { key, .. } => Some(key.clone()),
        _ => None,
    }
}
pub fn session_at(surfaces: &[Surface], active: usize) -> Option<usize> {
    match surfaces.get(active.checked_sub(1)?)? {
        Surface::Session(i) => Some(*i),
        Surface::Browser { .. }
        | Surface::Git { .. }
        | Surface::Sftp { .. }
        | Surface::Editor { .. }
        | Surface::Failed { .. }
        | Surface::Split { .. }
        | Surface::Issues { .. } => None,
    }
}
/// What size each tab's terminal should be drawn at.
///
/// The pane a tab sits in decides it; a tab in no pane keeps the whole content
/// area (`front`), so it is already the right shape the moment it appears.
///
/// The pane in front is the exception, and deliberately so: it keeps `front`,
/// which is the size last reported by *whoever is looking at it*. The window
/// reports that pane's own rectangle there, so at the window nothing changes.
/// A phone reports the one screen it has — it is never sent the division, a
/// small screen having no room to be divided — and that is the same number.
/// Reading the window's measurement for the front pane instead handed the tab
/// being watched the window's shape: too wide for a phone, so half of it hung
/// off the right with no way to reach it, and short of its foot, leaving a dead
/// band underneath. The panes behind it are only ever seen at the window, so
/// they keep the window's own measurement.
pub fn tab_sizes(
    tabs: usize,
    layout: &crate::layout::Layout,
    surfaces: &[Surface],
    geom: &[shikisha_shared::PaneGeom],
    front: (u16, u16),
) -> Vec<(u16, u16)> {
    let mut want = vec![front; tabs];
    let focus = layout.focus();
    for (id, sf) in layout.leaves() {
        if id == focus {
            continue;
        }
        let (Some(i), Some(g)) = (session_at(surfaces, sf), geom.iter().find(|g| g.id == id))
        else {
            continue;
        };
        if let Some(w) = want.get_mut(i) {
            *w = (g.rows, g.cols);
        }
    }
    want
}
/// Looks up the screen number (1-based) from a session's location.
/// The ball moves by session number, so route it through here when displaying it.
pub fn surface_at(surfaces: &[Surface], session: usize) -> usize {
    surfaces
        .iter()
        .position(|p| *p == Surface::Session(session.wrapping_sub(1)))
        .map(|i| i + 1)
        .unwrap_or(0)
}
/// The session currently being viewed. None if viewing a browser.
pub fn session_mut<'a>(tabs: &'a mut [Tab], surfaces: &[Surface], active: usize) -> Option<&'a mut Tab> {
    let i = session_at(surfaces, active)?;
    tabs.get_mut(i)
}
/// Trims the screen text sent to the phone.
/// Trailing blank lines from the terminal would otherwise hide the content, so
/// those are dropped from the end; line count is also capped to save bandwidth.
pub fn trim_for_phone(s: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    let start = end.saturating_sub(max_lines);
    lines[start..end]
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}
/// Starts the remote UI according to config (None if disabled)
/// Start the remote server WITHOUT making the caller wait for the bind (a
/// lingering earlier instance can hold the port for up to a second, and the
/// caller is the loop that answers every click). Returns None when remote is
/// disabled; otherwise a channel that delivers (the server if it came up,
/// error/note lines for the flash) once the bind settles.
/// The settings a board has to be served under for this machine's own window
/// to reach it, when the program is split in two and remote access is off.
///
/// `None` where nothing has to change: the program is not split, or remote
/// access is on and there is already a board listening for the window to use.
///
/// The loopback and nowhere else, on the port this installation always uses
/// (`netaddr::board_port`), so the window's own storage survives a restart.
/// This is not remote access turned on quietly -- nothing on the network can
/// reach 127.0.0.1, the listener says so of itself (`local_only`), and the
/// settings screen goes on reporting remote access as off, because it is
fn board_for_the_window(c: &config::Config) -> bool {
    c.split.unwrap_or(false) && !c.remote.enabled
}

pub fn start_remote_bg(
    cfg: Option<&config::Config>,
    password: Option<&str>,
) -> Option<std::sync::mpsc::Receiver<(Option<remote::RemoteUi>, Vec<String>)>> {
    let c = cfg?;
    // A board this machine's own window needs, where the settings alone would
    // have served none
    let local_only = board_for_the_window(c);
    if !c.remote.enabled && !local_only {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    // Resolving the address and token is local and quick — done here, so the
    // thread owns only the part that can actually stall (the bind itself).
    // A fixed token that is too short to be a secret must never quietly
    // become "the usual token instead": the person believes the string they
    // wrote is the key. Refuse to start and say why (status + settings note)
    if !local_only && c.remote.sticky_token && c.remote.fixed_token.trim().len() < FIXED_TOKEN_MIN {
        let _ = tx.send((None, vec![i18n::tp("err.remote.fixed_short", &[("n", &FIXED_TOKEN_MIN.to_string())])]));
        return Some(rx);
    }
    // A board for the window of a split program answers on the loopback and
    // nowhere else, on the port this installation always uses. What the
    // settings say about where remote access should bind is about remote
    // access, and none of this is that
    let where_ = match local_only {
        true => Ok((std::net::Ipv4Addr::LOCALHOST, None)),
        false => netaddr::resolve_bind(&c.remote.bind, c.remote.allow_public),
    };
    match where_ {
        Ok((ip, note)) => {
            let token = remote_token(c, password);
            let port = match local_only {
                true => netaddr::board_port(&config::root_dir()),
                false => c.remote.port,
            };
            let remote_password = c.remote.password.clone();
            let sticky = c.remote.sticky_token;
            std::thread::spawn(move || {
                let mut errors = Vec::new();
                let ui = match remote::RemoteUi::start_with(ip, port, token, remote_password, sticky) {
                    Ok(mut r) => {
                        r.local_only = local_only;
                        if let Some(n) = &note {
                            errors.push(n.clone());
                        }
                        r.note = note;
                        // Asked here rather than at the bind, because it is the
                        // slow part and this thread is the one that exists for
                        // slow parts. It ends in a real request through the
                        // address before any link is built from it.
                        if let Some(front) = tailscale::front(r.port()) {
                            r.reached_at(front);
                        }
                        Some(r)
                    }
                    Err(e) => {
                        errors.push(crate::i18n::tp(
                            "err.desk.remote_ui",
                            &[("e", &e.to_string())],
                        ));
                        None
                    }
                };
                let _ = tx.send((ui, errors));
            });
        }
        Err(e) => {
            let _ = tx.send((
                None,
                vec![crate::i18n::tp("err.desk.remote_ui", &[("e", &e.to_string())])],
            ));
        }
    }
    Some(rx)
}
/// Passes the current listening status along so the settings screen can show the QR code
pub fn publish_remote(info: &Arc<Mutex<webui::RemoteInfo>>, ui: &Option<remote::RemoteUi>) {
    let mut i = info.lock().unwrap();
    match ui {
        // A board put up for this machine's own window is not remote access
        // and is never shown as it: the phone card would offer a code that
        // scans and reaches nothing
        Some(r) if r.local_only => *i = Default::default(),
        Some(r) => {
            i.running = true;
            i.url = r.url.clone();
            i.note = r.note.clone().unwrap_or_default();
            // Handed over so that revoking a device from the settings page
            // also ends what that device is looking at
            let live = r.sessions();
            i.cut = Some(Arc::new(move |id: &str| live.drop_client(id)));
        }
        None => *i = Default::default(),
    }
}
/// The root of the portable layout (base for relative paths; where the exe and its folders sit side by side)
pub fn config_file_dir() -> std::path::PathBuf {
    config::root_dir()
}
/// Opens the settings screen inside our own window. Only launched once; from
/// the second time on, it just returns to the same location.
/// `query` is extra instruction appended to the URL (e.g. "&addtab=0"; empty by default)
/// Put the guide's panel on screen.
///
/// The same page the settings come from, so it is reached the same way and
/// shares their token. Where it stands is the loop's business; this only makes
/// it exist.
pub fn open_guide(
    web: &mut Option<webui::WebUi>,
    config_file: &std::path::Path,
    remote_info: &Arc<Mutex<webui::RemoteInfo>>,
    web_password: &Arc<Mutex<Option<String>>>,
    caps: &hooks::Caps,
) -> Result<()> {
    let url = ensure_web_url(web, config_file, remote_info, web_password, caps)?;
    // `<host>/?token=x` is the settings; the panel is `<host>/guide?token=x`
    let at = url.replace("/?token=", "/guide?token=");
    caps.browser_open(GUIDE_TAB, &at, shikisha_shared::BrowserProfile::shared_default())?;
    crate::guide::set_up(true);
    Ok(())
}

/// Take it away again, and let go of whatever box it was writing in -- that
/// was picked for it, and there is nothing to write in it now
fn shut_guide(caps: &hooks::Caps) {
    let _ = caps.browser_close(GUIDE_TAB);
    crate::guide::set_up(false);
}

pub fn open_settings(
    web: &mut Option<webui::WebUi>,
    config_file: &std::path::Path,
    remote_info: &Arc<Mutex<webui::RemoteInfo>>,
    web_password: &Arc<Mutex<Option<String>>>,
    caps: &hooks::Caps,
    query: &str,
) -> Result<()> {
    let url = ensure_web_url(web, config_file, remote_info, web_password, caps)?;
    // The settings screen is a local UI page. It holds no cookies, so the shared default profile is plenty.
    caps.browser_open(
        SETTINGS_TAB,
        &format!("{url}{query}"),
        shikisha_shared::BrowserProfile::shared_default(),
    )
}
/// A folder made for an issue or a pull request: write down which, and hold
/// its address for the input bar of the AI that starts there -- put there, not
/// sent, so the person reads it before anything happens
fn remember_work_item(
    desk: &str,
    folder: &std::path::Path,
    link: &serde_json::Value,
    drafts: &mut Vec<(std::path::PathBuf, String, Instant)>,
) {
    let s = |k: &str| link.get(k).and_then(|x| x.as_str()).unwrap_or_default().to_string();
    let number = link.get("number").and_then(|n| n.as_u64()).unwrap_or(0);
    let (kind, repo, url) = (s("kind"), s("repo"), s("url"));
    if number == 0 || !matches!(kind.as_str(), "issue" | "pr") || repo.is_empty() {
        return;
    }
    if let Err(e) = config::set_folder_work_item(desk, folder, &format!("{kind}:{repo}#{number}")) {
        append_hook_log(&format!("could not note what {} was made for: {e:#}", folder.display()));
    }
    if url.starts_with("https://github.com/") {
        drafts.retain(|(f, _, _)| !crate::uistate::same_folder(f, folder));
        drafts.push((folder.to_path_buf(), url, Instant::now()));
    }
}

/// What making a branch's folder said about what came along: that it is ready,
/// and -- named, never dropped -- what could not come, what was copied where a
/// link was asked for, and what could not be replaced
fn brought_note(branch: &str, b: &crate::worktree::Brought) -> String {
    let mut said = match b.missed.is_empty() {
        true => i18n::tp("msg.branch.made", &[("name", branch)]),
        false => i18n::tp("msg.branch.made_partly", &[("name", branch), ("missed", &b.missed.join(", "))]),
    };
    if !b.copied.is_empty() {
        said.push(' ');
        said.push_str(&i18n::tp("msg.branch.link_copied", &[("names", &b.copied.join(", "))]));
    }
    if !b.unreplaced.is_empty() {
        said.push(' ');
        said.push_str(&i18n::tp("msg.branch.unreplaced", &[("names", &b.unreplaced.join(", "))]));
    }
    said
}

/// A folder path, safe to carry in a query string.
///
/// Only what a Windows path can hold has to survive: separators, spaces, and
/// whatever a person named a folder. Anything outside the unreserved set is
/// written as its bytes, which is what the other side decodes
pub fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
/// The tab's working folder as an absolute path string, for attachments. Falls
/// back to what the shell in it says about itself when the tab was given none,
/// and to nothing at all when it says nothing.
pub fn tab_cwd_abs(t: &Tab) -> String {
    // Where somebody put this tab is where it belongs, and a `cd` typed inside
    // it does not move it. A tab with no folder of its own is a terminal on
    // another machine or a conversation with a model, and what the shell in it
    // says about itself is the only true answer there is. Failing that there
    // is none: the folder this program happens to be running from was the old
    // answer, and it is a place a file dropped from a phone had no business
    // landing in
    let reported = || {
        let r = t.reported_cwd();
        (!r.is_empty()).then(|| std::path::PathBuf::from(r))
    };
    let abs = match t.cwd().map(std::path::Path::to_path_buf) {
        Some(p) if p.is_absolute() => Some(p),
        Some(p) => std::env::current_dir().ok().map(|c| c.join(p)),
        None => reported(),
    };
    abs.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}
/// Ensure the local settings/result web server is running and hand back its
/// base URL (`http://127.0.0.1:<port>/?token=<token>`). Started lazily on first
/// use and kept for the process lifetime.
pub fn ensure_web_url(
    web: &mut Option<webui::WebUi>,
    config_file: &std::path::Path,
    remote_info: &Arc<Mutex<webui::RemoteInfo>>,
    web_password: &Arc<Mutex<Option<String>>>,
    caps: &hooks::Caps,
) -> Result<String> {
    match web.as_ref() {
        Some(w) => Ok(w.url.clone()),
        None => {
            let w = webui::WebUi::start_with(
                config_file.to_path_buf(),
                Arc::clone(remote_info),
                Arc::clone(web_password),
            )?;
            let u = w.url.clone();
            // The pages this server serves (settings, the result view) are the
            // app's own and must be heard in full by the window -- it judges a
            // page by the address it speaks from, and this one is only known now
            caps.trust_origin(&u);
            *web = Some(w);
            Ok(u)
        }
    }
}
/// Open (or re-point) the result view for a finished run, rendered as a chat.
///
/// Served by the same local web server as settings, at `/result?...&run=<id>`.
/// Reuses the single RESULT_TAB page so repeated results re-navigate in place
/// rather than stacking up tabs. Shares the default profile (no cookies needed).
pub fn open_result(
    web: &mut Option<webui::WebUi>,
    config_file: &std::path::Path,
    remote_info: &Arc<Mutex<webui::RemoteInfo>>,
    web_password: &Arc<Mutex<Option<String>>>,
    caps: &hooks::Caps,
    run_id: &str,
) -> Result<()> {
    let base = ensure_web_url(web, config_file, remote_info, web_password, caps)?;
    // base is ".../?token=<t>"; move to the /result page and carry the run id.
    let url = format!(
        "{}&run={}",
        base.replacen("/?token=", "/result?token=", 1),
        run_id
    );
    caps.browser_open(
        RESULT_TAB,
        &url,
        shikisha_shared::BrowserProfile::shared_default(),
    )
}
/// Builds a placed page's context from the screen layout.
/// Returns None for a page that's not in the layout (e.g. after it's closed).
pub fn page_ctx(
    surfaces: &[Surface],
    key: &str,
    url: String,
    complete: bool,
) -> Option<hooks::PageCtx> {
    surfaces.iter().enumerate().find_map(|(i, p)| match p {
        Surface::Browser { key: k, name, .. } if k == key => Some(hooks::PageCtx {
            index: i + 1,
            id: k.clone(),
            name: name.clone(),
            url: url.clone(),
            complete,
        }),
        _ => None,
    })
}
/// Which tabs automation should be told about again, re-arming each one it
/// names.
///
/// Kept out of the loop so the rule itself can be checked. The rule: only tabs
/// automation was already told about (they are the ones in `tracked`), only
/// while they are still working, and not before their time.
pub fn busy_repeat_due(
    now_ms: u64,
    every: u64,
    states: &[TabState],
    tracked: &mut std::collections::HashMap<usize, u64>,
) -> Vec<usize> {
    tracked.retain(|&idx, _| states.get(idx - 1).is_some_and(|s| *s == TabState::Busy));
    let mut due: Vec<usize> = tracked
        .iter()
        .filter(|(_, at)| now_ms >= **at)
        .map(|(&idx, _)| idx)
        .collect();
    due.sort_unstable();
    for idx in &due {
        tracked.insert(*idx, now_ms + every);
    }
    due
}
pub fn tab_ctx(t: &Tab, index: usize) -> TabCtx {
    TabCtx {
        index,
        name: t.title.clone(),
        id: t.id.clone(),
        state: t.state.label().to_string(),
        profile: t.profile_name().to_string(),
        output: t.last_response.clone().unwrap_or_default(),
        chain_depth: t.chain_depth,
        locked: t.locked,
        is_model: t.is_model(),
        // A rally brain's exact reply, kept verbatim so the orchestrator can
        // pull ```lua out of it without the terminal's line-wrapping mangling
        // long URLs. None for CLI tabs and plain chat.
        reply: t.model_reply(),
    }
}
/// A minimal context for a browser pane, so a quick action's Lua can run while a
/// browser tab is active. `tab.name` is the browser's key, ready to hand to the
/// browser_* functions (e.g. `shikisha.browser_go(tab.name, "to", url)`).
/// A file panel, as the thing a template runs against. Named the way a page is
/// named -- by its key, which is what `sftp_*` is told -- so the template does
/// not have to be handed the tab twice
/// Progress reports meant for a file panel, sent there and taken out of the
/// list. Everything else goes on to `exec_commands` untouched.
///
/// A template says how far it has got with `set_progress`, the same command a
/// hook uses for a tab. Which of the two it lands on is decided by what it was
/// aimed at, not by two different commands
fn panel_progress_out(
    cmds: Vec<crate::hooks::Command>,
    surfaces: &[Surface],
    tx: &std::sync::mpsc::Sender<String>,
) -> Vec<crate::hooks::Command> {
    cmds.into_iter()
        .filter(|cmd| {
            let crate::hooks::Command::SetProgress { value, label, target, .. } = cmd else {
                return true;
            };
            let Some(hooks::TabRef::Name(name)) = target else {
                return true;
            };
            if !surfaces
                .iter()
                .any(|s| matches!(s, Surface::Sftp { key, .. } if key == name))
            {
                return true;
            }
            let _ = tx.send(
                serde_json::json!({
                    "act": "progress",
                    "panel": name,
                    "ok": true,
                    "value": value,
                    "label": label,
                })
                .to_string(),
            );
            false
        })
        .collect()
}

pub fn panel_ctx(index: usize, key: &str) -> TabCtx {
    TabCtx {
        index,
        name: key.to_string(),
        id: Some(key.to_string()),
        state: "FILES".into(),
        profile: String::new(),
        output: String::new(),
        chain_depth: 0,
        locked: false,
        is_model: false,
        reply: None,
    }
}
pub fn browser_ctx(index: usize, key: &str) -> TabCtx {
    TabCtx {
        index,
        // A page is addressed by its key, which IS its id — the same string the
        // browser_* calls take
        name: key.to_string(),
        id: Some(key.to_string()),
        state: "WEB".into(),
        profile: String::new(),
        output: String::new(),
        chain_depth: 0,
        locked: false,
        is_model: false,
        reply: None,
    }
}
/// Grace period holding off auto-submit right after manual input (avoids keystroke cross-talk)
pub const MANUAL_GUARD_MS: u64 = 5000;
/// A person hands one named tab a line: the composer's Send, the discussion's
/// topic box, and the phone's own send all end here.
///
/// The tab is named rather than taken to be "the one in front". Those are the
/// same tab most of the time, which is exactly why the difference went unnoticed
/// -- until the topic box, which switches the view and hands over a line in the
/// same breath and cannot rely on the two arriving in that order.
///
/// How the line is delivered is the tab's business, not the caller's: a model
/// bridge has no prompt to type at and is told directly, anything else is typed
/// and submitted the way a person at its keyboard would. Deciding that out at
/// the edges meant every edge had to know, and the phone's edge did not.
pub fn hand_line(
    tabs: &mut [Tab],
    surfaces: &[Surface],
    target: usize,
    text: String,
    now_ms: u64,
    pending_send: &mut Vec<PendingSend>,
    ball: &mut ball::Ball,
) -> bool {
    // Every road here is a person's words, which is what a folder's automatic
    // name is written from (`crate::labels`)
    let heard = text.clone();
    let landed = hand_over(tabs, surfaces, target, text, true, now_ms, pending_send, ball);
    if landed && let Some(t) = session_at(surfaces, target).and_then(|i| tabs.get_mut(i)) {
        t.heard(&heard);
    }
    landed
}
/// The same, with a choice about the Enter at the end: `submit` false leaves
/// the text at the prompt for the person to finish (a quick command whose
/// "press Enter" is off). A model bridge has no prompt to leave anything at,
/// so it is always told
#[allow(clippy::too_many_arguments)]
pub fn hand_over(
    tabs: &mut [Tab],
    surfaces: &[Surface],
    target: usize,
    text: String,
    submit: bool,
    now_ms: u64,
    pending_send: &mut Vec<PendingSend>,
    ball: &mut ball::Ball,
) -> bool {
    let Some(t) = session_at(surfaces, target).and_then(|i| tabs.get_mut(i)) else {
        return false;
    };
    if t.locked {
        return false;
    }
    // Manual input breaks the chain -- except into a tab that was handed a
    // draft to finish, which is joining in rather than taking over.
    if ball.awaiting_human && ball.holder == target {
        ball.awaiting_human = false;
    } else {
        t.chain_depth = 0;
    }
    t.last_manual_ms = Some(now_ms);
    if t.is_model() {
        t.chat_send(text);
    } else {
        to_live(t);
        let seen = t.output_count();
        let chunks = paste_chunks(t, &text);
        pending_send.push(PendingSend::new(target, chunks, submit, seen, now_ms, text.chars().count()));
    }
    true
}

/// Where a quick command goes.
#[derive(Debug, Clone, PartialEq)]
pub enum QuickGo {
    /// Into a new tab, started in this folder with this command
    Open { cwd: std::path::PathBuf, command: String, program: String },
    /// Nowhere, and the dictionary key saying why
    Refuse(&'static str),
}

/// Where a quick command of this kind would go right now.
///
/// Always into a tab of its own, opened for it in the working folder in front
/// -- the folder of the tab being looked at -- because that is where a
/// relative path in a command means something and where an AI asked to do
/// something is meant to do it. A shell for a command; for a prompt, the AI
/// the button names, or the first one this PC can start. Never into a tab
/// that is already open: what is running there, or half typed there, is the
/// person's, and a button is not a reason to type over it.
///
/// With no folder in front (the board, a page, a tab that works nowhere in
/// particular), a command opens a shell in the home folder -- which is also
/// how a button starts a program of this PC's -- and a prompt goes nowhere:
/// an AI set to work on the whole of somebody's home folder is not a thing
/// to do by accident.
///
/// The same function answers the launcher's "where would this go" and the
/// press itself, so the two cannot disagree.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
pub fn quick_go(
    kind: crate::quick::Kind,
    ai: &str,
    surfaces: &[Surface],
    tabs: &[Tab],
    active: usize,
    covered: bool,
    ais: &[crate::uistate::AiChoice],
    home: Option<&std::path::Path>,
    desk: Option<&config::Desk>,
) -> QuickGo {
    use crate::quick::Kind;
    if kind == Kind::Folder {
        return QuickGo::Refuse("msg.quick.gone");
    }
    let folder = (!covered).then(|| surface_folder(surfaces, tabs, active)).flatten();
    match kind {
        Kind::Terminal => match folder.or(home) {
            // A folder on another machine opens that machine's shell: its
            // tab with nothing written runs the shell there
            Some(cwd) if far_folder(desk, cwd).is_some() => QuickGo::Open {
                cwd: cwd.to_path_buf(),
                command: String::new(),
                program: far_folder(desk, cwd).unwrap_or_default(),
            },
            Some(cwd) => QuickGo::Open {
                cwd: cwd.to_path_buf(),
                command: "powershell.exe".into(),
                program: "PowerShell".into(),
            },
            None => QuickGo::Refuse("msg.quick.no_home"),
        },
        _ => {
            let Some(f) = folder else { return QuickGo::Refuse("msg.quick.ai_needs_folder") };
            // The AI's own command, without the flag that lets it act without
            // asking: starting one from a button is not the person choosing
            // that, which is a box they tick themselves in the settings
            // On a MicroVM, the AI its machine was given
            if let Some(given) = machine_ai_of(desk, f) {
                return match given {
                    Ok(a) => QuickGo::Open { cwd: f.to_path_buf(), command: a.key, program: a.name },
                    Err(why) => QuickGo::Refuse(why),
                };
            }
            match quick_ai_choice(ai, ais) {
                Some(a) => QuickGo::Open { cwd: f.to_path_buf(), command: a.key.clone(), program: a.name.clone() },
                None => QuickGo::Refuse("msg.quick.no_ai"),
            }
        }
    }
}

/// The name of the machine a desk folder is on, when it is not this one
fn far_folder(desk: Option<&config::Desk>, dir: &std::path::Path) -> Option<String> {
    desk?.folders
        .iter()
        .find(|f| f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, dir)))?
        .host
        .as_ref()
        .map(|h| h.name.clone())
}

/// The same answer, in the words the launcher shows
pub fn quick_dest(go: &QuickGo) -> crate::quick::QuickDest {
    match go {
        QuickGo::Open { cwd, program, .. } => crate::quick::QuickDest {
            how: "open",
            name: i18n::tp(
                "msg.quick.dest.open",
                &[
                    ("folder", &cwd.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| cwd.display().to_string())),
                    ("program", program),
                ],
            ),
        },
        QuickGo::Refuse(why) => crate::quick::QuickDest { how: "none", name: i18n::t(why) },
    }
}

/// The folder a command with nowhere else to be opens in
pub fn home_folder() -> Option<std::path::PathBuf> {
    ["USERPROFILE", "HOME"]
        .iter()
        .find_map(std::env::var_os)
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
}

/// A name for a tab a quick command opens: the button's, made one of a kind
/// in the desk, with an automation name to match
pub fn quick_tab_names(label: &str, fallback: &str, tabs: &[Tab]) -> (String, String) {
    let base = if label.trim().is_empty() { fallback.to_string() } else { label.trim().to_string() };
    let title = (1..)
        .map(|n| if n == 1 { base.clone() } else { format!("{base} {n}") })
        .find(|t| !tabs.iter().any(|x| &x.title == t))
        .unwrap_or(base);
    let slug: String = title
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let stem = if slug.is_empty() { "quick".to_string() } else { slug };
    let id = (1..)
        .map(|n| if n == 1 { stem.clone() } else { format!("{stem}-{n}") })
        .find(|i| !tabs.iter().any(|x| x.id.as_deref() == Some(i.as_str())))
        .unwrap_or(stem);
    (title, id)
}

/// The AI a prompt with none named starts, in the order the tab form offers
/// them (`AI_CLIS` in the settings page)
pub const QUICK_AI_ORDER: &[&str] = &["claude", "codex", "gemini", "aider", "kimi"];

/// Which of the AIs this PC can start a prompt opens: the one it names, or
/// with none named the first in `QUICK_AI_ORDER` -- not whichever profile
/// happens to be read first
/// The AI a folder's tabs run when the folder is on a MicroVM: the one its
/// machine was given (the project's `machine_ai`), whatever this PC has.
/// `None` for a folder that is not on one -- this PC's AIs are the answer
/// there. A machine given no AI, or one this app does not know, is refused
/// with the words that say so: a tab typing `claude` on a machine that has
/// only Codex, or nothing, is a `command not found` and nothing else
pub fn machine_ai_of(
    desk: Option<&config::Desk>,
    dir: &std::path::Path,
) -> Option<Result<crate::uistate::AiChoice, &'static str>> {
    let d = desk?;
    let f = d
        .folders
        .iter()
        .find(|f| f.cwd.as_deref().is_some_and(|c| crate::uistate::same_folder(c, dir)))?;
    f.host.as_ref().filter(|h| h.is_made())?;
    let given = f
        .project
        .as_deref()
        .and_then(|n| d.projects.iter().find(|p| p.name == n))
        .and_then(|p| p.machine_ai.clone());
    Some(match given.as_deref().map(str::trim) {
        Some(k) if !k.eq_ignore_ascii_case(crate::microvm::NO_AI) => match crate::profile::machine_ai(k) {
            Some(m) => Ok(crate::uistate::AiChoice { key: m.key.clone(), name: m.name, command: m.key }),
            None => Err("msg.quick.no_machine_ai"),
        },
        _ => Err("msg.quick.no_machine_ai"),
    })
}

/// The AI to hand work in `dir` to: its machine's on a MicroVM, else the one
/// the settings chose among this PC's
fn ai_for_folder(
    desk: Option<&config::Desk>,
    dir: &std::path::Path,
    ai: &str,
    ais: &[crate::uistate::AiChoice],
) -> Result<crate::uistate::AiChoice, String> {
    match machine_ai_of(desk, dir) {
        Some(r) => r.map_err(i18n::t),
        None => quick_ai_choice(ai, ais).cloned().ok_or_else(|| i18n::t("msg.quick.no_ai")),
    }
}

pub fn quick_ai_choice<'a>(
    ai: &str,
    ais: &'a [crate::uistate::AiChoice],
) -> Option<&'a crate::uistate::AiChoice> {
    if !ai.is_empty() {
        return ais.iter().find(|a| a.key == ai);
    }
    QUICK_AI_ORDER
        .iter()
        .find_map(|k| ais.iter().find(|a| a.key == *k))
        .or_else(|| ais.first())
}

/// How long the screen of a tab a quick command opened has to hold still
/// before the line is typed into it.
///
/// Longer than a startup hook waits (`Tab::ready_for_startup_hook`), and on
/// purpose: a program just started often draws a line, goes quiet while it
/// loads, and only then asks its own first question. A line typed into that
/// quiet lands on the question when it comes (seen with Aider: its "create a
/// git repository?" took the prompt as the answer). The whole detection window
/// has to pass, so that a question on screen has been read as one.
///
/// And never given up on while a question stands (a folder's trust prompt is
/// one): the startup hook's "ready anyway after 15 seconds" would type the
/// line into it. The person answers it, and the line follows -- or, if nobody
/// does within `QUICK_WAIT`, it is said and dropped
pub const QUICK_SETTLE_MS: u64 = 2_500;
pub const QUICK_WAIT: Duration = Duration::from_secs(90);

/// Open a tab running `command` in `cwd` and have `text` typed into it once the
/// program in it is ready: what a quick command does, and what the git panel's
/// "Resolve" does with its instruction. Answers the tab's title; the caller
/// tells the settings watcher, which is what starts the tab
#[allow(clippy::too_many_arguments)]
fn open_and_say(
    desk: &str,
    cwd: &std::path::Path,
    command: &str,
    program: &str,
    label: &str,
    text: String,
    submit: bool,
    tabs: &[Tab],
    pending: &mut Vec<PendingQuick>,
    reveal: &mut Option<(String, Instant)>,
) -> Option<String> {
    let (title, tab_id) = quick_tab_names(label, program, tabs);
    let line = serde_json::json!({"name": title, "id": tab_id, "command": command});
    if !config::append_tab(desk, line, Some(cwd)) {
        return None;
    }
    *reveal = Some((tab_id.clone(), Instant::now() + Duration::from_secs(20)));
    pending.push(PendingQuick {
        id: tab_id,
        title: title.clone(),
        label: label.to_string(),
        cwd: cwd.to_path_buf(),
        text,
        submit,
        until: Instant::now() + QUICK_WAIT,
    });
    Some(title)
}

/// A pull request's base brought into the folder its branch is in, as the
/// thread that ran it answers
/// One question the git panel asked of a folder, on its way to that folder's
/// line: the primitive by name and what came after the tab, with the place
/// already settled on the board's thread
struct GitJob {
    panel: String,
    act: String,
    method: &'static str,
    dir: std::path::PathBuf,
    at: Option<crate::elsewhere::Elsewhere>,
    protect: Vec<String>,
    who: Option<crate::git::As>,
    params: Vec<serde_json::Value>,
    /// Given when it is put on a line, and handed back with the answer
    seq: u64,
}

/// What a line hands back: the question's number, and the answer as the panel
/// reads it
struct GitDone {
    seq: u64,
    panel: String,
    act: String,
    payload: serde_json::Value,
}

/// How long the panel waits on a line before saying the folder is not
/// answering yet. The line goes on: a second git in the same folder would only
/// find the first one's lock, and a commit whose hooks take two minutes still
/// lands -- so what the line says late is handed over when it comes
const GIT_PANEL_WAIT: Duration = Duration::from_secs(60);

/// The acts whose answer is a reading of the folder as it is. Only the newest
/// one asked is worth drawing: a status asked twice answers twice, and the
/// first answer is already out of date when the second is asked
const GIT_READS: &[&str] = &["status", "branch", "branches", "diff", "graph", "detail", "hunks", "remote_branches"];

/// The git panel's questions, run off the board's own thread.
///
/// Each folder has one line, a thread taking its questions in the order they
/// were asked: git in one folder is one thing at a time -- a stage, then the
/// commit that takes it -- and two at once would each find the other's lock.
/// Different folders go on at once. Git is started and waited for, and on a
/// network share, a large repository, a server or a MicroVM that wait is long;
/// on the board's thread it held every terminal still while it went on
struct GitLines {
    lines: std::collections::HashMap<String, std::sync::mpsc::Sender<GitJob>>,
    tx: std::sync::mpsc::Sender<GitDone>,
    rx: std::sync::mpsc::Receiver<GitDone>,
    seq: u64,
    /// The newest question of each kind, per panel: an older one's answer is
    /// not drawn
    newest: std::collections::HashMap<(String, String), u64>,
    /// Questions asked and not answered yet: (seq, panel, act, since, and
    /// whether the panel was already told this one is slow)
    waiting: Vec<(u64, String, String, Instant, bool)>,
}

impl GitLines {
    fn new() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self { lines: Default::default(), tx, rx, seq: 0, newest: Default::default(), waiting: Vec::new() }
    }

    /// A folder's line, by the folder as it is spelt, whatever machine it is on
    fn key(job: &GitJob) -> String {
        let at = job.at.as_ref().map(|a| a.machine_key()).unwrap_or_default();
        format!("{at}\u{1f}{}", job.dir.to_string_lossy().replace('\\', "/").to_lowercase())
    }

    fn ask(&mut self, mut job: GitJob) {
        self.seq += 1;
        job.seq = self.seq;
        self.newest.insert((job.panel.clone(), job.act.clone()), job.seq);
        let key = Self::key(&job);
        self.waiting.push((job.seq, job.panel.clone(), job.act.clone(), Instant::now(), false));
        let job = match self.lines.get(&key) {
            Some(line) => match line.send(job) {
                Ok(()) => return,
                // The line has ended (let go of after a while unasked): a new
                // one is started, and the question goes first on it
                Err(std::sync::mpsc::SendError(job)) => job,
            },
            None => job,
        };
        self.start_line(key, job);
    }

    fn start_line(&mut self, key: String, first: GitJob) {
        let (line_tx, line_rx) = std::sync::mpsc::channel::<GitJob>();
        let done = self.tx.clone();
        let spawned = std::thread::Builder::new().name("git-line".into()).spawn(move || {
            // For as long as the app runs. A line that let itself go when idle
            // could go in the instant a question was handed to it, and take
            // the question with it; a waiting thread costs nothing
            while let Ok(job) = line_rx.recv() {
                let answer = git_answer(&job);
                if done.send(GitDone { seq: job.seq, panel: job.panel, act: job.act, payload: answer }).is_err() {
                    return;
                }
            }
        });
        if spawned.is_err() {
            // Said, rather than left waiting until it is given up on
            let _ = self.tx.send(GitDone {
                seq: first.seq,
                panel: first.panel,
                act: first.act.clone(),
                payload: serde_json::json!({"act": first.act, "ok": false, "error": i18n::t("err.git.not_answering")}),
            });
            return;
        }
        let _ = line_tx.send(first);
        self.lines.insert(key, line_tx);
    }

    /// The answers ready now, as the panel reads them, and a "not answering"
    /// for each question waited on too long
    fn answers(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(d) = self.rx.try_recv() {
            if let Some(at) = self.waiting.iter().position(|w| w.0 == d.seq) {
                self.waiting.remove(at);
            }
            let stale = GIT_READS.contains(&d.act.as_str())
                && self.newest.get(&(d.panel.clone(), d.act.clone())).is_some_and(|n| *n > d.seq);
            if stale {
                continue;
            }
            let mut payload = d.payload;
            payload["panel"] = serde_json::json!(d.panel);
            out.push(payload.to_string());
        }
        // Slow: said once, and the answer still handed over when it comes
        for w in self.waiting.iter_mut().filter(|w| !w.4 && w.3.elapsed() >= GIT_PANEL_WAIT) {
            w.4 = true;
            out.push(
                serde_json::json!({"act": w.2, "ok": false, "slow": true, "error": i18n::t("err.git.not_answering"), "panel": w.1})
                    .to_string(),
            );
        }
        out
    }
}

/// One question answered, on a folder's line
fn git_answer(job: &GitJob) -> serde_json::Value {
    let root = match crate::gitops::root(&job.dir, job.at.as_ref()) {
        Ok(r) => r,
        Err(e) => return serde_json::json!({"act": job.act, "ok": false, "error": plain_error(&e)}),
    };
    match crate::gitops::call(job.method, &root, &job.protect, job.who.as_ref(), &job.params) {
        Ok(data) => serde_json::json!({"act": job.act, "ok": true, "data": data}),
        Err(e) => {
            // A commit refused on a shared branch is not a failure, it is a
            // question -- and the panel asks it in its own words, so the
            // reason is named rather than shown as it came
            let on = (job.act == "commit")
                .then(|| crate::gitops::call("git_branch", &root, &job.protect, None, &[]).ok())
                .flatten();
            let shared = on.as_ref().and_then(|b| b.get("protected").and_then(|p| p.as_bool())).unwrap_or(false);
            serde_json::json!({
                "act": job.act,
                "ok": false,
                "error": plain_error(&e),
                "why": if shared { "protected" } else { "" },
                // Named here: the panel's own idea of the branch is only there
                // once it has read the status
                "branch": on.as_ref().and_then(|b| b.get("name").cloned()),
            })
        }
    }
}

struct PrCatchUp {
    project: String,
    number: u64,
    /// Handed back as it came, so the screen that asked can tell its answer
    seq: serde_json::Value,
    dir: std::path::PathBuf,
    /// `origin/<base>`
    base: String,
    result: anyhow::Result<u64>,
    /// The branch, when the folder is on another machine: found out there on
    /// the thread, so the board does not ask again over the network
    far_branch: Option<String>,
}

/// Hand the merge stopped in `dir` to a new tab of `choice`, told what the
/// project's merge prompt says with this folder's branch, `base` and conflicted
/// files filled in. A tab already at it is brought forward instead: one AI on a
/// merge at a time. The git column and a pull request's page both come here
fn resolve_in_tab(
    desk: &config::Desk,
    dir: &std::path::Path,
    base: &str,
    choice: &crate::uistate::AiChoice,
    tabs: &[Tab],
    pending: &mut Vec<PendingQuick>,
    reveal: &mut Option<(String, Instant)>,
) -> Result<serde_json::Value, String> {
    resolve_in_tab_knowing(desk, dir, base, None, choice, tabs, pending, reveal)
}

/// The same, told the branch and the conflicted files where they are already
/// known -- found out on another machine, on a thread -- rather than asking git
/// for them again from the board
#[allow(clippy::too_many_arguments)]
fn resolve_in_tab_knowing(
    desk: &config::Desk,
    dir: &std::path::Path,
    base: &str,
    known: Option<(String, Vec<String>)>,
    choice: &crate::uistate::AiChoice,
    tabs: &[Tab],
    pending: &mut Vec<PendingQuick>,
    reveal: &mut Option<(String, Instant)>,
) -> Result<serde_json::Value, String> {
    hand_to_ai_tab(desk, dir, &i18n::t("git.catch_up.tab"), choice, tabs, pending, reveal, || {
        let (branch, files) = known.unwrap_or_else(|| {
            (crate::git::branch(dir).ok().flatten().unwrap_or_default(), crate::git::conflicts(dir).unwrap_or_default())
        });
        desk.git_of(dir)
            .merge_prompt()
            .replace("{folder}", &dir.display().to_string())
            .replace("{branch}", &branch)
            .replace("{base}", base)
            // The language the screen is in, named in itself: what the AI is asked
            // to answer in, and to say the next step in
            .replace("{language}", &i18n::t("lang.self"))
            .replace("{files}", &files.iter().map(|f| format!("  - {f}")).collect::<Vec<_>>().join("\n"))
    })
}

/// Work handed to a new tab of `choice` in `dir`, under `label`, with `prompt`
/// as its first message. A tab already at the same work there is brought
/// forward instead, and the prompt is not written: one AI on it at a time
#[allow(clippy::too_many_arguments)]
fn hand_to_ai_tab(
    desk: &config::Desk,
    dir: &std::path::Path,
    label: &str,
    choice: &crate::uistate::AiChoice,
    tabs: &[Tab],
    pending: &mut Vec<PendingQuick>,
    reveal: &mut Option<(String, Instant)>,
    prompt: impl FnOnce() -> String,
) -> Result<serde_json::Value, String> {
    if let Some((title, name)) = opened_for(label, dir, tabs, pending) {
        *reveal = Some((name, Instant::now() + Duration::from_secs(20)));
        return Ok(serde_json::json!({"title": title, "already": true}));
    }
    match open_and_say(&desk.name, dir, &choice.key, &choice.name, label, prompt(), true, tabs, pending, reveal) {
        Some(title) => Ok(serde_json::json!({"title": title, "already": false})),
        None => Err(i18n::tp("msg.quick.open_failed", &[("label", label)])),
    }
}

/// What the failed checks of a pull request's commit were, as a thread read
/// them from GitHub, for the tab that is to fix them
struct CiFix {
    project: String,
    number: u64,
    seq: serde_json::Value,
    title: String,
    url: String,
    head: String,
    /// The commit the checks ran on. What the prompt names when there is no
    /// pull request to name
    sha: String,
    dir: std::path::PathBuf,
    result: anyhow::Result<serde_json::Value>,
}

/// A tab opened under `label` in `dir` that is still running, or one on its
/// way there: its title and the name to bring it forward by. Pressing again
/// shows that one, rather than setting a second AI on the same work
pub fn opened_for(label: &str, dir: &std::path::Path, tabs: &[Tab], pending: &[PendingQuick]) -> Option<(String, String)> {
    // The title `quick_tab_names` gives: the label, or the label and a number
    let named = |title: &str| {
        title == label
            || title
                .strip_prefix(label)
                .and_then(|rest| rest.strip_prefix(' '))
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
    };
    let open = tabs.iter().find(|t| {
        named(&t.title) && !t.exited() && t.cwd().is_some_and(|c| crate::uistate::same_folder(c, dir))
    });
    if let Some(t) = open {
        return Some((t.title.clone(), t.id.clone().unwrap_or_else(|| t.title.clone())));
    }
    pending
        .iter()
        .find(|p| p.label == label && crate::uistate::same_folder(&p.cwd, dir))
        .map(|p| (p.title.clone(), p.id.clone()))
}

pub fn quick_ready(t: &Tab, now_ms: u64) -> bool {
    // An unopened terminal on a MicroVM shows this app's own line: output,
    // but not the program's, and typing into it would start the machine
    t.far_open()
        && t.had_output()
        && !matches!(t.state, crate::detect::TabState::Busy | crate::detect::TabState::Question)
        && t.ms_since_change(now_ms) >= QUICK_SETTLE_MS
}

/// A line waiting for a tab a quick command has just opened
pub struct PendingQuick {
    /// The new tab's automation name
    pub id: String,
    /// What its tab strip will say
    pub title: String,
    pub label: String,
    /// The folder it opens in
    pub cwd: std::path::PathBuf,
    pub text: String,
    pub submit: bool,
    pub until: Instant,
}
/// The screen to move to, following the ball. None if it shouldn't move.
///
/// Don't follow right after a human touches the screen. Getting yanked away
/// mid-read is the worst outcome, so once someone touches it, stay quiet for a while.
/// Which desk to start from.
///
/// What's remembered is the desk's id, not its number and not its name.
/// Numbers shift with reordering or additions, which would turn "resume where
/// I left off yesterday" into something else entirely; a name changes the
/// moment somebody renames the desk, and the app then opened the first desk
/// instead and brought none of that desk's conversations back. A file written
/// before ids were kept holds the name, so a name is still understood when no
/// id answers to it.
/// Falls back to the first one if not found (e.g. it was deleted).
pub fn starting_desk(enabled: bool, last: Option<&str>, desks: &[config::Desk]) -> usize {
    if !enabled {
        return 0;
    }
    last.and_then(|want| {
        desks
            .iter()
            .position(|d| !d.id.is_empty() && d.id == want)
            .or_else(|| desks.iter().position(|d| d.name == want))
    })
    .unwrap_or(0)
}
/// When automation may move what the person is looking at.
///
/// `shikisha.show()` is the only thing that moves the view, so this is the only
/// gate it has to pass — one rule rather than one per path. Handing work to a tab
/// (`send_to_tab`) no longer moves anything by itself: a script that wants to be
/// watched says so, and the person's answer to that request lives here.
#[derive(Clone, Copy)]
pub struct ViewMove {
    /// Their standing answer: may automation switch tabs at all
    allowed: bool,
    /// When they last moved the view themselves
    touched_ms: u64,
    /// The settings screen is up. Never pull someone out of what they are reading
    settings_open: bool,
}
impl ViewMove {
    fn may(&self, now_ms: u64) -> bool {
        self.allowed
            && !self.settings_open
            && now_ms.saturating_sub(self.touched_ms) >= VIEW_GUARD_MS
    }
}
/// How long the screen is left alone after a person moves it themselves.
///
/// Getting yanked away mid-read is the worst outcome, so once someone takes the
/// wheel, automation waits its turn.
pub const VIEW_GUARD_MS: u64 = 8_000;
/// Whether a human touched it recently. False if never touched at all.
///
/// Treating time 0 as "touched" here would silently drop every auto-send for
/// the guard period after app startup (this used to be why startup automation
/// didn't run).
pub fn touched_recently(t: &Tab, now_ms: u64) -> bool {
    t.last_manual_ms
        .is_some_and(|m| now_ms.saturating_sub(m) < MANUAL_GUARD_MS)
}
/// An excerpt collapsed onto a single line, for logging. Full text isn't
/// readable, so keep only the beginning.
/// What a phone is told when a tab finishes, for the people who asked to be
/// told rather than writing a hook for it.
///
/// The name, because a phone that buzzes without saying which tab finished
/// sends you to the PC to find out. The opening of the answer, because most of
/// the time that IS the answer and the walk can be skipped entirely. And a way
/// back, when one was asked for.
///
/// `reply` is a link to a page holding this tab's answer and a box to reply
/// in. It is absent unless the person ticked the box for this tab, and the
/// absence is total: no link, and no address either. Somebody who said "just
/// tell me what it said" did not ask for the machine's address to travel with
/// it. What never travels in either case is the access token -- a chat message
/// is not a place to put the key to a terminal.
pub fn on_done_message(name: &str, output: &str, reply: Option<&str>) -> String {
    let mut msg = i18n::tp("msg.notify.on_done", &[("name", name)]);
    let said = log_excerpt(output, 160);
    if !said.is_empty() {
        msg.push('\n');
        msg.push_str(&said);
    }
    if let Some(link) = reply {
        msg.push('\n');
        msg.push('\n');
        msg.push_str(&i18n::t("msg.notify.reply_here"));
        msg.push('\n');
        msg.push_str(link);
    }
    msg
}
pub fn log_excerpt(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = one.chars().take(max).collect();
    if one.chars().count() > max {
        out.push('…');
    }
    out
}
/// The 🔍 environment survey: one fixed, read-only probe per shell family.
/// Curated on purpose — OS/distro, key tool availability, and the running
/// middleware that matters for command suggestions. A full package dump
/// (dpkg -l and friends) floods both the terminal and the AI's context for
/// no accuracy gain. Output is wrapped in markers so the loop can capture it
pub const POSIX_PROBE: &str = r#"echo "===SHIKISHA ENV==="; uname -a; cat /etc/os-release 2>/dev/null | head -4; sw_vers 2>/dev/null; echo "--- tools ---"; for c in docker kubectl git python3 node java nginx mysql psql redis-cli systemctl apt-get yum dnf; do command -v $c >/dev/null 2>&1 && echo $c; done; echo "--- running ---"; ps -eo comm= 2>/dev/null | sort -u | grep -iE "nginx|httpd|apache|mysqld|mariadb|postgres|redis|php|java|node|docker|containerd|tomcat" | head -15; echo "===ENV END===""#;
pub const PS_PROBE: &str = r#""===SHIKISHA ENV==="; $PSVersionTable.PSVersion.ToString(); (Get-CimInstance Win32_OperatingSystem).Caption; "--- tools ---"; foreach ($c in "docker","kubectl","git","python","node","java","mysql","psql") { if (Get-Command $c -ErrorAction SilentlyContinue) { $c } }; "--- running ---"; (Get-Service | Where-Object Status -eq "Running" | Select-Object -ExpandProperty Name) -match "sql|nginx|apache|redis|docker|iis|w3svc|tomcat" | Select-Object -First 15; "===ENV END===""#;
pub const CMD_PROBE: &str =
    "echo ===SHIKISHA ENV=== & ver & echo --- tools --- & where docker git python node java mysql 2>nul & echo ===ENV END===";
/// Pick the probe whose syntax matches the terminal: the launch command for
/// local tabs, the prompt's shape for SSH and other indirections (a POSIX
/// shell being the overwhelming default on the far side)
pub fn survey_probe(cmdline: &str, screen: &str) -> &'static str {
    let head = cmdline.split_whitespace().next().unwrap_or("");
    let base = std::path::Path::new(head)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(head)
        .to_ascii_lowercase();
    match base.as_str() {
        "powershell" | "pwsh" => return PS_PROBE,
        "cmd" => return CMD_PROBE,
        "wsl" | "bash" | "sh" | "zsh" | "fish" => return POSIX_PROBE,
        _ => {}
    }
    let last = screen
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("");
    if last.trim_start().starts_with("PS ") {
        PS_PROBE
    } else if last.contains(":\\") && last.trim_end().ends_with('>') {
        CMD_PROBE
    } else {
        POSIX_PROBE
    }
}
/// Copy the newest run's replay.lua into the user's Downloads folder.
/// `Ok(None)` = no run has recorded anything replayable yet
pub fn save_replay_to_downloads() -> std::io::Result<Option<std::path::PathBuf>> {
    let Some(dir) = exchange::latest_run() else {
        return Ok(None);
    };
    let text = std::fs::read_to_string(dir.join("replay.lua")).unwrap_or_default();
    let live = text
        .lines()
        .any(|l| !l.trim().is_empty() && !l.trim_start().starts_with("--"));
    if !live {
        return Ok(None);
    }
    let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("run");
    // Downloads is where a "download button" is expected to land things;
    // fall back to the logs folder rather than failing when it's missing
    let base = std::env::var_os("USERPROFILE")
        .map(|p| std::path::PathBuf::from(p).join("Downloads"))
        .filter(|p| p.is_dir())
        .unwrap_or_else(config::logs_dir);
    let dest = base.join(format!("shikisha-macro-{name}.lua"));
    std::fs::write(&dest, text)?;
    Ok(Some(dest))
}
/// The list of ids in the same order they're laid out on screen.
///
/// Targets are counted by screen position. A name and a number both point to
/// the same thing (numbers shift with reordering, so using names when writing is recommended).
/// The folder a surface (1..) works in, if it works in one. A page works in
/// none; a panel works in the folder it reports on.
/// Whether pressing a working folder's name moves the screen. Not when that
/// folder's own tab is already in front and nothing covers the board -- the
/// press is somebody finding their place. With a tab of no folder in front
/// (the Issue tab, a page), the folder is not what is being looked at, however
/// recently it was
/// What a folder's name and summary come back under, followed by a number
const LABEL_TAG: &str = "folder_label:";

/// How long a folder's name is waited for before its requests are read again
/// next time. The AI is given ninety seconds, and asked twice at most
const LABEL_GIVE_UP: Duration = Duration::from_secs(5 * 60);

/// Calling a drawn branch after the work, now that the work has a name.
///
/// A folder cut with nobody's name in mind is on a name this app drew
/// (`mighty-gannet`), and that name is what a pull request, a merge and every
/// list of branches afterwards will say. So the AI that writes the folder's
/// name writes a branch name with it, and it is used here -- once, on the
/// first name that is written, while the branch is still the drawn one and has
/// never been pushed ([`crate::worktree::auto_rename_plan`] holds the gates).
///
/// The settings file is handed in rather than looked up, so that this can be
/// run against a scratch one: a test that wrote to the real settings would be
/// editing the machine it runs on.
///
/// Everything here is best-effort: a folder that keeps its drawn name has lost
/// nothing, so nothing is reported to the person as a failure. What did happen
/// is written to the log in git's own words, because a command this app ran on
/// its own is still a command somebody may have to account for.
fn rename_drawn_branch(settings: &std::path::Path, job: &LabelJob, slug: &str) {
    if !job.rename || slug.trim().is_empty() {
        return;
    }
    let Some(drawn) = job.drawn.as_deref() else { return };
    let Some(plan) = crate::worktree::auto_rename_plan(&job.folder, drawn, slug, &job.prefix) else {
        return;
    };
    if let Err(why) = crate::worktree::rename(&plan) {
        append_hook_log(&format!("the branch in {} kept its name: {why:#}", job.folder.display()));
        return;
    }
    append_hook_log(&format!("{} ({})", plan.line(), job.folder.display()));
    // The settings hold the branch a folder is on, and no longer a drawn name:
    // this happens once, and the next name written is only a name
    if let Err(why) = config::set_folder_branch_at(settings, &job.desk, &job.folder, &plan.to, None) {
        append_hook_log(&format!(
            "the branch in {} is now {} and the settings still say {}: {why:#}",
            job.folder.display(),
            plan.to,
            plan.from
        ));
    }
}

/// A folder's name and summary being written: which folder, in which desk,
/// from which requests (handed back if it fails)
struct LabelJob {
    tag: String,
    desk: String,
    folder: std::path::PathBuf,
    asks: Vec<String>,
    started: Instant,
    /// The branch name this app drew for this folder, while it is still on it.
    /// Absent for a branch somebody named, which is never renamed here
    drawn: Option<String>,
    /// What its project puts in front of every branch of it
    prefix: String,
    /// Whether this desk lets an automatic name reach the branch
    rename: bool,
}

/// The tags a draft's answer comes back under
const DRAFT_ISSUE_TAG: &str = "issue_draft";
const DRAFT_PR_TAG: &str = "pr_draft";

/// What CI ran on, in one phrase for the prompt to name.
///
/// The pull request when the branch is on one, and the commit itself when it
/// is not: the checks run on the push, so a branch nobody has opened a pull
/// request for can have failed just the same, and the words that name it
/// cannot then say "pull request" at all
pub fn ci_ran_on(number: u64, title: &str, url: &str, head: &str, sha: &str) -> String {
    match number {
        0 => i18n::tp(
            "ai.ci.on_commit",
            &[("sha", &sha.chars().take(7).collect::<String>()), ("branch", head)],
        ),
        n => i18n::tp("ai.ci.on_pr", &[("n", &n.to_string()), ("title", title), ("url", url)]),
    }
}

/// A prompt with its words filled in: each `{name}` becomes its value, and
/// `{main}` becomes the main material -- or, when the prompt does not say where
/// it goes, the material follows it. An empty prompt is the material alone
fn fill_or_append(prompt: &str, words: &[(&str, &str)], main: &str, material: &str) -> String {
    let mut out = prompt.to_string();
    for (name, value) in words {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    let mark = format!("{{{main}}}");
    if out.contains(&mark) {
        out.replace(&mark, material)
    } else if out.trim().is_empty() {
        material.to_string()
    } else {
        format!("{out}\n\n{material}")
    }
}

pub fn folder_press_moves(front: Option<&std::path::Path>, want: &std::path::Path, covered: bool) -> bool {
    covered || !front.is_some_and(|f| crate::uistate::same_folder(f, want))
}

pub fn surface_folder<'a>(surfaces: &'a [Surface], tabs: &'a [Tab], surface: usize) -> Option<&'a std::path::Path> {
    match surfaces.get(surface.checked_sub(1)?)? {
        Surface::Session(i) => tabs.get(*i)?.cwd(),
        Surface::Git { dir, .. }
        | Surface::Editor { dir, .. }
        | Surface::Sftp { dir, .. }
        | Surface::Failed { dir, .. }
        | Surface::Split { dir, .. } => dir.as_deref(),
        Surface::Browser { .. } | Surface::Issues { .. } => None,
    }
}
pub fn surface_keys(surfaces: &[Surface], tabs: &[Tab]) -> Vec<hooks::TabKey> {
    surfaces
        .iter()
        .map(|p| match p {
            Surface::Session(i) => tabs.get(*i).map(|t| t.key()).unwrap_or_default(),
            // A page and a panel are addressed the same way a session is:
            // by the name automation knows them by, never the one on screen
            Surface::Browser { key, .. }
            | Surface::Git { key, .. }
            | Surface::Sftp { key, .. }
            | Surface::Editor { key, .. }
            | Surface::Failed { key, .. }
            | Surface::Split { key, .. }
            | Surface::Issues { key } => hooks::TabKey { id: Some(key.clone()) },
        })
        .collect()
}
/// A hand-off that can't be delivered yet. Runs once the recipient becomes ready to receive input.
///
/// It's not unusual for the target to still be starting up. Since a dropped
/// hand-off is invisible to everyone, we hold onto it ourselves instead.
pub struct Waiting {
    cmd: Command,
    /// Give up once this time passes. Holding onto it any longer wouldn't help — eventually nobody remembers it anyway.
    give_up_ms: u64,
}
/// Whether this hand-off is one that can wait for the recipient to become ready.
///
/// Only "delivering something" can wait. Restarts and notifications have
/// nothing to do with whether the recipient is ready.
pub fn can_wait(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::SendPrompt { .. } | Command::DraftPrompt { .. }
    )
}
/// The destination of that hand-off
pub fn target_of(cmd: &Command) -> Option<&hooks::TabRef> {
    match cmd {
        Command::SendPrompt { target, .. } | Command::DraftPrompt { target, .. } => Some(target),
        _ => None,
    }
}
/// Whether the recipient is in a state where it can accept input.
/// `now_ms` is the main loop's clock — the same one the readiness gate measures
/// "the screen has held still" against
pub fn ready_to_receive(t: &Tab, now_ms: u64) -> bool {
    t.ready_for_startup_hook(now_ms)
}
/// How long to hold before giving up. Whoever wrote it isn't watching anymore by the time this long has passed.
pub const WAIT_FOR_TAB_MS: u64 = 30_000;
/// Lines waiting to be shown in the panel of the page being driven from words.
///
/// A queue rather than an argument threaded through every caller, for the
/// same reason the hook log is one: this is something said on the way past,
/// by code that is in the middle of doing something else, to a screen that is
/// nowhere near it
static WORDS_NOTES: std::sync::Mutex<Vec<(String, bool)>> = std::sync::Mutex::new(Vec::new());

/// Say a line in the panel of the page being driven (see [`WORDS_NOTES`])
pub fn say_in_words_panel(text: String, bad: bool) {
    if let Ok(mut g) = WORDS_NOTES.lock() {
        // A person reads the last line, not the hundredth: an unattended run
        // must not grow a queue nobody will ever look at
        if g.len() > 64 {
            g.remove(0);
        }
        g.push((text, bad));
    }
}

/// Everything said since the last look
pub fn take_words_notes() -> Vec<(String, bool)> {
    WORDS_NOTES.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
}

/// Executes the operation requests queued by Lua hooks.
/// Auto-sends inherit chain depth (the invisible ball) and stop once the cap is hit.
#[allow(clippy::too_many_arguments)]
pub fn exec_commands(
    cmds: Vec<Command>,
    tabs: &mut [Tab],
    surfaces: &[Surface],
    panes: &mut crate::layout::Layout,
    max_chain: u32,
    auto_enabled: bool,
    now_ms: u64,
    rows: u16,
    cols: u16,
    notifier: &notify::Notifier,
    flash: &mut Option<String>,
    ball: &mut ball::Ball,
    pending_send: &mut Vec<PendingSend>,
    waiting: &mut Vec<Waiting>,
    active: &mut usize,
    // `asked`: divisions automation wants, for the loop to carry out on its
    // next turn. Not done here because what dividing means depends on whether
    // a split row is in front, which the loop knows and this does not.
    // `shut`: panes it wants closed, for the same reason -- the last pane of a
    // split is the split, and taking that row away is the loop's business
    asked: &mut Vec<(crate::layout::PaneId, bool)>,
    shut: &mut Vec<crate::layout::PaneId>,
    // Whether automation may move the view right now (see ViewMove)
    view: ViewMove,
) {
    let keys = surface_keys(surfaces, tabs);
    let index_of = |r: &hooks::TabRef| r.resolve(&keys);
    // From a screen number to its location in the tabs array. None for a browser.
    let session_of = |surface: usize| session_at(surfaces, surface);
    for cmd in cmds {
        // If the recipient can't accept input yet, hold onto it and deliver it later.
        // Sending it now would be silently dropped, invisible to whoever wrote it.
        if can_wait(&cmd) {
            let not_yet = target_of(&cmd)
                .and_then(index_of)
                .and_then(session_of)
                .and_then(|i| tabs.get(i))
                .map(|t| !ready_to_receive(t, now_ms))
                .unwrap_or(false);
            if not_yet {
                if let Some(t) = target_of(&cmd) {
                    append_hook_log(&format!("Waiting for it to become ready to receive: {t:?}"));
                }
                waiting.push(Waiting {
                    cmd,
                    give_up_ms: now_ms + WAIT_FOR_TAB_MS,
                });
                continue;
            }
        }
        match cmd {
            Command::Log(msg) => append_hook_log(&msg),
            // Switch the displayed tab (spectator mode). 0 is the operating board (INDEX).
            // The target, whether a session or a browser, is addressed by screen number.
            Command::ShowTab { target } => {
                if !view.may(now_ms) {
                    // The person said no, or is mid-read, or is in the settings.
                    // Kept in the log so "it said show and the screen didn't move"
                    // can be traced rather than guessed at.
                    append_hook_log(&format!(
                        "ShowTab {target:?} ignored (allowed={}, settings={}, {}ms since they moved it)",
                        view.allowed,
                        view.settings_open,
                        now_ms.saturating_sub(view.touched_ms),
                    ));
                } else if matches!(target, hooks::TabRef::Index(0)) {
                    *active = 0;
                } else if let Some(pane) = index_of(&target) {
                    *active = pane;
                } else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                }
            }
            // A rally's final result. Written to data/last-result.json, the log, and the UI.
            // External integrations read this file (the process itself keeps running as an interactive app).
            Command::SetResult { code, reason, origin } => {
                // A result means the automated chain (rally, discussion, …) has
                // concluded: hand the ring back to the human. Beyond being
                // semantically right, this is what lets the discussion topic
                // banner reappear once a round finishes — the ring sits Held on
                // the last speaker until something puts it back in idle.
                ball.reset();
                let at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let json = serde_json::json!({
                    "code": code, "reason": reason, "tab": origin, "at": at,
                });
                let path = config::state_path("last-result.json");
                if let Err(e) = crate::crypto::write_atomic(&path, &json.to_string()) {
                    append_hook_log(&format!("Failed to write result: {e}"));
                }
                append_hook_log(&format!("Result code={code} reason={reason} (tab{origin})"));
                *flash = Some(i18n::tp(
                    "msg.result",
                    &[("code", &code.to_string()), ("reason", &reason)],
                ));
            }
            Command::Restart { target, fresh } => {
                let Some(target) = index_of(&target) else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                    continue;
                };
                let Some(at) = session_of(target) else { continue };
                let alone = only_one_here(tabs, at);
                if let Some(t) = tabs.get_mut(at) {
                    append_hook_log(&format!("restart tab{target} (lua)"));
                    *flash = Some(restart_tab(t, alone, !fresh, rows, cols));
                }
            }
            // The division of the screen. Carried out at once: whoever asked
            // said so in as many words, unlike ShowTab, which is a side effect
            // of automation running elsewhere and so has to ask first
            Command::Pane(op) => {
                use crate::hooks::PaneOp;
                match op {
                    // Handed to the loop rather than carried out here, so
                    // that automation, the keys and the pane's own button all
                    // divide the same way: inside a split it divides that
                    // split, outside one it makes a split row. Which of those
                    // it is depends on what is in front, and that is the
                    // loop's to know
                    PaneOp::Split(dir) => {
                        asked.push((panes.focus(), matches!(dir, crate::layout::Dir::Col)));
                        append_hook_log(&format!("pane split {dir:?} (lua)"));
                    }
                    PaneOp::Close => {
                        shut.push(panes.focus());
                        append_hook_log("pane closed (lua)");
                    }
                    PaneOp::Focus(dir) => {
                        if panes.focus_move(dir) {
                            *active = panes.focused_surface();
                        }
                    }
                    PaneOp::Equalize => panes.equalize(),
                }
            }
            // A tab telling us which conversation it is running. Written down
            // against that tab, and beside the exe, so a restart — or a restart
            // of the whole app — can pick the conversation back up
            Command::SetSession { id, origin } => {
                let Some(t) = session_of(origin).and_then(|i| tabs.get_mut(i)) else {
                    append_hook_log(&format!("set_session from tab{origin}: no such tab"));
                    continue;
                };
                let s = tab::Session { id, source: tab::SessionSource::Hook };
                append_hook_log(&format!("tab{origin} \"{}\" is running {}", t.title, s.short()));
                t.session = Some(s);
            }
            // A tab saying what it was just asked. Kept on the tab for its
            // folder's automatic name; the words are not logged
            Command::ReportPrompt { text, origin } => {
                let Some(t) = session_of(origin).and_then(|i| tabs.get_mut(i)) else {
                    append_hook_log(&format!("report_prompt from tab{origin}: no such tab"));
                    continue;
                };
                t.heard(&text);
            }
            // A tab saying what it is doing, rather than being read. Believed
            // over the screen, and dropped when it is older than something
            // already applied — hooks are separate processes that race
            Command::SetState { state, sent_ms, origin } => {
                let Some(known) = TabState::from_label(&state) else {
                    append_hook_log(&format!("set_state from tab{origin}: {state:?} is not a state"));
                    continue;
                };
                let Some(t) = session_of(origin).and_then(|i| tabs.get_mut(i)) else {
                    append_hook_log(&format!("set_state from tab{origin}: no such tab"));
                    continue;
                };
                if !t.hook_says(known, sent_ms) {
                    append_hook_log(&format!(
                        "tab{origin} \"{}\" said {state} out of order — dropped",
                        t.title
                    ));
                }
            }
            Command::SetStatus { key, value, target, origin } => {
                let at = target.as_ref().and_then(index_of).unwrap_or(origin);
                if let Some(t) = session_of(at).and_then(|i| tabs.get_mut(i)) {
                    t.set_status(&key, &value);
                }
            }
            Command::SetProgress { value, label, target, origin } => {
                let at = target.as_ref().and_then(index_of).unwrap_or(origin);
                if let Some(t) = session_of(at).and_then(|i| tabs.get_mut(i)) {
                    t.progress = value.map(|v| (v, label.clone()));
                }
            }
            Command::Notify { dest, text } => {
                append_hook_log(&format!(
                    "NOTIFY[{}] {text}",
                    dest.as_deref().unwrap_or("(primary)")
                ));
                *flash = Some(notifier.send_opt(dest.as_deref(), &text));
            }
            Command::SendKeys { target, keys } => {
                if !auto_enabled {
                    continue;
                }
                let Some(target) = index_of(&target) else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                    continue;
                };
                if let Some(t) = session_of(target).and_then(|i| tabs.get(i)) {
                    if touched_recently(t, now_ms) {
                        continue;
                    }
                    let _ = t.write_bytes(keys.as_bytes());
                }
            }
            Command::DraftPrompt {
                target,
                text,
                origin,
            } => {
                if !auto_enabled {
                    continue;
                }
                let Some(idx) = index_of(&target) else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                    continue;
                };
                let depth = session_of(origin)
                    .and_then(|i| tabs.get(i))
                    .map(|t| t.chain_depth)
                    .unwrap_or(0)
                    + 1;
                if depth > max_chain {
                    *flash = Some(i18n::t("msg.chain_limit"));
                    append_hook_log(&format!(
                        "chain limit ({max_chain}): draft tab{origin} -> tab{idx}"
                    ));
                    continue;
                }
                if let Some(t) = session_of(idx).and_then(|i| tabs.get_mut(i)) {
                    if touched_recently(t, now_ms) {
                        continue;
                    }
                    // Sending this same thing to a recipient that doesn't
                    // understand the markers (a plain shell) would have the
                    // markers ignored and the newline inside it run as-is.
                    // Better to refuse and leave a reason than to silently drop the newline.
                    if !t.accepts_bracketed_paste() {
                        let msg = i18n::tp("msg.draft_unsupported", &[("tab", &t.title)]);
                        append_hook_log(&msg);
                        *flash = Some(msg);
                        continue;
                    }
                    // Don't send submit (Enter). A human adds to it and sends it themselves.
                    let seen = t.output_count();
                    let chunks = paste_chunks(t, &text);
                    pending_send.push(PendingSend::new(idx, chunks, false, seen, now_ms, text.chars().count()));
                    // A human is part of the loop too. If they add to it and
                    // send it, the chain continues, so count the depth the same
                    // way as an auto-send.
                    t.chain_depth = depth;
                    ball.draft(origin, idx, depth, now_ms);
                    append_hook_log(&format!(
                        "Draft tab{origin} -> tab{idx} (depth {depth}): {}",
                        log_excerpt(&text, 60)
                    ));
                }
            }
            Command::WordsNote { text, bad } => {
                append_hook_log(&format!("words: {}", log_excerpt(&text, 80)));
                say_in_words_panel(text, bad);
            }
            Command::Note { target, text } => {
                // Display only: no chain depth, no ball, no submit reservation,
                // and no manual-input guard. Writing on a screen interrupts
                // nothing, so none of the things that protect a turn apply.
                let Some(target) = index_of(&target) else {
                    append_hook_log(&format!("Note target not found: {target:?}"));
                    continue;
                };
                if let Some(t) = session_of(target).and_then(|i| tabs.get(i)) {
                    t.note(&text);
                    append_hook_log(&format!("note tab{target}: {}", log_excerpt(&text, 60)));
                }
            }
            Command::SendPrompt {
                target,
                text,
                origin,
            } => {
                if !auto_enabled {
                    continue;
                }
                let Some(target) = index_of(&target) else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                    append_hook_log(&format!("Send target not found: {target:?}"));
                    continue;
                };
                let depth = session_of(origin)
                    .and_then(|i| tabs.get(i))
                    .map(|t| t.chain_depth)
                    .unwrap_or(0)
                    + 1;
                if depth > max_chain {
                    *flash = Some(i18n::tp("msg.chain_limit", &[("max", &max_chain.to_string())]));
                    append_hook_log(&format!("chain limit ({max_chain}): tab{origin} -> tab{target}"));
                    continue;
                }
                let Some(t) = session_of(target).and_then(|i| tabs.get_mut(i)) else {
                    continue;
                };
                if touched_recently(t, now_ms) {
                    *flash = Some(i18n::t("msg.manual_guard"));
                    continue;
                }
                t.chain_depth = depth;
                if t.is_browser_brain() {
                    // A model steering the browser: replay the conversation
                    // (history-backed) so it remembers earlier moves, mark the
                    // turn so BUSY -> DONE -> on_done fires, and let on_done pull
                    // the ```lua out of the reply. The relayed screen text is
                    // fed as context but not echoed as a giant prompt line.
                    t.rally_relay(text.clone());
                    append_hook_log(&format!("brain's turn tab{target} ({} chars)", text.chars().count()));
                } else if t.is_model() {
                    // model bridge: hits complete() on a thread, injects the
                    // response into the screen, and writes it to say.txt too.
                    // Detection (BUSY -> DONE -> on_done) runs on the injected activity.
                    t.dispatch_model(text.clone());
                    append_hook_log(&format!("model's turn tab{target} ({} chars)", text.chars().count()));
                } else {
                    let seen = t.output_count();
                    let chunks = paste_chunks(t, &text);
                    pending_send.push(PendingSend::new(target, chunks, true, seen, now_ms, text.chars().count()));
                    append_hook_log(&format!("Paste tab{target} ({} chars)", text.chars().count()));
                }
                // A self-send (seeding a persona at launch, the opening nudge,
                // a model's self-kick) starts things moving but isn't a hand-off
                // between participants. Leaving the ring parked on it would make
                // the "start the discussion" banner believe a round is already
                // running, so only a genuine pass to another participant moves it.
                if origin != target {
                    ball.throw(origin, target, depth, now_ms);
                }
                append_hook_log(&format!(
                    "auto-send tab{origin} -> tab{target} (depth {depth}): {}",
                    log_excerpt(&text, 120)
                ));
            }
        }
    }
}
/// Key handling while in copy mode
pub fn handle_copy_key(
    t: &mut Tab,
    key: &KeyEvent,
    size: Size,
    flash: &mut Option<String>,
) -> Result<()> {
    let (rows_v, cols_v) = pty_dims(size);
    let Some(mut cs) = t.copy.take() else {
        return Ok(());
    };
    let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
    let cur = p.screen().scrollback();
    let mut keep = true;
    // While the search line is open it takes every key: a search for "quit"
    // must not be read as four commands
    if let Some(typed) = cs.find.as_mut() {
        match key.code {
            KeyCode::Esc => cs.find = None,
            KeyCode::Backspace => {
                typed.pop();
            }
            KeyCode::Enter => {
                let needle = std::mem::take(typed);
                cs.find = None;
                if !needle.is_empty() {
                    cs.last = needle;
                    let from = abs_line(cur, rows_v, cs.cursor_row);
                    match tab::find_line(&mut p, &cs.last, from, true, cols_v) {
                        Some(d) => show_line(&mut p, &mut cs, d, rows_v),
                        None => {
                            *flash = Some(i18n::tp("msg.find.none", &[("what", &cs.last)]))
                        }
                    }
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => typed.push(c),
            _ => {}
        }
        // Typing has to be visible. Copy mode borrows the message line for it
        // rather than growing a bar of its own, which would move the terminal
        // under the reader's eyes at the moment they are reading it
        if let Some(typed) = cs.find.as_ref() {
            *flash = Some(i18n::tp("msg.find.typing", &[("what", typed)]));
        }
        drop(p);
        t.copy = Some(cs);
        return Ok(());
    }
    match key.code {
        // Look for something in the history. Opens a line to type into; the
        // search itself runs on Enter
        KeyCode::Char('/') | KeyCode::Char('?') => {
            cs.find = Some(String::new());
            *flash = Some(i18n::tp("msg.find.typing", &[("what", "")]));
        }
        // The same search again, further back — or, capitalised, back the
        // other way. The pair vi has used for fifty years
        KeyCode::Char('n') | KeyCode::Char('N') if !cs.last.is_empty() => {
            let up = key.code == KeyCode::Char('n');
            let from = abs_line(cur, rows_v, cs.cursor_row);
            match tab::find_line(&mut p, &cs.last, from, up, cols_v) {
                Some(d) => show_line(&mut p, &mut cs, d, rows_v),
                None => *flash = Some(i18n::tp("msg.find.none", &[("what", &cs.last)])),
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            p.screen_mut().set_scrollback(0);
            keep = false;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if cs.cursor_row > 0 {
                cs.cursor_row -= 1;
            } else {
                p.screen_mut().set_scrollback(cur + 1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if cs.cursor_row + 1 < rows_v {
                cs.cursor_row += 1;
            } else {
                p.screen_mut().set_scrollback(cur.saturating_sub(1));
            }
        }
        KeyCode::PageUp => p.screen_mut().set_scrollback(cur + rows_v as usize),
        KeyCode::PageDown => {
            p.screen_mut().set_scrollback(cur.saturating_sub(rows_v as usize));
        }
        // To the oldest point (clamped to what's actually retained)
        KeyCode::Home | KeyCode::Char('g') => {
            p.screen_mut().set_scrollback(usize::MAX / 2);
        }
        KeyCode::End | KeyCode::Char('G') => {
            p.screen_mut().set_scrollback(0);
            cs.cursor_row = rows_v.saturating_sub(1);
        }
        // Start / clear selection
        KeyCode::Char('v') | KeyCode::Char(' ') => {
            cs.anchor = match cs.anchor {
                Some(_) => None,
                None => Some(abs_line(cur, rows_v, cs.cursor_row)),
            };
        }
        // Copy the selected range (or the cursor's line if none) and return
        KeyCode::Char('y') | KeyCode::Enter => {
            let here = abs_line(cur, rows_v, cs.cursor_row);
            let (lo, hi) = match cs.anchor {
                Some(a) => (a.min(here), a.max(here)),
                None => (here, here),
            };
            let text = extract_text(&mut p, lo, hi, cols_v);
            p.screen_mut().set_scrollback(0);
            drop(p);
            *flash = Some(copy_text(&text));
            t.copy = None;
            return Ok(());
        }
        // Copy the whole history
        KeyCode::Char('a') => {
            let text = extract_text(&mut p, 0, usize::MAX / 2, cols_v);
            p.screen_mut().set_scrollback(0);
            drop(p);
            *flash = Some(copy_text(&text));
            t.copy = None;
            return Ok(());
        }
        _ => {}
    }
    if keep {
        t.copy = Some(cs);
    }
    Ok(())
}
/// INDEX = home screen: session list + menu
/// The block-letter wordmark (3 lines). Per-character width is uneven, so
/// measure actual character width rather than counting including right-edge padding.
pub const WORDMARK: [&str; 3] = [
    "█▀▀ █ █ █ █ █ █ █▀▀ █ █ █▀█    ▀█▀ █▀▀ █▀█ █▄█",
    "▀▀█ █▀█ █ █▀▄ █ ▀▀█ █▀█ █▀█ ▀▀  █  █▀▀ █▀▄ █ █",
    "▀▀▀ ▀ ▀ ▀ ▀ ▀ ▀ ▀▀▀ ▀ ▀ ▀ ▀     ▀  ▀▀▀ ▀ ▀ ▀ ▀",
];
/// The wording used when collapsed to a single line
pub const WORDMARK_SMALL: &str = "◢◤ SHIKISHA-TERM";

/// Set, change, or remove the master password (INDEX menu [k])
pub fn manage_master_password(
    shell: &mut dyn Shell,
    cfg: Option<&config::Config>,
    password: &mut Option<String>,
) -> Result<String> {
    let Some(path) = cfg.and_then(|c| c.secrets_path()) else {
        return Ok(i18n::t("msg.password.no_secrets"));
    };
    if !path.exists() {
        return Ok(i18n::tp("msg.password.missing", &[("path", &path.display().to_string())]));
    }
    let text = std::fs::read_to_string(&path)?;

    if crypto::is_encrypted(&text) {
        // Change or remove
        let Some(old) = shell.ask_password(&i18n::t("prompt.password.current"),
            &i18n::t("prompt.password.current_note"),
        )?
        else {
            return Ok(i18n::t("msg.password.cancelled"));
        };
        let env: crypto::Envelope = serde_json::from_str(&text)?;
        let plain = match crypto::decrypt(&env, &old) {
            Ok(p) => p,
            Err(e) => return Ok(format!(">> {e}")),
        };
        let Some(new) = shell.ask_password(&i18n::t("prompt.password.new"),
            &i18n::t("prompt.password.new_note"),
        )? else {
            return Ok(i18n::t("msg.password.cancelled"));
        };
        if new.is_empty() {
            crypto::write_atomic(&path, &plain)?;
            *password = None;
            return Ok(i18n::t("msg.password.removed"));
        }
        let confirm = shell.ask_password(&i18n::t("prompt.password.confirm"), "")?;
        if confirm.as_deref() != Some(new.as_str()) {
            return Ok(i18n::t("msg.password.mismatch"));
        }
        crypto::write_atomic(&path, &serde_json::to_string_pretty(&crypto::encrypt(&plain, &new)?)?)?;
        *password = Some(new);
        Ok(i18n::t("msg.password.changed"))
    } else {
        // First-time setup
        let Some(new) = shell.ask_password(&i18n::t("prompt.password.set"),
            &i18n::t("prompt.password.set_note"),
        )? else {
            return Ok(i18n::t("msg.password.cancelled"));
        };
        if new.is_empty() {
            return Ok(i18n::t("msg.password.empty"));
        }
        let confirm = shell.ask_password(&i18n::t("prompt.password.confirm"), "")?;
        if confirm.as_deref() != Some(new.as_str()) {
            return Ok(i18n::t("msg.password.mismatch"));
        }
        crypto::encrypt_file(&path, &new)?;
        *password = Some(new);
        Ok(i18n::t("msg.password.encrypted"))
    }
}

/// A gentle, optional nudge shown once at startup when secrets are stored but
/// left unencrypted. Silent when nothing is stored yet (nothing to protect) or
/// the file is already encrypted, so it only speaks up when there is a real
/// plaintext secret sitting on disk without a master password.
pub fn plaintext_secrets_warning(cfg: Option<&config::Config>) -> Option<String> {
    let path = cfg?.secrets_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    if crypto::is_encrypted(&text) {
        return None;
    }
    let has_secret = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.as_object().map(|o| !o.is_empty()))
        .unwrap_or(false);
    has_secret.then(|| i18n::t("msg.secrets.unencrypted"))
}

/// What the view becomes when somebody asks to look at screen number `n`.
#[derive(Debug, PartialEq)]
pub struct LookAt {
    pub active: usize,
    pub board_open: bool,
    pub settings_open: bool,
}

/// Looking at screen number `n`: 0 is the board, a screen over everything,
/// which leaves the tab in front where it was; 1.. are the running things,
/// which live in panes, and picking one is a deliberate exit from settings.
/// `None` for a number nothing is at.
///
/// The one answer for every way of asking -- a digit after the prefix, a row
/// pressed in the list, a notification clicked -- so none of them can come to
/// mean something the others do not
pub fn look_at(n: usize, surface_count: usize, active: usize, settings_open: bool) -> Option<LookAt> {
    match n {
        0 => Some(LookAt { active, board_open: true, settings_open }),
        n if n <= surface_count => Some(LookAt { active: n, board_open: false, settings_open: false }),
        _ => None,
    }
}

/// Converts an intent from the screen into keystrokes the loop already understands.
///
/// The window and the phone use the same page. If there were two separate places
/// doing this conversion, the same press could end up meaning different things
/// depending on which one it came from.
/// Intents that can't be converted to a keystroke (load-complete, resize, etc.) return empty.
pub fn keys_for(ev: &shikisha_shared::Ev) -> Vec<Event> {
    use shikisha_shared::Ev;
    let plain = |c: char| Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    // The prefix a person would actually press, not the one we shipped. A
    // button that went on pressing Ctrl+B after the prefix moved would be a
    // button that silently stopped working
    let prefixed = |c: char| {
        let p = crate::keys::prefix_now();
        vec![
            Event::Key(KeyEvent::new(p.code, p.mods)),
            plain(c),
        ]
    };
    match ev {
        // "I want to look at this tab" is not here. It was Ctrl+B and a digit,
        // and a digit is one character: tab 10 and on were pressed, sent,
        // and turned into nothing. It goes to the mailbox by number instead
        // (`Mailbox::selects`, `look_at`)
        //
        // The tab bar's + is prefixed so it works no matter which tab is showing
        Ev::AddTab { .. } => prefixed('t'),
        // The board's menu is a plain keystroke while looking at INDEX.
        // Adding the prefix key would mean only characters present on both sides work.
        Ev::Menu { key } => key.chars().next().map(plain).map(|k| vec![k]).unwrap_or_default(),
        // The desk-switcher button. Prefixed (Ctrl+B w) so it opens the
        // list no matter which tab is showing — a bare 'w' would be typed into
        // the visible session instead (the old Menu "w" bug: "wwww").
        Ev::OpenDesk => prefixed('w'),
        Ev::Stop => prefixed('x'),
        // The status bar's ↻. Same key a person at the window would press, so the
        // restart itself (cancel this tab's loops, kill, relaunch) lives in one place
        Ev::Restart => prefixed('r'),
        Ev::Key { text, named, ctrl, shift, alt } => {
            if let Some(n) = named {
                // The modifiers a named key was pressed with. A character
                // arrives already shifted, so this is the only place they are
                // not already in the key itself
                let mut mods = KeyModifiers::NONE;
                if *shift {
                    mods |= KeyModifiers::SHIFT;
                }
                if *alt {
                    mods |= KeyModifiers::ALT;
                }
                named_key(n)
                    .map(|code| vec![Event::Key(KeyEvent::new(code, mods))])
                    .unwrap_or_default()
            } else if let Some(c) = ctrl.as_ref().and_then(|s| s.chars().next()) {
                // Shift and Alt held with Ctrl come along: Ctrl+Shift+M is a
                // key of its own to bind, even though a program is sent the
                // same byte for it as for Ctrl+M
                let mut mods = KeyModifiers::CONTROL;
                if *shift {
                    mods |= KeyModifiers::SHIFT;
                }
                if *alt {
                    mods |= KeyModifiers::ALT;
                }
                vec![Event::Key(KeyEvent::new(KeyCode::Char(c), mods))]
            } else if let Some(t) = text {
                t.chars().map(plain).collect()
            } else {
                Vec::new()
            }
        }
        // The palette picked an action by name. Run it as the keystroke it
        // stands for, through the very path a button or a keypress takes -- so
        // a rebound key and a moved prefix are both already accounted for
        Ev::RunKey { name } => match crate::keys::char_for(name) {
            Some(c) => prefixed(c),
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Bring one line of history into view, with the cursor on it.
///
/// Put near the middle rather than at an edge: a match at the very bottom of
/// the screen shows what came before it and nothing of what came after, which
/// is half the reason for looking
pub fn show_line<CB: vt100::Callbacks>(
    p: &mut vt100::Parser<CB>,
    cs: &mut CopyState,
    d: usize,
    rows: u16,
) {
    let want = d.saturating_sub((rows / 2) as usize);
    p.screen_mut().set_scrollback(want);
    let at = p.screen().scrollback();
    cs.cursor_row = (rows as usize)
        .saturating_sub(1)
        .saturating_sub(d.saturating_sub(at))
        .min(rows.saturating_sub(1) as usize) as u16;
}

/// Put text on the clipboard of whatever is showing this, and say what happened.
///
/// A runtime with no desktop has no clipboard: the ask is dropped, and the
/// person is told so rather than left believing it worked.
pub fn copy_text(text: &str) -> String {
    match crate::clipboard() {
        Some(c) => {
            c.set_text(text.to_string());
            i18n::t("msg.copied")
        }
        None => i18n::t("err.copy_failed"),
    }
}

/// How many tabs are busy across every desk: what a shell shows a person
/// before it asks whether to quit anyway
pub fn quit_busy(tabs: &[Tab], parked: &[Vec<Tab>]) -> usize {
    let busy = |t: &Tab| t.state == TabState::Busy;
    tabs.iter().filter(|t| busy(t)).count()
        + parked.iter().flatten().filter(|t| busy(t)).count()
}

/// One recorded step → one line of the dialect every Lua surface here speaks
/// (the rally, quick actions, ▶ run mode). JSON escaping is used for the
/// strings — Lua's double-quoted literals accept everything the recorder will
/// realistically produce (\n, \t, \", \\). An XPath selector becomes the
/// `{xpath=...}` table form `sel_of` already understands; a click keeps its
/// element's text as a trailing comment so a selector broken by a site change
/// can be repaired (by a person or an AI) without re-recording.
pub fn recorded_lua(name: &str, step: &RecordedStep) -> Option<String> {
    let n = serde_json::to_string(name).ok()?;
    let s = serde_json::to_string(&step.sel).ok()?;
    let s = if step.xpath { format!("{{xpath={s}}}") } else { s };
    let hint = step.hint.replace(['\n', '\r'], " ");
    let comment = if hint.trim().is_empty() {
        String::new()
    } else {
        format!(" -- {}", hint.trim())
    };
    Some(match step.act.as_str() {
        "fill" => format!(
            "browser_fill({n}, {s}, {})",
            serde_json::to_string(&step.value).ok()?
        ),
        "click" => format!("browser_click({n}, {s}){comment}"),
        "press" => format!(
            "browser_press({n}, {})",
            serde_json::to_string(&step.value).ok()?
        ),
        // Never the typed password itself — a fill-from-secrets step to finish by hand
        "secret" => format!("browser_fill_secret({n}, {s}, \"KEY\") -- set your secrets key name"),
        _ => return None,
    })
}

#[cfg(test)]
mod remote_token_tests {
    use super::*;

    /// A sticky pairing uses the person's own string — and only a usable one
    /// (16+ chars); a short or blank string falls back to the ordinary token
    /// instead of turning the board into a guessable one
    #[test]
    fn sticky_fixed_token_wins_only_when_usable() {
        let mut cfg = config::Config::default();
        cfg.remote.sticky_token = true;
        cfg.remote.fixed_token = "  my-own-token-0123456789  ".into();
        assert_eq!(remote_token(&cfg, None), "my-own-token-0123456789");
        // Too short to be a secret: never becomes the token (start_remote_bg
        // refuses to bring the server up at all in that state)
        cfg.remote.fixed_token = "short".into();
        assert_ne!(remote_token(&cfg, None), "short");
        assert!(remote_token(&cfg, None).len() >= FIXED_TOKEN_MIN);
        cfg.remote.enabled = true;
        assert!(start_remote_bg(Some(&cfg), None)
            .and_then(|rx| rx.recv().ok())
            .is_some_and(|(ui, errs)| ui.is_none() && errs.iter().any(|e| e.contains("16"))),
            "a short fixed token does not start the remote");
        cfg.remote.enabled = false;
        // Off: the written string is ignored even if usable
        cfg.remote.sticky_token = false;
        cfg.remote.fixed_token = "my-own-token-0123456789".into();
        assert_ne!(remote_token(&cfg, None), "my-own-token-0123456789");
    }
}

#[cfg(test)]
mod survey_tests {
    use super::*;

    /// Pressing a folder moves the screen unless that folder's own tab is in
    /// front. With the Issue tab in front the folder last looked at is not the
    /// one being looked at, and pressing it has to take somebody there
    #[test]
    fn a_folder_pressed_over_a_tab_of_no_folder_goes_to_the_folder() {
        let a = std::path::Path::new("C:/work/a");
        let b = std::path::Path::new("C:/work/b");
        assert!(!folder_press_moves(Some(a), a, false), "its own tab in front moves nothing");
        assert!(folder_press_moves(Some(b), a, false), "another folder's tab in front moves");
        assert!(folder_press_moves(None, a, false), "the Issue tab in front moves to the folder");
        assert!(folder_press_moves(Some(a), a, true), "the board over it moves back to the folder");
    }

    /// The echoed command line carries BOTH markers inside one line and must
    /// never be captured; the real output block (bare marker on its own
    /// line) must be. And an echo alone (not sent yet) captures nothing
    #[test]
    fn env_block_comes_from_output_not_echo() {
        let echo_only = "D:\\run>echo ===SHIKISHA ENV=== & ver & echo ===ENV END===";
        assert!(extract_env_block(echo_only).is_none(), "an echo line alone is not captured");

        let screen = "D:\\run>echo ===SHIKISHA ENV=== & ver & echo --- tools --- & where git 2>nul & echo ===ENV END===\n\
                      ===SHIKISHA ENV=== \n\
                      \n\
                      Microsoft Windows [Version 10.0.26200]\n\
                      --- tools --- \n\
                      C:\\Program Files\\Git\\cmd\\git.exe\n\
                      ===ENV END=== \n\
                      \n\
                      D:\\run>";
        let got = extract_env_block(screen).expect("the output block is captured");
        assert!(got.contains("Microsoft Windows"), "{got}");
        assert!(got.contains("git.exe"), "{got}");
        assert!(!got.contains("where git"), "the echo line is not included: {got}");
    }

    /// A second press while the first tab is still on its way finds that tab,
    /// in that folder only
    #[test]
    fn a_tab_on_its_way_is_found_again_in_its_own_folder() {
        let here = std::env::temp_dir().join("shikisha-opened-for-here");
        let there = std::env::temp_dir().join("shikisha-opened-for-there");
        let pending = vec![PendingQuick {
            id: "resolve-conflicts".into(),
            title: "Resolve conflicts".into(),
            label: "Resolve conflicts".into(),
            cwd: here.clone(),
            text: String::new(),
            submit: true,
            until: Instant::now() + QUICK_WAIT,
        }];
        assert_eq!(
            opened_for("Resolve conflicts", &here, &[], &pending),
            Some(("Resolve conflicts".to_string(), "resolve-conflicts".to_string()))
        );
        assert_eq!(opened_for("Resolve conflicts", &there, &[], &pending), None, "another folder's merge is its own");
        assert_eq!(opened_for("Review", &here, &[], &pending), None, "another kind of work is not this one");
    }

    /// The probe picker follows argv first, then the prompt's shape
    #[test]
    fn probe_matches_the_shell() {
        assert_eq!(survey_probe("bash", ""), POSIX_PROBE);
        assert_eq!(survey_probe("powershell", ""), PS_PROBE);
        assert_eq!(survey_probe("cmd", ""), CMD_PROBE);
        // Named by a whole path, spelled the way this system spells one
        #[cfg(windows)]
        assert_eq!(survey_probe(r"C:\Windows\System32\cmd.exe", ""), CMD_PROBE);
        #[cfg(unix)]
        assert_eq!(survey_probe("/usr/bin/zsh", ""), POSIX_PROBE);
        assert_eq!(survey_probe("wsl", ""), POSIX_PROBE);
        assert_eq!(survey_probe("ssh user@host", "user@host:~$ "), POSIX_PROBE);
        assert_eq!(survey_probe("ssh user@host", "PS C:\\Users\\a> "), PS_PROBE);
        assert_eq!(survey_probe("ssh user@host", "C:\\Users\\a> "), CMD_PROBE);
    }
}

#[cfg(test)]
mod tests {
    /// The sign-in address an AI prints is whole again out of the rows it
    /// was broken into: by the terminal (a wrapped row) or by the program
    /// drawing its own screen (a row written to its last column). A row
    /// that ends short is a line's end, and the address stops at a blank
    #[test]
    fn a_sign_in_address_is_whole_again_out_of_the_rows_it_was_broken_into() {
        let cols: u16 = 20;
        let rows = vec![
            ("Use the url below:".to_string(), false),
            ("https://example.test".to_string(), false), // written to the last column
            ("/a?b=1&c=2".to_string(), true),           // wrapped by the terminal
            ("&d=3 Paste code".to_string(), false),
            ("here >".to_string(), false),
        ];
        let text = super::join_rows(&rows, cols);
        assert_eq!(text, "Use the url below:\nhttps://example.test/a?b=1&c=2&d=3 Paste code\nhere >\n");
        assert_eq!(super::web_address_in(&text), "https://example.test/a?b=1&c=2&d=3");
        assert_eq!(super::web_address_in("nothing here"), "");
        assert_eq!(super::web_address_in("see \"https://a.test/x\"."), "https://a.test/x");
    }

    use super::*;

    /// The whole of the renaming, from the AI's answer to the settings, against
    /// a real repository and a settings file of its own.
    ///
    /// The gates are [`crate::worktree::auto_rename_plan`]'s and are tested
    /// there. What is proved here is the joining: that the folder's project
    /// prefix is used, that the settings end up saying the branch the folder
    /// is really on, that the drawn name is gone so this happens once, and
    /// that a desk which said no is not renamed behind its back
    #[test]
    fn the_work_renames_a_drawn_branch_and_the_settings_follow() {
        let Some(main) = scratch_repo("glue") else { return };
        let cut = |branch: &str| {
            let plan = crate::worktree::plan(&main, branch, Some("main")).expect("it can be planned");
            crate::worktree::create(&plan).expect("it can be made");
            plan.folder
        };
        let folder = cut("mighty-gannet");
        let settings = main.join("settings.json");
        let write_settings = |at: &std::path::Path, drawn: &str| {
            std::fs::write(
                &settings,
                serde_json::json!({"desks": [{"name": "work", "folders": [{
                    "cwd": at.display().to_string(), "name": "mighty-gannet", "tabs": [],
                    "source": {"origin": "https://example.invalid/x.git", "branch": drawn,
                               "base": "origin/main", "drawn": drawn},
                }]}]})
                .to_string(),
            )
            .expect("the settings can be written");
        };
        let branch_in_settings = || {
            let cfg: config::Config =
                serde_json::from_str(&std::fs::read_to_string(&settings).expect("settings")).expect("json");
            let desk = cfg.resolve_desks().0.remove(0);
            (desk.folders[0].source.clone(), desk.folders[0].drawn.clone())
        };
        let job = |at: &std::path::Path, rename: bool| LabelJob {
            tag: "folder_label:1".into(),
            desk: "work".into(),
            folder: at.to_path_buf(),
            asks: Vec::new(),
            started: Instant::now(),
            drawn: Some("mighty-gannet".into()),
            prefix: "yourname/".into(),
            rename,
        };

        // A desk that said no keeps the drawn name, whatever the AI wrote
        write_settings(&folder, "mighty-gannet");
        rename_drawn_branch(&settings, &job(&folder, false), "login-form-crash");
        assert_eq!(crate::repo::branch_of(&folder).as_deref(), Some("mighty-gannet"));

        // And with it on: the branch, and the settings, say the work
        rename_drawn_branch(&settings, &job(&folder, true), "login-form-crash");
        assert_eq!(crate::repo::branch_of(&folder).as_deref(), Some("yourname/login-form-crash"));
        let (source, drawn) = branch_in_settings();
        assert!(
            matches!(&source, config::Source::Worktree { branch, .. } if branch == "yourname/login-form-crash"),
            "the settings still name the old branch: {source:?}"
        );
        assert_eq!(drawn, None, "it would be renamed again on the next name written");

        // The next name written is only a name: the branch is somebody's now
        rename_drawn_branch(&settings, &job(&folder, true), "something-else");
        assert_eq!(crate::repo::branch_of(&folder).as_deref(), Some("yourname/login-form-crash"));

        // An answer with no branch name in it leaves the branch alone
        let second = cut("polite-marmot");
        write_settings(&second, "polite-marmot");
        let mut asked = job(&second, true);
        asked.drawn = Some("polite-marmot".into());
        rename_drawn_branch(&settings, &asked, "   ");
        assert_eq!(crate::repo::branch_of(&second).as_deref(), Some("polite-marmot"));
    }

    /// A repository of this test's own, or nothing where git is not installed
    fn scratch_repo(tag: &str) -> Option<std::path::PathBuf> {
        let at = std::env::temp_dir()
            .join(format!("shikisha-rename-{tag}-{}", crate::random_hex(6)))
            .join("proj");
        std::fs::create_dir_all(&at).ok()?;
        let git = |args: &[&str]| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(&at).args(args);
            let _ = crate::detach_console(&mut run).output();
        };
        git(&["init", "-q", "-b", "main", "."]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "one"]);
        crate::repo::branch_of(&at).is_some().then_some(at)
    }

    /// A tab a quick command opens is named after the button, and has an
    /// automation name a script can type -- a made-up one when the button's
    /// name has no letters a name can be made of
    #[test]
    fn a_tab_a_quick_command_opens_has_names_of_its_own() {
        assert_eq!(quick_tab_names("Run tests!", "PowerShell", &[]), ("Run tests!".into(), "run-tests".into()));
        assert_eq!(quick_tab_names("一覧", "PowerShell", &[]), ("一覧".into(), "quick".into()));
        assert_eq!(quick_tab_names("  ", "Claude Code", &[]), ("Claude Code".into(), "claude-code".into()));
    }

    /// With no AI named, a prompt starts the one the tab form offers first,
    /// whatever order the profiles happen to be read in; a named one that is
    /// not on this PC starts nothing rather than something else
    #[test]
    fn a_prompt_with_no_ai_named_starts_the_first_one_offered() {
        let ais = vec![
            crate::uistate::AiChoice { key: "aider".into(), name: "Aider".into(), command: "aider".into() },
            crate::uistate::AiChoice { key: "codex".into(), name: "Codex CLI".into(), command: "codex --flag".into() },
        ];
        assert_eq!(quick_ai_choice("", &ais).map(|a| a.key.as_str()), Some("codex"));
        assert_eq!(quick_ai_choice("aider", &ais).map(|a| a.key.as_str()), Some("aider"));
        assert!(quick_ai_choice("gemini", &ais).is_none());
        assert!(quick_ai_choice("", &[]).is_none());
    }

    /// A folder on a MicroVM hands its work to the AI its machine was given,
    /// never to one only this PC has; a machine given none says so, and its
    /// terminal is the machine's own shell rather than this PC's PowerShell
    #[test]
    fn a_microvm_folder_runs_what_its_machine_was_given() {
        let far = std::path::PathBuf::from("/home/user/site");
        let here = std::env::temp_dir();
        let desk = config::Desk {
            folders: vec![
                config::Folder {
                    cwd: Some(far.clone()),
                    host: Some(config::HostSpec { name: "vm".into(), kind: Some("e2b".into()), instance: Some("m1".into()), ..Default::default() }),
                    project: Some("site".into()),
                    ..Default::default()
                },
                config::Folder { cwd: Some(here.clone()), ..Default::default() },
            ],
            projects: vec![config::ProjectSpec { name: "site".into(), machine_ai: Some("none".into()), ..Default::default() }],
            ..Default::default()
        };
        assert_eq!(machine_ai_of(Some(&desk), &far), Some(Err("msg.quick.no_machine_ai")));
        assert_eq!(machine_ai_of(Some(&desk), &here), None, "a folder here is asked about a machine");
        let ais = vec![crate::uistate::AiChoice { key: "claude".into(), name: "Claude Code".into(), command: "claude".into() }];
        assert_eq!(
            ai_for_folder(Some(&desk), &far, "", &ais),
            Err(i18n::t("msg.quick.no_machine_ai")),
            "this PC's AI was handed a MicroVM folder"
        );
        assert_eq!(ai_for_folder(Some(&desk), &here, "", &ais).map(|a| a.key), Ok("claude".to_string()));
        assert_eq!(
            quick_go(crate::quick::Kind::Terminal, "", &[], &[], 0, true, &ais, Some(&far), Some(&desk)),
            QuickGo::Open { cwd: far.clone(), command: String::new(), program: "vm".into() }
        );
    }

    /// Where a button goes with nothing open: a command opens in the home
    /// folder, a prompt goes nowhere, and neither guesses a folder
    #[test]
    fn with_no_folder_in_front_a_command_opens_at_home_and_a_prompt_waits() {
        let ais = vec![crate::uistate::AiChoice { key: "claude".into(), name: "Claude Code".into(), command: "claude".into() }];
        let dir = std::env::temp_dir();
        // No tabs at all: nothing to send to, so a new one opens -- but only
        // with a folder in front, which the board is not
        assert_eq!(
            quick_go(crate::quick::Kind::Ai, "", &[], &[], 0, true, &ais, Some(&dir), None),
            QuickGo::Refuse("msg.quick.ai_needs_folder")
        );
        assert_eq!(
            quick_go(crate::quick::Kind::Terminal, "", &[], &[], 0, true, &ais, Some(&dir), None),
            QuickGo::Open { cwd: dir.clone(), command: "powershell.exe".into(), program: "PowerShell".into() }
        );
        assert_eq!(
            quick_go(crate::quick::Kind::Terminal, "", &[], &[], 0, true, &ais, None, None),
            QuickGo::Refuse("msg.quick.no_home")
        );
        // A folder sends nothing, wherever it is pressed
        assert_eq!(
            quick_go(crate::quick::Kind::Folder, "", &[], &[], 0, false, &ais, Some(&dir), None),
            QuickGo::Refuse("msg.quick.gone")
        );
    }

    /// The file panel's own folder is a fence, and `..` is not a gate in it.
    ///
    /// The far side already keeps this promise. Without the same one here, a
    /// panel opened on one project would be a way to read -- and send -- every
    /// file on the machine
    #[test]
    #[cfg(windows)]
    fn the_panels_folder_is_as_far_as_it_goes() {
        let root = std::path::Path::new("D:/work/site");
        let under = |at: &str| local_under(root, at).map(|p| display_path_of(&p));

        assert_eq!(under("public").as_deref(), Some("D:/work/site/public"), "inside is allowed");
        assert_eq!(under("D:/work/site/public/a.txt").as_deref(),
                   Some("D:/work/site/public/a.txt"), "an absolute path is allowed if it is inside");
        assert_eq!(under("public/../a.txt").as_deref(), Some("D:/work/site/a.txt"),
                   "going out and back in is still inside");
        assert_eq!(under(""), Some("D:/work/site".to_string()), "the root itself");

        assert_eq!(under(".."), None, "one level up is outside");
        assert_eq!(under("public/../../../secrets"), None, "a roundabout way out is still outside");
        assert_eq!(under("C:/Windows"), None, "another drive is outside");
        assert_eq!(under("D:/work/site-two"), None, "a different folder that only starts with the same name");
    }

    /// The same fence, drawn where paths look like this instead.
    #[test]
    #[cfg(unix)]
    fn the_panels_folder_is_as_far_as_it_goes_on_unix() {
        let root = std::path::Path::new("/work/site");
        let under = |at: &str| local_under(root, at).map(|p| display_path_of(&p));

        assert_eq!(under("public").as_deref(), Some("/work/site/public"), "inside is allowed");
        assert_eq!(
            under("/work/site/public/a.txt").as_deref(),
            Some("/work/site/public/a.txt"),
            "an absolute path is allowed if it is inside"
        );
        assert_eq!(under("public/../a.txt").as_deref(), Some("/work/site/a.txt"), "going out and back in is still inside");
        assert_eq!(under(""), Some("/work/site".to_string()), "the root itself");

        assert_eq!(under(".."), None, "one level up is outside");
        assert_eq!(under("public/../../../secrets"), None, "a roundabout way out is still outside");
        assert_eq!(under("/etc"), None, "outside the root is outside");
        assert_eq!(under("/work/site-two"), None, "a different folder that only starts with the same name");
    }

    /// An editor in a folder on a MicroVM reads and saves that machine's
    /// files, fenced by the folder's path there -- and what can be refused is
    /// refused at once, without the machine being asked (asking starts one
    /// that is paused)
    #[test]
    fn an_editor_in_a_folder_on_a_microvm_works_there() {
        crate::i18n::init(Some("en"), &[std::path::PathBuf::from("lang")]);
        let vm = crate::elsewhere::Elsewhere::Cloud(crate::config::HostSpec {
            name: "vm".into(),
            kind: Some("e2b".into()),
            instance: Some("i-1".into()),
            ..Default::default()
        });
        let editor = |at: Option<crate::elsewhere::Elsewhere>, dir: &str| Surface::Editor {
            key: "ed".into(),
            name: "ed".into(),
            dir: Some(std::path::PathBuf::from(dir)),
            at,
        };
        let far = [editor(Some(vm.clone()), "/home/user/proj")];
        let place = files_at("ed", &far, &[]).expect("the editor has a folder");
        assert_eq!(place, FilesAt::There { at: vm.clone(), root: "/home/user/proj".into() });
        assert!(place.holds("src/main.rs"));
        assert!(!place.holds("../../etc/passwd"), "a path climbs out of the folder");

        let caps = crate::caps::Capabilities::disabled();
        let (tx, rx) = std::sync::mpsc::channel();
        for (act, args) in [
            ("read", serde_json::json!({"path": "../../../etc/passwd"})),
            ("write", serde_json::json!({"path": "/etc/passwd", "text": "x"})),
            ("ls", serde_json::json!({"at": "../.."})),
        ] {
            let said = files_answer("ed", act, &args, &far, &[], &caps, &tx)
                .unwrap_or_else(|| panic!("{act} outside the folder went to the machine"));
            let said: serde_json::Value = serde_json::from_str(&said).unwrap();
            assert_eq!(said["ok"], false, "{act}: {said}");
            assert_eq!(said["panel"], "ed");
        }
        // An empty search is answered as nothing without asking anybody
        let said = files_answer("ed", "find", &serde_json::json!({"q": " "}), &far, &[], &caps, &tx)
            .expect("an empty search asked the machine");
        assert!(said.contains("\"hits\":[]"), "{said}");
        assert!(rx.try_recv().is_err(), "nothing was sent to a thread");

        // The same editor in a folder on this machine stays on this machine
        let here = [editor(None, "D:/work/site")];
        assert!(matches!(files_at("ed", &here, &[]), Some(FilesAt::Here(_))));
    }

    /// What the phone is told when a tab finishes.
    ///
    /// The point of the feature is not being told THAT something finished --
    /// that only says "come back to the PC". It is being told enough to decide
    /// whether to, and, when it was asked for, a way to answer from where you
    /// are standing.
    #[test]
    fn a_finished_tab_says_enough_to_act_on() {
        crate::i18n::init(Some("en"), &[std::path::PathBuf::from("lang")]);
        let msg = on_done_message(
            "reviewer",
            "  Found 3 problems.\n  The first is in tab.rs.  ",
            Some("http://100.64.1.2:8787/r/K3fQ92mZxAbC"),
        );
        let lines: Vec<&str> = msg.lines().collect();
        assert!(lines[0].contains("reviewer"), "which tab: {}", lines[0]);
        // The answer itself, folded onto one line -- a notification is not a
        // place to reproduce a screen
        assert_eq!(lines[1], "Found 3 problems. The first is in tab.rs.");
        // ...then a blank line, a label, and the link, so the link is not
        // mistaken for part of what the AI said
        assert_eq!(lines[2], "");
        assert!(!lines[3].is_empty(), "there is a word before the link");
        assert_eq!(lines[4], "http://100.64.1.2:8787/r/K3fQ92mZxAbC");
        assert_eq!(lines.len(), 5);
        // The link is a ticket, never the board's key
        assert!(!msg.contains("?t="), "the token is not in it: {msg:?}");

        // Not asked for: the answer and nothing else. Not the link, and not
        // the machine's address either
        let quiet = on_done_message("builder", "done", None);
        assert_eq!(quiet.lines().count(), 2);
        assert!(!quiet.contains("http"), "the address is not given either: {quiet:?}");
        assert!(!quiet.ends_with('\n'));

        // Nothing said (a tab that finished silently): just the name
        assert_eq!(on_done_message("x", "   ", None).lines().count(), 1);

        // A long answer is cut where a person can still read it, and says so
        let long = on_done_message("x", &"あ".repeat(400), None);
        let said = long.lines().nth(1).unwrap();
        assert_eq!(said.chars().count(), 161, "160 characters plus an ellipsis");
        assert!(said.ends_with('…'));
    }

    /// The whole way through, from the message the window sends when a key is
    /// pressed to the bytes the program receives.
    ///
    /// The pieces are checked on their own above; this is the one that would
    /// catch them being connected wrongly -- a modifier dropped on the way in
    /// looks exactly like a terminal that does not support the protocol, and
    /// the program's own workaround hides it.
    #[test]
    fn a_shifted_return_arrives_shifted_all_the_way_to_the_program() {
        let pressed = |named: &str, shift: bool| {
            let ev = shikisha_shared::Ev::Key {
                text: None,
                named: Some(named.into()),
                ctrl: None,
                shift,
                alt: false,
            };
            match keys_for(&ev).first() {
                Some(Event::Key(k)) => Some(*k),
                _ => None,
            }
        };

        // A letter held with Ctrl keeps its Shift too, so Ctrl+Shift+M can be
        // told from Ctrl+M when a key is bound to it
        let chord = keys_for(&shikisha_shared::Ev::Key {
            text: None, named: None, ctrl: Some("m".into()), shift: true, alt: false,
        });
        match chord.first() {
            Some(Event::Key(k)) => assert_eq!(k.modifiers, KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            other => panic!("Ctrl+Shift+M did not arrive: {other:?}"),
        }

        let plain = pressed("enter", false).expect("Enter did not arrive");
        let shifted = pressed("enter", true).expect("Shift+Enter did not arrive");
        assert!(!plain.modifiers.contains(KeyModifiers::SHIFT));
        assert!(
            shifted.modifiers.contains(KeyModifiers::SHIFT),
            "a modifier the window sent was dropped on the way"
        );

        // Without a program asking, both are a Return, exactly as before
        assert_eq!(key_to_bytes_with(&plain, 0), Some(b"\r".to_vec()));
        assert_eq!(key_to_bytes_with(&shifted, 0), Some(b"\r".to_vec()));
        // With one asking, they are finally two different keys
        assert_eq!(key_to_bytes_with(&plain, 1), Some(b"\r".to_vec()));
        assert_eq!(key_to_bytes_with(&shifted, 1), Some(b"\x1b[13;2u".to_vec()));
    }

    /// Shift+Enter, which every AI CLI wants and no ordinary terminal can
    /// spell.
    ///
    /// Enter is one byte and Shift+Enter is the same byte, so "send this" and
    /// "start a new line" arrive as the same keystroke. The newer keyboard
    /// exists for exactly this, and a program only gets it after asking -- so
    /// the first half of this test is the one that matters most: with nobody
    /// asking, every key is spelled exactly as it was before.
    #[test]
    fn a_program_that_asked_can_tell_shift_enter_from_enter() {
        let k = |code, mods| KeyEvent::new(code, mods);
        let bytes = |key: &KeyEvent, flags| key_to_bytes_with(key, flags);

        // Nobody asked: every one of these is what it always was
        for (code, mods) in [
            (KeyCode::Enter, KeyModifiers::SHIFT),
            (KeyCode::Enter, KeyModifiers::NONE),
            (KeyCode::Tab, KeyModifiers::SHIFT),
            (KeyCode::Esc, KeyModifiers::CONTROL),
        ] {
            let key = k(code, mods);
            assert_eq!(
                bytes(&key, 0),
                key_to_bytes(&key),
                "the spelling changed though nobody asked for it: {code:?} {mods:?}"
            );
        }
        assert_eq!(bytes(&k(KeyCode::Enter, KeyModifiers::SHIFT), 0), Some(b"\r".to_vec()));

        // Asked for: the four keys that had no way to be told apart
        assert_eq!(
            bytes(&k(KeyCode::Enter, KeyModifiers::SHIFT), 1),
            Some(b"\x1b[13;2u".to_vec()),
            "Shift+Enter is still a plain Enter"
        );
        assert_eq!(
            bytes(&k(KeyCode::Enter, KeyModifiers::CONTROL), 1),
            Some(b"\x1b[13;5u".to_vec())
        );
        assert_eq!(
            bytes(&k(KeyCode::Backspace, KeyModifiers::ALT), 1),
            Some(b"\x1b[127;3u".to_vec())
        );
        assert_eq!(
            bytes(&k(KeyCode::BackTab, KeyModifiers::NONE), 1),
            Some(b"\x1b[9;2u".to_vec()),
            "Shift+Tab carries its modifier in its name from the moment it is pressed"
        );

        // ...and everything else keeps the spelling it had, asked for or not.
        // A program that wanted the whole protocol was told this terminal only
        // does this much, so it is not waiting for the rest
        assert_eq!(bytes(&k(KeyCode::Enter, KeyModifiers::NONE), 1), Some(b"\r".to_vec()));
        assert_eq!(bytes(&k(KeyCode::Tab, KeyModifiers::NONE), 1), Some(b"\t".to_vec()));
        assert_eq!(
            bytes(&k(KeyCode::Char('c'), KeyModifiers::CONTROL), 1),
            Some(vec![0x03])
        );
        assert_eq!(bytes(&k(KeyCode::Up, KeyModifiers::NONE), 1), Some(b"\x1b[A".to_vec()));
    }

    /// Being told again that a tab is still working -- the whole of the rule,
    /// which is the part worth pinning down.
    ///
    /// A tab that has been working for twenty minutes without a word is either
    /// thinking or hung, and nothing in this app can tell those apart. The
    /// automation that asked for the work can, so it is asked again -- and the
    /// three guards are what keep that from becoming a nuisance: only tabs it
    /// was told about in the first place, only while the work is still running,
    /// and never before the interval is up.
    #[test]
    fn a_tab_that_keeps_working_is_mentioned_again_but_only_on_those_terms() {
        use std::collections::HashMap;
        let every = 300_000; // five minutes
        let busy = vec![TabState::Busy, TabState::Busy, TabState::Done];
        let mut tracked: HashMap<usize, u64> = HashMap::new();

        // Tab 1 is the only one automation was told about
        tracked.insert(1, 300_000);
        assert!(
            busy_repeat_due(299_000, every, &busy, &mut tracked).is_empty(),
            "it called before the time"
        );
        assert_eq!(
            busy_repeat_due(300_000, every, &busy, &mut tracked),
            vec![1],
            "it did not call when the time came"
        );
        assert_eq!(tracked.get(&1), Some(&600_000), "it did not set the next time");
        assert!(
            busy_repeat_due(300_001, every, &busy, &mut tracked).is_empty(),
            "it called twice in a row"
        );

        // Tab 2 is working too, but automation was never told about it: it is
        // not this app's place to start
        assert!(!tracked.contains_key(&2), "it counts a tab nobody asked about");

        // The work ends, and the asking stops with it -- including for a tab
        // that has gone to waiting on a person
        let answered = vec![TabState::Question, TabState::Busy, TabState::Done];
        assert!(
            busy_repeat_due(900_000, every, &answered, &mut tracked).is_empty(),
            "it keeps calling about a tab that is waiting for a person"
        );
        assert!(tracked.is_empty(), "a finished tab's schedule is still there");

        // A tab that disappeared takes its place in the queue with it
        tracked.insert(9, 0);
        assert!(
            busy_repeat_due(1_000_000, every, &busy, &mut tracked).is_empty(),
            "it calls about a tab that no longer exists"
        );
    }

    /// A phone that is watching decides the shape of the terminal, and the
    /// window takes it back the moment nobody is.
    ///
    /// Both viewers re-report their own measurement as they redraw, so the
    /// answer must not depend on which of them spoke last: the window redraws
    /// its pane tree on every tab switch and re-reported there, which used to
    /// snatch the terminal back to the window's width a frame after a phone had
    /// fitted it to its screen.
    #[test]
    fn a_watching_phone_decides_the_shape_of_the_terminal() {
        let window = (40, 118);
        let phone = Some((44, 45));
        assert_eq!(
            terminal_size(window, phone, true),
            Size { width: 45, height: 44 },
            "the terminal does not fit the size of the phone that is watching"
        );
        // Nobody watching from afar: the window wears its own measurement again,
        // without waiting for anyone to resize anything
        assert_eq!(
            terminal_size(window, phone, false),
            Size { width: 118, height: 40 },
            "the terminal keeps the phone's size though nobody is watching"
        );
        // A phone that has connected but not yet measured itself decides nothing
        assert_eq!(
            terminal_size(window, None, true),
            Size { width: 118, height: 40 },
            "a phone that did not report its size decided the terminal's size"
        );
    }

    /// The tab in front is sized by whoever is watching it, panes behind it by
    /// the window.
    ///
    /// A phone is never sent the division — a small screen has no room to be
    /// divided — so it reports the one screen it has. That report lands in the
    /// same `(rows, cols)` the window writes for its focused pane, and it has to
    /// reach the terminal. Reading the window's own measurement for the front
    /// pane instead left the tab being watched wearing the window's shape: too
    /// wide for the phone, so half of it hung off the right edge, and short of
    /// its foot, leaving a dead band underneath.
    #[test]
    fn the_tab_in_front_is_sized_by_whoever_is_watching_it() {
        use shikisha_shared::PaneGeom;
        let mut layout = crate::layout::Layout::single(1);
        let front = layout.split(crate::layout::Dir::Row, 2);
        let back = layout
            .leaves()
            .into_iter()
            .find(|(id, _)| *id != front)
            .expect("there is only one pane after splitting")
            .0;
        let surfaces = vec![Surface::Session(0), Surface::Session(1)];
        let geom = vec![
            PaneGeom { id: back, rows: 50, cols: 200, rect: (0, 0, 800, 900) },
            // What the window measured for the pane in front. The phone is
            // looking at that same tab through a screen a fraction of the size
            PaneGeom { id: front, rows: 50, cols: 100, rect: (800, 0, 800, 900) },
        ];
        let want = tab_sizes(2, &layout, &surfaces, &geom, (24, 40));
        assert_eq!(want[1], (24, 40), "the terminal does not fit the screen of the person looking at it");
        assert_eq!(want[0], (50, 200), "the pane behind lost the window's measurement");
        // Undivided — every phone's case, and the window's most of the time —
        // the one pane there is takes the reported size whole
        let alone = crate::layout::Layout::single(1);
        assert_eq!(
            tab_sizes(1, &alone, &surfaces, &[], (24, 40))[0],
            (24, 40),
            "with no split, the reported size is not used"
        );
    }

    fn step(act: &str, sel: &str, value: &str, xpath: bool, hint: &str) -> RecordedStep {
        RecordedStep {
            child: "0/web".into(),
            act: act.into(),
            sel: sel.into(),
            value: value.into(),
            xpath,
            hint: hint.into(),
        }
    }

    /// Row *n* of the folders is screen *n*, whatever is standing there.
    ///
    /// The engine is handed this list, and a call arriving from a tab is
    /// credited its position in it -- the number every report that call makes
    /// is then written down under. Handing over the tabs with the panels
    /// bolted on the end made the two agree only while every screen was a tab.
    #[test]
    fn the_folders_are_in_the_order_of_the_screens() {
        let opts = tab::TabOptions { cwd: Some(std::env::temp_dir()), ..Default::default() };
        let mut tabs = vec![
            Tab::spawn("hippo".into(), &[crate::test_shell()], None, 10, 40, opts.clone()).unwrap(),
            Tab::spawn("raven".into(), &[crate::test_shell()], None, 10, 40, opts).unwrap(),
        ];
        tabs[0].id = Some("hippo".into());
        tabs[1].id = Some("raven".into());
        // A page sitting in front of both of them, as the settings put it
        let surfaces = vec![
            Surface::Browser { key: "swift".into(), name: "検索".into(), dir: None },
            Surface::Session(0),
            Surface::Session(1),
        ];
        let places = places_by_surface(&surfaces, &tabs);
        let keys: Vec<_> = places.iter().map(|p| p.key.id.clone()).collect();
        assert_eq!(
            keys,
            surface_keys(&surfaces, &tabs).iter().map(|k| k.id.clone()).collect::<Vec<_>>(),
            "the folders are not in the order of the screens"
        );
        assert_eq!(
            hooks::TabRef::Name("raven".into()).resolve(
                &places.iter().map(|p| p.key.clone()).collect::<Vec<_>>()
            ),
            Some(3),
            "a tab's own calls arrive under somebody else's number"
        );
        // The page has no folder of its own, and still holds its place
        assert_eq!(places[0].dir, std::path::PathBuf::new());
        assert_eq!(places[2].dir, std::env::temp_dir());
        for t in &mut tabs {
            t.kill();
        }
    }

    /// The way back is asked for by the number the screen drew the tab under,
    /// and a page in front of the tabs must not move it onto another tab.
    #[test]
    fn the_way_back_answers_for_the_tab_that_was_pressed() {
        let opts = tab::TabOptions { cwd: Some(std::env::temp_dir()), ..Default::default() };
        let mut tabs = vec![
            Tab::spawn("hippo".into(), &[crate::test_shell()], None, 10, 40, opts.clone()).unwrap(),
            Tab::spawn("raven".into(), &[crate::test_shell()], None, 10, 40, opts).unwrap(),
        ];
        // A page and the settings in front, as they were on 2026-09-23 when
        // the list opened empty: raven is screen 4, and the tab list has two
        let surfaces = vec![
            Surface::Browser { key: "shrimp".into(), name: "検索".into(), dir: None },
            Surface::Browser { key: "settings".into(), name: "settings".into(), dir: None },
            Surface::Session(0),
            Surface::Session(1),
        ];
        let (tx, _rx) = std::sync::mpsc::channel();
        let asked = |n| past_of(&surfaces, &tabs, n, &tx).map(|p| (p.tab, p.name));
        assert_eq!(asked(3), Some((3, "hippo".into())), "screen 3 is hippo");
        assert_eq!(asked(4), Some((4, "raven".into())), "raven's list never came back");
        // A page has no conversation to go back to
        assert_eq!(asked(1), None);
        // What the choice relaunches is found the same way
        assert_eq!(session_at(&surfaces, 4), Some(1));
        for t in &mut tabs {
            t.kill();
        }
    }

    /// A viewer from afar is told the division once, each other pane's picture
    /// when it changes, and all of it again when it has only just arrived.
    #[test]
    fn a_viewer_from_afar_is_told_about_the_panes() {
        let opts = tab::TabOptions { cwd: Some(std::env::temp_dir()), ..Default::default() };
        let mut tabs = vec![
            Tab::spawn("a".into(), &[crate::test_shell()], None, 10, 40, opts.clone()).unwrap(),
            Tab::spawn("b".into(), &[crate::test_shell()], None, 10, 40, opts).unwrap(),
        ];
        let surfaces = vec![Surface::Session(0), Surface::Session(1)];
        let mut layout = crate::layout::Layout::single(1);
        let mut relay = PaneRelay::default();
        // A prompt still arriving is a picture that really changed, and is
        // rightly sent again; wait for both shells to go quiet so "nothing
        // changed" below means it
        let (start, mut quiet, mut last) = (Instant::now(), Instant::now(), 0u64);
        while start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(100));
            let n: u64 = tabs.iter().map(|t| t.output_count()).sum();
            if n != last {
                (last, quiet) = (n, Instant::now());
            } else if quiet.elapsed() > Duration::from_millis(800) {
                break;
            }
        }

        // Undivided: nothing to picture. The division itself is not sent
        // from here at all -- it travels with the state, because it is the
        // same moment as what is in it (`view::PanesState`)
        let first = relay.changes(&layout, &surfaces, &tabs);
        assert!(first.is_empty(), "the division is being sent twice again: {first:?}");

        // Divided: a picture of the pane left behind, and only that
        layout.split(crate::layout::Dir::Row, 2);
        let split = relay.changes(&layout, &surfaces, &tabs);
        assert_eq!(split.len(), 1, "{split:?}");
        assert!(split[0].contains("\"panescreen\""), "{split:?}");
        assert!(relay.changes(&layout, &surfaces, &tabs).is_empty(), "it sends the same picture again");

        // A viewer who just arrived is told the pictures it has not seen
        let seed = relay.seed();
        assert_eq!(seed.len(), 1, "{seed:?}");
        assert!(seed[0].contains("\"panescreen\""), "{seed:?}");
        for t in tabs.iter_mut() {
            t.kill();
        }
    }

    /// A shell has no conversation, so restarting one says nothing about
    /// conversations.
    #[test]
    fn a_restarted_shell_is_not_told_it_lost_a_conversation() {
        let opts = tab::TabOptions { cwd: Some(std::env::temp_dir()), ..Default::default() };
        let mut shell = Tab::spawn("sh".into(), &[crate::test_shell()], None, 10, 40, opts).unwrap();
        assert!(!shell.is_ai());
        let (plan, why) = resume_plan(&shell, true, true);
        assert_eq!(plan, tab::Resume::Fresh);
        assert_eq!(why, None, "it tells the shell the conversation cannot be carried over");
        shell.kill();
    }

    /// Two tabs running the same CLI in the same folder cannot both claim
    /// "the newest conversation here" — and a wrong guess would hand one of
    /// them the other's conversation, which is worse than starting a new one.
    #[test]
    fn a_guess_is_refused_when_another_tab_could_be_the_one() {
        let opts = tab::TabOptions {
            cwd: Some(std::env::temp_dir()),
            ..Default::default()
        };
        let argv = vec![crate::test_shell()];
        let mut tabs = vec![
            Tab::spawn("A".into(), &argv, None, 10, 40, opts.clone()).unwrap(),
            Tab::spawn("B".into(), &argv, None, 10, 40, opts).unwrap(),
        ];
        // A CLI that can only be told "continue the newest one here"
        let only_newest = crate::profile::ResumeSpec {
            newest_here: vec!["--continue".into()],
            ..Default::default()
        };
        tabs[0].resume = Some(only_newest.clone());
        assert!(!only_one_here(&tabs, 0), "there is a partner with the same CLI in the same folder");
        let (plan, why) = resume_plan(&tabs[0], only_one_here(&tabs, 0), true);
        assert_eq!(plan, tab::Resume::Fresh);
        assert_eq!(why, Some("msg.resume.ambiguous"), "it starts fresh and says why");

        // Alone, the same tab may continue what ran here last
        let (plan, why) = resume_plan(&tabs[0], true, true);
        assert_eq!(plan, tab::Resume::NewestHere);
        assert_eq!(why, None);

        // ...and knowing WHICH conversation it was settles it either way:
        // this is why an id is worth minting at launch
        tabs[0].resume = Some(crate::profile::ResumeSpec {
            with_id: vec!["--resume".into(), "{id}".into()],
            ..only_newest
        });
        let mine = tab::Session {
            id: "1234".into(),
            source: tab::SessionSource::Minted,
        };
        tabs[0].session = Some(mine.clone());
        let (plan, why) = resume_plan(&tabs[0], false, true);
        assert_eq!(plan, tab::Resume::Id(mine), "a partner is there, but there is no way to mix them up");
        assert_eq!(why, None);

        // Asking for a clean start is never argued with
        assert_eq!(resume_plan(&tabs[0], true, false).0, tab::Resume::Fresh);
        for t in tabs.iter_mut() {
            t.kill();
        }
    }

    /// A recorded step must come out as ONE line of the shared Lua dialect,
    /// runnable by run_scoped as-is (record → paste → run must round-trip).
    #[test]
    fn recorded_steps_become_runnable_lua_lines() {
        assert_eq!(
            recorded_lua("web", &step("fill", "#q", "hello", false, "")).as_deref(),
            Some(r##"browser_fill("web", "#q", "hello")"##)
        );
        assert_eq!(
            recorded_lua("web", &step("click", "#go", "", false, "")).as_deref(),
            Some(r##"browser_click("web", "#go")"##)
        );
        assert_eq!(
            recorded_lua("web", &step("press", "", "enter", false, "")).as_deref(),
            Some(r##"browser_press("web", "enter")"##)
        );
        // A typed password never lands in the line — only a secrets-store stub
        let secret = recorded_lua("web", &step("secret", "#pw", "hunter2", false, "")).unwrap();
        assert!(!secret.contains("hunter2"), "password leaked: {secret}");
        assert!(secret.contains("browser_fill_secret"));
        // Unknown acts are dropped, not guessed at
        assert_eq!(recorded_lua("web", &step("hover", "#x", "", false, "")), None);
        // Quotes and newlines in values survive as valid Lua escapes
        assert_eq!(
            recorded_lua("web", &step("fill", "#q", "a\"b\nc", false, "")).as_deref(),
            Some("browser_fill(\"web\", \"#q\", \"a\\\"b\\nc\")")
        );
        // A text-anchored click becomes the {xpath=...} table form
        assert_eq!(
            recorded_lua(
                "web",
                &step("click", r##"//a[normalize-space(.)="Sign in"]"##, "", true, "")
            )
            .as_deref(),
            Some(
                r##"browser_click("web", {xpath="//a[normalize-space(.)=\"Sign in\"]"})"##
            )
        );
        // A positional click carries its element's text as a repair hint,
        // flattened to one line so the comment can't swallow the next step
        assert_eq!(
            recorded_lua("web", &step("click", "div:nth-of-type(11) > a", "", false, "俳句\nとは")).as_deref(),
            Some(r##"browser_click("web", "div:nth-of-type(11) > a") -- 俳句 とは"##)
        );
    }

    /// The recorded dialect must actually run in the sandbox it claims to
    /// round-trip into (bare browser_* names, that browser only).
    #[test]
    fn recorded_lines_parse_in_the_run_sandbox_dialect() {
        for s in [
            step("fill", "#q", "あいうえお", false, ""),
            step("click", r##"//a[normalize-space(.)="次へ \"仮\""]"##, "", true, ""),
            step("click", "div:nth-of-type(3) > a", "", false, "リンクの見出し"),
        ] {
            let line = recorded_lua("web", &s).unwrap();
            assert!(
                hooks::lint_lua(&line).is_none(),
                "recorded line does not compile: {line}"
            );
        }
    }

    fn parser_with_lines(rows: u16, cols: u16, n: usize) -> vt100::Parser {
        let mut p = vt100::Parser::new(rows, cols, 100);
        for i in 1..=n {
            p.process(format!("line{i}\r\n").as_bytes());
        }
        p
    }

    /// Settings, as a test writes them. `<sh>` stands for "something that
    /// holds a terminal open" and becomes whatever this system calls that
    fn desk_from_json(json: &str) -> config::Desk {
        let json = json.replace("<sh>", &crate::test_shell());
        let cfg: config::Config = serde_json::from_str(&json).unwrap();
        cfg.resolve_desks().0.into_iter().next().unwrap()
    }


    /// A conversation comes back with the app, or it plainly does not.
    ///
    /// This is the decision that used to have only one answer. Every tab was
    /// launched fresh at startup, the remembered id was read afterwards and
    /// only ever handed over by Ctrl+B r, and nothing on screen said so — you
    /// closed the app in the middle of a job, opened it again, and were looking
    /// at an empty prompt where a conversation had been.
    ///
    /// Every way of NOT carrying one is tested here, because each of them is
    /// silent by design: a tab starting fresh is what a tab normally does.
    #[test]
    fn a_tab_is_launched_back_into_what_it_was_saying() {
        let desk = desk_from_json(
            r#"{"desks":[{"name":"W","folders":[{"tabs":[{"name":"AGENT","command":"claude"}]}]}]}"#,
        );
        let cfg = &desk.tabs[0].cfg;
        let argv = vec!["claude".to_string()];
        let here = Some(std::path::PathBuf::from("D:\\Work"));
        let remembered = |program: &str, session: &str| crate::lastsession::Saved {
            version: 1,
            desks: vec![crate::lastsession::SavedWs {
                name: "W".into(),
                id: None,
                panes: None,
                tabs: vec![crate::lastsession::SavedTab {
                    title: "AGENT".into(),
                    id: None,
                    cwd: Some("D:\\Work".into()),
                    program: program.into(),
                    session: session.into(),
                    source: "Minted".into(),
                }],
            }],
        };
        let plan = |saved: &crate::lastsession::Saved| {
            carried_conversation(Some(saved), &desk, &argv, cfg, &here, "AGENT", false)
        };

        // This tab was told to start clean, so nothing is carried however well
        // it is remembered
        let known = remembered("claude", "11111111-1111-4111-8111-111111111111");
        let mut off = cfg.clone();
        off.restore_conversation = Some(false);
        let told = carried_conversation(Some(&known), &desk, &argv, &off, &here, "AGENT", false);
        assert_eq!(
            told.plan,
            tab::Resume::Fresh,
            "it carries the conversation over even with the setting off"
        );
        assert!(!told.lost, "a tab told to start clean has lost nothing");

        // A conversation that is no longer on this machine. Handing the CLI an
        // id it has never heard of makes it refuse to start, in red, in its own
        // words -- which is not an answer to "I reopened the app"
        let gone = plan(&known);
        assert_eq!(gone.plan, tab::Resume::Fresh, "it hands over a conversation that is gone");
        assert!(gone.lost, "a tab whose conversation is gone comes up saying nothing about it");

        // Remembered under another program: the same name a year later can be
        // a different CLI, and resuming a conversation into one is nonsense
        let other = plan(&remembered("codex", "11111111-1111-4111-8111-111111111111"));
        assert_eq!(other.plan, tab::Resume::Fresh, "it hands over another CLI's conversation");
        assert!(!other.lost, "another CLI's conversation was never this tab's to lose");

        // A CLI with no way of being told which conversation to resume. Gemini
        // can be handed a new id and can be told "the latest", but not "that
        // one" -- and "the latest" is a guess, not this tab's conversation
        let gemini = vec!["gemini".to_string()];
        assert_eq!(
            carried_conversation(
                Some(&remembered("gemini", "11111111-1111-4111-8111-111111111111")),
                &desk,
                &gemini,
                cfg,
                &here,
                "AGENT",
                false,
            )
            .plan,
            tab::Resume::Fresh,
            "it hands a conversation to a CLI that cannot be told one"
        );

        // Nothing remembered at all -- a tab that is new since last time
        let empty = crate::lastsession::Saved { version: 1, desks: Vec::new() };
        let fresh = plan(&empty);
        assert_eq!(fresh.plan, tab::Resume::Fresh);
        assert!(!fresh.lost, "a tab that is new since last time has lost nothing");
    }

    /// The Vault's choice outranks what the tab was saying last time.
    ///
    /// Reopening a past conversation names the one to resume, deliberately, a
    /// moment ago. "What this tab happened to be running when the app closed"
    /// is not an answer to that, and quietly preferring it would make the Vault
    /// open the wrong conversation.
    #[test]
    fn a_reopened_conversation_outranks_the_remembered_one() {
        let desk = desk_from_json(
            r#"{"desks":[{"name":"W","folders":[{"tabs":[
                {"name":"AGENT","command":"claude","resume":"picked-from-the-vault"}
            ]}]}]}"#,
        );
        assert_eq!(
            resume_plan_of(desk.tabs[0].cfg.resume.as_deref()),
            tab::Resume::Id(tab::Session {
                id: "picked-from-the-vault".into(),
                source: tab::SessionSource::Store,
            })
        );
    }

    /// Every menu key the board displays must be received by INDEX.
    ///
    /// Showing it with no receiver means nothing happens when it's pressed.
    /// No crash, no warning — only the person who pressed it would ever notice.
    ///
    /// This actually happened with `e` (settings), `i` (QR), `t` (notify).
    /// They were being sent with the prefix key, so only `?`, `w`, `r` — the
    /// characters that happened to also exist on the prefix-key side — worked,
    /// which made the cause hard to see since it was only half broken.
    #[test]
    fn every_key_the_board_offers_is_answered_on_index() {
        let src = include_str!("runtime.rs");
        // Slice out just the INDEX branch
        let head = "// INDEX = home screen";
        let from = src.find(head).expect("Couldn't find the INDEX branch");
        // The end of the branch has a marker planted.
        // Cutting by character count falls short, and searching for braces hits nested ones along the way.
        let len = src[from..]
            .find("INDEX-END")
            .expect("Missing the end-of-INDEX-branch marker");
        let body = &src[from..from + len];

        for (key, _) in crate::shell::MENU {
            let want = format!("KeyCode::Char('{key}')");
            assert!(
                body.contains(&want),
                "the board sends {key}, but INDEX has nothing to receive it"
            );
        }
    }

    /// The tab bar's + must arrive with the prefix key attached, so it works no matter which tab is being viewed
    /// Two pointers, each shown until its step is over, never again after.
    #[test]
    fn the_first_run_pointer_moves_on_and_never_comes_back() {
        // Nothing yet: point at "add a folder", and remember having done so
        assert_eq!(super::coach_step(0, 0, false), (Some(1), 1));
        // ...and it stays up on the next frame, once "shown" is written down
        assert_eq!(super::coach_step(0, 1, false), (Some(1), 1), "step 1 disappears on the next frame");
        // One folder, nothing started in it: point at its +
        assert_eq!(super::coach_step(1, 1, false), (Some(2), 1));
        // An AI (or a branch) appeared: over, for good
        assert_eq!(super::coach_step(1, 1, true), (None, 2));
        assert_eq!(super::coach_step(1, 2, false), (None, 2), "a closed step came back");
        // Two folders at once: the second pointer is skipped, not shown later
        assert_eq!(super::coach_step(2, 1, false), (None, 1));
        // Somebody from before the pointer existed is not pointed at anything
        assert_eq!(super::coach_step(3, 0, false), (None, 0));
        // Folders all removed later: not a first run any more
        assert_eq!(super::coach_step(0, 2, false), (None, 2));
    }

    /// The first-start setup is asked on a first start, once. A machine with
    /// settings is never asked, and a first start after it was answered -- no
    /// folder added yet, so still a first start by the settings -- is not
    /// asked again.
    #[test]
    fn the_first_start_setup_is_asked_once_and_only_on_a_first_start() {
        assert!(super::setup_wanted(true, false), "a first start is not asked");
        assert!(!super::setup_wanted(true, true), "answered, and asked again");
        assert!(!super::setup_wanted(false, false), "somebody with settings is asked");
    }

    /// "Refresh" says what it found about the page it was pressed on
    #[test]
    fn the_setups_refresh_speaks_about_its_own_page() {
        let ai = |id: &str, name: &str| crate::uistate::SetupAi { id: id.into(), name: name.into(), install: true };
        let with = crate::uistate::SetupState { installed: vec![ai("claude", "Claude Code")], missing: vec![], gh: false };
        assert_eq!(super::setup_found(&with, 1), crate::i18n::tp("msg.setup.found", &[("names", "Claude Code")]));
        assert_eq!(super::setup_found(&with, 2), crate::i18n::t("msg.setup.gh.found_none"), "page 2 talks about the AIs");
        let gh = crate::uistate::SetupState { gh: true, ..Default::default() };
        assert_eq!(super::setup_found(&gh, 2), crate::i18n::t("msg.setup.gh.found"));
        assert_eq!(super::setup_found(&gh, 1), crate::i18n::t("msg.setup.found_none"), "page 1 talks about GitHub");
    }

    /// The add-a-tab dialog sits where the style guide puts a dialog: at most
    /// 560 wide, centred, 56 down -- and a window too small to leave any board
    /// around it gives the dialog all of itself rather than a corner.
    #[test]
    fn the_add_a_tab_dialog_sits_where_a_dialog_does() {
        use super::dialog_rect;
        // A roomy window, content area starting under a 30px bar
        let (x, y, w, h) = dialog_rect((0, 30, 1400, 900));
        assert_eq!((w, h), (560, 640));
        assert_eq!(x, (1400 - 560) / 2, "not centred across");
        assert_eq!(y, 30 + 56, "not 56 down from the top of the content area");
        // Narrower than a dialog: as wide as it can be with an edge each side
        let (_, _, w, _) = dialog_rect((0, 0, 500, 900));
        assert_eq!(w, 500 - 32);
        // Shorter than a dialog: it stops above the bottom edge
        let (_, y, _, h) = dialog_rect((0, 0, 1400, 600));
        assert_eq!(y + h, 600 - 16, "it runs off the bottom");
        // Too small to float over anything: the whole area
        assert_eq!(dialog_rect((0, 0, 200, 900)), (0, 0, 200, 900));
        assert_eq!(dialog_rect((0, 0, 1400, 300)), (0, 0, 1400, 300));
        // Nothing measured yet stays nothing
        assert_eq!(dialog_rect((0, 0, 0, 0)), (0, 0, 0, 0));
    }

    /// One thing's settings stand in the same place, one size up -- and the
    /// settings themselves stand nowhere: they are given the window.
    #[test]
    fn a_sheet_is_a_dialog_one_size_up() {
        use super::{SettingsPlace, sheet_rect};
        let (x, y, w, h) = sheet_rect((0, 30, 1400, 900));
        assert_eq!((w, h), (1040, 760));
        assert_eq!(x, (1400 - 1040) / 2, "not centred across");
        assert_eq!(y, 30 + 56, "not 56 down, where a dialog starts");
        // A window with no room around it hands the whole of itself over
        assert_eq!(sheet_rect((0, 0, 500, 900)), (0, 0, 500, 900));
        assert_eq!(sheet_rect((0, 0, 1400, 400)), (0, 0, 1400, 400));
        // ...and a dialog still floats there, being the smaller of the two
        assert_ne!(dialog_rect((0, 0, 500, 900)), (0, 0, 500, 900));
        // Each place puts the page where it says
        let full = (0, 30, 1400, 900);
        assert_eq!(SettingsPlace::Full.rect(full), full);
        assert_eq!(SettingsPlace::Sheet.rect(full), sheet_rect(full));
        assert_eq!(SettingsPlace::Dialog.rect(full), dialog_rect(full));
        // ...and only the settings themselves cover the board
        assert!(!SettingsPlace::Full.floats());
        assert!(SettingsPlace::Sheet.floats() && SettingsPlace::Dialog.floats());
    }

    /// A tab past the ninth can be picked. Sent as Ctrl+B and a digit, tab 10
    /// and on were pressed in the list and turned into nothing, and with a few
    /// folders on a desk the tenth tab is not unusual
    #[test]
    fn a_tab_past_the_ninth_can_be_looked_at() {
        use super::{look_at, LookAt};
        assert_eq!(
            look_at(12, 12, 1, true),
            Some(LookAt { active: 12, board_open: false, settings_open: false })
        );
        assert_eq!(look_at(13, 12, 1, false), None, "it moved to a tab that does not exist");
        assert_eq!(
            look_at(0, 12, 4, true),
            Some(LookAt { active: 4, board_open: true, settings_open: true }),
            "opening the board forgot the previous tab and the settings"
        );
        // And nothing turns it into keystrokes any more, where it was lost
        assert!(super::keys_for(&shikisha_shared::Ev::Select { tab: 3 }).is_empty());
    }

    #[test]
    fn the_add_tab_button_arrives_prefixed() {
        let evs = super::keys_for(&shikisha_shared::Ev::AddTab { pane: None, folder: None });
        assert_eq!(evs.len(), 2, "two keystrokes: the prefix and the key");
        let Event::Key(k) = &evs[0] else { panic!("the prefix is not a keystroke") };
        assert_eq!(k.code, KeyCode::Char('b'));
        assert!(k.modifiers.contains(KeyModifiers::CONTROL));
        let Event::Key(k) = &evs[1] else { panic!("the key is not a keystroke") };
        assert_eq!(k.code, KeyCode::Char('t'));
        assert!(k.modifiers.is_empty());
    }

    /// The desk-switcher button must arrive prefixed (Ctrl+B w) so it opens
    /// the list from any tab. The old Menu "w" path was a plain 'w', which just
    /// got typed into whatever session was showing ("wwww") instead of opening.
    #[test]
    fn the_desk_button_arrives_prefixed() {
        let evs = super::keys_for(&shikisha_shared::Ev::OpenDesk);
        assert_eq!(evs.len(), 2, "two keystrokes: the prefix and 'w'");
        let Event::Key(k) = &evs[0] else { panic!("the prefix is not a keystroke") };
        assert_eq!(k.code, KeyCode::Char('b'));
        assert!(k.modifiers.contains(KeyModifiers::CONTROL));
        let Event::Key(k) = &evs[1] else { panic!("the key is not a keystroke") };
        assert_eq!(k.code, KeyCode::Char('w'));
        assert!(k.modifiers.is_empty());
    }

    /// What gets handed out is written down once, and everyone reads that.
    ///
    /// Several things distribute this app and each used to carry its own list.
    /// They drifted without a sound: one copier was carrying the wording files
    /// and nothing else, so the detection profiles and the automation manual
    /// never reached a test machine at all, and no one could have noticed. A
    /// payload that is named once cannot arrive in some places and not others.
    #[test]
    fn one_list_says_what_gets_handed_out() {
        let list = include_str!("../../../dist.list");
        let mut patterns: Vec<&str> = Vec::new();
        for line in list.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') || t.starts_with('[') {
                continue;
            }
            patterns.push(t);
        }
        assert!(patterns.len() > 5, "dist.list was not read ({} entries)", patterns.len());

        // A pattern matching nothing is a typo that deploys quietly and forever.
        //
        // One payload is not in the repository at all: the ConPTY beside the
        // exe is Microsoft's binary, fetched and hash-checked at build time,
        // so a fresh checkout has an empty folder where it will go. The guard
        // does not lapse for it -- it moves. What must agree there is this
        // list and the tool that writes those files, and a rename in one
        // without the other is exactly the silent loss this test exists for.
        let fetcher = include_str!("../../../tools/conpty.ps1");
        for p in &patterns {
            let rel = p.trim_end_matches("/**");
            let (dir, file_pat) = rel.rsplit_once('/').unwrap_or((".", rel));
            // cargo test runs from this crate's directory; what is handed out
            // lives in the repository above it
            let hit = std::fs::read_dir(crate::repo_root().join(dir)).ok().is_some_and(|mut e| {
                e.any(|f| {
                    f.ok().is_some_and(|f| {
                        let name = f.file_name().to_string_lossy().to_string();
                        match file_pat.split_once('*') {
                            Some((h, t)) => name.starts_with(h) && name.ends_with(t),
                            None => name == file_pat || file_pat.is_empty(),
                        }
                    })
                })
            });
            if hit {
                continue;
            }
            assert!(
                !file_pat.contains('*') && fetcher.contains(file_pat),
                "nothing matches `{p}` in dist.list, and no tool knows how to fetch it (a typo?)"
            );
        }

        // ...and the consumers must go through it rather than keeping their own copy
        let build_rs = include_str!("../../../build.rs");
        assert!(build_rs.contains("dist.list"), "build.rs does not read dist.list");
        let release = include_str!("../../../.github/workflows/release.yml");
        assert!(release.contains("stage.ps1"), "release.yml does not call the shared packaging step");
        for hardcoded in ["Copy-Item -Recurse \"lang\"", "docs/AUTOMATION.md\", \"docs/AUTOMATION.ja.md\""] {
            assert!(
                !release.contains(hardcoded),
                "release.yml keeps its own list of what ships: {hardcoded}"
            );
        }
    }

    /// Which page the restart applies to.
    ///
    /// A page has no process, so "put it back the way it started" is only possible
    /// where we recorded how it was opened. The settings screen and the result
    /// view ride in the pane list like any other page, but they are the app's own
    /// furniture — restarting them means nothing, so they are refused by name.
    #[test]
    fn a_panel_is_told_what_happened_without_where_it_came_from() {
        let raw = "runtime error: main は共有ブランチです\nstack traceback:\n\t[C]: in upvalue 'fn'";
        assert_eq!(plain_error(raw), "main は共有ブランチです");
        // Something that was already plain comes through unharmed
        assert_eq!(plain_error("git は見つかりません"), "git は見つかりません");
    }

    #[test]
    fn the_apps_own_screens_are_not_restartable_pages() {
        let caps: crate::hooks::Caps = std::rc::Rc::new(crate::caps::Capabilities::new(
            Default::default(),
            std::path::PathBuf::from("."),
            std::collections::HashMap::new(),
            std::collections::HashMap::new(),
            Default::default(),
        ));
        let page = |k: &str| Surface::Browser { key: k.into(), name: k.into(), dir: None };
        let surfaces = vec![
            page(SETTINGS_TAB),
            page(RESULT_TAB),
            page("shop"),
            Surface::Session(0),
        ];
        // active is 1-based over the surfaces
        assert_eq!(restartable_page(&surfaces, 1, &caps), None, "the settings screen is not included");
        assert_eq!(restartable_page(&surfaces, 2, &caps), None, "run results are not included");
        // A user's page only qualifies once we know how it was opened
        assert_eq!(restartable_page(&surfaces, 3, &caps), None, "not included while it does not know how to open it");
        assert_eq!(restartable_page(&surfaces, 4, &caps), None, "sessions are handled by session_mut, not here");
        assert_eq!(restartable_page(&surfaces, 0, &caps), None, "the board (INDEX) has nothing to go back to");
    }

    /// The status bar's restart button must land on the same keystroke a person
    /// at the window would press, and must carry the prefix so it works from
    /// whichever tab is showing. Without the prefix an 'r' would simply be typed
    /// into the session (the "wwww" bug the desk button already ran into).
    #[test]
    fn the_restart_button_arrives_prefixed() {
        let evs = super::keys_for(&shikisha_shared::Ev::Restart);
        assert_eq!(evs.len(), 2, "two keystrokes: the prefix and 'r'");
        let Event::Key(k) = &evs[0] else { panic!("the prefix is not a keystroke") };
        assert_eq!(k.code, KeyCode::Char('b'));
        assert!(k.modifiers.contains(KeyModifiers::CONTROL));
        let Event::Key(k) = &evs[1] else { panic!("the key is not a keystroke") };
        assert_eq!(k.code, KeyCode::Char('r'));
        assert!(k.modifiers.is_empty());
        // Ctrl+B r has to still be the tab restart on the receiving side
        let body = include_str!("runtime.rs");
        assert!(
            body.contains("// Ctrl+B r restarts this tab"),
            "the receiver's Ctrl+B r is gone"
        );
    }

    /// The board's menu must arrive as a plain keystroke, without the prefix key attached.
    ///
    /// Adding Ctrl+B would mean only characters that also exist on the prefix-key side work.
    #[test]
    fn a_menu_press_arrives_as_a_plain_key() {
        for (key, _) in crate::shell::MENU {
            let evs = super::keys_for(&shikisha_shared::Ev::Menu {
                key: key.to_string(),
            });
            assert_eq!(evs.len(), 1, "{key}: not exactly one keystroke");
            let Event::Key(k) = &evs[0] else {
                panic!("{key}: not a keystroke")
            };
            assert_eq!(k.code, KeyCode::Char(key.chars().next().unwrap()));
            assert!(
                k.modifiers.is_empty(),
                "{key}: it has a prefix ({:?})",
                k.modifiers
            );
        }
    }

    /// It must hold onto a hand-off when the recipient can't accept it yet.
    ///
    /// An AI CLI doesn't draw its input box the instant it launches. Flushing
    /// text in before that gets silently dropped, and to whoever wrote it,
    /// it just looks like "nothing happened".
    ///
    /// Only "delivering something" can wait. Restarts and notifications have
    /// nothing to do with whether the recipient is ready.
    #[test]
    fn only_a_handoff_waits_for_the_other_side() {
        use hooks::{Command, TabRef};
        let draft = Command::DraftPrompt {
            target: TabRef::Name("ai".into()),
            text: "x".into(),
            origin: 1,
        };
        let send = Command::SendPrompt {
            target: TabRef::Name("ai".into()),
            text: "x".into(),
            origin: 1,
        };
        assert!(can_wait(&draft) && can_wait(&send), "what is handed over cannot wait");
        assert_eq!(target_of(&draft).map(|t| format!("{t:?}")).as_deref(),
                   Some("Name(\"ai\")"));

        for other in [
            Command::Restart { target: TabRef::Index(1), fresh: false },
            Command::Notify { dest: Some("slack".into()), text: "x".into() },
            Command::Log("x".into()),
            Command::SendKeys { target: TabRef::Index(1), keys: "y".into() },
        ] {
            assert!(!can_wait(&other), "it holds on to something that does not need to wait: {other:?}");
        }
    }

    /// What gets passed to a browser's hook must be built from the screen layout.
    ///
    /// The number matches the one a human presses. The name is the
    /// human-readable one, distinct from the id automation addresses it by.
    #[test]
    fn a_page_knows_its_number_and_both_of_its_names() {
        let surfaces = vec![
            Surface::Browser { key: "html".into(), name: "HTML解析".into(), dir: None },
            Surface::Session(0),
        ];
        let page = page_ctx(&surfaces, "html", "https://example.com/".into(), true)
            .expect("it is in the list but was not found");
        assert_eq!(page.index, 1, "it differs from the number on screen");
        assert_eq!(page.id, "html", "the name automation uses is wrong");
        assert_eq!(page.name, "HTML解析", "the name a person reads is not shown");
        assert!(page.complete);

        // Nothing is passed for a page not in the layout (e.g. after it's closed)
        assert!(page_ctx(&surfaces, "shop", String::new(), true).is_none());
    }

    /// Automation assignments must be numbered the way the screen is.
    ///
    /// The number a human presses, the number a script addresses, and the
    /// number the ball flies to have to be the same, or none of it can be
    /// tracked. The number is never remembered anywhere — it's reassigned
    /// every time config is read, so it never drifts out of sync even after reordering.
    #[test]
    fn the_scripts_are_numbered_the_way_the_screen_is() {
        let desk = desk_from_rows(&[
            ("HTML解析", "html", "browser https://example.com/"),
            ("エンジニア", "ai", "claude"),
        ]);
        let mut desk = desk;
        desk.tabs[0].cfg.automation = Some("scripts/html".into());
        desk.tabs[1].cfg.automation = Some("scripts/ai".into());

        let got = automation_by_pane(&desk);
        // Ordered by screen number: the browser is 1, claude is 2
        assert_eq!(
            got,
            vec![
                (1, TabAuto::Path("scripts/html".to_string())),
                (2, TabAuto::Path("scripts/ai".to_string())),
            ],
            "the assignment is off"
        );
    }

    /// A discussion participant's/referee's tab id must resolve correctly to a screen number
    #[test]
    fn discuss_agents_resolve_to_panes() {
        let desk = desk_from_rows(&[
            ("参加A", "ai1", "claude"),
            ("参加B", "ai2", "codex"),
            ("審判", "ref", "claude"),
        ]);
        assert_eq!(surface_of_id(&desk, "ai1"), Some(1));
        assert_eq!(surface_of_id(&desk, "ai2"), Some(2));
        assert_eq!(surface_of_id(&desk, "ref"), Some(3));
        assert_eq!(surface_of_id(&desk, "いない"), None);
        // The name on screen is a label, not an address: two tabs may share one
        assert_eq!(surface_of_id(&desk, "審判"), None);
    }

    /// An aim is not automation, and must not take a tab's own automation away.
    ///
    /// `drives` used to mean "browser-driving mode", and a tab that had it was
    /// handed the built-in agent at launch INSTEAD of the automation written
    /// for it -- silently, with nothing on screen saying so. It now means the
    /// aim last picked on screen (🎯), which is attached when there is a goal
    /// and handed back when it is let go, so the two no longer fight.
    #[test]
    fn an_aim_does_not_replace_the_tabs_own_automation() {
        let mut desk = desk_from_rows(&[
            ("エージェント", "ai", "claude"),
            ("ページ", "br", "browser https://example.com/"),
        ]);
        desk.tabs[0].cfg.drives = Some("br".into());
        desk.tabs[0].cfg.automation = Some("scripts/mine".into());

        assert_eq!(
            automation_by_pane(&desk),
            vec![(1, TabAuto::Path("scripts/mine".to_string()))],
            "a tab with its own target has had its automation taken"
        );
    }

    /// The tab a pane asked for is the row that arrived, wherever it landed.
    ///
    /// A tab is written into its own folder's list, so one added to any folder
    /// but the last lands in the middle of the rows. The pane used to be handed
    /// "the last row nobody is looking at", which in that case is somebody
    /// else's tab -- and when that tab's folder had been put out of sight, the
    /// folder came back with it
    #[test]
    fn the_pane_gets_the_row_that_arrived_not_the_last_one() {
        let keys = |v: &[&str]| v.iter().map(|k| k.to_string()).collect::<Vec<_>>();
        let rows = |v: &[&str]| {
            v.iter().enumerate().map(|(i, k)| (i + 1, k.to_string())).collect::<Vec<_>>()
        };
        let was = keys(&["tab:1", "tab:2", "tab:3"]);
        assert_eq!(
            arrived_row(&was, &rows(&["tab:1", "page:fox", "tab:2", "tab:3"])),
            Some(2),
            "the pane was given a row that was already there"
        );
        assert_eq!(
            arrived_row(&was, &rows(&["tab:1", "tab:2", "tab:3", "page:fox"])),
            Some(4),
            "a tab added to the last folder is still the one that arrived"
        );
        // Nothing has arrived: the pane goes on waiting rather than being
        // handed whatever stands at the end
        assert_eq!(arrived_row(&was, &rows(&["tab:1", "tab:2", "tab:3"])), None);
        assert_eq!(arrived_row(&was, &rows(&["tab:1", "tab:3"])), None, "a row going is not one arriving");
        assert_eq!(
            arrived_row(&was, &rows(&["tab:1", "tab:9", "tab:3"])),
            None,
            "a row renamed while the form was open is not a new tab"
        );
    }

    /// The screen order must match the order written in config.
    ///
    /// Sessions and browsers are kept separately. Letting that internal
    /// distinction leak into the ordering would push the browser written
    /// first to the back. This actually happened, and the result was
    /// "HTML should be first in order — why did it end up second?"
    #[test]
    fn the_order_on_screen_is_the_order_in_the_settings() {
        let desk = desk_from_rows(&[
            ("HTML解析", "html", "browser https://example.com/"),
            ("エンジニア", "ai", "claude"),
        ]);
        let tabs = ["エンジニア"];
        let hosted = vec!["html".to_string()];

        let surfaces = surfaces_of(Some(&desk), &tabs, &hosted, &[], false);
        assert_eq!(
            surfaces,
            vec![Surface::Browser { key: "html".into(), name: "HTML解析".into(), dir: None }, Surface::Session(0)],
            "they are not in the order of the settings"
        );
        // A session must be resolvable from its screen number
        assert_eq!(session_at(&surfaces, 1), None, "number 1 should be the browser");
        assert_eq!(session_at(&surfaces, 2), Some(0));
        // The ball moves by session number; what's displayed is the screen number
        assert_eq!(surface_at(&surfaces, 1), 2);
    }

    fn desk_from_rows(rows: &[(&str, &str, &str)]) -> config::Desk {
        let tabs = rows
            .iter()
            .map(|(name, id, cmd)| {
                config::FlatTab {
                    cfg: config::TabConfig {
                        name: Some(name.to_string()),
                        id: Some(id.to_string()),
                        command: config::CommandSpec::Line(cmd.to_string()),
                        ..Default::default()
                    },
                    depth: 0,
                    folder: 0,
                }
            })
            .collect();
        config::Desk {
            name: "試験".into(),
            id: "shiken".into(),
            folders: vec![config::Folder::default()],
            tabs,
            automation: None,
            browsers: Vec::new(),
            secrets_allow: Vec::new(),
            secrets_allow_all: false,
            stops: Vec::new(),
            discuss: None,
            ..Default::default()
        }
    }

    /// A browser that hasn't been opened yet must still keep the position written in config.
    ///
    /// If the number shifted based on open order, whatever a script points to
    /// would change every run. Failure to open should just be shown through
    /// state, not by moving the slot.
    #[test]
    fn a_browser_keeps_its_place_even_before_it_opens() {
        let desk = desk_from_rows(&[
            ("HTML解析", "html", "browser https://example.com/"),
            ("エンジニア", "ai", "claude"),
        ]);
        let tabs = ["エンジニア"];
        let surfaces = surfaces_of(Some(&desk), &tabs, &[], &[], false);
        assert_eq!(
            surfaces,
            vec![Surface::Browser { key: "html".into(), name: "HTML解析".into(), dir: None }, Surface::Session(0)],
            "before it is opened, the numbers are off"
        );
    }

    /// Things not written in config must be appended at the end.
    /// There's no way to decide a position for a browser automation opened
    /// later, or a tab launched via arguments.
    #[test]
    fn what_the_settings_do_not_mention_goes_last() {
        let desk = desk_from_rows(&[("エンジニア", "ai", "claude")]);
        let tabs = ["エンジニア", "あとから"];
        let hosted = vec!["settings".to_string()];
        let surfaces = surfaces_of(Some(&desk), &tabs, &hosted, &[], false);
        assert_eq!(
            surfaces,
            vec![
                Surface::Session(0),
                Surface::Session(1),
                Surface::Browser { key: "settings".into(), name: "settings".into(), dir: None }
            ]
        );
    }

    /// The number that switches to the settings tab must point at its
    /// existing location if it's already open.
    ///
    /// Using `surfaces.len() + 1` points one slot too far, since settings is
    /// already in the layout — this used to leave the screen solid black when pressed
    /// (this happens when pressing "add tab" while settings is already open).
    #[test]
    fn settings_active_points_at_the_open_settings_tab() {
        // Not open yet: points to the slot right after the end
        let before = vec![Surface::Session(0), Surface::Session(1)];
        assert_eq!(settings_active(&before), 3, "before it is opened, it is after the last");

        // Already open: points to its existing location (the end). Not one slot further.
        let after = vec![
            Surface::Session(0),
            Surface::Session(1),
            Surface::Browser { key: "settings".into(), name: "settings".into(), dir: None },
        ];
        assert_eq!(settings_active(&after), 3, "when open, it is where it is");
    }

    /// The activity wave reflects actual output, not decoration, so it must stay flat when nothing came out
    #[test]
    fn activity_wave_reflects_real_output() {
        let argv = vec![crate::test_shell()];
        let mut t =
            Tab::spawn("SHELL".into(), &argv, None, 20, 100, tab::TabOptions::default()).unwrap();
        assert_eq!(t.activity().len(), tab::ACTIVITY_LEN);
        assert!(t.activity().iter().all(|l| *l == 0), "silent right after starting");

        // Ticking after output arrives should bring up the most recent frame
        t.write_bytes(b"echo hello\r").unwrap();
        let start = Instant::now();
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(25));
            t.tick(start);
            if *t.activity().last().unwrap() > 0 {
                break;
            }
        }
        assert!(
            t.activity().iter().any(|l| *l > 0),
            "output raises the waveform: {:?}",
            t.activity()
        );
        t.kill();
    }

    /// Sending must be two stages: "type the text" then "submit it".
    ///
    /// Writing it all in one go means Enter arrives before the AI CLI's input
    /// box has finished processing the paste, leaving the text typed but never
    /// submitted (this actually happened with sends from a phone).
    /// The order a listing comes in, and the fields it carries. Promised to
    /// three callers now -- the file panel, the transfer panel, and a script
    /// walking a folder -- so it is checked once here rather than assumed
    /// three times
    #[test]
    fn a_folder_is_listed_with_its_folders_first_and_then_by_name() {
        let dir = std::env::temp_dir().join(format!("shikisha-rows-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zeta")).unwrap();
        std::fs::create_dir_all(dir.join("alpha")).unwrap();
        std::fs::write(dir.join("b.txt"), "hello").unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();

        let rows = local_rows(&dir).unwrap();
        let names: Vec<&str> = rows.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "zeta", "a.txt", "b.txt"], "folders first, then by name");
        assert!(rows[0].dir && !rows[2].dir);
        let b = rows.iter().find(|e| e.name == "b.txt").unwrap();
        assert_eq!(b.size, 5);
        assert!(b.modified > 0, "a file that exists has a time on it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What the panel will and will not hold up against another file. Checked
    /// here because both refusals happen before anything is asked of a server,
    /// and a refusal that arrives after a megabyte has crossed the wire is not
    /// a refusal, it is an apology
    #[test]
    fn only_text_of_a_readable_size_is_compared() {
        let text = |name: &str, bytes: &[u8]| diff_text(name, bytes, None).map(|r| r.text);
        assert_eq!(text("a.txt", b"one\ntwo\n").as_deref(), Ok("one\ntwo\n"));
        // Empty is text: two empty files are the same, which is an answer
        assert_eq!(text("a.txt", b"").as_deref(), Ok(""));
        let big = vec![b'x'; SFTP_DIFF_MAX + 1];
        assert!(text("big.js", &big).is_err(), "a file past the limit is refused");
        let edge = vec![b'x'; SFTP_DIFF_MAX];
        assert!(text("big.js", &edge).is_ok(), "the limit itself is still read");
        // A picture: the zero bytes in it are what says so
        assert!(text("logo.png", &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0]).is_err());
        // A spreadsheet's CSV is text, read as the words it holds
        let sjis = crate::charset::write_as("氏名,住所\r\n山田太郎,東京都\r\n", encoding_rs::SHIFT_JIS).unwrap();
        assert_eq!(text("a.csv", &sjis).as_deref(), Ok("氏名,住所\r\n山田太郎,東京都\r\n"));
        // ...and read as something it is not, when that is what was chosen
        let wrong = diff_text("a.csv", &sjis, Some(encoding_rs::UTF_8)).unwrap();
        assert!(wrong.text.contains('\u{FFFD}') && !wrong.exact);
    }

    /// The encoding a comparison names: once when both sides agree, and never
    /// for a side that is plain ASCII, which reads the same in any of them
    #[test]
    fn a_comparison_names_the_encoding_its_files_are_in() {
        // A line as long as a real one: two characters are too few to tell
        // Japanese from anything else on a machine not set up for Japanese
        let line = "1001,山田太郎,東京都千代田区\n";
        let sjis = crate::charset::write_as(line, encoding_rs::SHIFT_JIS).unwrap();
        let utf8 = line.as_bytes();
        let ascii = b"yamada\n";
        let read = |b: &[u8]| diff_text("a", b, None).unwrap();
        let (s, u, a) = (read(&sjis), read(utf8), read(ascii));
        assert_eq!(diff_encodings((&s, &sjis), (&s, &sjis)), "Shift_JIS");
        assert_eq!(diff_encodings((&a, ascii), (&s, &sjis)), "Shift_JIS");
        assert_eq!(diff_encodings((&a, ascii), (&a, ascii)), "UTF-8");
        assert_eq!(diff_encodings((&s, &sjis), (&u, utf8)), "Shift_JIS / UTF-8");
    }

    #[test]
    fn a_prompt_is_typed_first_and_submitted_after() {
        let argv = vec![crate::test_shell()];
        // Wide, because the prompt is the folder the tests run in: a long one
        // wrapped `echo shikisha-ok` onto two rows at 60 columns, and the text
        // was never found on the screen it had been typed into
        let mut t =
            Tab::spawn("shell".into(), &argv, None, 20, 250, tab::TabOptions::default()).unwrap();

        let screen = |t: &Tab| tab::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen());
        let has_line = |t: &Tab, want: &str| {
            screen(t).lines().any(|l| l.trim() == want)
        };
        let wait_for = |t: &Tab, want: &str| {
            for _ in 0..60 {
                if has_line(t, want) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };

        // Wait for the prompt to appear
        for _ in 0..60 {
            if screen(&t).contains('>') {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        for chunk in paste_chunks(&t, "echo shikisha-ok") {
            t.write_passthrough(&chunk).unwrap();
        }
        // Waited for rather than slept through. A fixed wait is a guess about
        // how busy the machine is, and on a loaded one a real shell had not
        // echoed the text yet -- so the screen was read empty and the test
        // failed for a reason that was never about what it is checking
        let mut typed = false;
        for _ in 0..60 {
            if screen(&t).contains("echo shikisha-ok") {
                typed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(typed, "the text goes into the input box: {}", screen(&t));
        // Long enough that an Enter nobody sent would have run it by now. A
        // slow machine only makes this wait more generous, never less
        std::thread::sleep(Duration::from_millis(200));
        assert!(
            !has_line(&t, "shikisha-ok"),
            "it has not run yet: {}",
            screen(&t)
        );

        // The reserved submit arrives
        t.write_bytes(b"\r").unwrap();
        assert!(wait_for(&t, "shikisha-ok"), "it runs: {}", screen(&t));

        t.kill();
    }

    /// The "don't auto-send right after a human touches it" protection must
    /// not misfire right after startup.
    ///
    /// Initializing the touched timestamp to 0 used to be mistaken for
    /// "touched just now" for the whole guard period after app startup,
    /// silently dropping startup automation.
    #[test]
    fn an_untouched_tab_is_not_mistaken_for_one_just_typed_into() {
        let argv = vec![crate::test_shell()];
        let mut t =
            Tab::spawn("T".into(), &argv, None, 20, 60, tab::TabOptions::default()).unwrap();

        // Right after startup: nobody has touched it yet, so the protection never kicks in, no matter when asked
        assert!(!touched_recently(&t, 0), "the moment it starts");
        assert!(!touched_recently(&t, 1_000), "one second later");
        assert!(
            !touched_recently(&t, MANUAL_GUARD_MS - 1),
            "within the guard time, it may send if nothing was touched"
        );

        // The guard kicks in once a human touches it
        t.last_manual_ms = Some(10_000);
        assert!(touched_recently(&t, 10_000), "right after being touched");
        assert!(
            touched_recently(&t, 10_000 + MANUAL_GUARD_MS - 1),
            "still in effect within the guard time"
        );
        assert!(
            !touched_recently(&t, 10_000 + MANUAL_GUARD_MS),
            "released once the time passes"
        );

        t.kill();
    }

    /// Shorthand for the tests: is this step Wait / Hand / Submit?
    fn waited(s: &Step) -> bool {
        matches!(s, Step::Wait)
    }
    fn handed(s: &Step) -> bool {
        matches!(s, Step::Hand(_))
    }
    fn submitted(s: &Step) -> bool {
        matches!(s, Step::Submit { .. })
    }

    /// Submit (Enter) must wait until paste intake has really finished.
    ///
    /// The recipient reads the paste one character at a time and falls behind;
    /// an Enter written into the same queue is taken as part of the paste and
    /// counts as a newline, leaving the text unsent in the input box. So the
    /// body goes over a chunk at a time and the Enter only follows the last one.
    #[test]
    fn the_enter_waits_for_the_paste_to_finish_being_taken_in() {
        let one = |n: usize| vec![vec![b'x'; 8]; n];

        // A one-chunk paste: out at once, then the settling rule as before
        let mut p = PendingSend::new(1, one(1), true, 100, 1_000, 8);
        assert!(handed(&p.step(100, 1_000)), "the first chunk is handed over at once");
        assert!(waited(&p.step(200, 1_100)), "a reaction starting is not enough to send");
        assert!(waited(&p.step(300, 2_000)), "still growing");
        assert!(waited(&p.step(400, 3_000)), "still growing");
        assert!(waited(&p.step(400, 3_100)), "not yet, right after it stops");
        assert!(waited(&p.step(400, 3_100 + SUBMIT_QUIET_MS - 1)), "not quiet for long enough");
        assert!(submitted(&p.step(400, 3_100 + SUBMIT_QUIET_MS)), "it sends once it settles");

        // Restart the measurement if activity resumes partway through
        let mut p = PendingSend::new(1, one(1), true, 0, 0, 8);
        assert!(handed(&p.step(0, 0)), "the first chunk");
        assert!(waited(&p.step(0, 100)), "quiet, but not long enough");
        assert!(waited(&p.step(50, 200)), "it started again, so it measures again");
        assert!(waited(&p.step(50, 300)), "here it sees it stop again");
        assert!(waited(&p.step(50, 300 + SUBMIT_QUIET_MS - 1)), "measuring again");
        assert!(submitted(&p.step(50, 300 + SUBMIT_QUIET_MS)), "settled again");

        // Send anyway once the cap is hit, even if it never settles
        let mut p = PendingSend::new(1, one(1), true, 0, 0, 8);
        assert!(handed(&p.step(0, 0)), "the first chunk");
        let mut out = 0;
        for t in (100..SUBMIT_GIVE_UP_MS).step_by(100) {
            out += 1;
            assert!(!submitted(&p.step(out, t)), "it waits while it keeps growing ({t}ms)");
        }
        out += 1;
        assert!(submitted(&p.step(out, SUBMIT_GIVE_UP_MS)), "it sends once the limit is reached");
    }

    /// The whole body has to be handed over before the Enter, and the next
    /// piece only goes out once the recipient has drawn (= caught up).
    ///
    /// This is the bug the chunking exists for: Codex CLI drew *nothing at all*
    /// for two seconds while taking in a long paste, so "output has stopped"
    /// looked exactly like "it has finished", the Enter went out into the middle
    /// of the paste, and 20,000 characters sat unsent in the input box.
    #[test]
    fn the_body_goes_over_a_piece_at_a_time_and_the_enter_comes_last() {
        let mut p = PendingSend::new(1, vec![vec![b'a'], vec![b'b'], vec![b'c']], true, 0, 0, 3);
        assert!(handed(&p.step(0, 0)), "the first chunk goes at once");
        // Silent recipient: not a word drawn. It must not be given the rest at
        // once, and above all must not be sent Enter.
        assert!(waited(&p.step(0, 10)), "nothing more is handed over until it draws");
        assert!(waited(&p.step(0, PASTE_ACK_MS - 1)), "nothing is handed over before the wait is up");
        assert!(handed(&p.step(0, PASTE_ACK_MS)), "if it never draws, it hands over after waiting");
        // Drawing means it has caught up, so the rest can go straight away
        let last = PASTE_ACK_MS + 1;
        assert!(handed(&p.step(9, last)), "once it draws, the next is handed over at once");
        // Only now does the settling rule start, and it is measured from the
        // first pass that sees the recipient still — not from the last piece
        assert!(waited(&p.step(9, last + 10)), "here it starts watching for it to stop");
        assert!(waited(&p.step(9, last + 10 + SUBMIT_QUIET_MS - 1)), "not quiet for long enough");
        assert!(submitted(&p.step(9, last + 10 + SUBMIT_QUIET_MS)), "it sends after handing over everything");

        // A draft is placed and left alone: the body goes over, the Enter never does
        let mut p = PendingSend::new(1, vec![vec![b'a']], false, 0, 0, 1);
        assert!(handed(&p.step(0, 0)), "the text is handed over");
        assert!(waited(&p.step(0, 10)), "it starts watching for it to stop");
        assert!(submitted(&p.step(0, 10 + SUBMIT_QUIET_MS)), "the text is all handed over");
        assert!(!p.submit, "a draft does not press Enter");
    }

    /// Two messages to one tab are two messages.
    ///
    /// They are handed over in turn, from the front: the second one's text must
    /// not start going over until the first one's Enter has. Sent together,
    /// what arrives is one message with both in it, followed by an Enter on an
    /// empty line — which is exactly what it looked like from outside: "the
    /// text I meant to send never went".
    #[test]
    fn a_second_message_waits_for_the_first_ones_enter() {
        let mut queue = [PendingSend::new(1, vec![vec![b'A']], true, 0, 0, 1),
            PendingSend::new(1, vec![vec![b'B']], true, 0, 0, 1),
            PendingSend::new(2, vec![vec![b'C']], true, 0, 0, 1)];
        // One pass: the front one for tab1 acts, the one behind it waits, and
        // another tab is nobody's business
        let mut holding: Vec<usize> = Vec::new();
        let acted: Vec<bool> = queue
            .iter_mut()
            .map(|p| {
                if holding.contains(&p.tab) {
                    return false;
                }
                holding.push(p.tab);
                !waited(&p.step(0, 0))
            })
            .collect();
        assert_eq!(acted, vec![true, false, true], "the same tab waits its turn; different tabs run side by side");

        // The one behind has handed over nothing at all, so nothing of it can
        // have landed inside the message in front
        assert_eq!(queue[1].handed, 0, "text further back flowed in first");
    }

    /// A person typing into a tab mid-paste must not be typed into the middle
    /// of the paste. The rest of it goes over first, in one piece.
    #[test]
    fn typing_pushes_the_rest_of_the_paste_out_first() {
        let mut p = PendingSend::new(1, vec![vec![b'a'], vec![b'b'], vec![b'c']], true, 0, 0, 3);
        assert!(handed(&p.step(0, 0)), "the first chunk");
        assert_eq!(p.rest(500), b"bc".to_vec(), "the rest goes out in one go");
        assert_eq!(p.rest(500), Vec::<u8>::new(), "it is not sent twice");
        // The Enter still follows, measured from the moment the rest went over
        assert!(waited(&p.step(0, 510)), "from here it measures the quiet again");
        assert!(submitted(&p.step(0, 510 + SUBMIT_QUIET_MS)), "sending comes after that");
    }

    /// A long message is seconds of work with nothing on screen to show for
    /// it, so the screen is told how far it has got -- by the send itself.
    ///
    /// Read off the chunks that are actually owed, never a tally kept beside
    /// them: a second count of the same thing is a second thing to get wrong,
    /// and this one is read by a person watching a box they have just emptied.
    #[test]
    fn a_send_says_how_far_it_has_got() {
        let chunks = vec![vec![b'a'], vec![b'b'], vec![b'c'], vec![b'd']];
        let mut p = PendingSend::new(1, chunks, true, 0, 0, 3_400);
        assert_eq!(p.sending(), (0.0, 3_400), "nothing has gone over, and it is 3,400 characters long");
        assert!(handed(&p.step(0, 0)), "the first chunk goes at once");
        assert_eq!(p.sending().0, 0.25, "one of the four is in");
        assert!(handed(&p.step(1, PASTE_ACK_MS)), "and the second");
        assert_eq!(p.sending().0, 0.5, "two of the four");
        // Somebody typed: the rest goes over in one piece, and the screen must
        // say so rather than sitting at half
        assert_eq!(p.rest(PASTE_ACK_MS), b"cd".to_vec(), "the rest goes out in one go");
        assert_eq!(p.sending(), (1.0, 3_400), "all of it is in, and it is still the same message");
    }

    /// A provider edited while its tab is open reaches that tab.
    ///
    /// Re-resolving the providers is only half of it: a tab holds the
    /// connection it was launched with, so on its own that changes nothing the
    /// tab can see. Reported from use — the wait was set to 0 ("as long as it
    /// takes"), saved, and the tab still gave up at 180 seconds, which was the
    /// wait it had been holding since it opened. The same silence applied to a
    /// corrected endpoint and to a new key.
    #[test]
    fn a_provider_edited_now_reaches_the_tab_that_is_using_it() {
        let settings = |secs: u64| {
            let cfg: config::Config = serde_json::from_str(&format!(
                r#"{{"providers":{{"t":{{"base_url":"http://127.0.0.1:1/v1","timeout_sec":{secs}}}}},"desks":[{{"name":"d"}}]}}"#
            ))
            .unwrap();
            cfg
        };
        let argv = vec!["model".to_string(), "t/m".to_string()];

        let conns = config::app_providers(&settings(180), &|_| None);
        let conn = bridge::conn_in(&conns, &argv).expect("the connection is found");
        assert_eq!(conn.timeout, Some(Duration::from_secs(180)));
        let mut tabs = [Tab::spawn(
            "model".into(),
            &argv,
            None,
            10,
            40,
            tab::TabOptions { model: Some(conn), ..Default::default() },
        )
        .expect("started")];

        // The wait is changed to "as long as it takes" and saved
        reload_providers(&settings(0), &|_| None, &mut tabs, &mut []);
        assert_eq!(
            tabs[0].model.as_ref().and_then(|c| c.timeout),
            None,
            "the tab keeps the old wait time after the setting changed"
        );
        tabs[0].kill();
    }

    /// A paste is cut at character boundaries, so no character is ever split
    /// across two writes (a broken character would be drawn as garbage).
    #[test]
    fn a_paste_is_cut_between_characters() {
        let t = Tab::spawn("cmd".into(), &[crate::test_shell()], None, 24, 80, tab::TabOptions::default())
            .expect("started");
        let text = "あ".repeat(PASTE_CHUNK); // 3 bytes each: boundaries never land on PASTE_CHUNK
        let chunks = paste_chunks(&t, &text);
        assert!(chunks.len() > 1, "long text is split");
        for c in &chunks {
            assert!(
                std::str::from_utf8(c).is_ok(),
                "a character is broken across chunks"
            );
        }
        let joined: String = chunks.iter().map(|c| String::from_utf8_lossy(c).into_owned()).collect();
        assert!(joined.contains(&text), "joined back, it is the original text");
        let mut t = t;
        t.kill();
    }

    /// Automation may move the view, but the person outranks it.
    ///
    /// This is the ONLY gate: `show()` is the only thing that moves the screen,
    /// and handing work to a tab no longer moves anything by itself. Before, the
    /// two lived on different paths with different rules — `show()` obeyed
    /// neither the setting nor the guard, so "don't switch on me" was a promise
    /// the app did not keep during a rally.
    #[test]
    fn the_person_outranks_automation_over_the_view() {
        let g = VIEW_GUARD_MS;
        let gate = |allowed, touched_ms, settings_open| ViewMove { allowed, touched_ms, settings_open };

        // Long since they touched it, and they allow it: automation may move the view
        assert!(gate(true, 0, false).may(g));
        // They said no
        assert!(!gate(false, 0, false).may(g), "it switches regardless of the setting");
        // They are reading the settings screen
        assert!(!gate(true, 0, true).may(g), "it pulls you away from the settings screen");

        // They just moved the view themselves — stay out of the way
        assert!(!gate(true, 1_000, false).may(1_000), "it pulls you away while you are reading");
        assert!(!gate(true, 1_000, false).may(1_000 + g - 1));
        // ...and step back in once enough time has passed
        assert!(gate(true, 1_000, false).may(1_000 + g));
    }

    /// The wheel scrolls back, and typing brings you back to the present.
    ///
    /// If you type while still scrolled back, the typed characters appear at
    /// the bottom of the screen, so it looks like "I typed but nothing showed up".
    #[test]
    fn the_wheel_goes_back_and_typing_comes_home() {
        assert_eq!(scrolled_to(0, 3), 3, "it did not scroll back");
        assert_eq!(scrolled_to(3, -1), 2);
        // Doesn't go past the present even if it overshoots
        assert_eq!(scrolled_to(2, -100), 0);
        assert_eq!(scrolled_to(0, -1), 0);
        // All the way back (the terminal side caps how much is actually retained)
        assert_eq!(scrolled_to(5, i32::MAX), 5 + i32::MAX as usize);

        // Typing returns to the present. While still scrolled back, typed
        // characters appear at the bottom of the screen, so they're invisible.
        let mut p = vt100::Parser::new(3, 20, 100);
        p.process(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n");
        p.screen_mut().set_scrollback(2);
        assert_eq!(p.screen().scrollback(), 2);
        p.screen_mut().set_scrollback(scrolled_to(2, i32::MIN));
        assert_eq!(p.screen().scrollback(), 0, "it does not return to now");
    }

    /// A full-screen program must be handed the scroll itself, unmodified.
    ///
    /// It rewinds its own contents itself, so any history we hold is useless
    /// to it (the alternate screen has nothing to scroll back into). Claude Code is one such program.
    #[test]
    fn a_full_screen_program_is_told_that_the_wheel_turned() {
        use vt100::MouseProtocolEncoding as E;
        // The modern encoding: 64 is up, 65 is down, position is 1-based
        assert_eq!(wheel_bytes(true, 0, 0, E::Sgr), b"\x1b[<64;1;1M".to_vec());
        assert_eq!(wheel_bytes(false, 4, 9, E::Sgr), b"\x1b[<65;10;5M".to_vec());
        // The legacy encoding is one byte per value (32 is added)
        assert_eq!(
            wheel_bytes(true, 0, 0, E::Default),
            vec![0x1b, b'[', b'M', 96, 33, 33]
        );
    }

    /// The failed CI handed to an AI names what it ran on, whether that is a
    /// pull request or a commit on a branch that has none yet.
    ///
    /// The prompt used to open with "pull request #{pr}", so a branch with no
    /// pull request could not be handed over at all: the words would have said
    /// "pull request #0"
    #[test]
    fn the_failed_ci_names_what_it_ran_on() {
        crate::i18n::init(Some("en"), &[crate::repo_root()]);
        let on_pr = ci_ran_on(12, "Fix the thing", "https://x/pull/12", "feature", "abc1234def");
        assert!(on_pr.contains("#12") && on_pr.contains("Fix the thing") && on_pr.contains("https://x/pull/12"), "{on_pr}");
        let on_commit = ci_ran_on(0, "", "", "feature", "abc1234def5678");
        assert!(on_commit.contains("abc1234") && on_commit.contains("feature"), "{on_commit}");
        assert!(!on_commit.contains("abc1234def"), "the whole commit is spelled out: {on_commit}");
        assert!(!on_commit.contains('#'), "a branch with no pull request is given a number: {on_commit}");

        // And the prompt it goes into is left with nothing to fill in
        let filled = |number: u64, ci: &str| {
            crate::config::GitSpec::default()
                .ci_prompt()
                .replace("{ci}", ci)
                .replace("{pr}", &match number { 0 => String::new(), n => n.to_string() })
                .replace("{branch}", "feature")
                .replace("{folder}", "D:/work")
                .replace("{language}", "English")
                .replace("{url}", "")
                .replace("{title}", "")
                .replace("{checks}", "[]")
        };
        for (number, ci) in [(12, on_pr.as_str()), (0, on_commit.as_str())] {
            let text = filled(number, ci);
            assert!(!text.contains('{'), "a word was left unfilled for {number}: {text}");
            assert!(text.contains(ci), "the prompt does not say what CI ran on: {text}");
        }
    }

    /// A browser in the row must not hide the tabs behind it.
    ///
    /// Everything that points at a tab counts by screen number, browsers
    /// included. Counting sessions instead would make the tabs sitting behind
    /// however many browsers there are look like "numbers that don't exist".
    /// (With the layout Analysis=1 browser / AI=2 session, the AI was unreachable.)
    #[test]
    fn a_browser_in_the_row_does_not_hide_the_tabs_behind_it() {
        let surfaces = vec![
            Surface::Browser { key: "html".into(), name: "解析".into(), dir: None },
            Surface::Session(0),
        ];
        let keys = surface_keys(&surfaces, &[]);
        assert_eq!(
            hooks::TabRef::Index(2).resolve(&keys),
            Some(2),
            "it cannot point at the tab behind the browser"
        );
        assert_eq!(
            hooks::TabRef::Name("html".into()).resolve(&keys),
            Some(1),
            "it cannot point at the browser by its automation name"
        );
        assert_eq!(
            hooks::TabRef::Name("解析".into()).resolve(&keys),
            None,
            "the name on screen does not reach it"
        );
    }




    /// On first run, INDEX must show onboarding guidance (never leave the user
    /// unsure what to do).
    /// Launching must start from the desk that was previously open.
    ///
    /// Always starting from the first one means extra switching effort every
    /// launch whenever what you want to try is the second one. During
    /// debugging, that gets repeated dozens of times.
    #[test]
    fn it_opens_where_you_left_off() {
        let list = |pairs: &[(&str, &str)]| -> Vec<config::Desk> {
            pairs
                .iter()
                .map(|(id, name)| config::Desk { id: id.to_string(), name: name.to_string(), ..Default::default() })
                .collect()
        };
        let desks = list(&[("conductor", "指揮者"), ("tamago", "たまごカート編集部"), ("check", "検証")]);

        assert_eq!(
            starting_desk(true, Some("tamago"), &desks),
            1,
            "it does not go back to what was open before"
        );

        // What's remembered is the id, not the number, so it still tracks after reordering
        let reordered = list(&[("check", "検証"), ("tamago", "たまごカート編集部"), ("conductor", "指揮者")]);
        assert_eq!(starting_desk(true, Some("tamago"), &reordered), 1);
        assert_eq!(starting_desk(true, Some("conductor"), &reordered), 2, "reordering opens a different desk");

        // ...and after renaming: the desk renamed while the app was closed is
        // still the one that opens
        let renamed = list(&[("conductor", "指揮者"), ("tamago", "ワイアード＆エコ"), ("check", "検証")]);
        assert_eq!(starting_desk(true, Some("tamago"), &renamed), 1, "a renamed desk is taken for a deleted one");

        // A file from before ids were kept holds the name
        assert_eq!(starting_desk(true, Some("検証"), &desks), 2, "the name an older version wrote is not read");

        // Deleted, no memory of it, or disabled -> falls back to the first one
        assert_eq!(starting_desk(true, Some("消えた"), &desks), 0);
        assert_eq!(starting_desk(true, None, &desks), 0);
        assert_eq!(starting_desk(false, Some("check"), &desks), 0, "it is turned off");
        assert_eq!(starting_desk(true, Some("conductor"), &[]), 0, "it does not crash on an empty list");
    }


    /// The wordmark's 3 lines must be the same width (mismatched widths look broken)
    #[test]
    fn the_wordmark_rows_line_up() {
        let w: Vec<usize> = WORDMARK.iter().map(|l| l.chars().count()).collect();
        assert!(
            w.iter().all(|n| *n == w[0]),
            "rows differ in width: {w:?}"
        );
    }



    #[test]
    fn phone_view_drops_trailing_blank_lines() {
        // Sending the terminal's blank lines as-is would hide the content on the phone
        let screen = "hello\nworld\n\n\n\n\n";
        assert_eq!(trim_for_phone(screen, 200), "hello\nworld");
        // Only the tail gets sent when it's too long
        let long: String = (1..=300).map(|i| format!("line{i}\n")).collect();
        let out = trim_for_phone(&long, 10);
        assert_eq!(out.lines().count(), 10);
        assert!(out.ends_with("line300"));
        assert_eq!(trim_for_phone("   \n\n", 200), "");
    }

    #[test]
    fn tab_starts_in_the_configured_folder() {
        let dir = std::env::temp_dir().join("shikisha-cwd-test");
        std::fs::create_dir_all(&dir).unwrap();
        let opts = tab::TabOptions {
            cwd: Some(dir.clone()),
            ..Default::default()
        };
        // Each shell's own way of saying where it is standing
        let argv = match cfg!(windows) {
            true => vec!["cmd.exe".to_string(), "/c".into(), "cd".into()],
            false => vec!["sh".to_string(), "-c".into(), "pwd".into()],
        };
        let mut t = Tab::spawn("cwd".into(), &argv, None, 10, 60, opts).unwrap();
        // Waited for rather than slept through, for the same reason as
        // `a_prompt_is_typed_first_and_submitted_after`: how long a real shell
        // takes to print its first line is a fact about how busy the machine
        // is, and a fixed number is a guess at it
        let mut screen = String::new();
        for _ in 0..60 {
            screen = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents();
            if screen.contains("shikisha-cwd-test") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        t.kill();
        assert!(
            screen.contains("shikisha-cwd-test"),
            "it starts in the working folder it was given: {screen}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A folder that is not on this machine stops the launch.
    ///
    /// This test used to promise the opposite — that a tab starts anyway,
    /// "easier to recover from than a launch failure". It was not a recovery.
    /// The folder was dropped and the command ran in the app's own folder, so
    /// an agent configured for one project came up somewhere else entirely,
    /// with a full set of permissions and nothing on screen saying so. There is
    /// no recovering from work done in the wrong place.
    ///
    /// What the person gets instead is a tab that holds its screen and says
    /// which folder is missing (see `tab::held_tests`), and a folder that turns
    /// up later restarts it on its own.
    #[test]
    fn a_folder_that_is_not_here_stops_the_launch() {
        let opts = tab::TabOptions {
            cwd: Some(std::path::PathBuf::from("Z:/does/not/exist")),
            ..Default::default()
        };
        let argv = vec![crate::test_shell()];
        let out = Tab::spawn("nowhere".into(), &argv, None, 10, 60, opts);
        assert!(out.is_err(), "it must not start in a folder that does not exist");
    }

    #[test]
    fn hot_reload_applies_changes_without_restarting_untouched_tabs() {
        let desk0 = desk_from_json(
            r#"{"desks":[{"name":"T","folders":[{"tabs":[
                {"name":"one","command":"<sh>"},
                {"name":"two","command":"<sh>"}
            ]}]}]}"#,
        );
        let mut tabs = Vec::new();
        let mut errs = Vec::new();
        spawn_desk(&desk0, 24, 80, &mut tabs, &mut errs, None);
        assert_eq!(tabs.len(), 2, "{errs:?}");
        let one_before = tabs[0].signature();

        // one: gains a lock (applies immediately) / two: removed / three: added
        let desk1 = desk_from_json(
            r#"{"desks":[{"name":"T","folders":[{"tabs":[
                {"name":"one","command":"<sh>","locked":true},
                {"name":"three","command":"<sh>"}
            ]}]}]}"#,
        );
        let msg = apply_ws_config(&mut tabs, &desk1, 24, 80, &mut errs, &mut Default::default(), None);

        assert_eq!(
            tabs.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
            vec!["one", "three"],
            "in the order of the settings"
        );
        assert!(tabs[0].locked, "locking takes effect without a restart");
        assert!(!tabs[0].needs_restart, "no restart needed if the launch settings are the same");
        assert_eq!(tabs[0].signature(), one_before, "the running session is kept");
        assert!(msg.contains("added 1") && msg.contains("stopped 1"), "{msg}");

        // A change to the encoding requires a rebuild, so it gets deferred and flagged
        let desk2 = desk_from_json(
            r#"{"desks":[{"name":"T","folders":[{"tabs":[
                {"name":"one","command":"<sh>","encoding":"shift_jis"},
                {"name":"three","command":"<sh>"}
            ]}]}]}"#,
        );
        let msg2 = apply_ws_config(&mut tabs, &desk2, 24, 80, &mut errs, &mut Default::default(), None);
        assert!(tabs[0].needs_restart, "it is marked as needing a restart");
        assert!(msg2.contains("1 need a restart"), "{msg2}");

        for t in tabs.iter_mut() {
            t.kill();
        }
    }

    #[test]
    fn scrollback_view_shows_history() {
        let mut p = parser_with_lines(5, 20, 30);
        p.screen_mut().set_scrollback(10);
        let contents = p.screen().contents();
        assert!(
            contents.contains("line17"),
            "earlier lines should be visible: {contents}"
        );
        assert!(
            !contents.contains("line30"),
            "the latest line should be off screen: {contents}"
        );
    }

    #[test]
    fn extract_lines_from_scrollback() {
        let mut p = parser_with_lines(5, 20, 30);
        // The bottom row (d=0) is the blank prompt line. d=1 is line30, d=3 is line28.
        let text = extract_text(&mut p, 1, 3, 20);
        assert_eq!(text, "line28\nline29\nline30\n");
        // The scroll position is restored after extraction
        assert_eq!(p.screen().scrollback(), 0);
    }

    #[test]
    fn extract_joins_wrapped_lines() {
        // A 5-row screen: row0="abcdefghij" (wrapped), row1="KLMNO", row2 onward empty.
        // Counting from the bottom of the screen, the wrapped row is d=4, its continuation is d=3.
        let mut p = vt100::Parser::new(5, 10, 100);
        p.process(b"abcdefghijKLMNO\r\n");
        let text = extract_text(&mut p, 3, 4, 10);
        assert_eq!(text, "abcdefghijKLMNO\n");
    }
}

#[cfg(test)]
mod shutdown_tests {
    /// A shell that says it closed ends the run.
    ///
    /// Checked against the source because the loop is a single 4,000-line
    /// function: there is no seam to call into, and the promise is worth more
    /// than the purity of how it is checked. The window's half of it -- that
    /// closing is reported at all -- is checked where the window lives.
    #[test]
    fn a_shell_that_closed_ends_the_run() {
        let src = include_str!("runtime.rs");
        let mut lines = src.lines().map(str::trim);
        assert!(
            lines.any(|l| l == "if shell.mail().closed {") && lines.next() == Some("break;"),
            "closing does not end the loop"
        );
    }
}


#[cfg(test)]
mod git_line_tests {
    use super::*;

    fn repo() -> std::path::PathBuf {
        let at = std::env::temp_dir().join(format!("shikisha-gitline-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&at).unwrap();
        let git = |args: &[&str]| {
            let mut c = std::process::Command::new("git");
            c.arg("-C").arg(&at).args(args);
            assert!(crate::detach_console(&mut c).output().unwrap().status.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(at.join("a.txt"), "one").unwrap();
        at
    }

    fn job(dir: &std::path::Path, act: &str, method: &'static str, params: Vec<serde_json::Value>) -> GitJob {
        GitJob {
            panel: "p1".into(),
            act: act.into(),
            method,
            dir: dir.to_path_buf(),
            at: None,
            protect: Vec::new(),
            who: None,
            params,
            seq: 0,
        }
    }

    fn all(lines: &mut GitLines) -> Vec<serde_json::Value> {
        let until = Instant::now() + Duration::from_secs(20);
        let mut out = Vec::new();
        while !lines.waiting.is_empty() && Instant::now() < until {
            out.extend(lines.answers().iter().map(|s| serde_json::from_str::<serde_json::Value>(s).unwrap()));
            std::thread::sleep(Duration::from_millis(20));
        }
        out
    }

    /// A folder's questions are answered in the order they were asked, off
    /// this thread; a reading asked again since is drawn once, the newest
    #[test]
    fn a_folders_git_is_answered_in_order_and_only_the_newest_reading_is_drawn() {
        let dir = repo();
        let mut lines = GitLines::new();
        lines.ask(job(&dir, "status", "git_status", vec![]));
        lines.ask(job(&dir, "stage", "git_stage", vec![serde_json::json!(["a.txt"])]));
        lines.ask(job(&dir, "status", "git_status", vec![]));
        let got = all(&mut lines);
        let statuses: Vec<&serde_json::Value> = got.iter().filter(|g| g["act"] == "status").collect();
        assert_eq!(statuses.len(), 1, "an older reading was drawn too: {got:?}");
        // Asked after the stage, and answered after it: it sees the file staged
        assert_eq!(statuses[0]["data"][0]["staged"], true, "{got:?}");
        assert_eq!(statuses[0]["panel"], "p1", "the answer does not say which panel it is for");
        assert!(got.iter().any(|g| g["act"] == "stage" && g["ok"] == true), "{got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A line slow to answer is said to be slow, once, and what it says late
    /// is still handed over -- a commit whose hooks took two minutes landed
    #[test]
    fn a_slow_answer_is_said_to_be_slow_and_still_handed_over() {
        let mut lines = GitLines::new();
        let long_ago = Instant::now().checked_sub(GIT_PANEL_WAIT + Duration::from_secs(1)).unwrap();
        lines.waiting.push((99, "p1".into(), "commit".into(), long_ago, false));
        let said: Vec<serde_json::Value> = lines.answers().iter().map(|j| serde_json::from_str(j).unwrap()).collect();
        assert_eq!(said.len(), 1);
        assert_eq!(said[0]["slow"], true);
        assert!(lines.answers().is_empty(), "said to be slow twice");
        lines
            .tx
            .send(GitDone { seq: 99, panel: "p1".into(), act: "commit".into(), payload: serde_json::json!({"act": "commit", "ok": true}) })
            .unwrap();
        let late: Vec<serde_json::Value> = lines.answers().iter().map(|j| serde_json::from_str(j).unwrap()).collect();
        assert_eq!(late.len(), 1, "the late answer was dropped");
        assert_eq!(late[0]["ok"], true);
    }

    /// A question about a folder that is not a repository answers, and says why
    #[test]
    fn a_folder_that_is_no_repository_is_said_as_that() {
        let dir = std::env::temp_dir().join(format!("shikisha-norepo-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let mut lines = GitLines::new();
        lines.ask(job(&dir, "status", "git_status", vec![]));
        let got = all(&mut lines);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["ok"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

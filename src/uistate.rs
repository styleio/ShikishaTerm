//! Carries the state the screen needs, separated from how it looks.
//!
//! Nothing here says "how to display it." It only holds what's happening.
//! The receiving side decides the appearance.
//!
//! The reason for that separation: the same state is used by several places.
//!   - the window (our own native window)
//!   - the local terminal (the TUI we've had all along)
//!   - the phone (remote display)
//!
//! Previously the phone display was built separately, and as a result only
//! one side ended up broken. The ASCII art getting mangled, the lines all
//! running together — both happened because "show the screen" was written
//! twice. Route everything through here and it's written once.

use serde::Serialize;

/// State of a single tab
#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct TabState {
    /// 1-based tab number (same number a person presses)
    pub index: usize,
    pub name: String,
    /// Name referenced from automation
    pub id: Option<String>,
    /// WAIT / BUSY / DONE / QUESTION / EXIT, exactly as `TabState::label`
    /// spells them — the page uses this as a CSS class. Used to pick the display
    pub state: String,
    /// Human-readable state name (translated). Use this one for display
    pub state_label: String,
    /// Detection profile name (Codex CLI, etc). Lets you eyeball whether it matched correctly
    pub profile: String,
    pub locked: bool,
    /// Chain depth. 0 = a conversation a human started
    pub depth: u32,
    /// Recent output volume (old to new, each 0..=7). Rendered as a bar graph
    pub activity: Vec<u8>,
    /// Which folder it works in, as a position in `UiState::groups`. Absent for
    /// a tab that is in no folder, which is drawn under no heading
    #[serde(default)]
    pub group: Option<usize>,
    /// "pty" or "browser". Changes how it's displayed
    pub kind: String,
    /// This pty tab is a model bridge (OpenAI-compatible API). The shell offers
    /// a chat input box for it instead of leaving it as a silent idle screen.
    #[serde(default)]
    pub model: bool,
    /// A chat reply is being generated right now (drives the shell's spinner).
    #[serde(default)]
    pub busy: bool,
    /// The settings page. It rides in the pane list like a browser, but the
    /// shell keeps it out of the tab strip and reaches it via a fixed gear.
    #[serde(default)]
    pub settings: bool,
    /// Which AI this tab runs (claude / codex / gemini / deepseek / …), if any.
    /// A plain identity, not a look: the display side turns it into a brand
    /// colour so the "run several AIs side by side" story reads at a glance.
    #[serde(default)]
    pub ai: Option<String>,
    /// Whether this tab acts without pausing for confirmation — the prerequisite
    /// for driving another tab (operate). The shell greys out / blocks picking an
    /// operate target when the operator (active tab) can't. Always false for a
    /// browser or the settings pane.
    #[serde(default)]
    pub auto: bool,
    /// Whether relaunching this makes any sense. A session always can be; a
    /// placed page can be reopened at the URL it started on; the app's own
    /// furniture (the settings form, the result view) cannot, because there is
    /// nothing behind it to put back.
    ///
    /// Per tab rather than one flag for the focused one, because the ↻ lives
    /// in each pane's caption now: a control that appears where it would do
    /// nothing is worse than one that is not offered.
    #[serde(default)]
    pub restartable: bool,
    /// What the thing in this tab says it is doing, in its own words.
    ///
    /// The state dot is read off the screen and can only ever say "busy" or
    /// "waiting"; this is the agent telling us "running tests, 3 of 5". The
    /// newest one is what the tab row has room for
    #[serde(default)]
    pub status: Option<String>,
    /// How far along, 0..=1, when it has said. Shown beside the status
    #[serde(default)]
    pub progress: Option<f32>,
    /// Where this tab is: the branch, its pull request, the ports it opened.
    ///
    /// A different kind of thing from `status`, which is why it gets its own
    /// line. That one is what the tab last *said*; this is where it *is*, and
    /// it stays true while nothing is being said at all.
    ///
    /// Sent in pieces rather than as one line, because how they share a narrow
    /// row is the display's business: a branch name can be any length, and the
    /// short precious parts beside it must not be the ones that get cut
    #[serde(default)]
    pub place: Option<PlaceState>,
    /// What it is costing right now: processor and memory. Its own value, not
    /// part of `place` -- where a tab is and what it is spending are different
    /// questions, and this one changes every couple of seconds
    #[serde(default)]
    pub cost: Option<String>,
    /// Whether what was said here can be read back as text (reader.rs). The
    /// display offers the reader only where there is something to read
    #[serde(default)]
    pub readable: bool,
}

/// The Vault overlay's contents: what was searched and what turned up.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct VaultState {
    pub query: String,
    pub hits: Vec<crate::vault::Hit>,
    /// True when the search stopped before the end -- so the overlay can say
    /// "more than these" rather than implying it is the whole of the past
    pub capped: bool,
}

/// Where a tab is, in the parts it is made of.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct PlaceState {
    #[serde(default)]
    pub branch: Option<String>,
    /// Already written the way it reads: `#12`, `#12 merged`
    #[serde(default)]
    pub pr: Option<String>,
    #[serde(default)]
    pub ports: Vec<u16>,
}

/// The colours a project is given when nobody has chosen one. Eight, because
/// past that they stop being telling apart and start being decoration
pub const PALETTE: [&str; 8] = [
    "#d97757", "#19c37d", "#4285f4", "#a06bff",
    "#e0a80a", "#12b3a8", "#e5644d", "#7f8cff",
];

/// A folder, as a heading over the tabs working in it.
///
/// A group is a folder, so this is worked out from where the tabs actually
/// are rather than from what the settings say -- a tab that ended up
/// somewhere else is somewhere else, and a list that insisted otherwise
/// would be a list that lies.
///
/// Nothing is drawn when there is only one: someone who has never asked for a
/// second folder should not have to learn that the first one has a name.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct GroupState {
    /// The heading: what someone named it, else the branch, else the folder
    pub name: String,
    /// The whole path, for the tooltip
    pub folder: String,
    /// The colour of this folder's project, ready to draw. Folders sharing one
    /// are branches of one repository, and the list draws them as a family.
    /// Absent when the folder is not in a repository at all -- there is no
    /// family to belong to, so there is nothing for a colour to say
    #[serde(default)]
    pub color: Option<String>,
    /// Whether this folder is a branch cut from the family's checkout
    #[serde(default)]
    pub linked: bool,
    /// Whether the folder is actually on this machine. Settings travel between
    /// PCs on a sync folder or a stick, and a path that is right on one of them
    /// is simply not there on the other -- which the list has to say, because
    /// the tabs in that folder are being held back rather than run
    #[serde(default)]
    pub health: crate::folders::Health,
}

impl GroupState {
    /// The folders these tabs are in, in the order they first appear, each
    /// paired with the folder itself so a tab can find its own.
    ///
    /// Tabs that are in no folder at all -- a browser is in none -- get no
    /// heading and belong to nothing, which is why the answer is looked up by
    /// path rather than handed out by position
    pub fn all(
        tabs: &[crate::tab::Tab],
        chosen: &std::collections::HashMap<String, String>,
    ) -> Vec<(std::path::PathBuf, GroupState)> {
        let mut out: Vec<(std::path::PathBuf, GroupState)> = Vec::new();
        for t in tabs {
            let Some(cwd) = t.cwd() else { continue };
            if out.iter().any(|(k, _)| k == cwd) {
                continue;
            }
            // What to call it: what someone typed, else the branch it is on,
            // else the folder's own name. A branch first because with several
            // of them open, the branch is the thing that tells them apart
            let name = t
                .group_name()
                .map(str::to_string)
                .filter(|n| !n.trim().is_empty())
                .or_else(|| t.place.branch.clone())
                .or_else(|| cwd.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_default();
            out.push((
                cwd.to_path_buf(),
                GroupState {
                    name,
                    folder: cwd.display().to_string(),
                    color: t
                        .place
                        .family
                        .as_deref()
                        .map(|f| Self::color_of(f, chosen)),
                    linked: t.place.linked,
                    // Filled in by whoever is drawing: whether a folder is
                    // here is a question for the disk, and the disk is asked
                    // away from the list being built
                    health: Default::default(),
                },
            ));
        }
        out
    }

    /// The colour a project is drawn in: the one someone chose for it, or one
    /// picked from its own name.
    ///
    /// Derived when nobody has said, so a family has a colour without anyone
    /// being asked for one, and the same colour every time it is asked. Chosen
    /// when someone has -- the answer is kept against the folder git shares,
    /// so every branch of the project changes together
    pub fn color_of(family: &std::path::Path, chosen: &std::collections::HashMap<String, String>) -> String {
        let key = family.display().to_string();
        if let Some(c) = chosen.get(&key).or_else(|| chosen.get(&key.to_lowercase())) {
            if !c.trim().is_empty() {
                return c.trim().to_string();
            }
        }
        let mut h: u32 = 2166136261;
        for b in key.to_lowercase().bytes() {
            h = (h ^ b as u32).wrapping_mul(16777619);
        }
        PALETTE[(h % PALETTE.len() as u32) as usize].to_string()
    }
}

/// What making a branch would do, answered while the name is being typed.
///
/// The dialog shows where the folder will be and the command that will make
/// it, and both come from here -- worked out by the same code that will run
/// it, never by the page guessing at the same rules a second time.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct BranchPlan {
    /// The folder it would be cut from, as the page named it
    pub from: String,
    /// What has been typed so far, so a late answer to an old question can be
    /// told from the answer to this one
    pub branch: String,
    pub folder: String,
    /// The command, exactly as it will run
    pub line: String,
    /// The name that was asked about, exactly as it was sent -- empty when
    /// nobody has typed one. What came back is only about this question, and
    /// the dialog has to be able to tell that
    #[serde(default)]
    pub asked: String,
    /// What the new branch will grow from
    #[serde(default)]
    pub base: String,
    /// The others it could grow from instead, best first
    #[serde(default)]
    pub bases: Vec<String>,
    /// What the new folder will not have and cannot get from git -- the
    /// ignored things that are actually there, offered to come along
    #[serde(default)]
    pub carry: Vec<crate::worktree::Carry>,
    /// Why it cannot be done, when it cannot
    #[serde(default)]
    pub error: Option<String>,
    /// Set once it has actually been made
    #[serde(default)]
    pub done: bool,
}

/// Folders to choose from, when somewhere new is being opened.
///
/// The list is made here rather than by a dialog the operating system draws,
/// because half the people using this are holding a phone and there is no
/// dialog to draw for them. One list, walked the same way from either.
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct BrowseState {
    /// Where the list is standing. Empty means the top, where the drives are
    pub at: String,
    /// The folder above, when there is one
    #[serde(default)]
    pub up: Option<String>,
    /// What is inside, folders only -- files are not somewhere to work
    pub dirs: Vec<String>,
    /// Why nothing is listed, when nothing is
    #[serde(default)]
    pub error: Option<String>,
}

impl BrowseState {
    /// Where the walk starts: the person's own folder, then the drives.
    ///
    /// Their own first, because that is where work is, and a list that opens
    /// on `A:` makes everyone scroll past floppy disks to reach it
    fn top() -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(home) = std::env::var("USERPROFILE") {
            if !home.is_empty() {
                out.push(home);
            }
        }
        for letter in 'A'..='Z' {
            let root = format!("{letter}:\\");
            if std::path::Path::new(&root).is_dir() {
                out.push(root);
            }
        }
        out
    }

    /// What is inside a folder, or the drives when nothing is named.
    ///
    /// Folders only, in the order a person reads them, and capped: a folder
    /// with fifty thousand entries in it is not a list anyone scrolls, and
    /// building it would stall the frame it was asked in
    pub fn of(path: &str) -> Self {
        let at = path.trim().to_string();
        if at.is_empty() {
            return Self { at, up: None, dirs: Self::top(), error: None };
        }
        let here = std::path::Path::new(&at);
        // A drive has no folder above it, but there is still somewhere to go
        // back to -- the list of drives itself. Without this, stepping into
        // one is a door that only opens inwards
        let up = Some(here.parent().map(|p| p.display().to_string()).unwrap_or_default());
        let mut dirs = Vec::new();
        let mut error = None;
        match std::fs::read_dir(here) {
            Ok(entries) => {
                for e in entries.flatten().take(4000) {
                    let p = e.path();
                    // Skip what the person cannot open anyway, and the places
                    // tools keep their own things
                    let hidden = p
                        .file_name()
                        .map(|n| {
                            let n = n.to_string_lossy();
                            n.starts_with('.') || n.starts_with('$')
                        })
                        .unwrap_or(false);
                    if hidden || !p.is_dir() {
                        continue;
                    }
                    dirs.push(p.display().to_string());
                    if dirs.len() >= 400 {
                        break;
                    }
                }
                dirs.sort_by_key(|d| d.to_lowercase());
            }
            Err(e) => error = Some(e.to_string()),
        }
        Self { at, up, dirs, error }
    }
}

/// Current position of the automation ring
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct BallState {
    /// Tab currently holding it (0 = the human)
    pub holder: usize,
    /// Where it was thrown from most recently
    pub from: usize,
    pub depth: u32,
    pub max: u32,
    /// "idle" / "flying" / "caught" / "held"
    pub phase: String,
    /// Progress while in flight, 0.0..=1.0
    pub progress: f32,
    /// Waiting for a human to add to the draft
    pub awaiting_human: bool,
}

/// Row of controls shown above the browser.
///
/// Whether to show them at all is decided by settings or Lua; whether each
/// one is pressable is answered by the browser. If something is shown as
/// pressable when it can't go back, the person who pressed it thinks
/// something broke
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct NavState {
    pub back: bool,
    pub forward: bool,
    pub reload: bool,
    /// The second reload, which throws away what is held first
    #[serde(default)]
    pub reload_hard: bool,
    /// URL field (how a person navigates to an arbitrary page)
    pub edit: bool,
    pub can_back: bool,
    pub can_forward: bool,
    /// Where it's currently open
    pub at: String,
    /// Whether it's loading (only for top-frame navigation; doesn't fire for in-SPA navigation)
    #[serde(default)]
    pub loading: bool,
}

/// Everything shown on screen, all in one place.
///
/// Words about appearance (color, width, symbols) don't belong here.
/// The moment one goes in, every receiving side is locked into the same look
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct UiState {
    pub workspace: String,
    /// The folders this workspace's tabs are working in. One means nothing is
    /// drawn: the heading only exists to tell folders apart
    #[serde(default)]
    pub groups: Vec<GroupState>,
    /// The answer to "what would happen if I made this branch"
    #[serde(default)]
    pub branch: Option<BranchPlan>,
    /// Folders to choose from, while somewhere new is being opened
    #[serde(default)]
    pub browse: Option<BrowseState>,
    pub workspaces: Vec<String>,
    pub ws_index: usize,
    /// What the focused pane is showing (0 = nothing is in it yet)
    pub active: usize,
    /// Whether INDEX is covering the window. A screen, not a pane: the board
    /// is a view OF the running things rather than one of them, so it has no
    /// place in a layout of them
    #[serde(default)]
    pub board: bool,
    /// Whether the settings form is covering the window. Also a screen rather
    /// than a pane: it asks about the whole app, not about one corner of it
    #[serde(default)]
    pub settings_open: bool,
    pub auto_enabled: bool,
    pub remote_on: bool,
    /// Whether at least one phone/browser is currently connected over the remote
    /// link. Drives the window's "remote connected — click to disconnect" pill.
    #[serde(default)]
    pub remote_conn: bool,
    /// Whether the pairing is a fixed token (config remote.sticky_token). The
    /// disconnect cuts the same either way; what differs is what comes after,
    /// so the button has to say which one it is rather than claim the other.
    #[serde(default)]
    pub remote_sticky: bool,
    /// What the focused tab is aimed at, as a screen number, when it has been
    /// aimed at anything. The picker on screen is the only place an aim is
    /// chosen and it is written down against that tab, so this is how it comes
    /// back after a restart -- not a second setting to keep in step with.
    #[serde(default)]
    pub aim: Option<usize>,
    /// First launch, before any settings exist yet
    pub first_run: bool,
    pub tabs: Vec<TabState>,
    pub ball: BallState,
    /// Transient notification (saved, emergency stop, etc.)
    pub flash: Option<String>,
    /// Whether help is being shown
    pub help_open: bool,
    /// What this whole app is costing the machine, for the board header --
    /// honest about our own weight rather than leaving it to a task manager
    #[serde(default)]
    pub self_cost: Option<String>,
    /// The Vault, when its overlay is open: a query and what it found. Absent
    /// the rest of the time, so the state stays small
    #[serde(default)]
    pub vault: Option<VaultState>,
    /// The help itself: the keys in force, paired with the dictionary key for
    /// the line describing each. Built from the same table the window
    /// dispatches on, so a rebound key cannot leave the help telling people to
    /// press something that no longer does anything
    #[serde(default)]
    pub help_rows: Vec<(String, String)>,
    /// Whether the workspace picker is being shown
    pub ws_open: bool,
    /// If a QR for phone pairing is being shown, the destination it encodes
    pub qr: Option<String>,
    /// The QR image itself (inline SVG). Making it a separate image request
    /// meant that of the two servers serving the same screen (window and
    /// phone), only one could actually render the image — opening it from
    /// the phone produced a broken link. Carrying it in `state` lets either
    /// side draw the same thing, and no second request is needed
    #[serde(default)]
    pub qr_svg: Option<String>,
    /// Controls shown above the browser being viewed (None = don't show)
    pub nav: Option<NavState>,
    /// How many lines back from the current screen we're scrolled (0 = current).
    /// Without knowing we've scrolled back, it looks like output has stopped
    pub scrolled: usize,
    /// Which build this is (lets you confirm you're not looking at a stale executable)
    pub build: String,
    /// Whether what's being viewed can be put back the way it started: a session
    /// relaunches its command, a page reopens exactly as it was opened. False on
    /// the board, and on the app's own screens (settings, results). The screen
    /// shows its restart button from this rather than working it out again
    #[serde(default)]
    pub restartable: bool,
    /// If the current workspace is an AI-vs-AI discussion, the session number
    /// (1-based) of the opening speaker. The dashboard shows a "start the
    /// discussion" card that sends the typed topic there. None = not a discussion.
    #[serde(default)]
    pub discuss_start: Option<usize>,
    /// The opening speaker's display name, for the start card's label.
    #[serde(default)]
    pub discuss_start_name: Option<String>,
    /// Whether the discussion is currently at rest: nobody is generating and
    /// the automation ring is idle. When true, the shell floats a prominent
    /// "pose a topic" banner over whatever tab is in view, so you never have to
    /// hunt for the opening speaker. While a participant is speaking it hides,
    /// so the AI screens are never covered. Only meaningful with `discuss_start`.
    #[serde(default)]
    pub discuss_idle: bool,
}

/// Whether this tab's conversation can be read back as flowing text.
///
/// True when the CLI keeps a record we know how to find and we know which
/// conversation is this tab's — claude and codex today. It rides in the tab
/// state rather than in the phone's own snapshot because the reader is a
/// property of the tab, and both surfaces ask the same question of it
fn readable(t: &crate::tab::Tab) -> bool {
    t.session.is_some() && t.resume.as_ref().is_some_and(|r| r.verify.is_some())
}

impl TabState {
    /// Build from a running tab
    pub fn of(index: usize, t: &crate::tab::Tab) -> Self {
        Self {
            index,
            name: t.title.clone(),
            id: t.id.clone(),
            state: t.state.label().to_string(),
            state_label: t.state.display(),
            profile: t.profile_name().to_string(),
            locked: t.locked,
            depth: t.chain_depth,
            activity: t.activity().to_vec(),
            group: None,
            kind: "pty".into(),
            restartable: true,
            model: t.is_model(),
            busy: t.is_generating(),
            settings: false,
            ai: t.ai_kind(),
            auto: t.auto_runs(),
            status: t.status_line(),
            progress: t.progress.as_ref().map(|(p, _)| *p),
            place: (t.place != crate::repo::Place::default()).then(|| PlaceState {
                branch: t.place.branch.clone(),
                pr: t.place.pr.clone(),
                ports: t.place.ports.clone(),
            }),
            cost: t.usage.line(),
            readable: readable(t),
        }
    }

    /// Build from a browser placed inside the window.
    ///
    /// It isn't a session, so it has no state and no output volume.
    /// Rather than pad it out to look similar, it's more readable left as-is
    pub fn browser(index: usize, key: &str, name: &str) -> Self {
        Self {
            index,
            name: name.to_string(),
            id: Some(key.to_string()),
            state: "WEB".into(),
            state_label: crate::i18n::t("tui.state.web"),
            profile: String::new(),
            locked: false,
            depth: 0,
            activity: Vec::new(),
            group: None,
            kind: "browser".into(),
            // The same two keys `main::restartable_page` refuses, and for the
            // same reason: they are opened and closed by the app, so "open it
            // again" is not a thing a person can want from them
            restartable: key != "settings" && key != "result",
            status: None,
            progress: None,
            // A browser is not in a folder and starts nothing, so it has
            // nowhere to be, and nothing of its own to cost
            place: None,
            cost: None,
            model: false,
            busy: false,
            settings: key == "settings",
            ai: None,
            auto: false,
            // Nothing was said here to read back
            readable: false,
        }
    }
}

impl BallState {
    pub fn of(b: &crate::ball::Ball, max: u32, now_ms: u64) -> Self {
        use crate::ball::Phase;
        let (phase, progress) = match b.phase(now_ms) {
            Phase::Idle => ("idle", 0.0),
            Phase::Flying { progress, .. } => ("flying", progress),
            Phase::Caught { .. } => ("caught", 1.0),
            Phase::Held { .. } => ("held", 1.0),
        };
        Self {
            holder: b.holder,
            from: b.from,
            depth: b.depth,
            max,
            phase: phase.into(),
            progress,
            awaiting_human: b.awaiting_human,
        }
    }
}

impl UiState {
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms a browser placed inside the window gets the next number in
    /// the tab sequence.
    ///
    /// It used to have no row in either the list or the board, and couldn't
    /// be switched to. Showing/hiding it was a separate operation, Ctrl+B o,
    /// which called it "part of the tabs" while not actually being a tab.
    ///
    /// If the numbering isn't contiguous, the number a person presses no longer matches the contents
    #[test]
    fn a_browser_takes_the_next_tab_number() {
        let a = TabState::browser(3, "shop", "通販サイト");
        let b = TabState::browser(4, "mail", "メール");
        assert_eq!((a.index, b.index), (3, 4));
        assert_eq!(a.kind, "browser", "セッションと同じ見せ方になっている");
        assert_eq!(a.id.as_deref(), Some("shop"), "自動化から指す名前が違う");
        // The human-readable name and the name automation refers to are different things
        assert_eq!(a.name, "通販サイト", "設定した表示名が出ていない");
        // It isn't a session, so don't pad it out to look like one
        assert!(a.activity.is_empty() && a.profile.is_empty() && a.depth == 0);
        assert!(!a.locked);
    }


    /// A tab that is really running, in a folder of its own.
    fn in_folder(folder: &str, named: Option<&str>) -> crate::tab::Tab {
        let dir = std::env::temp_dir().join(format!("shikisha-group-{folder}"));
        std::fs::create_dir_all(&dir).unwrap();
        let opts = crate::tab::TabOptions {
            cwd: Some(dir),
            group: named.map(str::to_string),
            ..Default::default()
        };
        crate::tab::Tab::spawn("t".into(), &["cmd.exe".to_string()], None, 6, 40, opts).unwrap()
    }

    #[test]
    fn the_folders_are_the_ones_the_tabs_are_actually_in() {
        // Two tabs in one folder are one heading, not two: what groups them is
        // the folder, so nothing has to be declared for them to be together
        let mut tabs = vec![in_folder("one", None), in_folder("one", None), in_folder("two", None)];
        let found = GroupState::all(&tabs, &Default::default());
        for t in tabs.iter_mut() {
            t.kill();
        }
        assert_eq!(found.len(), 2, "フォルダの数だけ");
        assert!(found[0].0.ends_with("shikisha-group-one"));
        assert_eq!(found[0].1.name, "shikisha-group-one", "名前が無ければフォルダ自身の名前");
        // Not in a repository, so it belongs to no family and has no colour
        assert_eq!(found[0].1.color, None);
        assert!(!found[0].1.linked);
    }

    #[test]
    fn a_folder_someone_named_is_called_that() {
        let mut tabs = vec![in_folder("named", Some("feature/login"))];
        let found = GroupState::all(&tabs, &Default::default());
        tabs[0].kill();
        assert_eq!(found[0].1.name, "feature/login");
    }

    #[test]
    fn folders_can_be_walked_into_and_back_out_of() {
        let root = std::env::temp_dir().join(format!("shikisha-browse-{}", crate::random_hex(6)));
        std::fs::create_dir_all(root.join("work").join("inner")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        std::fs::write(root.join("notes.txt"), "x").unwrap();

        let at = BrowseState::of(&root.display().to_string());
        // Folders only: a file is not somewhere to work, and the places tools
        // keep their own things are not either
        assert_eq!(at.dirs.len(), 1, "{:?}", at.dirs);
        assert!(at.dirs[0].ends_with("work"));
        assert_eq!(at.up.as_deref(), Some(root.parent().unwrap().display().to_string().as_str()));

        // The top is the drives, and every drive can get back to it
        let top = BrowseState::of("");
        assert!(top.up.is_none(), "一番上には戻る先が無い");
        assert!(!top.dirs.is_empty(), "ドライブが出ている");
        let drive = BrowseState::of(&top.dirs.last().cloned().unwrap());
        assert!(drive.up.is_some(), "ドライブから一覧へ戻れる");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn one_project_gets_one_colour_and_two_get_two() {
        // What the list draws branches of one repository with. The answer has
        // to be the same every time it is asked, or the colours would move
        // around as tabs open and close
        let none = Default::default();
        let a = std::path::Path::new("D:/work/myproject/.git");
        let b = std::path::Path::new("D:/work/other/.git");
        assert_eq!(GroupState::color_of(a, &none), GroupState::color_of(a, &none));
        assert!(PALETTE.contains(&GroupState::color_of(a, &none).as_str()));
        assert_ne!(GroupState::color_of(a, &none), GroupState::color_of(b, &none));

        // Someone said which colour they wanted, and that is the answer for
        // every branch of that project -- the key is the folder they share
        let mut chosen = std::collections::HashMap::new();
        chosen.insert(a.display().to_string(), "#123456".to_string());
        assert_eq!(GroupState::color_of(a, &chosen), "#123456");
        assert_ne!(GroupState::color_of(b, &chosen), "#123456", "他所の色まで変えない");
    }

    fn tab(index: usize, name: &str) -> TabState {
        TabState {
            index,
            name: name.into(),
            id: None,
            state: "WAIT".into(),
            state_label: "WAIT".into(),
            profile: "GENERIC".into(),
            locked: false,
            depth: 0,
            activity: vec![0; 4],
            group: None,
            kind: "pty".into(),
            restartable: true,
            model: false,
            busy: false,
            settings: false,
            ai: None,
            auto: false,
            status: None,
            progress: None,
            place: None,
            cost: None,
            readable: false,
        }
    }

    /// Confirms appearance never leaks into state.
    ///
    /// Putting a color or symbol here would lock the window, TUI, and phone
    /// all into the same look. Keeping them separate lets each pick its own
    #[test]
    fn the_state_carries_no_appearance() {
        let s = UiState {
            workspace: "検証".into(),
            tabs: vec![tab(1, "実装")],
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        for looky in ["color", "#", "rgb", "▁", "●", "width", "px"] {
            assert!(
                !json.contains(looky),
                "見た目が混ざっている ({looky}): {json}"
            );
        }
    }



    /// Confirms identical states compare equal.
    ///
    /// If we sent every frame, an unchanged screen would still trigger a
    /// redraw. Making it comparable lets us send only when it actually changed
    #[test]
    fn identical_states_compare_equal() {
        let a = UiState {
            tabs: vec![tab(1, "実装")],
            ..Default::default()
        };
        let mut b = a.clone();
        assert_eq!(a, b);
        b.tabs[0].state = "BUSY".into();
        assert_ne!(a, b, "状態が変わったのに同じと判定された");
    }
}

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
    RESULT_TAB, ScreenPush, Size, Surface, Ui, pty_dims, remote_floor, screen_push, surfaces_of,
    terminal_size, title_of,
};
use crate::workspace::{
    apply_ws_config, build_engine, extract_env_block, open_declared_browsers, panel_places,
    spawn_workspace, surface_of_id, switch_workspace,
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
    workspace::{TabAuto, automation_by_pane, carried_conversation},
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
/// nowhere quietly answers about the app's own repository
pub fn tab_places(tabs: &[Tab]) -> Vec<hooks::TabPlace> {
    tabs.iter()
        .map(|t| {
            let dir = match t.cwd().map(std::path::Path::to_path_buf) {
                Some(p) if p.is_absolute() => p,
                Some(p) => std::env::current_dir().map(|c| c.join(&p)).unwrap_or(p),
                None => std::path::PathBuf::new(),
            };
            hooks::TabPlace {
                key: t.key(),
                dir,
                remote: t.remote().cloned(),
                protect: t.protect().to_vec(),
            }
        })
        .collect()
}
pub fn resume_plan(t: &Tab, alone: bool, keep: bool) -> (tab::Resume, Option<&'static str>) {
    if !keep {
        return (tab::Resume::Fresh, None);
    }
    let Some(spec) = t.resume.as_ref() else {
        return (tab::Resume::Fresh, Some("msg.resume.unsupported"));
    };
    // Nothing has happened in this tab yet, and it was having a conversation
    // when the app last closed. "Carry the conversation over" can only mean
    // that one — which is why this needs no key of its own
    let want = match (t.spoke(), t.previous.clone()) {
        (false, Some(before)) => Some(before),
        _ => t.session.clone(),
    };
    if let Some(s) = want {
        if !spec.with_id.is_empty() {
            // A conversation can be deleted between one run and the next. Ask
            // before handing the CLI an id it has never heard of: it would say
            // so in its own words, in red, in a place the person has no reason
            // to connect with the key they just pressed
            let gone = spec
                .verify
                .as_ref()
                .is_some_and(|v| !sessionfind::exists(v, &s.id));
            if gone {
                append_hook_log(&format!("\"{}\" no longer has {}", t.title, s.short()));
                return (tab::Resume::Fresh, Some("msg.resume.gone"));
            }
            return (tab::Resume::Id(s), None);
        }
    }
    if !spec.newest_here.is_empty() {
        if alone {
            return (tab::Resume::NewestHere, None);
        }
        return (tab::Resume::Fresh, Some("msg.resume.ambiguous"));
    }
    (tab::Resume::Fresh, Some("msg.resume.unknown"))
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
pub fn split_focused(
    l: &mut crate::layout::Layout,
    dir: crate::layout::Dir,
    surface_count: usize,
    active: usize,
) -> usize {
    let next = free_surface(l, surface_count, active);
    l.split(dir, next);
    l.focused_surface()
}
pub fn free_surface(l: &crate::layout::Layout, surface_count: usize, from: usize) -> usize {
    (1..=surface_count)
        .map(|n| (from + n) % (surface_count + 1))
        .find(|n| *n != 0 && l.pane_of(*n).is_none())
        .unwrap_or(0)
}
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
    let mut workspaces: Vec<config::Workspace> = Vec::new();
    if let Some(c) = &cfg {
        let (ws, errs) = c.resolve_workspaces();
        workspaces = ws;
        startup_errors.extend(errs);
        // Resolve and cache the model bridge's connection info (at this point encrypted
        // secrets aren't unlocked yet; it's resolved again below once the password is
        // confirmed, so plaintext secrets/no-auth setups are already covered here).
        bridge::set_providers(c, None);
    }

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
    let remembered = config::load_last_workspace();
    let mut ws_index = starting_workspace(
        cfg.as_ref().and_then(|c| c.restore_workspace).unwrap_or(true),
        remembered.as_deref(),
        &workspaces.iter().map(|w| w.name.clone()).collect::<Vec<_>>(),
    );
    if let Some(w) = workspaces.get(ws_index) {
        // Knowing where we started is a handy clue later, when tracking down "why is
        // this the screen we're on".
        append_hook_log(&format!(
            "Startup: workspace \"{}\" ({})",
            w.name,
            match remembered.as_deref() {
                Some(r) if r == w.name => "resuming last session",
                _ => "first workspace",
            }
        ));
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
        ssh::use_secrets(c.resolve_tokens(None));
    }
    let mut last_session = crate::lastsession::Saved::load();
    if !cmd_args.is_empty() {
        tabs.push(Tab::spawn(
            title_of(&cmd_args),
            &cmd_args,
            None,
            rows,
            cols,
            tab::TabOptions::default(),
        )?);
    } else if let Some(w) = workspaces.get(ws_index) {
        // If we're resuming where we left off, launch that same workspace too.
        // Hard-coding this to the first workspace would restore only the name while
        // showing a screen with different contents.
        spawn_workspace(w, rows, cols, &mut tabs, &mut startup_errors, Some(&last_session));
    }
    // No config yet = first run. Guide the user so the experience isn't just
    // "a single shell opens and nothing else happens", leaving them unsure what to do.
    let first_run = cmd_args.is_empty() && cfg.is_none();
    if tabs.is_empty() && workspaces.is_empty() {
        let argv = vec!["powershell.exe".to_string()];
        tabs.push(Tab::spawn(
            "SHELL".into(),
            &argv,
            None,
            rows,
            cols,
            tab::TabOptions::default(),
        )?);
    }

    // Re-fit the PTY size now that every tab exists
    (rows, cols) = pty_dims(shell.size()?);
    for t in &tabs {
        let _ = t.resize(rows, cols);
    }

    // The Lua hook engine is per-workspace (shared variables are scoped inside it too).
    // Unused workspaces don't get one built; it's created on demand when switched to.
    let mut max_chain = cfg.as_ref().and_then(|c| c.max_chain).unwrap_or(10);
    let mut done_confirm_ms = cfg
        .as_ref()
        .and_then(|c| c.done_confirm_ms)
        .unwrap_or(profile::DEFAULT_DONE_CONFIRM_MS);
    // If secrets are encrypted, ask for the master password at startup
    let mut password: Option<String> = None;
    if let Some(path) = cfg.as_ref().and_then(|c| c.secrets_path()) {
        if std::fs::read_to_string(&path)
            .map(|t| crypto::is_encrypted(&t))
            .unwrap_or(false)
        {
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
                            break;
                        }
                    }
                    // On cancel, continue without secrets (only notifications become unusable)
                    None => {
                        startup_errors
                            .push(i18n::t("prompt.password.skipped"));
                        break;
                    }
                }
            }
        }
    }

    // ...and the connections, for the same reason and at the same moment: what
    // was handed over before the prompt came from a store that could not be
    // opened yet, so an encrypted one had nothing in it
    if let Some(c) = cfg.as_ref() {
        ssh::use_secrets(c.resolve_tokens(password.as_deref()));
    }
    // Resolve the model bridge's connection info again now that the password is confirmed
    // (encrypted-secret keys get unlocked here too). Tabs spawned before the
    // prompt hold keys that could not be decrypted yet, so they are handed the
    // real ones here — otherwise they go on sending an empty bearer token (→ 401).
    if let Some(c) = &cfg {
        if password.is_some() {
            reload_providers(c, password.as_deref(), tabs.iter_mut());
        }
    }

    // Notification destinations (Slack / Telegram). Lua can only send to destinations
    // registered here.
    let mut notifier = match cfg.as_ref() {
        Some(c) => {
            let (dests, err) = c.resolve_notify(password.as_deref());
            if let Some(e) = err {
                startup_errors.push(e);
            }
            notify::Notifier::new(dests, c.primary_notify.clone())
        }
        None => notify::Notifier::new(Default::default(), None),
    };
    // Names inside the secrets file changed shape; a file written by an
    // earlier version is brought forward here rather than in the ordinary
    // migration steps, because those run before anyone has said the master
    // password and this one may have to open an encrypted store
    if let Some(c) = cfg.as_ref() {
        if let Some(path) = c.secrets_path() {
            match config::migrate_secrets(&path, password.as_deref(), &workspaces) {
                Ok(true) => append_hook_log("secrets: names brought forward to the new shape"),
                Ok(false) => {}
                Err(e) => startup_errors.push(format!("secrets: {e:#}")),
            }
        }
    }
    // Capabilities granted to automation (empty by default). An advanced feature that
    // can only be enabled by writing it into the config file.
    let caps: hooks::Caps = std::rc::Rc::new(match cfg.as_ref() {
        Some(c) => caps::Capabilities::new(
            c.capabilities.clone(),
            config_file_dir(),
            c.resolve_tokens(password.as_deref()),
            c.resolve_secret_terms(password.as_deref()),
            c.automation_permissions.clone(),
        ),
        None => caps::Capabilities::disabled(),
    });
    let mut engines: Vec<Option<HookEngine>> = (0..workspaces.len().max(1)).map(|_| None).collect();
    // If we have a window, put browsers inside it
    caps.set_host(shell.host());
    // ...and if pages can be drawn either here or on a connected device, the
    // person's setting says which
    shell.draw_pages(placed::Draw::of(
        cfg.as_ref().and_then(|c| c.browser_draw.as_deref()).unwrap_or_default(),
    ));
    caps.set_workspace(ws_index);
    if let Some(w) = workspaces.get(ws_index) {
        // A script's `token` means this workspace's, and no other's
        caps.set_workspace_id(&w.id);
        engines[ws_index] = build_engine(cfg.as_ref(), Some(w), &mut startup_errors, &caps);
        // Declared browsers are NOT opened here: placing a page occupies the
        // window thread, and at startup the person is often already clicking.
        // The board goes up first; the loop opens them right after (below).
    } else {
        engines[0] = build_engine(cfg.as_ref(), None, &mut startup_errors, &caps);
    }
    let mut open_browsers_after_first_paint = true;
    let mut first_paint_done = false;
    let slot = ws_index.min(engines.len().saturating_sub(1));
    let mut engine = engines[slot].take();
    // The current ad-hoc "operate a target" attachment, as (source pane, target),
    // so a repeated goal to the same target doesn't re-brief from scratch.
    let mut operating: Option<(usize, usize)> = None;
    // ✨ finished command suggestions arrive from worker threads (the
    // assistant AI call takes seconds); polled once per tick below
    let (suggest_tx, suggest_rx) = std::sync::mpsc::channel::<String>();
    // The git panel's slow half: fetch, pull and push answer from a thread
    let (git_tx, git_rx) = std::sync::mpsc::channel::<String>();
    // Everything the file panel asks of a server, which is all of it: a folder
    // on the far end is a network round trip and the window cannot wait for one
    let (sftp_tx, sftp_rx) = std::sync::mpsc::channel::<String>();
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
    // Whether automation may switch which tab is on screen (see ViewMove)
    let mut auto_switch = cfg.as_ref().and_then(|c| c.auto_switch).unwrap_or(true);
    // Whether the ✕ puts the window away rather than quitting (see the loop)
    let mut resident = cfg.as_ref().and_then(|c| c.resident).unwrap_or(true);
    // What Claude's subscription has left, on a thread of its own. Nothing
    // is asked until a Claude tab exists (limits::Meter::want)
    let limits = crate::limits::Meter::start();
    let mut claude_usage_on = cfg.as_ref().and_then(|c| c.claude_usage).unwrap_or(true);
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
    if let Some(ws) = workspaces.get(ws_index) {
        for t in tabs.iter_mut() {
            t.previous = last_session.conversation_for(&ws.name, t);
        }
        if let Some(saved) = last_session.panes_for(&ws.name) {
            // Whether those panes still point at surfaces that exist is not
            // decided here: the loop clamps the tree to what is on screen every
            // frame, which is the one place that knows
            pane_layout = saved;
            active = pane_layout.focused_surface();
        }
    }
    // When to next write down what is on screen. Rare events (a conversation
    // learned, a workspace switched) are worth writing at once; a divider being
    // dragged is not, and a delay keeps a drag from writing a file per frame
    let mut save_at: Option<std::time::Instant> = None;
    // The zoom level waiting to be written down, and when to write it
    let mut font_size: Option<u8> = None;
    let mut tab_width: Option<u16> = None;
    let mut font_save_at: Option<std::time::Instant> = None;
    let mut tab_save_at: Option<std::time::Instant> = None;
    // Whether the composer is shut, as the window's own page last said. The
    // pen a placed page draws for itself follows it
    let mut composer_shut = false;
    // The placed page currently showing that pen, if any
    let mut pen_shown: Option<String> = None;
    // A pane waiting for the tab it asked for, and how many surfaces there
    // were when it asked. Cleared when the tab arrives or the form is shut
    let mut awaiting_tab: Option<(u32, usize)> = None;
    // What was last written, so an unchanged screen writes nothing at all
    let mut last_saved: Option<(crate::layout::Layout, Vec<Option<tab::Session>>)> = None;
    // Which key does what, this run. Read once and re-read when the settings
    // change, the same as everything else that can be edited while running
    // When to look again at where the tabs are. Starts now so the first frame
    // already knows, rather than showing a sidebar that fills in a beat later
    let mut place_at = std::time::Instant::now();
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
    let mut thanks_show = false;
    // Where thanks would go: the Store's review page for the Store's copy, the
    // repository for the zip's
    let thanks_kind = if config::packaged() { "store" } else { "github" };
    // What the Vault overlay is showing right now: the last search and its
    // hits. Kept across frames so the results stay put until the next search,
    // and dropped from the state entirely while the overlay is closed
    let mut vault_view: Option<crate::uistate::VaultState> = None;
    // What making a branch would do. Answered while the name is being typed,
    // and cleared once the folder exists so the dialog can close itself
    let mut branch_view: Option<crate::uistate::BranchPlan> = None;
    // What it would take to have a missing working folder here. Answered when
    // one is opened, and cleared once the folder exists so the dialog closes
    let mut repair_view: Option<crate::uistate::RepairPlan> = None;
    // The folders being looked through, while somewhere new is being chosen
    let mut browse_view: Option<crate::uistate::BrowseState> = None;
    // What this whole app is costing the machine, refreshed on the same beat as
    // the per-tab figures. Shown in the board's header
    let mut self_cost: Option<String> = None;
    // Somewhere to ask about pull requests, on its own thread. Quiet and
    // harmless when the person has no GitHub token: it simply never knows
    // anything, and no row grows a line
    let prs = crate::pr::Watch::start();
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
    for e in &startup_errors {
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
    // Workspaces use a virtual-desktop model: switching means hiding, not stopping.
    // Each workspace keeps its own set of tabs, launched the first time it's activated.
    // Launched tabs live in `tabs`; the shelf reserves space for the remaining workspaces.
    let mut ws_tabs: Vec<Vec<Tab>> = Vec::new();
    // One pane tree per workspace, parked here while that workspace is off screen
    let mut ws_panes: Vec<crate::layout::Layout> = Vec::new();
    ws_tabs.resize_with(workspaces.len(), Vec::new);
    ws_panes.resize_with(workspaces.len(), || crate::layout::Layout::single(0));
    // Watch the config file for changes (saving takes effect without a restart)
    let mut watcher = watch::Watcher::new(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
    // Look for a newer version: now, and once a day while this runs. Looking
    // is all it does by itself; installing is a button on the settings screen
    update::start(cfg.as_ref().and_then(|c| c.update_check).unwrap_or(true));
    let mut cfg = cfg;

    let mut ws_open = false;
    let mut help_open = false;
    let mut qr_open = false;
    // While the settings overlay is up, automation (ball-follow, ShowTab) must
    // not yank the screen to another tab — settings is a place of its own, not a
    // tab you get pushed out of. Only an explicit human tab/workspace pick, or
    // "close settings", leaves it.
    let mut settings_open = false;
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

    loop {
        // Install the remote server the moment its background bind lands.
        // Errors and notes surface exactly as the old synchronous path did.
        if let Some(rx) = &remote_rx {
            if let Ok((ui, mut errs)) = rx.try_recv() {
                remote_ui = ui;
                remote_rx = None;
                // Pages drawn on a connected device are driven through this
                if let (Some(r), Some(line)) = (remote_ui.as_ref(), shell.far_pages()) {
                    r.set_page_line(line);
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
        }

        // Open the workspace's declared browsers on the iteration AFTER the
        // first full draw: the board answers clicks first, then the window
        // thread pays the (brief) cost of placing pages.
        if open_browsers_after_first_paint && first_paint_done {
            open_browsers_after_first_paint = false;
            if let Some(w) = workspaces.get(ws_index) {
                open_declared_browsers(w, &caps, &mut startup_errors);
            }
        }
        first_paint_done = true;

        // Point the remote settings proxy at the (loopback) config server. Starting
        // it here, lazily but eagerly-once, means the phone can open settings even
        // before anyone has opened it on the PC. The config UI stays on loopback.
        if !settings_linked {
            if let Some(r) = remote_ui.as_ref() {
                if let Ok(u) = ensure_web_url(&mut web, &config_file, &remote_info, &web_password, &caps) {
                    if let (Some(origin), Some(tok)) =
                        (u.split("/?").next(), u.split("token=").nth(1))
                    {
                        r.set_settings_backend(origin.to_string(), tok.to_string());
                        settings_linked = true;
                    }
                }
            }
        }

        // What's laid out on screen, in the order written in config.
        // The upper bound of pressable numbers needs more than just the session count.
        let hosted = caps.hosted_names();
        let titles: Vec<&str> = tabs.iter().map(|t| t.title.as_str()).collect();
        let surfaces = surfaces_of(workspaces.get(ws_index), &titles, &hosted);
        let surface_count = surfaces.len();
        // Keep the tree and `active` in step. Anything in the loop may set
        // `active` (a digit, an automation, the settings screen closing); the
        // focused pane follows it, and moving focus between panes sets `active`
        // at the point it happens. One sync point, so neither can drift.
        pane_layout.clamp(surface_count);
        if pane_layout.focused_surface() != active {
            pane_layout.show(active);
        }
        // Who the terminals are cut to, settled once per pass rather than by
        // whichever viewer last reported (see `terminal_size`). Both viewers
        // re-measure and re-report as they redraw, so reading it here — from
        // who is actually looking — is what keeps the two of them from taking
        // the terminal off each other.
        (rows, cols) = pty_dims(terminal_size(
            (shell.geom_rows(), shell.geom_cols()),
            shell.phone_size(),
            remote_ui.as_ref().is_some_and(|r| r.watched()),
        ));
        // This is the only place a terminal is resized — two places deciding
        // meant a split pane was told its size twice per frame, and whichever
        // ran last won.
        {
            let want = tab_sizes(
                tabs.len(),
                &pane_layout,
                &surfaces,
                &shell.geom_panes(),
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
        if watcher.changed() {
            if let Some(newcfg) = config::load() {
                let (new_ws, errs) = newcfg.resolve_workspaces();
                startup_errors.extend(errs);
                // Which workspace was active before this reload. Its live tabs are
                // in `tabs` (not the cache), so it's skipped when re-keying below.
                let prev_ws_index = ws_index;
                // The language is only read at startup, so changing it in settings
                // doesn't apply to the current screen. Add a note to the board's
                // notification prompting the user to close and reopen.
                // (the settings GUI's alert doesn't show inside the in-app WebView,
                // so we convey it here instead)
                let lang_restart = i18n::would_change(newcfg.language.as_deref());
                // Apply immediately to the workspace being viewed; others get it on switch
                let target = new_ws
                    .iter()
                    .position(|w| Some(&w.name) == workspaces.get(ws_index).map(|w| &w.name))
                    .unwrap_or(0);
                let mut msg = i18n::t("msg.config_reloaded");
                if let Some(w) = new_ws.get(target) {
                    msg = apply_ws_config(&mut tabs, w, rows, cols, &mut startup_errors);
                    ws_index = target;
                    // Bring browsers in line with config too: open added ones, close
                    // removed ones, redraw the bar and band. If reopening were required
                    // to take effect, editing settings would be pointless
                    // (pages already open are left untouched).
                    open_declared_browsers(w, &caps, &mut startup_errors);
                }
                // Re-key the cached background tabs by workspace NAME, not by
                // position. A reload can reorder workspaces (adding/moving one),
                // and a position-indexed cache would then hand a workspace another
                // one's tabs — the bug where switching to a freshly added workspace
                // showed a different one's tabs. Tabs whose workspace survives move
                // with it; a removed workspace's background tabs are killed; the
                // active workspace's tabs live in `tabs`, so its slot stays empty.
                let mut cached_by_name: std::collections::HashMap<String, Vec<Tab>> =
                    std::collections::HashMap::new();
                for (i, w) in workspaces.iter().enumerate() {
                    if i == prev_ws_index {
                        continue;
                    }
                    if let Some(slot) = ws_tabs.get_mut(i) {
                        let cached = std::mem::take(slot);
                        if !cached.is_empty() {
                            cached_by_name.insert(w.name.clone(), cached);
                        }
                    }
                }
                ws_tabs = new_ws
                    .iter()
                    .map(|w| cached_by_name.remove(&w.name).unwrap_or_default())
                    .collect();
                // Workspaces that vanished from config: their background tabs are done.
                for mut orphaned in cached_by_name.into_values() {
                    for t in orphaned.iter_mut() {
                        t.kill();
                    }
                }
                // The per-workspace Lua engine cache is indexed by position, and that
                // position shifts whenever workspaces are added/removed here. Reset it
                // to match the new count (all None) so switching to a newly added
                // workspace can't index out of bounds; each inactive workspace's engine
                // is rebuilt on demand on the next switch (the active one is rebuilt below).
                engines = (0..new_ws.len().max(1)).map(|_| None).collect();
                // The parked pane trees are indexed the same way, so they shift
                // with it. A tree kept against a moved position would divide the
                // wrong workspace into panes pointing at the wrong tabs, which
                // looks deliberate and is not — start those over instead.
                ws_panes = (0..new_ws.len().max(1))
                    .map(|_| crate::layout::Layout::single(0))
                    .collect();
                workspaces = new_ws;
                ai_choices = startable_ais();
                max_chain = newcfg.max_chain.unwrap_or(10);
                auto_switch = newcfg.auto_switch.unwrap_or(true);
                resident = newcfg.resident.unwrap_or(true);
                claude_usage_on = newcfg.claude_usage.unwrap_or(true);
                update::set_auto(newcfg.update_check.unwrap_or(true));
                busy_repeat_ms = newcfg.busy_repeat_sec.filter(|s| *s > 0).map(|s| s * 1000);
                busy_again.clear();
                done_confirm_ms = newcfg
                    .done_confirm_ms
                    .unwrap_or(profile::DEFAULT_DONE_CONFIRM_MS);
                // Rebuild notification destinations, capabilities, and automation scripts
                let (dests, err) = newcfg.resolve_notify(password.as_deref());
                if let Some(e) = err {
                    startup_errors.push(e);
                }
                notifier = notify::Notifier::new(dests, newcfg.primary_notify.clone());
                // Only swap out the parts that come from config. Rebuilding it
                // entirely would leave nobody aware of pages already placed in the
                // window, so they'd stay stuck on screen with no way to remove them
                // (this used to happen: the moment settings were saved, the settings
                // screen would stick around and tabs would stop responding).
                caps.set_config(
                    newcfg.capabilities.clone(),
                    newcfg.resolve_tokens(password.as_deref()),
                    newcfg.resolve_secret_terms(password.as_deref()),
                    newcfg.automation_permissions.clone(),
                );
                // ...and the same for the connections, which keep their own
                // copy: a password taken out of the settings stops working
                ssh::use_secrets(newcfg.resolve_tokens(password.as_deref()));
                if let Some(eng) = engine.as_ref() {
                    eng.set_ai_engine(newcfg.ai_engine.clone().filter(|s| !s.is_empty()));
                }
                engine = build_engine(
                    Some(&newcfg),
                    workspaces.get(ws_index),
                    &mut startup_errors,
                    &caps,
                );
                started_fired.clear();
                started_fired.resize(tabs.len(), false);
                if active > tabs.len() {
                    active = if tabs.is_empty() { 0 } else { 1 };
                }
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
                // Re-resolve the model bridge's connection info, and hand it to
                // the tabs — including the ones parked in workspaces that are
                // not on screen, which are just as open as the ones that are
                if let Some(c) = &cfg {
                    reload_providers(
                        c,
                        password.as_deref(),
                        tabs.iter_mut().chain(ws_tabs.iter_mut().flatten()),
                    );
                }
                watcher.retarget(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
                let mut note = remote_changed.unwrap_or(msg);
                if lang_restart {
                    note.push_str(&i18n::t("msg.lang_restart"));
                }
                flash = Some(format!(">> {note}"));
                // A settings save may have changed the quick actions — push them
                // into the shell so the composer updates without a reload.
                shell.push_actions(&crate::shell::actions_json());
                shell.push_theme();
                let (next, errs) = crate::keys::Keys::load(cfg.as_ref());
                keymap = next;
                startup_errors.extend(errs);
            }
        }

        // Settings that could not be carried into this version, said once.
        // Shown once the screen is free, so it doesn't overwrite other output.
        if flash.is_none() {
            if let Some((from, path)) = update::take_carry_failure() {
                flash = Some(i18n::tp("msg.update.carry_failed", &[("version", &from), ("path", &path)]));
            }
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
            // or by a stick being plugged in -- so it is noticed here, off the
            // table the watch already keeps, and the tab goes back through the
            // ordinary restart below. Only ever in this direction: a folder
            // that has GONE leaves a running tab alone, because stopping
            // somebody's agent mid-sentence is worse than the folder being gone
            for t in tabs.iter_mut().chain(ws_tabs.iter_mut().flatten()) {
                if t.held().is_some() && tab::Held::of(t.cwd()).is_none() {
                    t.release();
                }
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
                if t.restart_as(rows, cols, plan).is_ok() {
                    if let Some(f) = started_fired.get_mut(i) {
                        *f = false;
                    }
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
                    for e in keys_for(&shikisha_shared::Ev::Select { tab }) {
                        shell.inject(e);
                    }
                }
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
            if !fired_notes.is_empty() {
                if let Some(eng) = engine.as_mut() {
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
                            surface_count,
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
                            ViewMove { allowed: auto_switch, touched_ms: view_touched_ms, settings_open },
                        );
                    }
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
                for (i, t) in tabs.iter_mut().enumerate() {
                    t.usage = cost.get(&i).copied().unwrap_or_default();
                    let branch = t.cwd().and_then(crate::repo::branch_of);
                    // Where it pushes to is only worth working out when there
                    // is a branch to ask about, and only worth asking about
                    // when GitHub is where it lives
                    let repo = branch
                        .as_ref()
                        .and_then(|_| t.cwd())
                        .and_then(crate::repo::origin_of);
                    // What is known right now, and a nudge to find out. The
                    // asking happens elsewhere; a row that waited on GitHub
                    // would be a window that stops drawing
                    let pr = match (&repo, &branch) {
                        (Some(r), Some(b)) => prs.of(r, b).map(|p| p.short()),
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
            // second, and none of those is worth a file
            if save_at.is_none_or(|at| std::time::Instant::now() >= at) {
                let mark = (
                    pane_layout.clone(),
                    tabs.iter().map(|t| t.session.clone()).collect::<Vec<_>>(),
                );
                if Some(&mark) != last_saved.as_ref() {
                    if let Some(ws) = workspaces.get(ws_index) {
                        last_session.remember(&ws.name, &tabs, Some(&pane_layout));
                        last_session.write();
                    }
                    last_saved = Some(mark);
                }
                save_at = Some(std::time::Instant::now() + Duration::from_secs(3));
            }

            // Retire the API keys of tabs that are gone. Told the live set
            // rather than each closure: tabs leave in several ways, and a key
            // that outlives its tab is a working key nobody is watching
            if let Some(a) = api_server.as_ref() {
                a.retain_tabs(&tabs.iter().map(|t| t.title.clone()).collect::<Vec<_>>());
            }

            // Fire hooks -> resume waiting coroutines -> run the queued operations
            if let Some(eng) = engine.as_mut() {
                // Let the loop read the current state (shikisha.state)
                eng.set_ai_engine(cfg.as_ref().and_then(|c| c.ai_engine.clone()).filter(|s| !s.is_empty()));
                eng.set_states(tab_states(&tabs));
                // The tabs, and then the git panels: naming either one names
                // the folder it is looking at
                let mut folders = tab_places(&tabs);
                folders.extend(panel_places(&surfaces));
                eng.set_places(folders);
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
                        if new != TabState::Exited && old == TabState::Exited {
                            if let Some(f) = started_fired.get_mut(idx - 1) {
                                *f = false;
                            }
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
                        append_hook_log(&format!(
                            "on_done fired tab{idx}: response {} chars: {}",
                            ctx.output.chars().count(),
                            log_excerpt(&ctx.output, 100)
                        ));
                        eng.fire("on_done", &ctx, None);
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
                let cmds = eng.drain_commands();
                if !cmds.is_empty() {
                    let now_ms = start.elapsed().as_millis() as u64;
                    exec_commands(
                        cmds,
                        &mut tabs,
                        &surfaces,
                        &mut pane_layout,
                        surface_count,
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
                    workspace: workspaces
                        .get(ws_index)
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
                // A workspace with no Lua of its own still has an engine's
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
                        // answered out of this list — the first call from a
                        // tab used to be credited to nobody and thrown away,
                        // and the first call is the one carrying the id of the
                        // conversation to come back to
                        eng.set_states(tab_states(&tabs));
                        eng.set_places(tab_places(&tabs));
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
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Resize { rows, cols, .. }) => {
                        shell.set_phone_size(Some((rows, cols)));
                        shell.queue_input(Event::Resize(cols, rows));
                    }
                    // A Lua quick-action fired from the phone. It's not a keystroke,
                    // so route it straight to the same queue the window's ipc path
                    // fills (drained and run against the active tab below).
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::RunAction { index }) => {
                        shell.mail().run_actions.push(index);
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
                    // The bar's button, pressed on the phone: the same queue the
                    // board's press fills. A person's answer from wherever they
                    // are looking
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Button { from: Some(name) }) => {
                        shell.mail().presses.push(name);
                    }
                    remote::RemoteCmd::Ui(ev @ shikisha_shared::Ev::VaultSearch { .. })
                    | remote::RemoteCmd::Ui(ev @ shikisha_shared::Ev::VaultOpen { .. }) => {
                        shell.queue_vault(ev);
                    }
                    // Giving a branch its own folder, and putting a working
                    // folder back on this machine. Neither is a keystroke, so
                    // neither can be turned into one -- they go to the same
                    // queues the window's dialogs fill
                    remote::RemoteCmd::Ui(ev @ shikisha_shared::Ev::Branch { .. }) => {
                        shell.mail().branches.extend(shikisha_shared::BranchAsk::of(ev));
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
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::Browse { path, open }) => {
                        shell.mail().browses.push((path, open));
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
                    // Arranging the screen, from a device with room to
                    // arrange it. The same queues the window's own presses
                    // fill -- one place decides what a split means
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FocusPane { id }) => {
                        shell.mail().focus_panes.push(id);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::ClosePane { id }) => {
                        shell.mail().close_panes.push(id);
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
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderName { folder, name }) => {
                        shell.mail().folder_names.push((folder, name));
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderClose { folder }) => {
                        shell.mail().folder_closes.push(folder);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderDiscard { folder }) => {
                        shell.mail().folder_discards.push(folder);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FolderColor { folder, color }) => {
                        shell.mail().folder_colors.push((folder, color));
                    }
                    // How big the text is, and how wide the tab bar is, as the
                    // person looking wants them
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::FontSize { px }) => {
                        shell.mail().font_size = Some(px);
                    }
                    remote::RemoteCmd::Ui(shikisha_shared::Ev::TabWidth { px }) => {
                        shell.mail().tab_width = Some(px);
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
                        if !on {
                            if let Some(eng) = engine.as_mut() {
                                eng.cancel_all();
                            }
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
                casting = want;
            } else if let Some(key) = &casting {
                // Even if the target hasn't changed, push out one frame of the current
                // screen when a new viewer joins. Otherwise a static page would leave
                // them waiting for a change forever, staring at nothing.
                if r.take_keyframe_request() {
                    let _ = caps.browser_screencast(key, true);
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
                    surface_count,
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

        // If the current workspace is a discussion, find the opening speaker
        // (first participant) so the dashboard can offer a "start" card.
        let (discuss_start, discuss_start_name) = workspaces
            .get(ws_index)
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
        let folder_count = workspaces
            .get(ws_index)
            .map(|w| w.folders.iter().filter(|f| f.cwd.is_some()).count())
            .unwrap_or(0);
        let past_the_plus = tabs.iter().any(|t| t.is_ai() || t.place.linked);
        let (coach, seen) = coach_step(folder_count, coach_seen, past_the_plus);
        if seen != coach_seen {
            coach_seen = seen;
            let _ = crate::crypto::write_atomic(&config::state_path("coach"), &seen.to_string());
        }
        // Asked only while a Claude tab exists and the setting is on; shown
        // only while such a tab is in view (the page decides that)
        limits.want(claude_usage_on && tabs.iter().any(|t| t.ai_kind().as_deref() == Some("claude")));
        let usage = limits.current().map(|l| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            crate::uistate::UsageState::of(&l, now)
        });
        let ui = Ui {
            ais: ai_choices.clone(),
            coach,
            usage,
            thanks: thanks_show.then(|| thanks_kind.to_string()),
            update: update::ask(),
            first_run,
            push_wanted: cfg.as_ref().is_some_and(|c| {
                c.notify.values().any(|d| matches!(d, notify::Destination::Phone {}))
            }),
            active,
            board: board_open,
            settings: settings_open,
            // The flag itself, engine or no engine. It used to be sent only
            // while a Lua engine existed, which left the bar saying AUTO ON
            // after an emergency stop in a workspace with no automation of
            // its own -- and the stop still means something there: it is
            // what interrupted the AIs, and what keeps a hand-over from
            // starting until it is turned back on
            auto: Some(auto_enabled),
            ws_names: workspaces.iter().map(|w| w.name.clone()).collect(),
            ws_index,
            ws_open,
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
            folders: workspaces
                .get(ws_index)
                .map(|w| {
                    w.folders
                        .iter()
                        .filter_map(|f| {
                            f.cwd.clone().map(|c| (c, f.name.clone().unwrap_or_default()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
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
            remote_on: remote_ui.is_some(),
            remote_conn: remote_ui.as_ref().is_some_and(|r| r.has_state_clients()),
            remote_sticky: cfg.as_ref().is_some_and(|c| c.remote.sticky_token),
            aim: aim_of(workspaces.get(ws_index), &surfaces, &tabs, active),
            nav,
            asks: caps.asks_now(),
            away: caps.drawn_away(),
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
            // A page placed in the focused pane is a window of its own: nothing
            // of ours can be drawn over it, so the message would sit behind the
            // page (or, in a split, be cut off at the pane's edge). Hand it to
            // that page to draw, the way the pen is handed over
            if let (Some(text), Some(key)) = (flash.as_deref(), focused_page(&pane_layout, &ui.surfaces))
            {
                // Plain, like the window's own: a flash is what the screen
                // shows with `toast(S.flash)`, and it does not mark warnings
                let _ = caps.browser_toast(&key, text, false);
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
        if let Some(r) = remote_ui.as_ref() {
            if r.has_state_clients() && last_remote_push.elapsed() >= remote_floor(r.max_pending()) {
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

        // A pane asked for a tab and the form has produced one. It is the
        // surface nothing is showing yet -- newly written config is the only
        // way a surface appears with no pane behind it -- so the pane that
        // asked takes it, and asks for nothing more
        // The form is a surface too, from the moment it opens. It is not the
        // answer to the question -- it IS the question -- so it is left out of
        // both counts below. Counting it made the baseline move under its own
        // feet: it was already there when the wait began and gone again by the
        // time the tab arrived, so the total came back to where it started and
        // the new tab looked like nothing new
        let is_form = |n: usize| {
            matches!(ui_surface_at(&surfaces, n), Some(Surface::Browser { key, .. })
                if key == SETTINGS_TAB)
        };
        let real_surfaces = (1..=surface_count).filter(|n| !is_form(*n)).count();
        if let Some(id) = shell.mail().take_add_tab_pane() {
            awaiting_tab = Some((id, real_surfaces));
        }
        // Nothing calls the wait off. Not the form closing -- saving CLOSES it,
        // and the tab it wrote does not exist until the settings file has been
        // read back, so ending the wait there meant the tab arrived to find
        // nobody waiting. Not the pane filling up either: it fills with the
        // form itself for as long as that is open, and reading that as "filled"
        // ended the wait one frame after it began. A wait that is never
        // answered simply never fires, and an empty pane stays empty, which is
        // exactly what it was before anybody asked
        if let Some((id, was)) = awaiting_tab {
            let taken: std::collections::HashSet<usize> =
                pane_layout.leaves().into_iter().map(|(_, s)| s).collect();
            let fresh = (real_surfaces > was)
                .then(|| (1..=surface_count).rev().find(|n| !taken.contains(n) && !is_form(*n)))
                .flatten();
            if let Some(fresh) = fresh {
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

        // ⊞ / ⊟ in a pane's caption. Divides that pane, not whichever one had
        // focus: the button is attached to a pane, so it must mean that one
        for (id, down) in shell.mail().take_pane_splits() {
            if !pane_layout.focus_pane(id) {
                continue;
            }
            let dir = if down { layout::Dir::Col } else { layout::Dir::Row };
            active = split_focused(&mut pane_layout, dir, surface_count, active);
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
            if let Some(msg) = restart_surface(
                active,
                keep,
                &mut tabs,
                &surfaces,
                &mut engine,
                &caps,
                rows,
                cols,
            ) {
                flash = Some(msg);
            }
        }
        for id in shell.mail().take_close_panes() {
            if pane_layout.close(id) {
                active = pane_layout.focused_surface();
                view_touched_ms = start.elapsed().as_millis() as u64;
            } else {
                flash = Some(i18n::t("msg.pane_last"));
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
            let covered = help_open || ws_open || qr_open;
            // The settings form is a screen, not a pane: it covers the content
            // area and the layout waits underneath. It asks about the whole
            // app, so seating it in one corner of the app made as little sense
            // as seating the board there -- and once the panes were hidden to
            // let it cover, the pane it was sitting in had no size to give it
            // Nothing is placed in a rectangle with no size: before the page
            // has measured itself there is no window to cover, and a page
            // given nothing is a page nobody can find again
            let room = shell.geom_full().2 > 0 && shell.geom_full().3 > 0;
            if settings_open && !covered && room {
                caps.show_at(&[(SETTINGS_TAB.to_string(), shell.geom_full())]);
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
                .filter(|_| !covered)
                .collect();
            caps.show_at(&shown);
            }
        }
        // Hand off that the bar's button was pressed. The board (or the phone)
        // names the page the bar stands under, by the name automation gives it;
        // the bar is only ever drawn for the workspace in view, so that name is
        // this workspace's
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

        // Lua quick-actions tapped in the bar: look up the code (kept server-side)
        // and run it against the active tab. Its commands drain with the hooks'.
        for index in shell.mail().take_run_actions() {
            let Some(code) = cfg
                .as_ref()
                .and_then(|c| c.actions.get(index))
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
                let mut folders = tab_places(&tabs);
                folders.extend(panel_places(&surfaces));
                eng.set_states(tab_states(&tabs));
                eng.set_places(folders);
                let spec = cfg.as_ref().map(|c| c.git.clone()).unwrap_or_default();
                let code = spec
                    .message_lua
                    .filter(|l| !l.trim().is_empty())
                    .unwrap_or_else(|| crate::hooks::COMMIT_MESSAGE_LUA.to_string());
                let _ = eng.call_primitive_as(
                    None,
                    grants::Subject::Human,
                    "set_var",
                    &[serde_json::json!("git_tab"), serde_json::json!(panel)],
                );
                let _ = eng.call_primitive_as(
                    None,
                    grants::Subject::Human,
                    "set_var",
                    &[
                        serde_json::json!("git_hint"),
                        serde_json::json!(spec.message_hint.unwrap_or_default()),
                    ],
                );
                shell.push_git(&serde_json::json!({"act": "message", "busy": true}).to_string());
                eng.start_snippet("message", &code);
                continue;
            }
            if matches!(act.as_str(), "fetch" | "pull" | "push" | "resolve") {
                // "message" is the AI writing one, which is a read of the diff
                // followed by a wait on a program -- the same reason as the
                // network ones for not doing it on this thread
                let name = match act.as_str() {
                    // Untangling writes the file back, which is the same reach
                    // as anything else that edits the tree
                    "resolve" => "git_apply".to_string(),
                    _ => format!("git_{act}"),
                };
                let dir = panel_places(&surfaces)
                    .into_iter()
                    .find(|p| p.key.matches(&panel))
                    .map(|p| p.dir);
                let answer = match (caps.allows(&name, grants::Subject::Human), dir) {
                    (false, _) => Some(i18n::tp(
                        "err.hooks.not_permitted",
                        &[("name", &name), ("who", &i18n::t("grant.who.human"))],
                    )),
                    (true, None) => Some(i18n::t("err.git.no_tab")),
                    (true, Some(dir)) => {
                        // Say it started, so a button that will be a while
                        // does not look like a button that did nothing
                        shell.push_git(
                            &serde_json::json!({"act": act, "busy": true}).to_string(),
                        );
                        let tx = git_tx.clone();
                        let act2 = act.clone();
                        let ai = cfg
                            .as_ref()
                            .and_then(|c| c.ai_engine.clone())
                            .filter(|s| !s.is_empty());
                        std::thread::spawn(move || {
                            let done = match act2.as_str() {
                                "fetch" => crate::git::fetch(&dir),
                                "pull" => crate::git::pull(&dir),
                                "push" => crate::git::push(&dir),
                                #[allow(unreachable_patterns)]
                                // Every file git left marked, one at a time.
                                // Nothing is staged and nothing is committed:
                                // what comes back is written into the tree, and
                                // the person reads it as a diff like any other
                                "resolve" => crate::git::tangled(&dir).and_then(|files| {
                                    let mut done: Vec<String> = Vec::new();
                                    let mut failed: Vec<String> = Vec::new();
                                    for f in &files {
                                        let at = dir.join(f);
                                        let said = std::fs::read_to_string(&at)
                                            .map_err(anyhow::Error::from)
                                            .and_then(|body| {
                                                crate::webui::resolve_conflict(f, &body, ai.as_deref())
                                            })
                                            .and_then(|text| {
                                                std::fs::write(&at, text).map_err(Into::into)
                                            });
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
                                Err(e) => serde_json::json!({
                                    "act": act2, "ok": false, "error": plain_error(&e.to_string()),
                                }),
                            };
                            let _ = tx.send(js.to_string());
                        });
                        None
                    }
                };
                if let Some(said) = answer {
                    shell.push_git(
                        &serde_json::json!({"act": act, "ok": false, "error": said}).to_string(),
                    );
                }
                continue;
            }
            if engine.is_none() {
                engine = crate::hooks::HookEngine::with_caps(crate::hooks::Caps::clone(&caps)).ok();
            }
            let Some(eng) = engine.as_mut() else { continue };
            let mut folders = tab_places(&tabs);
            folders.extend(panel_places(&surfaces));
            eng.set_states(tab_states(&tabs));
            eng.set_places(folders);
            let who = serde_json::Value::String(panel.clone());
            let files = serde_json::json!(paths);
            let (method, params): (&str, Vec<serde_json::Value>) = match act.as_str() {
                "status" => ("git_status", vec![who.clone()]),
                "branch" => ("git_branch", vec![who.clone()]),
                "branches" => ("git_branches", vec![who.clone()]),
                "diff" => (
                    "git_diff",
                    vec![
                        who.clone(),
                        serde_json::json!({
                            "path": paths.first().cloned().unwrap_or_default(),
                            "staged": args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false),
                        }),
                    ],
                ),
                "graph" => (
                    "git_graph",
                    vec![
                        who.clone(),
                        serde_json::json!({
                            "all": args.get("all").and_then(|v| v.as_bool()).unwrap_or(true),
                            "remotes": args.get("remotes").and_then(|v| v.as_bool()).unwrap_or(false),
                            "branch": args.get("branch").and_then(|v| v.as_str()).unwrap_or(""),
                        }),
                    ],
                ),
                "detail" => ("git_detail", vec![who.clone(), serde_json::json!(text)]),
                "hunks" => (
                    "git_hunks",
                    vec![
                        who.clone(),
                        serde_json::json!({
                            "path": paths.first().cloned().unwrap_or_default(),
                            "staged": args.get("staged").and_then(|v| v.as_bool()).unwrap_or(false),
                            "commit": args.get("commit").and_then(|v| v.as_str()).unwrap_or(""),
                        }),
                    ],
                ),
                // One piece of a diff, put where the button said
                "hunk" => (
                    "git_apply",
                    vec![
                        who.clone(),
                        serde_json::json!(text),
                        serde_json::json!({
                            "cached": args.get("cached").and_then(|v| v.as_bool()).unwrap_or(false),
                            "reverse": args.get("reverse").and_then(|v| v.as_bool()).unwrap_or(false),
                        }),
                    ],
                ),
                "stage" => ("git_stage", vec![who.clone(), files]),
                "unstage" => ("git_unstage", vec![who.clone(), files]),
                "commit" => (
                    "git_commit",
                    vec![
                        who.clone(),
                        serde_json::json!(text),
                        serde_json::json!({
                            "amend": args.get("amend").and_then(|v| v.as_bool()).unwrap_or(false),
                        }),
                    ],
                ),
                "checkout" => ("git_checkout", vec![who.clone(), serde_json::json!(text)]),
                "merge" => ("git_merge", vec![who.clone(), serde_json::json!(text)]),
                // "make a branch and commit there" -- the answer to a refusal
                // rather than a way around it
                "branch_new" => ("git_branch_create", vec![who.clone(), serde_json::json!(text)]),
                _ => continue,
            };
            let answer = eng.call_primitive_as(None, grants::Subject::Human, method, &params);
            let payload = match answer {
                Ok(data) => serde_json::json!({ "act": act, "ok": true, "data": data }),
                Err(e) => {
                    // A commit refused on a shared branch is not a failure, it
                    // is a question -- and the panel asks it in its own words,
                    // so the reason is named rather than shown as Lua said it
                    let shared = act == "commit"
                        && eng
                            .call_primitive_as(
                                None,
                                grants::Subject::Human,
                                "git_branch",
                                &[who.clone()],
                            )
                            .ok()
                            .and_then(|b| b.get("protected").and_then(|p| p.as_bool()))
                            .unwrap_or(false);
                    serde_json::json!({
                        "act": act,
                        "ok": false,
                        "error": plain_error(&e),
                        "why": if shared { "protected" } else { "" },
                    })
                }
            };
            let js = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
            shell.push_git(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"git\":{js}}}"));
            }
        }
        // Answers from the Lua that was left running (the commit message)
        if let Some(eng) = engine.as_mut() {
            for (tag, said) in eng.take_snippets() {
                let payload = match said {
                    Ok(text) => serde_json::json!({"act": tag, "ok": true, "data": text}),
                    Err(why) => serde_json::json!({"act": tag, "ok": false, "error": why}),
                };
                let js = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".into());
                shell.push_git(&js);
                if let Some(r) = remote_ui.as_ref() {
                    r.push_state(format!("{{\"git\":{js}}}"));
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

        // What the file panel asked for. Reading this machine is answered on
        // the spot; anything that touches the server goes to a thread, because
        // a folder listing over a network is a wait and this loop draws the
        // window
        for (panel, act, args) in shell.mail().take_sftps() {
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

        // ▶ run mode: composer Lua against the shown browser, in the rally's
        // sandbox (browser functions on that one tab, nothing else). The verdict
        // returns as a toast on both surfaces.
        for code in shell.mail().take_run_luas() {
            let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) else {
                continue;
            };
            // Running needs an engine; make a bare one if this workspace didn't
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
        // workspace's settings; the change-watcher then launches it, resumed,
        // through the ordinary reload -- the one place a tab is safely made
        for query in shell.mail().take_vault_queries() {
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
                        });
                    }
                }
            }
            let found = crate::vault::search(&query, 40);
            hits.extend(found.hits);
            vault_view = Some(crate::uistate::VaultState {
                query,
                hits,
                capped: found.capped,
            });
        }
        // A folder renamed in the list, or taken out of it. Both are changes
        // to the settings, so the reload that follows is what actually shows
        for (folder, name) in shell.mail().take_folder_names() {
            let ws = workspaces.get(ws_index).map(|w| w.name.clone()).unwrap_or_default();
            if let Err(e) = config::rename_folder(&ws, std::path::Path::new(&folder), &name) {
                flash = Some(format!("{e:#}"));
            }
        }
        // Thrown away for good. Refused first, while nothing has happened yet,
        // so a folder with work in it is never closed on the way to a no.
        // Then the tabs are ended by taking the folder out of the settings --
        // git will not remove a folder something is still standing in -- and
        // the removal itself waits for them to actually be gone
        for folder in shell.mail().take_folder_discards() {
            let at = std::path::PathBuf::from(&folder);
            if let Err(e) = crate::worktree::ready_to_discard(&at) {
                flash = Some(format!("{e:#}"));
                continue;
            }
            let ws = workspaces.get(ws_index).map(|w| w.name.clone()).unwrap_or_default();
            match config::remove_folder(&ws, &at) {
                Ok(()) => {
                    flash = Some(i18n::tp(
                        "msg.folder.discarded",
                        &[("path", &at.display().to_string())],
                    ));
                    crate::worktree::discard_soon(at);
                }
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        // The folders whose tabs are on their way out. Tried again each time
        // round until git can have it, and given up on out loud rather than
        // silently -- a folder that was asked to go and did not is a surprise
        // waiting in the settings
        for folder in shell.mail().take_folder_closes() {
            let ws = workspaces.get(ws_index).map(|w| w.name.clone()).unwrap_or_default();
            match config::remove_folder(&ws, std::path::Path::new(&folder)) {
                // Said out loud, because the folder is still on disk and this
                // is the only sign that it was left there on purpose
                Ok(()) => flash = Some(i18n::tp("msg.folder.closed", &[("path", &folder)])),
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        // Somewhere new to work. Looking hands back what is inside; choosing
        // writes the folder into the settings, and the reload opens it
        for (path, open) in shell.mail().take_browses() {
            if !open {
                browse_view = Some(crate::uistate::BrowseState::of(&path));
                continue;
            }
            let ws = workspaces.get(ws_index).map(|w| w.name.clone()).unwrap_or_default();
            let at = std::path::PathBuf::from(&path);
            match config::append_folder(&ws, None, &at, None) {
                Ok(()) => {
                    browse_view = None;
                    flash = Some(i18n::tp("msg.folder.opened", &[("path", &path)]));
                }
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        // A colour chosen for a project. Written against the folder git shares
        // between its branches, so all of them change at once
        for (folder, color) in shell.mail().take_folder_colors() {
            let at = std::path::PathBuf::from(&folder);
            if let Some(family) = crate::repo::family_of(&at) {
                if let Err(e) = config::set_folder_color(&family, &color) {
                    flash = Some(format!("{e:#}"));
                }
            }
        }
        // A working folder that is not on this machine. The same call answers
        // "what would it take" and does it, so the lines shown before it
        // happens are the lines that happen
        for (folder, choose, branch, take) in shell.mail().take_repairs() {
            let at = std::path::PathBuf::from(&folder);
            let ws = workspaces.get(ws_index);
            let ws_name = ws.map(|w| w.name.clone()).unwrap_or_default();
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
            if let Some(spec) = chosen.as_ref() {
                if let Err(e) = config::set_folder_source(&ws_name, &at, spec) {
                    flash = Some(format!("{e:#}"));
                }
            }
            // What the settings say now: the answer just given, or what was
            // written down when the folder was made
            let source = match chosen.as_ref() {
                Some(spec) => spec.read(),
                None => ws
                    .and_then(|w| w.folders.iter().find(|f| f.cwd.as_deref() == Some(at.as_path())))
                    .map(|f| f.source.clone())
                    .unwrap_or_default(),
            };
            // The project the settings already name, when they do. Preferred
            // over working it out from the path: what somebody wrote down beats
            // what a folder's shape suggests
            let checkout = ws.and_then(|w| {
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
            let name = ws
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
                    if take && !steps.is_empty() {
                        // Stopped at the first thing that will not work: the
                        // second step is built on the first having happened
                        match steps.iter().try_for_each(|s| crate::folders::take(s, &source)) {
                            Ok(()) => view.done = at.is_dir(),
                            Err(e) => view.error = Some(format!("{e:#}")),
                        }
                        // What was true a moment ago is not true now. Said out
                        // loud rather than waited out, so the folder stops
                        // being called missing the instant it is made -- and
                        // the tabs held back for it start on the next beat
                        crate::folders::watch().forget(&at);
                        if view.done {
                            flash = Some(i18n::tp(
                                "msg.folder.ready",
                                &[("name", &view.name)],
                            ));
                        }
                    }
                }
                Err(blocked) => {
                    // Nothing was ever written down about this folder. This is
                    // the only case that asks, and the answer ends the asking
                    // for every machine, not just this one
                    view.asking = matches!(blocked, crate::folders::Blocked::Unknown);
                    view.projects = projects_here(ws);
                    view.said = blocked_said(&blocked);
                    view.blocked = Some(blocked);
                }
            }
            repair_view = Some(view);
        }
        // Another branch of a project already open. The same call answers "what
        // would this do" and does it, so the line shown before it happens is
        // the line that happens
        for ask in shell.mail().take_branches() {
            let from = std::path::PathBuf::from(&ask.from);
            let name = ask.branch.clone();
            // What this project can offer -- the branches to grow from, and
            // the things git will not carry -- is a fact about the folder, not
            // about what has been typed so far. Answered even when the name is
            // still empty, so the pickers are filled the moment the dialog opens
            let repo = crate::repo::main_checkout(&from);
            let offers = repo.as_deref().map(|main| {
                (crate::worktree::bases(main), crate::worktree::carryables(main))
            });
            let (bases, carryable) = offers.unwrap_or_default();
            // What it would grow from, even when there is no name yet to grow.
            // Echoing back the empty answer would leave the picker with nothing
            // to show until somebody typed
            let chosen = match ask.base.trim().is_empty() {
                true => repo
                    .as_deref()
                    .map(crate::worktree::default_base)
                    .unwrap_or_default(),
                false => ask.base.clone(),
            };
            // Nothing typed yet: propose one, so the dialog opens with a
            // complete answer and pressing the button is enough
            let wanted = match name.trim().is_empty() {
                true => repo.as_deref().map(crate::worktree::suggest).unwrap_or_default(),
                false => name.clone(),
            };
            let ws = workspaces
                .get(ws_index)
                .map(|w| w.name.clone())
                .unwrap_or_default();
            // What the new folder runs, chosen from what this machine has
            let start = start_of(&ask.start, &ai_choices);
            let mut view = crate::uistate::BranchPlan {
                from: from.display().to_string(),
                branch: name.clone(),
                asked: name.clone(),
                base: chosen.clone(),
                bases,
                carry: carryable,
                ..Default::default()
            };
            if ask.ais.is_empty() {
                match crate::worktree::plan(&from, &wanted, Some(&ask.base)) {
                    Err(e) => view.error = Some(format!("{e:#}")),
                    Ok(plan) => {
                        view.branch = plan.branch.clone();
                        view.folder = plan.folder.display().to_string();
                        view.line = plan.line();
                        view.base = plan.base.clone();
                        if ask.make {
                            // Made first, written down second: settings naming a
                            // folder that does not exist would launch tabs into
                            // nowhere on the next reload
                            let wrote = crate::worktree::create(&plan).and_then(|()| {
                                config::append_folder_starting(
                                    &ws,
                                    Some(&plan.main),
                                    &plan.folder,
                                    Some(&plan.branch),
                                    &start,
                                )
                            });
                            match wrote {
                                Ok(()) => {
                                    view.done = true;
                                    // What could not be brought along is said out
                                    // loud: the folder is made either way, and the
                                    // first build is what would otherwise fail
                                    let missed = crate::worktree::carry_into(&plan, &ask.carry);
                                    flash = Some(match missed.is_empty() {
                                        true => i18n::tp(
                                            "msg.branch.made",
                                            &[("name", &plan.branch)],
                                        ),
                                        false => i18n::tp(
                                            "msg.branch.made_partly",
                                            &[("name", &plan.branch), ("missed", &missed.join(", "))],
                                        ),
                                    });
                                }
                                Err(e) => view.error = Some(format!("{e:#}")),
                            }
                        }
                    }
                }
            } else {
                // One folder per AI, each branch named for its AI. Every line
                // is shown before any of them runs; one that cannot be made
                // stops the whole ask, because "three of the four were made"
                // is a state nobody asked for
                let fanned = crate::worktree::fan(&from, &wanted, Some(&ask.base), &ask.ais);
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
                    let mut made: Vec<String> = Vec::new();
                    let mut failed: Vec<String> = Vec::new();
                    for (ai, plan) in fanned.iter() {
                        let Ok(plan) = plan else { continue };
                        let one = start_of(ai, &ai_choices);
                        let wrote = crate::worktree::create(plan).and_then(|()| {
                            config::append_folder_starting(
                                &ws,
                                Some(&plan.main),
                                &plan.folder,
                                Some(&plan.branch),
                                &one,
                            )
                        });
                        match wrote {
                            Ok(()) => {
                                crate::worktree::carry_into(plan, &ask.carry);
                                made.push(plan.branch.clone());
                            }
                            Err(e) => {
                                append_hook_log(&format!("could not make {}: {e:#}", plan.branch));
                                failed.push(plan.branch.clone());
                            }
                        }
                    }
                    view.done = !made.is_empty();
                    flash = Some(match (made.is_empty(), failed.is_empty()) {
                        (true, _) => i18n::tp("msg.branch.fanned_none", &[("failed", &failed.join(", "))]),
                        (false, true) => i18n::tp("msg.branch.fanned", &[("names", &made.join(", "))]),
                        (false, false) => i18n::tp(
                            "msg.branch.fanned_partly",
                            &[("names", &made.join(", ")), ("failed", &failed.join(", "))],
                        ),
                    });
                    if made.is_empty() {
                        view.error = flash.clone();
                    }
                }
            }
            branch_view = Some(view);
        }
        for ev in shell.mail().take_vault_opens() {
            if let shikisha_shared::Ev::VaultOpen { program, id, cwd, title } = ev {
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
                let ws = workspaces.get(ws_index).map(|w| w.name.clone()).unwrap_or_default();
                if config::append_tab(&ws, tab, folder) {
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
        if !injects.is_empty() {
            if let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) {
                for input in injects {
                    let _ = caps.browser_inject(key, input);
                }
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
                if remember_aim(workspaces.get_mut(ws_index), operator_name.as_deref(), None) {
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
            let in_discuss = workspaces
                .get(ws_index)
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
                // may be localized ("ブラウザ") while browser_* resolves by key, so
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
                workspaces.get_mut(ws_index),
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
            // workspace didn't otherwise have any Lua (same gap as Lua actions).
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
                            // The referee is the workspace's, as it always was
                            // for a browser driven from the settings file. The
                            // ad-hoc path used to hand over an empty one, so
                            // whoever aimed on screen quietly had no stops
                            let stops = workspaces
                                .get(ws_index)
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
            if !goal.is_empty() {
                if let Some(eng) = engine.as_mut() {
                    eng.deliver_goal(active, &goal);
                }
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

        // The sidebar gear. Opens settings from any tab (the menu "e" key only
        // fires while INDEX is in view, so the gear needs its own path).
        // The workspace being viewed rides along so its group opens expanded.
        if let Some((section, ret, folder, tabpos)) = shell.take_open_settings() {
            // The gear passes the workspace being viewed, and the tab in view so
            // the page opens on its card; a deep-link shortcut may instead name
            // a section to land on and ask to return once saved.
            let mut query = format!("&ws={ws_index}");
            if let Some(f) = folder {
                query += &format!("&folder={}", urlish(&f));
            }
            if let Some(n) = tabpos {
                query += &format!("&tabpos={n}");
            }
            if let Some(s) = section {
                query += &format!("&section={s}");
            }
            if ret {
                query += "&ret=1";
            }
            flash = Some(
                match open_settings(&mut web, &config_file, &remote_info, &web_password, &caps, &query) {
                    Ok(()) => {
                        settings_open = true;
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
        if let Some(step) = shell.mail().take_coach_done() {
            if step > coach_seen {
                coach_seen = step;
                let _ = crate::crypto::write_atomic(&config::state_path("coach"), &step.to_string());
            }
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
        if shell.mail().take_help_site() {
            crate::webui::open_external(&i18n::t("tui.help.url"));
        }
        // The update card was answered. Either answer puts it away for this
        // version; "open" leads to the settings' Update card, where the one
        // button that fetches and installs is -- the card itself installs
        // nothing, so a press by mistake costs nothing
        if let Some(open) = shell.mail().take_update_card() {
            update::card_answered();
            if open {
                shell.mail().open_settings = Some((Some("update".into()), false, None, None));
            }
        }
        for idx in shell.mail().take_limit_acks() {
            if let Some(i) = session_at(&surfaces, idx) {
                if let Some(t) = tabs.get_mut(i) {
                    t.dismiss_limit_note();
                }
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
            if auto_enabled {
                if let (Some(eng), Some(page)) =
                    (engine.as_mut(), page_ctx(&surfaces, &name, url, complete))
                {
                    eng.fire_page("on_load", &page);
                }
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
            if shell.confirm_quit(quit_busy(&tabs, &ws_tabs)) {
                match what {
                    update::Apply::Store { version } => {
                        let _ = shell.install_store_update(&version);
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
        } else if (close_pressed || quit_chosen) && shell.confirm_quit(quit_busy(&tabs, &ws_tabs)) {
            break;
        }
        let Some(ev) = polled else {
            continue;
        };

        match ev {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                flash = None;
                // Overlays (help / QR / workspace list) take top priority
                if help_open {
                    help_open = false;
                    continue;
                }
                if qr_open {
                    qr_open = false;
                    continue;
                }
                if ws_open {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => ws_open = false,
                        KeyCode::Char(c @ '1'..='9') => {
                            let n = c as usize - '1' as usize;
                            if n < workspaces.len() {
                                switch_workspace(
                                    n,
                                    &mut ws_index,
                                    &mut tabs,
                                    &mut ws_tabs,
                                    &workspaces,
                                    &mut active,
                                    &mut pane_layout,
                                    &mut ws_panes,
                                    rows,
                                    cols,
                                    &mut startup_errors,
                                    &mut started_fired,
                                    cfg.as_ref(),
                                    &mut engine,
                                    &mut engines,
                                    &caps,
                                    &last_session,
                                );
                            }
                            ws_open = false;
                            // Switching workspace drops the settings overlay (it's
                            // hosted per-workspace); don't leave the flag stuck on.
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
                            if shell.confirm_quit(quit_busy(&tabs, &ws_tabs)) {
                                break;
                            }
                        }
                        // Open the command palette from any tab. It is drawn by
                        // the page, so this only nudges it open
                        KeyCode::Char(':') => shell.open_palette(),
                        // 0 is the board, which is a screen over everything;
                        // 1.. are the running things, which live in panes. One
                        // key row, two different kinds of destination
                        KeyCode::Char('0') => {
                            board_open = true;
                            view_touched_ms = start.elapsed().as_millis() as u64;
                        }
                        KeyCode::Char(c @ '1'..='9') => {
                            let n = c as usize - '0' as usize;
                            if n <= surface_count {
                                active = n;
                                board_open = false;
                                view_touched_ms = start.elapsed().as_millis() as u64;
                                // An explicit tab pick is a deliberate exit from settings.
                                settings_open = false;
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
                            flash = restart_surface(
                                active,
                                key.code == KeyCode::Char('r'),
                                &mut tabs,
                                &surfaces,
                                &mut engine,
                                &caps,
                                rows,
                                cols,
                            );
                        }
                        // Ctrl+B l toggles the input lock / w workspace list / ? help
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
                            if workspaces.len() > 1 {
                                ws_open = true;
                            } else {
                                flash = Some(i18n::t("msg.ws.only_one"));
                            }
                        }
                        KeyCode::Char('W') => {
                            if workspaces.len() > 1 {
                                let next = (ws_index + 1) % workspaces.len();
                                switch_workspace(
                                    next,
                                    &mut ws_index,
                                    &mut tabs,
                                    &mut ws_tabs,
                                    &workspaces,
                                    &mut active,
                                    &mut pane_layout,
                                    &mut ws_panes,
                                    rows,
                                    cols,
                                    &mut startup_errors,
                                    &mut started_fired,
                                    cfg.as_ref(),
                                    &mut engine,
                                    &mut engines,
                                    &caps,
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
                            let query = format!(
                                "&addtab={ws_index}{}&nonce={}",
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
                            active = split_focused(&mut pane_layout, dir, surface_count, active);
                            view_touched_ms = start.elapsed().as_millis() as u64;
                        }
                        // Ctrl+B s puts the tab bar away, and brings it back
                        // the width it was. The whole window is worth having
                        // for one screen, and the list of tabs is the part you
                        // are not reading while you read the other
                        KeyCode::Char('s') => shell.toggle_tab_bar(),
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
                            if workspaces.len() > 1 {
                                ws_open = true;
                            } else {
                                flash = Some(i18n::t("msg.ws.only_one"));
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
                        // workspace being viewed rides along too, so its group expands.
                        KeyCode::Char('e') => {
                            let query = format!("&ws={ws_index}&gen=1");
                            flash = Some(
                                match open_settings(&mut web, &config_file, &remote_info, &web_password, &caps, &query)
                                {
                                    Ok(()) => {
                                        // Once opened, switch to that tab.
                                        // Don't leave it opened but invisible.
                                        // If already open, switch to its existing location.
                                        settings_open = true;
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
                        KeyCode::Char('q') => {
                            if shell.confirm_quit(quit_busy(&tabs, &ws_tabs)) {
                                break;
                            }
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
                if let Some(t) = session_mut(&mut tabs, &surfaces, active) {
                    if !t.locked {
                        t.chain_depth = 0;
                        t.last_manual_ms = Some(now_ms);
                        to_live(t);
                        finish_paste(&mut pending_send, t, active, now_ms);
                        t.write_bytes(text.as_bytes())?;
                    }
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
    if let Some(ws) = workspaces.get(ws_index) {
        last_session.remember(&ws.name, &tabs, Some(&pane_layout));
        last_session.write();
    }
    for t in tabs.iter_mut() {
        t.kill();
    }
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
pub fn reload_providers<'a>(
    cfg: &config::Config,
    password: Option<&str>,
    tabs: impl Iterator<Item = &'a mut Tab>,
) {
    bridge::set_providers(cfg, password);
    for t in tabs {
        t.refresh_model_conn();
    }
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
        Surface::Session(_) | Surface::Git { .. } | Surface::Sftp { .. } => None,
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
    ws: Option<&mut config::Workspace>,
    operator: Option<&str>,
    aim: Option<&str>,
) -> bool {
    let Some(name) = operator else { return false };
    if let Some(ws) = ws {
        for t in ws.tabs.iter_mut() {
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
    ws: Option<&config::Workspace>,
    surfaces: &[Surface],
    tabs: &[Tab],
    surface: usize,
) -> Option<usize> {
    let t = session_at(surfaces, surface).and_then(|i| tabs.get(i))?;
    let me = t.id.clone()?;
    let aim = ws?
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
/// One thing the file panel asked for.
///
/// Returns the answer when there is one to give at once, and `None` when the
/// far end has been asked and a thread will send it along. Everything that
/// touches a server asks the same permission table a script does -- the panel
/// is a screen a person opened, so it reaches exactly what their own
/// automation would, and not one thing more.
#[allow(clippy::too_many_arguments)]
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
    let Some((local_root, spec, remote_root)) = surfaces.iter().find_map(|s| match s {
        Surface::Sftp { key, dir, spec, remote_dir, .. } if key == panel => {
            Some((dir.clone(), spec.clone(), remote_dir.clone()))
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
                    "server": spec.as_ref().map(|s| format!("{}@{}", s.user, s.address())),
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

    // Everything left goes to the far end, which needs an address
    let Some(spec) = spec else {
        return fail(i18n::t("err.sftp.no_address"));
    };

    // The name this act is asking permission under, and the job it becomes
    let at = |given: &str| match given.trim() {
        "" => match remote_root.trim() {
            "" => ".".to_string(),
            r => r.to_string(),
        },
        g => g.to_string(),
    };
    let (name, job): (&str, ssh::FileJob) = match act {
        "remote" => ("sftp_ls", ssh::FileJob::List { path: at(&str_of("at")) }),
        "mkdir" => ("sftp_mkdir", ssh::FileJob::MakeDir { path: str_of("path") }),
        "rename" => (
            "sftp_rename",
            ssh::FileJob::Rename { from: str_of("from"), to: str_of("to") },
        ),
        "rm" => ("sftp_rm", ssh::FileJob::Remove { path: str_of("path") }),
        "put" => (
            "sftp_put",
            ssh::FileJob::Put {
                from: std::path::PathBuf::from(str_of("from")),
                to: str_of("to"),
                overwrite: args.get("overwrite").and_then(|v| v.as_bool()).unwrap_or(false),
            },
        ),
        "get" => (
            "sftp_get",
            ssh::FileJob::Get { from: str_of("from"), to: std::path::PathBuf::from(str_of("to")) },
        ),
        // Reaching the far end at all, to say so before anything is saved
        "test" => ("sftp_ls", ssh::FileJob::List { path: at("") }),
        _ => return None,
    };
    // A transfer names a file on this machine, and that file has to be inside
    // the panel's own folder -- the same promise the far side gets
    if let (Some(root), ssh::FileJob::Put { from, .. }) = (&local_root, &job) {
        if local_under(root, &from.display().to_string()).is_none() {
            return fail(i18n::t("err.sftp.outside"));
        }
    }
    if let (Some(root), ssh::FileJob::Get { to, .. }) = (&local_root, &job) {
        if local_under(root, &to.display().to_string()).is_none() {
            return fail(i18n::t("err.sftp.outside"));
        }
    }
    if !caps.allows(name, grants::Subject::Human) {
        return fail(i18n::tp(
            "err.hooks.not_permitted",
            &[("name", name), ("who", &i18n::t("grant.who.human"))],
        ));
    }
    // The folder that was asked about, sent back with the answer: by the time
    // it arrives the person may have moved on, and a listing that lands in the
    // wrong folder is worse than one that never lands
    let asked = match &job {
        ssh::FileJob::List { path } => path.clone(),
        _ => String::new(),
    };
    let (act, panel) = (act.to_string(), panel.to_string());
    let tx = tx.clone();
    std::thread::spawn(move || {
        let said = crate::ssh::files(&spec, job, SFTP_WAIT_MS);
        let payload = match said {
            Ok(ssh::FileAnswer::Listing(rows)) => serde_json::json!({
                "act": act,
                "panel": panel,
                "ok": true,
                "at": asked,
                "rows": rows.iter().map(|e| serde_json::json!({
                    "name": e.name,
                    "dir": e.dir,
                    "size": e.size,
                    "modified": e.modified,
                })).collect::<Vec<_>>(),
            }),
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
pub fn local_rows(at: &std::path::Path) -> Result<Vec<serde_json::Value>> {
    let mut rows: Vec<(bool, String, u64, u64)> = Vec::new();
    for e in std::fs::read_dir(at)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        let Ok(m) = e.metadata() else { continue };
        let modified = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        rows.push((m.is_dir(), name, m.len(), modified));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    Ok(rows
        .into_iter()
        .map(|(dir, name, size, modified)| {
            serde_json::json!({"name": name, "dir": dir, "size": size, "modified": modified})
        })
        .collect())
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
pub fn projects_here(ws: Option<&config::Workspace>) -> Vec<crate::uistate::Project> {
    let mut out: Vec<crate::uistate::Project> = Vec::new();
    for f in ws.map(|w| w.folders.as_slice()).unwrap_or_default() {
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
/// Looks up a session's location from its screen number (1-based)
/// The surface a screen number stands for. Numbers are 1-based; 0 is no
/// surface at all -- a pane with nothing in it yet
pub fn ui_surface_at(surfaces: &[Surface], n: usize) -> Option<&Surface> {
    surfaces.get(n.checked_sub(1)?)
}
pub fn session_at(surfaces: &[Surface], active: usize) -> Option<usize> {
    match surfaces.get(active.checked_sub(1)?)? {
        Surface::Session(i) => Some(*i),
        Surface::Browser { .. } | Surface::Git { .. } | Surface::Sftp { .. } => None,
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
pub fn start_remote_bg(
    cfg: Option<&config::Config>,
    password: Option<&str>,
) -> Option<std::sync::mpsc::Receiver<(Option<remote::RemoteUi>, Vec<String>)>> {
    let c = cfg.filter(|c| c.remote.enabled)?;
    let (tx, rx) = std::sync::mpsc::channel();
    // Resolving the address and token is local and quick — done here, so the
    // thread owns only the part that can actually stall (the bind itself).
    // A fixed token that is too short to be a secret must never quietly
    // become "the usual token instead": the person believes the string they
    // wrote is the key. Refuse to start and say why (status + settings note)
    if c.remote.sticky_token && c.remote.fixed_token.trim().len() < FIXED_TOKEN_MIN {
        let _ = tx.send((None, vec![i18n::tp("err.remote.fixed_short", &[("n", &FIXED_TOKEN_MIN.to_string())])]));
        return Some(rx);
    }
    match netaddr::resolve_bind(&c.remote.bind, c.remote.allow_public) {
        Ok((ip, note)) => {
            let token = remote_token(c, password);
            let port = c.remote.port;
            let remote_password = c.remote.password.clone();
            let sticky = c.remote.sticky_token;
            std::thread::spawn(move || {
                let mut errors = Vec::new();
                let ui = match remote::RemoteUi::start_with(ip, port, token, remote_password, sticky) {
                    Ok(mut r) => {
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
                            "err.ws.remote_ui",
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
                vec![crate::i18n::tp("err.ws.remote_ui", &[("e", &e.to_string())])],
            ));
        }
    }
    Some(rx)
}
/// Passes the current listening status along so the settings screen can show the QR code
pub fn publish_remote(info: &Arc<Mutex<webui::RemoteInfo>>, ui: &Option<remote::RemoteUi>) {
    let mut i = info.lock().unwrap();
    match ui {
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
/// back to the app's own working folder when the tab has none configured.
pub fn tab_cwd_abs(t: &Tab) -> String {
    // Where somebody put this tab is where it belongs, and a `cd` typed inside
    // it does not move it. But for a tab nobody gave a folder, what the shell
    // in it says about itself beats the only other answer there was -- the
    // folder this program happens to be running from, which is a place a file
    // dropped from a phone had no business landing in.
    let reported = || {
        let r = t.reported_cwd();
        (!r.is_empty()).then(|| std::path::PathBuf::from(r))
    };
    let abs = match t.cwd().map(std::path::Path::to_path_buf) {
        Some(p) if p.is_absolute() => Some(p),
        Some(p) => std::env::current_dir().ok().map(|c| c.join(p)),
        None => reported().or_else(|| std::env::current_dir().ok()),
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
        Surface::Browser { key: k, name } if k == key => Some(hooks::PageCtx {
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
        pending_send.push(PendingSend::new(target, chunks, true, seen, now_ms));
    }
    true
}
/// The screen to move to, following the ball. None if it shouldn't move.
///
/// Don't follow right after a human touches the screen. Getting yanked away
/// mid-read is the worst outcome, so once someone touches it, stay quiet for a while.
/// Which workspace to start from.
///
/// What's remembered is the name, not the number. Numbers shift with
/// reordering or additions, which would turn "resume where I left off
/// yesterday" into something else entirely.
/// Falls back to the first one if not found (e.g. it was deleted or renamed).
pub fn starting_workspace(enabled: bool, last: Option<&str>, names: &[String]) -> usize {
    if !enabled {
        return 0;
    }
    last.and_then(|want| names.iter().position(|n| n == want))
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
pub fn surface_keys(surfaces: &[Surface], tabs: &[Tab]) -> Vec<hooks::TabKey> {
    surfaces
        .iter()
        .map(|p| match p {
            Surface::Session(i) => tabs.get(*i).map(|t| t.key()).unwrap_or_default(),
            // A page and a panel are addressed the same way a session is:
            // by the name automation knows them by, never the one on screen
            Surface::Browser { key, .. } | Surface::Git { key, .. } | Surface::Sftp { key, .. } => {
                hooks::TabKey { id: Some(key.clone()) }
            }
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
/// Executes the operation requests queued by Lua hooks.
/// Auto-sends inherit chain depth (the invisible ball) and stop once the cap is hit.
#[allow(clippy::too_many_arguments)]
pub fn exec_commands(
    cmds: Vec<Command>,
    tabs: &mut [Tab],
    surfaces: &[Surface],
    panes: &mut crate::layout::Layout,
    surface_count: usize,
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
                    PaneOp::Split(dir) => {
                        *active = split_focused(panes, dir, surface_count, *active);
                        append_hook_log(&format!("pane split {dir:?} (lua) -> surface {active}"));
                    }
                    PaneOp::Close => {
                        if panes.close(panes.focus()) {
                            *active = panes.focused_surface();
                        } else {
                            *flash = Some(i18n::t("msg.pane_last"));
                        }
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
                    pending_send.push(PendingSend::new(idx, chunks, false, seen, now_ms));
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
                    pending_send.push(PendingSend::new(target, chunks, true, seen, now_ms));
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
        // "I want to look at this tab" is the same thing as Ctrl+B <digit>
        Ev::Select { tab } if *tab <= 9 => {
            prefixed(char::from_digit(*tab as u32, 10).unwrap_or('0'))
        }
        // The tab bar's + is prefixed so it works no matter which tab is showing
        Ev::AddTab { .. } => prefixed('t'),
        // The board's menu is a plain keystroke while looking at INDEX.
        // Adding the prefix key would mean only characters present on both sides work.
        Ev::Menu { key } => key.chars().next().map(plain).map(|k| vec![k]).unwrap_or_default(),
        // The workspace-switcher button. Prefixed (Ctrl+B w) so it opens the
        // list no matter which tab is showing — a bare 'w' would be typed into
        // the visible session instead (the old Menu "w" bug: "wwww").
        Ev::OpenWs => prefixed('w'),
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
                vec![Event::Key(KeyEvent::new(
                    KeyCode::Char(c),
                    KeyModifiers::CONTROL,
                ))]
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

/// How many tabs are busy across every workspace: what a shell shows a person
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
            "短い固定トークンではリモートを起動しない");
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

    /// The echoed command line carries BOTH markers inside one line and must
    /// never be captured; the real output block (bare marker on its own
    /// line) must be. And an echo alone (not sent yet) captures nothing
    #[test]
    fn env_block_comes_from_output_not_echo() {
        let echo_only = "D:\\run>echo ===SHIKISHA ENV=== & ver & echo ===ENV END===";
        assert!(extract_env_block(echo_only).is_none(), "エコー行だけでは捕捉しない");

        let screen = "D:\\run>echo ===SHIKISHA ENV=== & ver & echo --- tools --- & where git 2>nul & echo ===ENV END===\n\
                      ===SHIKISHA ENV=== \n\
                      \n\
                      Microsoft Windows [Version 10.0.26200]\n\
                      --- tools --- \n\
                      C:\\Program Files\\Git\\cmd\\git.exe\n\
                      ===ENV END=== \n\
                      \n\
                      D:\\run>";
        let got = extract_env_block(screen).expect("出力ブロックを捕捉する");
        assert!(got.contains("Microsoft Windows"), "{got}");
        assert!(got.contains("git.exe"), "{got}");
        assert!(!got.contains("where git"), "エコー行は含めない: {got}");
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
    use super::*;

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

        assert_eq!(under("public").as_deref(), Some("D:/work/site/public"), "中は通る");
        assert_eq!(under("D:/work/site/public/a.txt").as_deref(),
                   Some("D:/work/site/public/a.txt"), "絶対でも中なら通る");
        assert_eq!(under("public/../a.txt").as_deref(), Some("D:/work/site/a.txt"),
                   "行って戻るだけなら中");
        assert_eq!(under(""), Some("D:/work/site".to_string()), "根そのもの");

        assert_eq!(under(".."), None, "一つ上は外");
        assert_eq!(under("public/../../../secrets"), None, "遠回りしても外");
        assert_eq!(under("C:/Windows"), None, "別のドライブは外");
        assert_eq!(under("D:/work/site-two"), None, "名前が続いているだけの別フォルダ");
    }

    /// The same fence, drawn where paths look like this instead.
    #[test]
    #[cfg(unix)]
    fn the_panels_folder_is_as_far_as_it_goes_on_unix() {
        let root = std::path::Path::new("/work/site");
        let under = |at: &str| local_under(root, at).map(|p| display_path_of(&p));

        assert_eq!(under("public").as_deref(), Some("/work/site/public"), "中は通る");
        assert_eq!(
            under("/work/site/public/a.txt").as_deref(),
            Some("/work/site/public/a.txt"),
            "絶対でも中なら通る"
        );
        assert_eq!(under("public/../a.txt").as_deref(), Some("/work/site/a.txt"), "行って戻るだけなら中");
        assert_eq!(under(""), Some("/work/site".to_string()), "根そのもの");

        assert_eq!(under(".."), None, "一つ上は外");
        assert_eq!(under("public/../../../secrets"), None, "遠回りしても外");
        assert_eq!(under("/etc"), None, "根の外は外");
        assert_eq!(under("/work/site-two"), None, "名前が続いているだけの別フォルダ");
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
        assert!(lines[0].contains("reviewer"), "どのタブか: {}", lines[0]);
        // The answer itself, folded onto one line -- a notification is not a
        // place to reproduce a screen
        assert_eq!(lines[1], "Found 3 problems. The first is in tab.rs.");
        // ...then a blank line, a label, and the link, so the link is not
        // mistaken for part of what the AI said
        assert_eq!(lines[2], "");
        assert!(!lines[3].is_empty(), "リンクの前に一言ある");
        assert_eq!(lines[4], "http://100.64.1.2:8787/r/K3fQ92mZxAbC");
        assert_eq!(lines.len(), 5);
        // The link is a ticket, never the board's key
        assert!(!msg.contains("?t="), "トークンが載っていない: {msg:?}");

        // Not asked for: the answer and nothing else. Not the link, and not
        // the machine's address either
        let quiet = on_done_message("builder", "done", None);
        assert_eq!(quiet.lines().count(), 2);
        assert!(!quiet.contains("http"), "住所も出さない: {quiet:?}");
        assert!(!quiet.ends_with('\n'));

        // Nothing said (a tab that finished silently): just the name
        assert_eq!(on_done_message("x", "   ", None).lines().count(), 1);

        // A long answer is cut where a person can still read it, and says so
        let long = on_done_message("x", &"あ".repeat(400), None);
        let said = long.lines().nth(1).unwrap();
        assert_eq!(said.chars().count(), 161, "160字＋省略記号");
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

        let plain = pressed("enter", false).expect("Enter が届いていない");
        let shifted = pressed("enter", true).expect("Shift+Enter が届いていない");
        assert!(!plain.modifiers.contains(KeyModifiers::SHIFT));
        assert!(
            shifted.modifiers.contains(KeyModifiers::SHIFT),
            "窓が送った修飾が途中で落ちている"
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
                "誰も頼んでいないのに綴りが変わっている: {code:?} {mods:?}"
            );
        }
        assert_eq!(bytes(&k(KeyCode::Enter, KeyModifiers::SHIFT), 0), Some(b"\r".to_vec()));

        // Asked for: the four keys that had no way to be told apart
        assert_eq!(
            bytes(&k(KeyCode::Enter, KeyModifiers::SHIFT), 1),
            Some(b"\x1b[13;2u".to_vec()),
            "Shift+Enter が普通のEnterのまま"
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
            "Shift+Tab は押された時点で修飾を名前に含んでいる"
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
            "時間より前に呼んでいる"
        );
        assert_eq!(
            busy_repeat_due(300_000, every, &busy, &mut tracked),
            vec![1],
            "時間になっても呼んでいない"
        );
        assert_eq!(tracked.get(&1), Some(&600_000), "次の時刻を置いていない");
        assert!(
            busy_repeat_due(300_001, every, &busy, &mut tracked).is_empty(),
            "続けざまに二度呼んでいる"
        );

        // Tab 2 is working too, but automation was never told about it: it is
        // not this app's place to start
        assert!(!tracked.contains_key(&2), "頼まれていないタブを数えている");

        // The work ends, and the asking stops with it -- including for a tab
        // that has gone to waiting on a person
        let answered = vec![TabState::Question, TabState::Busy, TabState::Done];
        assert!(
            busy_repeat_due(900_000, every, &answered, &mut tracked).is_empty(),
            "人を待っているタブについて呼び続けている"
        );
        assert!(tracked.is_empty(), "終わったタブの予定が残っている");

        // A tab that disappeared takes its place in the queue with it
        tracked.insert(9, 0);
        assert!(
            busy_repeat_due(1_000_000, every, &busy, &mut tracked).is_empty(),
            "もう無いタブについて呼んでいる"
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
            "見ているスマホの寸法に端末が合わない"
        );
        // Nobody watching from afar: the window wears its own measurement again,
        // without waiting for anyone to resize anything
        assert_eq!(
            terminal_size(window, phone, false),
            Size { width: 118, height: 40 },
            "誰も見ていないのに端末がスマホの寸法のまま"
        );
        // A phone that has connected but not yet measured itself decides nothing
        assert_eq!(
            terminal_size(window, None, true),
            Size { width: 118, height: 40 },
            "寸法を報告していないスマホが端末を決めてしまった"
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
            .expect("分割したのにペインが1つしかない")
            .0;
        let surfaces = vec![Surface::Session(0), Surface::Session(1)];
        let geom = vec![
            PaneGeom { id: back, rows: 50, cols: 200, rect: (0, 0, 800, 900) },
            // What the window measured for the pane in front. The phone is
            // looking at that same tab through a screen a fraction of the size
            PaneGeom { id: front, rows: 50, cols: 100, rect: (800, 0, 800, 900) },
        ];
        let want = tab_sizes(2, &layout, &surfaces, &geom, (24, 40));
        assert_eq!(want[1], (24, 40), "見ている本人の画面に端末が合わない");
        assert_eq!(want[0], (50, 200), "奥のペインが窓の実測を失った");
        // Undivided — every phone's case, and the window's most of the time —
        // the one pane there is takes the reported size whole
        let alone = crate::layout::Layout::single(1);
        assert_eq!(
            tab_sizes(1, &alone, &surfaces, &[], (24, 40))[0],
            (24, 40),
            "分割していないのに報告された寸法が使われない"
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
        assert!(!only_one_here(&tabs, 0), "同じCLI・同じフォルダの相方がいる");
        let (plan, why) = resume_plan(&tabs[0], only_one_here(&tabs, 0), true);
        assert_eq!(plan, tab::Resume::Fresh);
        assert_eq!(why, Some("msg.resume.ambiguous"), "理由を言って新規にする");

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
        assert_eq!(plan, tab::Resume::Id(mine), "相方がいても取り違えようがない");
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
    fn workspace_from(json: &str) -> config::Workspace {
        let json = json.replace("<sh>", &crate::test_shell());
        let cfg: config::Config = serde_json::from_str(&json).unwrap();
        cfg.resolve_workspaces().0.into_iter().next().unwrap()
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
        let ws = workspace_from(
            r#"{"workspaces":[{"name":"W","folders":[{"tabs":[{"name":"AGENT","command":"claude"}]}]}]}"#,
        );
        let cfg = &ws.tabs[0].cfg;
        let argv = vec!["claude".to_string()];
        let here = Some(std::path::PathBuf::from("D:\\Work"));
        let remembered = |program: &str, session: &str| crate::lastsession::Saved {
            version: 1,
            workspaces: vec![crate::lastsession::SavedWs {
                name: "W".into(),
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
            carried_conversation(Some(saved), &ws, &argv, cfg, &here, "AGENT")
        };

        // This tab was told to start clean, so nothing is carried however well
        // it is remembered
        let known = remembered("claude", "11111111-1111-4111-8111-111111111111");
        let mut off = cfg.clone();
        off.restore_conversation = Some(false);
        assert_eq!(
            carried_conversation(Some(&known), &ws, &argv, &off, &here, "AGENT"),
            tab::Resume::Fresh,
            "設定を切っても引き継いでいる"
        );

        // A conversation that is no longer on this machine. Handing the CLI an
        // id it has never heard of makes it refuse to start, in red, in its own
        // words -- which is not an answer to "I reopened the app"
        assert_eq!(plan(&known), tab::Resume::Fresh, "消えた会話を渡している");

        // Remembered under another program: the same name a year later can be
        // a different CLI, and resuming a conversation into one is nonsense
        assert_eq!(
            plan(&remembered("codex", "11111111-1111-4111-8111-111111111111")),
            tab::Resume::Fresh,
            "別のCLIの会話を渡している"
        );

        // A CLI with no way of being told which conversation to resume. Gemini
        // can be handed a new id and can be told "the latest", but not "that
        // one" -- and "the latest" is a guess, not this tab's conversation
        let gemini = vec!["gemini".to_string()];
        assert_eq!(
            carried_conversation(
                Some(&remembered("gemini", "11111111-1111-4111-8111-111111111111")),
                &ws,
                &gemini,
                cfg,
                &here,
                "AGENT",
            ),
            tab::Resume::Fresh,
            "指定できないCLIに会話を渡している"
        );

        // Nothing remembered at all -- a tab that is new since last time
        let empty = crate::lastsession::Saved { version: 1, workspaces: Vec::new() };
        assert_eq!(plan(&empty), tab::Resume::Fresh);
    }

    /// The Vault's choice outranks what the tab was saying last time.
    ///
    /// Reopening a past conversation names the one to resume, deliberately, a
    /// moment ago. "What this tab happened to be running when the app closed"
    /// is not an answer to that, and quietly preferring it would make the Vault
    /// open the wrong conversation.
    #[test]
    fn a_reopened_conversation_outranks_the_remembered_one() {
        let ws = workspace_from(
            r#"{"workspaces":[{"name":"W","folders":[{"tabs":[
                {"name":"AGENT","command":"claude","resume":"picked-from-the-vault"}
            ]}]}]}"#,
        );
        assert_eq!(
            resume_plan_of(ws.tabs[0].cfg.resume.as_deref()),
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
                "盤面は {key} を出しているのに、INDEX に受け手が無い"
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
        assert_eq!(super::coach_step(0, 1, false), (Some(1), 1), "1歩目が次のフレームで消える");
        // One folder, nothing started in it: point at its +
        assert_eq!(super::coach_step(1, 1, false), (Some(2), 1));
        // An AI (or a branch) appeared: over, for good
        assert_eq!(super::coach_step(1, 1, true), (None, 2));
        assert_eq!(super::coach_step(1, 2, false), (None, 2), "閉じた歩が戻ってきた");
        // Two folders at once: the second pointer is skipped, not shown later
        assert_eq!(super::coach_step(2, 1, false), (None, 1));
        // Somebody from before the pointer existed is not pointed at anything
        assert_eq!(super::coach_step(3, 0, false), (None, 0));
        // Folders all removed later: not a first run any more
        assert_eq!(super::coach_step(0, 2, false), (None, 2));
    }

    #[test]
    fn the_add_tab_button_arrives_prefixed() {
        let evs = super::keys_for(&shikisha_shared::Ev::AddTab { pane: None, folder: None });
        assert_eq!(evs.len(), 2, "前置キー + 本体の2打鍵");
        let Event::Key(k) = &evs[0] else { panic!("前置キーが打鍵でない") };
        assert_eq!(k.code, KeyCode::Char('b'));
        assert!(k.modifiers.contains(KeyModifiers::CONTROL));
        let Event::Key(k) = &evs[1] else { panic!("本体が打鍵でない") };
        assert_eq!(k.code, KeyCode::Char('t'));
        assert!(k.modifiers.is_empty());
    }

    /// The workspace-switcher button must arrive prefixed (Ctrl+B w) so it opens
    /// the list from any tab. The old Menu "w" path was a plain 'w', which just
    /// got typed into whatever session was showing ("wwww") instead of opening.
    #[test]
    fn the_workspace_button_arrives_prefixed() {
        let evs = super::keys_for(&shikisha_shared::Ev::OpenWs);
        assert_eq!(evs.len(), 2, "前置キー + 'w' の2打鍵");
        let Event::Key(k) = &evs[0] else { panic!("前置キーが打鍵でない") };
        assert_eq!(k.code, KeyCode::Char('b'));
        assert!(k.modifiers.contains(KeyModifiers::CONTROL));
        let Event::Key(k) = &evs[1] else { panic!("本体が打鍵でない") };
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
        assert!(patterns.len() > 5, "dist.list を読めていない ({} 件)", patterns.len());

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
                "dist.list の `{p}` に当てはまるものが1つも無く、取得する道具も知らない (綴り間違い?)"
            );
        }

        // ...and the consumers must go through it rather than keeping their own copy
        let build_rs = include_str!("../../../build.rs");
        assert!(build_rs.contains("dist.list"), "build.rs が dist.list を読んでいない");
        let release = include_str!("../../../.github/workflows/release.yml");
        assert!(release.contains("stage.ps1"), "release.yml が共通の配布処理を呼んでいない");
        for hardcoded in ["Copy-Item -Recurse \"lang\"", "docs/AUTOMATION.md\", \"docs/AUTOMATION.ja.md\""] {
            assert!(
                !release.contains(hardcoded),
                "release.yml が独自の配布物リストを持っている: {hardcoded}"
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
        let page = |k: &str| Surface::Browser { key: k.into(), name: k.into() };
        let surfaces = vec![
            page(SETTINGS_TAB),
            page(RESULT_TAB),
            page("shop"),
            Surface::Session(0),
        ];
        // active is 1-based over the surfaces
        assert_eq!(restartable_page(&surfaces, 1, &caps), None, "設定画面は対象外");
        assert_eq!(restartable_page(&surfaces, 2, &caps), None, "実行結果は対象外");
        // A user's page only qualifies once we know how it was opened
        assert_eq!(restartable_page(&surfaces, 3, &caps), None, "開き方を知らないうちは対象外");
        assert_eq!(restartable_page(&surfaces, 4, &caps), None, "セッションはここではなく session_mut の担当");
        assert_eq!(restartable_page(&surfaces, 0, &caps), None, "盤面(INDEX)には戻す先が無い");
    }

    /// The status bar's restart button must land on the same keystroke a person
    /// at the window would press, and must carry the prefix so it works from
    /// whichever tab is showing. Without the prefix an 'r' would simply be typed
    /// into the session (the "wwww" bug the workspace button already ran into).
    #[test]
    fn the_restart_button_arrives_prefixed() {
        let evs = super::keys_for(&shikisha_shared::Ev::Restart);
        assert_eq!(evs.len(), 2, "前置キー + 'r' の2打鍵");
        let Event::Key(k) = &evs[0] else { panic!("前置キーが打鍵でない") };
        assert_eq!(k.code, KeyCode::Char('b'));
        assert!(k.modifiers.contains(KeyModifiers::CONTROL));
        let Event::Key(k) = &evs[1] else { panic!("本体が打鍵でない") };
        assert_eq!(k.code, KeyCode::Char('r'));
        assert!(k.modifiers.is_empty());
        // Ctrl+B r has to still be the tab restart on the receiving side
        let body = include_str!("runtime.rs");
        assert!(
            body.contains("// Ctrl+B r restarts this tab"),
            "受け手の Ctrl+B r が消えている"
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
            assert_eq!(evs.len(), 1, "{key}: 打鍵が1つでない");
            let Event::Key(k) = &evs[0] else {
                panic!("{key}: 打鍵になっていない")
            };
            assert_eq!(k.code, KeyCode::Char(key.chars().next().unwrap()));
            assert!(
                k.modifiers.is_empty(),
                "{key}: 前置キーが付いている ({:?})",
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
        assert!(can_wait(&draft) && can_wait(&send), "渡すものが待てない");
        assert_eq!(target_of(&draft).map(|t| format!("{t:?}")).as_deref(),
                   Some("Name(\"ai\")"));

        for other in [
            Command::Restart { target: TabRef::Index(1), fresh: false },
            Command::Notify { dest: Some("slack".into()), text: "x".into() },
            Command::Log("x".into()),
            Command::SendKeys { target: TabRef::Index(1), keys: "y".into() },
        ] {
            assert!(!can_wait(&other), "待つ必要のないものを預かっている: {other:?}");
        }
    }

    /// What gets passed to a browser's hook must be built from the screen layout.
    ///
    /// The number matches the one a human presses. The name is the
    /// human-readable one, distinct from the id automation addresses it by.
    #[test]
    fn a_page_knows_its_number_and_both_of_its_names() {
        let surfaces = vec![
            Surface::Browser { key: "html".into(), name: "HTML解析".into() },
            Surface::Session(0),
        ];
        let page = page_ctx(&surfaces, "html", "https://example.com/".into(), true)
            .expect("並びにあるのに見つからない");
        assert_eq!(page.index, 1, "画面の番号と違う");
        assert_eq!(page.id, "html", "自動化から指す呼び名が違う");
        assert_eq!(page.name, "HTML解析", "人が読む名前が出ていない");
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
        let ws = ws_from(&[
            ("HTML解析", "html", "browser https://example.com/"),
            ("エンジニア", "ai", "claude"),
        ]);
        let mut ws = ws;
        ws.tabs[0].cfg.automation = Some("scripts/html".into());
        ws.tabs[1].cfg.automation = Some("scripts/ai".into());

        let got = automation_by_pane(&ws);
        // Ordered by screen number: the browser is 1, claude is 2
        assert_eq!(
            got,
            vec![
                (1, TabAuto::Path("scripts/html".to_string())),
                (2, TabAuto::Path("scripts/ai".to_string())),
            ],
            "割り当てがずれている"
        );
    }

    /// A discussion participant's/referee's tab id must resolve correctly to a screen number
    #[test]
    fn discuss_agents_resolve_to_panes() {
        let ws = ws_from(&[
            ("参加A", "ai1", "claude"),
            ("参加B", "ai2", "codex"),
            ("審判", "ref", "claude"),
        ]);
        assert_eq!(surface_of_id(&ws, "ai1"), Some(1));
        assert_eq!(surface_of_id(&ws, "ai2"), Some(2));
        assert_eq!(surface_of_id(&ws, "ref"), Some(3));
        assert_eq!(surface_of_id(&ws, "いない"), None);
        // The name on screen is a label, not an address: two tabs may share one
        assert_eq!(surface_of_id(&ws, "審判"), None);
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
        let mut ws = ws_from(&[
            ("エージェント", "ai", "claude"),
            ("ページ", "br", "browser https://example.com/"),
        ]);
        ws.tabs[0].cfg.drives = Some("br".into());
        ws.tabs[0].cfg.automation = Some("scripts/mine".into());

        assert_eq!(
            automation_by_pane(&ws),
            vec![(1, TabAuto::Path("scripts/mine".to_string()))],
            "狙いを持つタブが自分の自動化を奪われている"
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
        let ws = ws_from(&[
            ("HTML解析", "html", "browser https://example.com/"),
            ("エンジニア", "ai", "claude"),
        ]);
        let tabs = ["エンジニア"];
        let hosted = vec!["html".to_string()];

        let surfaces = surfaces_of(Some(&ws), &tabs, &hosted);
        assert_eq!(
            surfaces,
            vec![Surface::Browser { key: "html".into(), name: "HTML解析".into() }, Surface::Session(0)],
            "設定の順に並んでいない"
        );
        // A session must be resolvable from its screen number
        assert_eq!(session_at(&surfaces, 1), None, "1番はブラウザのはず");
        assert_eq!(session_at(&surfaces, 2), Some(0));
        // The ball moves by session number; what's displayed is the screen number
        assert_eq!(surface_at(&surfaces, 1), 2);
    }

    fn ws_from(rows: &[(&str, &str, &str)]) -> config::Workspace {
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
        config::Workspace {
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
        }
    }

    /// A browser that hasn't been opened yet must still keep the position written in config.
    ///
    /// If the number shifted based on open order, whatever a script points to
    /// would change every run. Failure to open should just be shown through
    /// state, not by moving the slot.
    #[test]
    fn a_browser_keeps_its_place_even_before_it_opens() {
        let ws = ws_from(&[
            ("HTML解析", "html", "browser https://example.com/"),
            ("エンジニア", "ai", "claude"),
        ]);
        let tabs = ["エンジニア"];
        let surfaces = surfaces_of(Some(&ws), &tabs, &[]);
        assert_eq!(
            surfaces,
            vec![Surface::Browser { key: "html".into(), name: "HTML解析".into() }, Surface::Session(0)],
            "開く前だと番号がずれる"
        );
    }

    /// Things not written in config must be appended at the end.
    /// There's no way to decide a position for a browser automation opened
    /// later, or a tab launched via arguments.
    #[test]
    fn what_the_settings_do_not_mention_goes_last() {
        let ws = ws_from(&[("エンジニア", "ai", "claude")]);
        let tabs = ["エンジニア", "あとから"];
        let hosted = vec!["settings".to_string()];
        let surfaces = surfaces_of(Some(&ws), &tabs, &hosted);
        assert_eq!(
            surfaces,
            vec![
                Surface::Session(0),
                Surface::Session(1),
                Surface::Browser { key: "settings".into(), name: "settings".into() }
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
        assert_eq!(settings_active(&before), 3, "開く前は末尾の次");

        // Already open: points to its existing location (the end). Not one slot further.
        let after = vec![
            Surface::Session(0),
            Surface::Session(1),
            Surface::Browser { key: "settings".into(), name: "settings".into() },
        ];
        assert_eq!(settings_active(&after), 3, "開いていればその場所");
    }

    /// The activity wave reflects actual output, not decoration, so it must stay flat when nothing came out
    #[test]
    fn activity_wave_reflects_real_output() {
        let argv = vec![crate::test_shell()];
        let mut t =
            Tab::spawn("SHELL".into(), &argv, None, 20, 100, tab::TabOptions::default()).unwrap();
        assert_eq!(t.activity().len(), tab::ACTIVITY_LEN);
        assert!(t.activity().iter().all(|l| *l == 0), "起動直後は無音");

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
            "出力があれば波形が立つ: {:?}",
            t.activity()
        );
        t.kill();
    }

    /// Sending must be two stages: "type the text" then "submit it".
    ///
    /// Writing it all in one go means Enter arrives before the AI CLI's input
    /// box has finished processing the paste, leaving the text typed but never
    /// submitted (this actually happened with sends from a phone).
    #[test]
    fn a_prompt_is_typed_first_and_submitted_after() {
        let argv = vec![crate::test_shell()];
        let mut t =
            Tab::spawn("shell".into(), &argv, None, 20, 60, tab::TabOptions::default()).unwrap();

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
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            screen(&t).contains("echo shikisha-ok"),
            "本文は入力欄に入る: {}",
            screen(&t)
        );
        assert!(
            !has_line(&t, "shikisha-ok"),
            "まだ実行はされていない: {}",
            screen(&t)
        );

        // The reserved submit arrives
        t.write_bytes(b"\r").unwrap();
        assert!(wait_for(&t, "shikisha-ok"), "実行される: {}", screen(&t));

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
        assert!(!touched_recently(&t, 0), "起動した瞬間");
        assert!(!touched_recently(&t, 1_000), "1秒後");
        assert!(
            !touched_recently(&t, MANUAL_GUARD_MS - 1),
            "ガード時間の内側でも、触られていなければ送ってよい"
        );

        // The guard kicks in once a human touches it
        t.last_manual_ms = Some(10_000);
        assert!(touched_recently(&t, 10_000), "触った直後");
        assert!(
            touched_recently(&t, 10_000 + MANUAL_GUARD_MS - 1),
            "ガード時間内はまだ効く"
        );
        assert!(
            !touched_recently(&t, 10_000 + MANUAL_GUARD_MS),
            "時間が過ぎたら解ける"
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
        let mut p = PendingSend::new(1, one(1), true, 100, 1_000);
        assert!(handed(&p.step(100, 1_000)), "最初のひと塊はすぐ渡す");
        assert!(waited(&p.step(200, 1_100)), "反応が始まっただけでは送らない");
        assert!(waited(&p.step(300, 2_000)), "まだ増えている");
        assert!(waited(&p.step(400, 3_000)), "まだ増えている");
        assert!(waited(&p.step(400, 3_100)), "止まった直後はまだ");
        assert!(waited(&p.step(400, 3_100 + SUBMIT_QUIET_MS - 1)), "静かな時間が足りない");
        assert!(submitted(&p.step(400, 3_100 + SUBMIT_QUIET_MS)), "落ち着いたら送る");

        // Restart the measurement if activity resumes partway through
        let mut p = PendingSend::new(1, one(1), true, 0, 0);
        assert!(handed(&p.step(0, 0)), "ひと塊目");
        assert!(waited(&p.step(0, 100)), "静かだがまだ足りない");
        assert!(waited(&p.step(50, 200)), "再開したので測り直す");
        assert!(waited(&p.step(50, 300)), "ここで改めて静止を観測");
        assert!(waited(&p.step(50, 300 + SUBMIT_QUIET_MS - 1)), "測り直し中");
        assert!(submitted(&p.step(50, 300 + SUBMIT_QUIET_MS)), "改めて落ち着いた");

        // Send anyway once the cap is hit, even if it never settles
        let mut p = PendingSend::new(1, one(1), true, 0, 0);
        assert!(handed(&p.step(0, 0)), "ひと塊目");
        let mut out = 0;
        for t in (100..SUBMIT_GIVE_UP_MS).step_by(100) {
            out += 1;
            assert!(!submitted(&p.step(out, t)), "増え続けている間は待つ ({t}ms)");
        }
        out += 1;
        assert!(submitted(&p.step(out, SUBMIT_GIVE_UP_MS)), "上限に達したら送る");
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
        let mut p = PendingSend::new(1, vec![vec![b'a'], vec![b'b'], vec![b'c']], true, 0, 0);
        assert!(handed(&p.step(0, 0)), "ひと塊目はすぐ");
        // Silent recipient: not a word drawn. It must not be given the rest at
        // once, and above all must not be sent Enter.
        assert!(waited(&p.step(0, 10)), "描かないうちは次を渡さない");
        assert!(waited(&p.step(0, PASTE_ACK_MS - 1)), "待ちきる前は渡さない");
        assert!(handed(&p.step(0, PASTE_ACK_MS)), "描かないままなら待って渡す");
        // Drawing means it has caught up, so the rest can go straight away
        let last = PASTE_ACK_MS + 1;
        assert!(handed(&p.step(9, last)), "描いたらすぐ次を渡す");
        // Only now does the settling rule start, and it is measured from the
        // first pass that sees the recipient still — not from the last piece
        assert!(waited(&p.step(9, last + 10)), "ここで静止を観測しはじめる");
        assert!(waited(&p.step(9, last + 10 + SUBMIT_QUIET_MS - 1)), "静かな時間が足りない");
        assert!(submitted(&p.step(9, last + 10 + SUBMIT_QUIET_MS)), "全部渡してから送信");

        // A draft is placed and left alone: the body goes over, the Enter never does
        let mut p = PendingSend::new(1, vec![vec![b'a']], false, 0, 0);
        assert!(handed(&p.step(0, 0)), "本文は渡す");
        assert!(waited(&p.step(0, 10)), "静止を観測しはじめる");
        assert!(submitted(&p.step(0, 10 + SUBMIT_QUIET_MS)), "本文は渡し終える");
        assert!(!p.submit, "下書きは Enter を打たない");
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
        let mut queue = vec![
            PendingSend::new(1, vec![vec![b'A']], true, 0, 0),
            PendingSend::new(1, vec![vec![b'B']], true, 0, 0),
            PendingSend::new(2, vec![vec![b'C']], true, 0, 0),
        ];
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
        assert_eq!(acted, vec![true, false, true], "同じタブは順番待ち、別のタブは並行");

        // The one behind has handed over nothing at all, so nothing of it can
        // have landed inside the message in front
        assert_eq!(queue[1].handed, 0, "後ろの本文が先に流れ込んでいる");
    }

    /// A person typing into a tab mid-paste must not be typed into the middle
    /// of the paste. The rest of it goes over first, in one piece.
    #[test]
    fn typing_pushes_the_rest_of_the_paste_out_first() {
        let mut p = PendingSend::new(1, vec![vec![b'a'], vec![b'b'], vec![b'c']], true, 0, 0);
        assert!(handed(&p.step(0, 0)), "ひと塊目");
        assert_eq!(p.rest(500), b"bc".to_vec(), "残りは一度に出す");
        assert_eq!(p.rest(500), Vec::<u8>::new(), "二度は出さない");
        // The Enter still follows, measured from the moment the rest went over
        assert!(waited(&p.step(0, 510)), "ここから静止を測り直す");
        assert!(submitted(&p.step(0, 510 + SUBMIT_QUIET_MS)), "送信はそのあと");
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
            let mut cfg = config::Config::default();
            cfg.providers.insert(
                "t".into(),
                config::ProviderSpec {
                    base_url: "http://127.0.0.1:1/v1".into(),
                    timeout_sec: Some(secs),
                    ..Default::default()
                },
            );
            cfg
        };
        let argv = vec!["model".to_string(), "t/m".to_string()];

        bridge::set_providers(&settings(180), None);
        let conn = bridge::launch_for(&argv).expect("接続が引ける");
        assert_eq!(conn.timeout, Some(Duration::from_secs(180)));
        let mut tabs = vec![Tab::spawn(
            "model".into(),
            &argv,
            None,
            10,
            40,
            tab::TabOptions { model: Some(conn), ..Default::default() },
        )
        .expect("起動")];

        // The wait is changed to "as long as it takes" and saved
        reload_providers(&settings(0), None, tabs.iter_mut());
        assert_eq!(
            tabs[0].model.as_ref().and_then(|c| c.timeout),
            None,
            "設定を変えてもタブが古い待ち時間を握ったまま"
        );
        tabs[0].kill();
    }

    /// A paste is cut at character boundaries, so no character is ever split
    /// across two writes (a broken character would be drawn as garbage).
    #[test]
    fn a_paste_is_cut_between_characters() {
        let t = Tab::spawn("cmd".into(), &[crate::test_shell()], None, 24, 80, tab::TabOptions::default())
            .expect("起動");
        let text = "あ".repeat(PASTE_CHUNK); // 3 bytes each: boundaries never land on PASTE_CHUNK
        let chunks = paste_chunks(&t, &text);
        assert!(chunks.len() > 1, "長い本文は分割される");
        for c in &chunks {
            assert!(
                std::str::from_utf8(c).is_ok(),
                "塊の途中で文字が割れている"
            );
        }
        let joined: String = chunks.iter().map(|c| String::from_utf8_lossy(c).into_owned()).collect();
        assert!(joined.contains(&text), "つなげたら元の本文に戻る");
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
        assert!(!gate(false, 0, false).may(g), "設定を無視して切り替えている");
        // They are reading the settings screen
        assert!(!gate(true, 0, true).may(g), "設定画面から引き剥がしている");

        // They just moved the view themselves — stay out of the way
        assert!(!gate(true, 1_000, false).may(1_000), "読んでいる最中に引き剥がしている");
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
        assert_eq!(scrolled_to(0, 3), 3, "遡れていない");
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
        assert_eq!(p.screen().scrollback(), 0, "今へ戻らない");
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

    /// A browser in the row must not hide the tabs behind it.
    ///
    /// Everything that points at a tab counts by screen number, browsers
    /// included. Counting sessions instead would make the tabs sitting behind
    /// however many browsers there are look like "numbers that don't exist".
    /// (With the layout Analysis=1 browser / AI=2 session, the AI was unreachable.)
    #[test]
    fn a_browser_in_the_row_does_not_hide_the_tabs_behind_it() {
        let surfaces = vec![
            Surface::Browser { key: "html".into(), name: "解析".into() },
            Surface::Session(0),
        ];
        let keys = surface_keys(&surfaces, &[]);
        assert_eq!(
            hooks::TabRef::Index(2).resolve(&keys),
            Some(2),
            "ブラウザの後ろのタブを指せていない"
        );
        assert_eq!(
            hooks::TabRef::Name("html".into()).resolve(&keys),
            Some(1),
            "ブラウザを自動化での呼び名で指せていない"
        );
        assert_eq!(
            hooks::TabRef::Name("解析".into()).resolve(&keys),
            None,
            "画面の名前では届かない"
        );
    }




    /// On first run, INDEX must show onboarding guidance (never leave the user
    /// unsure what to do).
    /// Launching must start from the workspace that was previously open.
    ///
    /// Always starting from the first one means extra switching effort every
    /// launch whenever what you want to try is the second one. During
    /// debugging, that gets repeated dozens of times.
    #[test]
    fn it_opens_where_you_left_off() {
        let names: Vec<String> = ["指揮者", "たまごカート編集部", "検証"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        assert_eq!(
            starting_workspace(true, Some("たまごカート編集部"), &names),
            1,
            "前に開いていたものに戻らない"
        );

        // What's remembered is the name, not the number, so it still tracks after reordering
        let reordered: Vec<String> = ["検証", "たまごカート編集部", "指揮者"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            starting_workspace(true, Some("たまごカート編集部"), &reordered),
            1
        );
        assert_eq!(
            starting_workspace(true, Some("指揮者"), &reordered),
            2,
            "並べ替えで別のワークスペースを開いている"
        );

        // Deleted, renamed, no memory of it, or disabled -> falls back to the first one
        assert_eq!(starting_workspace(true, Some("消えた"), &names), 0);
        assert_eq!(starting_workspace(true, None, &names), 0);
        assert_eq!(starting_workspace(false, Some("検証"), &names), 0, "切ってある");
        assert_eq!(starting_workspace(true, Some("指揮者"), &[]), 0, "空でも落ちない");
    }


    /// The wordmark's 3 lines must be the same width (mismatched widths look broken)
    #[test]
    fn the_wordmark_rows_line_up() {
        let w: Vec<usize> = WORDMARK.iter().map(|l| l.chars().count()).collect();
        assert!(
            w.iter().all(|n| *n == w[0]),
            "行ごとに幅が違う: {w:?}"
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
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let screen = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents();
        t.kill();
        assert!(
            screen.contains("shikisha-cwd-test"),
            "指定した作業フォルダで起動する: {screen}"
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
        assert!(out.is_err(), "存在しないフォルダのまま起動してはいけない");
    }

    #[test]
    fn hot_reload_applies_changes_without_restarting_untouched_tabs() {
        let ws0 = workspace_from(
            r#"{"workspaces":[{"name":"T","folders":[{"tabs":[
                {"name":"one","command":"<sh>"},
                {"name":"two","command":"<sh>"}
            ]}]}]}"#,
        );
        let mut tabs = Vec::new();
        let mut errs = Vec::new();
        spawn_workspace(&ws0, 24, 80, &mut tabs, &mut errs, None);
        assert_eq!(tabs.len(), 2, "{errs:?}");
        let one_before = tabs[0].signature();

        // one: gains a lock (applies immediately) / two: removed / three: added
        let ws1 = workspace_from(
            r#"{"workspaces":[{"name":"T","folders":[{"tabs":[
                {"name":"one","command":"<sh>","locked":true},
                {"name":"three","command":"<sh>"}
            ]}]}]}"#,
        );
        let msg = apply_ws_config(&mut tabs, &ws1, 24, 80, &mut errs);

        assert_eq!(
            tabs.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
            vec!["one", "three"],
            "設定の順序どおりに並ぶ"
        );
        assert!(tabs[0].locked, "ロックは再起動なしで反映される");
        assert!(!tabs[0].needs_restart, "起動条件が同じなら再起動不要");
        assert_eq!(tabs[0].signature(), one_before, "既存セッションは維持される");
        assert!(msg.contains("added 1") && msg.contains("stopped 1"), "{msg}");

        // A change to the encoding requires a rebuild, so it gets deferred and flagged
        let ws2 = workspace_from(
            r#"{"workspaces":[{"name":"T","folders":[{"tabs":[
                {"name":"one","command":"<sh>","encoding":"shift_jis"},
                {"name":"three","command":"<sh>"}
            ]}]}]}"#,
        );
        let msg2 = apply_ws_config(&mut tabs, &ws2, 24, 80, &mut errs);
        assert!(tabs[0].needs_restart, "要再起動の印が付く");
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
            "過去の行が見えるはず: {contents}"
        );
        assert!(
            !contents.contains("line30"),
            "最新行は画面外のはず: {contents}"
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
            "閉じてもループが終わらない"
        );
    }
}


//! SHIKISHA-TERM: portable, multi-session AI orchestration TUI
//!
//! Phase 3: multi-tab + INDEX dashboard + config.json
//!
//! Launch:
//!   SHIKISHA-TERM.exe                 # tabs from config.json (falls back to 1 PowerShell tab)
//!   SHIKISHA-TERM.exe claude          # debug: launch the given command in a single tab
//!
//! Controls (prefix key Ctrl+B):
//!   Ctrl+B q      quit / Ctrl+B 0-9 switch tab (0=INDEX) / Ctrl+B n/p next/prev tab
//!   Ctrl+B [      copy mode / Ctrl+B b send a literal Ctrl+B
//! Mouse: wheel=scroll (copy mode) / left-drag=select & copy instantly / right-click=paste

// The UI draws into our own window. We don't need a black console, so don't let
// Windows allocate one. (Only the terminal-facing --settings mode opens one itself, on demand.)
#![windows_subsystem = "windows"]

mod agenthook;
mod api;
mod attach;
mod ball;
mod bridge;
mod browser;
mod browserstate;
mod caps;
mod config;
mod crypto;
mod detect;
mod digest;
mod exchange;
mod hooks;
mod i18n;
mod lastsession;
mod keys;
mod layout;
mod netaddr;
mod notify;
mod pr;
mod profile;
mod reader;
mod remote;
mod repo;
mod session_log;
mod sessionfind;
mod shell;
mod tab;
mod theme;
mod toast;
mod usage;
mod vault;
mod uistate;
mod update;
mod watch;
mod ws;
mod webui;
mod worktree;
mod wspack;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};


use detect::TabState;
use hooks::{Command, HookEngine, TabCtx};
use tab::{CopyState, Tab, extract_text};

const STATUS_BAR_HEIGHT: u16 = 1;


/// Records the reason for an abnormal exit. The TUI occupies the whole screen,
/// so this keeps a panic message from disappearing unseen.
fn install_crash_log() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let where_ = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        append_hook_log(&format!("!!! Crashed {where_}: {info}"));
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(config::logs_dir().join("crash.log"))
        {
            use std::io::Write as _;
            let _ = writeln!(f, "{where_}: {info}");
        }
        prev(info);
    }));
}

fn main() -> Result<()> {
    install_crash_log();
    // Child-process mode for the model bridge. It receives its connection info via env,
    // relays stdin -> response, then exits. It never spins up the main window/WebView etc.
    // (headless HTTP calls only)
    if std::env::args().nth(1).as_deref() == Some("--bridge") {
        return bridge::run();
    }
    // Hook mode. An AI CLI runs this from inside its own process tree when a
    // conversation starts, handing over its session id on stdin; this reports
    // it back through the pipe and exits.
    //
    // Which tab it belongs to is not worked out here, and does not have to be:
    // a tab's children are launched holding that tab's own API key, so the
    // report arrives already knowing who sent it. Nothing is printed — the
    // agent is reading this process's output, and a hook must never make a
    // sound the agent could mistake for its own
    if std::env::args().nth(1).as_deref() == Some("--hook") {
        return hook_mode(std::env::args().nth(2).unwrap_or_default());
    }
    // Move the legacy layout (config.json under the root) into the config folder (once only).
    // This must happen before loading, or we'd start up with the pre-migration empty config.
    config::migrate_legacy_config();
    // Clean up the exchange hand-off area. Sweep old run folders left behind by an abnormal
    // exit, collecting them at startup (temp files from a normal exit are already gone by
    // the time they're consumed). Anything older than 30 days.
    exchange::sweep_old(30);
    // Wipe the scratch area for private (throwaway) browsers. The premise is that it
    // disappears when closed, so if anything is left from a previous abnormal exit, it's
    // all garbage.
    browser::sweep_private();
    // Where WebView2's user data (cookies, cache) lives is decided per WebView, from
    // the folder config names — see browser::profiles_root. It is NOT set process-wide
    // here: WEBVIEW2_USER_DATA_FOLDER applies to every WebView at once, which quietly
    // undid the whole point of per-page profiles. Every tab shared one cookie jar, so
    // "separate profile" and "private" were settings that did nothing.
    // Decide the display language (config, then OS; falls back to English if untranslated)
    i18n::init(
        config::load().and_then(|c| c.language).as_deref(),
        // The exe's own folder first: the translations ship with the program and
        // are only ever read, so they stay where it was installed. Installed from
        // the Store, the config folder is somewhere else entirely -- looking only
        // there would find no ja.json and quietly fall back to English, on a
        // Japanese machine, with nothing to say it had happened.
        &[
            config::exe_dir(),
            config_file_dir(),
            std::path::PathBuf::from("."),
        ],
    );
    // Settings-only mode (edit settings in a browser without launching the main app)
    if std::env::args().nth(1).as_deref() == Some("--settings") {
        // This is a text-conversation mode, so make sure there's somewhere to talk
        open_console();
        // We can still show a QR code even without the main app running (the connection
        // info is assembled from settings each time)
        let info = Arc::new(Mutex::new(webui::RemoteInfo::default()));
        // Standalone settings mode has no master password, since the main app isn't
        // running. Encrypted secrets can't be edited; the list shows them as locked.
        let pw = Arc::new(Mutex::new(None));
        let web = webui::WebUi::start_with(config::config_file_path(), info, pw)?;
        println!("{}", i18n::tp("msg.settings_opened", &[("url", &web.url)]));
        open_browser(&web.url);
        println!("{}", i18n::t("msg.settings_wait"));
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        web.shutdown();
        return Ok(());
    }
    // Self-check for screen relaying: open a browser, save one CDP frame, and exit.
    // Looking at the saved image tells us whether frames come through even for a hidden WebView.
    if std::env::args().nth(1).as_deref() == Some("--cast-test") {
        open_console();
        let url = std::env::args().nth(2).unwrap_or_else(|| "https://example.com/".into());
        return cast_test(&url);
    }

    // Running it pops up the window. Only add a launcher in front when there's a reason to.
    run_in_window()
}

/// `--cast-test <url>`: opens a browser, starts screen relaying, saves the first frame
/// to logs/cast-test.jpg, and exits. For self-checks that don't need a human's eyes.
fn cast_test(url: &str) -> Result<()> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;

    println!("{}", crate::i18n::tp("cli.cast_test.opening", &[("url", url)]));
    let browser = browser::Browser::spawn(url, "cast-test")?;
    browser.screencast(None, true)?;
    println!("{}", crate::i18n::t("cli.cast_test.relaying"));

    let status = config::logs_dir().join("cast-test.txt");
    // The very first frame tends to be blank white, before anything's drawn.
    // Collect for a few seconds and save the "last one" instead.
    let settle = Instant::now() + Duration::from_secs(5);
    let mut last: Option<(Vec<u8>, u32, u32)> = None;
    let mut count = 0u32;
    loop {
        for ev in browser.drain() {
            if let browser::Ev::Frame { data, w, h, .. } = ev {
                let bytes = b64.decode(data.as_bytes()).map_err(|e| {
                    anyhow::anyhow!(crate::i18n::tp(
                        "cli.cast_test.bad_base64",
                        &[("e", &e.to_string())]
                    ))
                })?;
                last = Some((bytes, w, h));
                count += 1;
            }
        }
        if Instant::now() >= settle {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    match last {
        Some((bytes, w, h)) => {
            let path = config::logs_dir().join("cast-test.jpg");
            std::fs::write(&path, &bytes)?;
            let msg = format!(
                "OK: {} ({}x{}, {} bytes, {} frames)\n",
                path.display(),
                w,
                h,
                bytes.len(),
                count
            );
            let _ = std::fs::write(&status, &msg);
            print!("{}", crate::i18n::tp("cli.cast_test.saved", &[("msg", &msg)]));
        }
        None => {
            let _ = std::fs::write(&status, "TIMEOUT: no frame in 5s\n");
            println!("{}", crate::i18n::t("cli.cast_test.no_frame"));
        }
    }
    Ok(())
}

/// Screen size. Only width and height are needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u16,
    pub height: u16,
}

/// A rectangle on screen
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// The terminal size (rows, cols) passed to the PTY.
///
/// `size` already IS the content area: the page measures `#main` — the region
/// to the right of the tab bar and above the status bar — and reports the
/// rows/columns that fit there directly (see the shell's `report()`), which is
/// what `surface.size()` and the resize event carry. So this only guards the
/// floor; it must NOT subtract the tab bar or status bar again.
///
/// It used to. Back when `size` was the whole window in character cells, the
/// app drew its own tab bar and status bar, so it carved them out here. Once the
/// WebView took over that chrome and started measuring the content area itself,
/// the subtraction became a *second* one: every AI was handed the tab bar's
/// width in columns fewer than it had, rendering into only part of the width
/// with a wide blank margin on the right — and on a phone-narrow screen, where
/// the total column count is barely above it, it collapsed almost to nothing.
///
/// The tab bar's width was still being carried in here long after that, unread
/// behind an underscore, and a whole config field was computed for the sole
/// purpose of feeding it. Both are gone: the width is the window's business,
/// measured in pixels, and it is measured where it is drawn.
fn pty_dims(size: Size) -> (u16, u16) {
    (size.height.max(3), size.width.max(10))
}




/// Absolute line position counted from the bottom of the screen
fn abs_line(offset: usize, rows: u16, cursor_row: u16) -> usize {
    offset + rows.saturating_sub(1).saturating_sub(cursor_row) as usize
}

/// Generates a tab name from argv ("ssh" -> "SSH")
fn title_of(argv: &[String]) -> String {
    argv.first()
        .map(|c| {
            std::path::Path::new(c)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(c)
                .to_uppercase()
        })
        .unwrap_or_else(|| "SHELL".into())
}


/// Set, change, or remove the master password (INDEX menu [k])
fn manage_master_password(
    surface: &mut WinSurface,
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
        let Some(old) = surface.ask_password(&i18n::t("prompt.password.current"),
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
        let Some(new) = surface.ask_password(&i18n::t("prompt.password.new"),
            &i18n::t("prompt.password.new_note"),
        )? else {
            return Ok(i18n::t("msg.password.cancelled"));
        };
        if new.is_empty() {
            crypto::write_atomic(&path, &plain)?;
            *password = None;
            return Ok(i18n::t("msg.password.removed"));
        }
        let confirm = surface.ask_password(&i18n::t("prompt.password.confirm"), "")?;
        if confirm.as_deref() != Some(new.as_str()) {
            return Ok(i18n::t("msg.password.mismatch"));
        }
        crypto::write_atomic(&path, &serde_json::to_string_pretty(&crypto::encrypt(&plain, &new)?)?)?;
        *password = Some(new);
        Ok(i18n::t("msg.password.changed"))
    } else {
        // First-time setup
        let Some(new) = surface.ask_password(&i18n::t("prompt.password.set"),
            &i18n::t("prompt.password.set_note"),
        )? else {
            return Ok(i18n::t("msg.password.cancelled"));
        };
        if new.is_empty() {
            return Ok(i18n::t("msg.password.empty"));
        }
        let confirm = surface.ask_password(&i18n::t("prompt.password.confirm"), "")?;
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
fn plaintext_secrets_warning(cfg: Option<&config::Config>) -> Option<String> {
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

/// How long a message stays part of the state. The screen it lands on is what
/// decides when the toast actually fades (up to nine seconds for a long
/// warning); this is the slightly later moment the app stops carrying it, so
/// nothing stale is handed to a surface that arrives afterwards.
const FLASH_LIFE: Duration = Duration::from_secs(12);

/// The set of things needed to draw into our own window
struct WinSurface {
    win: std::rc::Rc<crate::browser::Browser>,
    /// What the window measured for itself, in character cells.
    rows: u16,
    cols: u16,
    /// What a phone measured for its own screen, when one has said. Kept apart
    /// from the window's own numbers because they are two different answers to
    /// two different questions, and `terminal_size` picks between them.
    phone: Option<(u16, u16)>,
    /// The last state we sent. Only send again when it changes.
    last: Option<crate::uistate::UiState>,
    /// The terminal contents as the page last got them, row by row. Kept in
    /// pieces because that is the shape of a change: an AI's spinner turning
    /// over moves one line, and the page can be told to repair just that one
    last_screen_rows: Vec<String>,
    /// What the picture was made from when those rows were built. Rendering
    /// the grid only to find it identical cost about a millisecond, sixty
    /// times a second, whether or not anything had happened
    last_screen_key: Option<ScreenKey>,
    /// The last cursor (row, col, shown) we sent. Placing the cursor forces the
    /// page to recompute layout, so re-sending an unchanged one every frame kept
    /// the WebView busy at ~60Hz for nothing. Only send again when it moves.
    last_cursor: Option<(u16, u16, bool)>,
    /// The content area (x, y, width, height). Where the browser gets placed.
    area: (i32, i32, i32, i32),
    /// Every pane as the page last measured it. One entry while the content
    /// area is undivided, one per pane once it is split. The page is the only
    /// one that can measure this, so it is reported rather than computed here
    pane_geom: Vec<crate::browser::PaneGeom>,
    /// The whole content area. Where a screen that covers the window goes
    full: (i32, i32, i32, i32),
    /// Panes clicked in the window. The loop moves focus to them
    focus_panes: Vec<u32>,
    /// Panes whose ✕ was pressed. The loop closes the view, not the tab
    close_panes: Vec<u32>,
    /// Dividers dragged in the window, as (pane, its split's new first share)
    pane_ratios: Vec<(usize, f32)>,
    /// Panes whose ⊞ / ⊟ caption button was pressed (pane, split downwards?)
    pane_splits: Vec<(u32, bool)>,
    /// ↻ / ⟲ pressed in a pane's caption: which pane, and whether to carry the
    /// conversation over
    restart_panes: Vec<(u32, bool)>,
    /// The size the terminal is now drawn at, when it has just been changed
    font_size: Option<u8>,
    /// The width the tab bar is now drawn at, when its edge has just been
    /// dragged. 0 = put away
    tab_width: Option<u16>,
    /// Pages placed in the window that have taken the keyboard since the last
    /// drain, by the name automation addresses them with
    touches: Vec<String>,
    /// Whether a placed page should be drawing the pen (the composer is shut)
    pen: Option<bool>,
    /// The pane that asked for a tab, if one did. Where the new tab lands
    add_tab_pane: Option<u32>,
    /// The pane tree as last sent to the page. Only send it again when it changes
    last_layout: String,
    /// The terminal contents last sent for each unfocused pane, and what they
    /// were made from. The focused pane goes through `last_screen_rows`, since
    /// it keeps the full renderer
    last_pane_screens: std::collections::HashMap<u32, (ScreenKey, String)>,
    /// Intents that arrived from the window, converted into the form the loop reads.
    /// The loop only understands terminal key input, so everything gets funneled there.
    pending: std::collections::VecDeque<Event>,
    /// Names of pages whose bar button was pressed to signal "done" by a human
    presses: Vec<String>,
    /// Pages that finished loading (id, URL, whether refs are settled too)
    loads: Vec<(String, String, bool)>,
    /// Scroll-back requested via the wheel (positive = further into the past)
    scrolls: Vec<(i32, u16, u16)>,
    /// Navigation requested via the top bar
    gos: Vec<crate::browser::Go>,
    /// The answer to a location query we asked for (name inside the window, URL, can-go-back, can-go-forward)
    wheres: Vec<(String, String, bool, bool)>,
    /// Browser load start/end notifications (name inside the window, whether loading).
    /// The name is in "{ws}/{id}" form; converting to the id happens on the loop side
    /// (WinSurface doesn't know about caps). Same convention as `wheres`.
    loading: Vec<(String, bool)>,
    /// Relay-screen frames (JPEG byte buffers). The loop delivers these to phones.
    frames: Vec<Vec<u8>>,
    /// The window was closed. With nowhere left to draw, the loop has no choice but to shut down.
    closed: bool,
    /// The settings page's "close settings" button was pressed. The loop closes the settings tab.
    close_settings: bool,
    /// The sidebar gear (or a deep-link shortcut) was pressed. The loop opens the
    /// settings page. Carries an optional section to land on and whether to return
    /// to the board once saved (Some = requested, None = not requested).
    open_settings: Option<(Option<String>, bool)>,
    /// The status bar's "remote connected" control was pressed. The loop cuts every
    /// remote session (rotates the token, drops the connections).
    remote_cut: bool,
    /// Lines a person finished in the composer, each with the tab it is for,
    /// awaiting delivery. Filled from both surfaces: the window's ipc and the
    /// phone's relay.
    says: Vec<(usize, String)>,
    /// Quick-action chips (Lua) fired from the bar, by index into config.actions.
    /// The loop looks up the code and runs it against the active tab.
    run_actions: Vec<usize>,
    /// "Operate a target tab" requests from the 🎯 panel: (target tab index, goal).
    /// target 0 = detach. The loop attaches the active AI as the target's operator.
    operates: Vec<(usize, String)>,
    /// 📼 record-mode toggles from the composer (true = arm the shown browser's
    /// recorder, false = silence recording everywhere).
    record_arms: Vec<bool>,
    /// ▶ Lua typed into the composer, awaiting a sandboxed run against the
    /// shown browser.
    run_luas: Vec<String>,
    /// Recorded steps reported by pages. The loop turns each into one Lua
    /// line for the composer.
    recorded: Vec<RecordedStep>,
    /// Text/keys typed into the composer while viewing a browser tab. The loop
    /// injects them into the shown browser — the very same caps.browser_inject the
    /// phone's relay uses, so the desktop composer and the phone share one path.
    injects: Vec<crate::browser::Input>,
    /// The 🎯 panel's "save the replay" button. The loop copies the newest
    /// run's replay.lua into Downloads and answers with a flash message.
    replay_saves: bool,
    /// ✨ natural-language requests awaiting a command suggestion from the
    /// assistant AI, aimed at the active terminal tab.
    suggests: Vec<String>,
    /// 🔍 environment-survey button presses (the loop types the probe).
    surveys: usize,
    /// Vault searches awaiting an answer -- what to look for in past
    /// conversations. The loop runs the search and puts the hits into state
    vault_queries: Vec<String>,
    /// Past conversations asked to be reopened as resuming tabs
    vault_opens: Vec<crate::browser::Ev>,
    /// Branches asked about, and asked for: (folder cut from, branch, what to
    /// grow it from, make it, what to bring along)
    branches: Vec<(String, String, String, bool, Vec<String>)>,
    /// Colours chosen for a project: (a folder in it, the colour)
    folder_colors: Vec<(String, String)>,
    /// Folders being looked through, and the one finally chosen
    browses: Vec<(String, bool)>,
    /// Folders renamed in the list: (folder, the new name)
    folder_names: Vec<(String, String)>,
    /// Folders taken out of the list. The files stay where they are
    folder_closes: Vec<String>,
    /// Branch folders thrown away for good
    folder_discards: Vec<String>,
}

impl WinSurface {
    /// Puts an externally-arrived operation into the same queue as window keystrokes.
    /// Whether it came from a phone or not, the loop sees no difference.
    fn inject(&mut self, ev: Event) {
        self.pending.push_back(ev);
    }

    /// Takes ownership of the names of pages whose bar button was pressed.
    /// The window only has a single report channel, so this is the only place that consumes it.
    fn take_presses(&mut self) -> Vec<String> {
        std::mem::take(&mut self.presses)
    }

    /// True if "close settings" was pressed (and clears the flag if so)
    fn take_focus_panes(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.focus_panes)
    }

    fn take_close_panes(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.close_panes)
    }

    fn take_pane_ratios(&mut self) -> Vec<(usize, f32)> {
        std::mem::take(&mut self.pane_ratios)
    }

    fn take_pane_splits(&mut self) -> Vec<(u32, bool)> {
        std::mem::take(&mut self.pane_splits)
    }
    fn take_restart_panes(&mut self) -> Vec<(u32, bool)> {
        std::mem::take(&mut self.restart_panes)
    }

    fn take_font_size(&mut self) -> Option<u8> {
        self.font_size.take()
    }

    fn take_tab_width(&mut self) -> Option<u16> {
        self.tab_width.take()
    }

    fn take_touches(&mut self) -> Vec<String> {
        std::mem::take(&mut self.touches)
    }

    fn take_pen(&mut self) -> Option<bool> {
        self.pen.take()
    }

    fn take_add_tab_pane(&mut self) -> Option<u32> {
        self.add_tab_pane.take()
    }

    /// Put the tab bar away, or bring it back out.
    ///
    /// The page owns the width and answers with the new one, so this asks
    /// rather than decides -- there is one number and one place that holds it
    fn toggle_tab_bar(&self) {
        let _ = self.win.eval("window.__toggleTabBar && window.__toggleTabBar();");
    }

    fn take_close_settings(&mut self) -> bool {
        std::mem::take(&mut self.close_settings)
    }

    /// The pending "open settings" request (section, return-on-save), if any, clearing it.
    fn take_open_settings(&mut self) -> Option<(Option<String>, bool)> {
        self.open_settings.take()
    }

    /// Open the Vault overlay on this window's page (the keyboard path; the
    /// click path opens it in the page directly)
    fn open_vault(&self) {
        let _ = self.win.eval("window.__openVault && window.__openVault();");
    }

    /// Open the command palette on this window's page.
    fn open_palette(&self) {
        let _ = self.win.eval("window.__openPalette && window.__openPalette();");
    }

    /// True if the "remote connected" control was pressed (and clears the flag if so)
    fn take_remote_cut(&mut self) -> bool {
        std::mem::take(&mut self.remote_cut)
    }

    /// Takes ownership of pages that finished loading (id, URL, whether settled)
    fn take_loads(&mut self) -> Vec<(String, String, bool)> {
        std::mem::take(&mut self.loads)
    }

    /// Takes ownership of navigation requested via the top bar
    fn take_gos(&mut self) -> Vec<crate::browser::Go> {
        std::mem::take(&mut self.gos)
    }

    /// Takes ownership of wheel signals (tick count, row and column pointed at)
    fn take_scrolls(&mut self) -> Vec<(i32, u16, u16)> {
        std::mem::take(&mut self.scrolls)
    }

    /// Takes ownership of location answers
    fn take_wheres(&mut self) -> Vec<(String, String, bool, bool)> {
        std::mem::take(&mut self.wheres)
    }
    fn take_loading(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.loading)
    }

    /// Takes ownership of accumulated relay frames (the loop delivers them to phones)
    fn take_frames(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.frames)
    }

    /// Takes ownership of chat lines typed into model tabs
    fn take_says(&mut self) -> Vec<(usize, String)> {
        std::mem::take(&mut self.says)
    }

    /// Takes the indices of Lua quick-actions fired since the last drain.
    fn take_run_actions(&mut self) -> Vec<usize> {
        std::mem::take(&mut self.run_actions)
    }

    /// Takes the 📼 record-mode toggles since the last drain.
    fn take_record_arms(&mut self) -> Vec<bool> {
        std::mem::take(&mut self.record_arms)
    }

    /// Takes the composer Lua awaiting a sandboxed run (▶) since the last drain.
    fn take_run_luas(&mut self) -> Vec<String> {
        std::mem::take(&mut self.run_luas)
    }

    /// Takes the recorded steps reported by pages since the last drain.
    fn take_recorded(&mut self) -> Vec<RecordedStep> {
        std::mem::take(&mut self.recorded)
    }

    /// Deliver one recorded Lua line (already JSON-encoded) to the composer.
    fn push_recorded(&self, line_json: &str) {
        let _ = self.win.eval(&format!("window.__recorded({line_json});"));
    }

    /// Takes the pending ✨ suggestion requests since the last drain.
    /// Route a Vault intent that arrived from the phone into the same queues a
    /// window-origin one uses, so both are drained in one place
    fn queue_vault(&mut self, ev: crate::browser::Ev) {
        match ev {
            crate::browser::Ev::VaultSearch { query } => self.vault_queries.push(query),
            ev @ crate::browser::Ev::VaultOpen { .. } => self.vault_opens.push(ev),
            _ => {}
        }
    }

    fn take_vault_queries(&mut self) -> Vec<String> {
        std::mem::take(&mut self.vault_queries)
    }

    fn take_vault_opens(&mut self) -> Vec<crate::browser::Ev> {
        std::mem::take(&mut self.vault_opens)
    }

    fn take_branches(&mut self) -> Vec<(String, String, String, bool, Vec<String>)> {
        std::mem::take(&mut self.branches)
    }

    fn take_folder_colors(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.folder_colors)
    }

    fn take_browses(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.browses)
    }

    fn take_folder_names(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.folder_names)
    }

    fn take_folder_closes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.folder_closes)
    }

    fn take_folder_discards(&mut self) -> Vec<String> {
        std::mem::take(&mut self.folder_discards)
    }

    fn take_suggests(&mut self) -> Vec<String> {
        std::mem::take(&mut self.suggests)
    }

    /// Deliver a finished ✨ suggestion (JSON: {ok, cmd?/error?}) to the composer.
    fn push_suggested(&self, json: &str) {
        let _ = self.win.eval(&format!("window.__suggested({json});"));
    }

    /// Takes the pending 🔍 survey presses since the last drain.
    fn take_surveys(&mut self) -> usize {
        std::mem::take(&mut self.surveys)
    }

    /// Deliver 🔍 survey progress (JSON: {stage} / {ok, error?}) to the board.
    fn push_surveyed(&self, json: &str) {
        let _ = self.win.eval(&format!("window.__surveyed({json});"));
    }

    /// Deliver the verdict of a ▶ run (JSON: null = clean, string = the error).
    fn push_lua_done(&self, err_json: &str) {
        let _ = self.win.eval(&format!("window.__luaDone({err_json});"));
    }

    /// Takes the composer inputs bound for the shown browser since the last drain.
    fn take_injects(&mut self) -> Vec<crate::browser::Input> {
        std::mem::take(&mut self.injects)
    }

    /// Takes the pending operate-a-target requests (target index, goal).
    fn take_operates(&mut self) -> Vec<(usize, String)> {
        std::mem::take(&mut self.operates)
    }

    /// Push the current quick actions into the shell page so a settings edit
    /// reflects live — the window isn't reloaded on a config change. (The phone
    /// re-reads them on its next page load, i.e. when it returns to the board.)
    fn push_actions(&self, actions_json: &str) {
        let _ = self.win.eval(&format!("window.__setActions({actions_json});"));
    }

    /// Push the colours in, for the same reason and by the same road.
    ///
    /// A scheme picked in the settings has to land on the window that is open,
    /// not on the next one. Everything is a variable already, so this is one
    /// rule being replaced -- the terminal's sixteen included, since the cells
    /// name their colour rather than carry it
    fn push_theme(&self) {
        let look = crate::config::load().map(|c| c.appearance).unwrap_or_default();
        let scheme = look.scheme();
        let vars = serde_json::to_string(&scheme.css_vars()).unwrap_or_else(|_| "\"\"".into());
        let light = crate::theme::is_light(&scheme);
        let _ = self
            .win
            .eval(&format!("window.__setTheme({vars}, {light});"));
    }

    fn take_events(&mut self, active_tab: Option<&Tab>) {
        use crate::browser::Ev;
        for ev in self.win.drain() {
            match ev {
                Ev::Resize { rows, cols, area, full, panes } => {
                    self.rows = rows;
                    self.cols = cols;
                    self.area = area;
                    self.full = full;
                    self.pane_geom = panes;
                    // Only to wake the loop, so a window dragged to a new size
                    // reaches the terminals on the next pass instead of after a
                    // sleep. The numbers themselves are read off `self` where
                    // the choice between the viewers is made (`terminal_size`).
                    self.pending.push_back(Event::Resize(cols, rows));
                }
                Ev::FocusPane { id } => self.focus_panes.push(id),
                Ev::ClosePane { id } => self.close_panes.push(id),
                Ev::PaneRatio { divider, ratio } => self.pane_ratios.push((divider, ratio)),
                Ev::SplitPane { id, down } => self.pane_splits.push((id, down)),
                Ev::RestartPane { id, keep } => self.restart_panes.push((id, keep)),
                Ev::FontSize { px } => self.font_size = Some(px),
                Ev::TabWidth { px } => self.tab_width = Some(px),
                Ev::JsError { msg } => {
                    crate::append_hook_log(&format!("Screen failure: {msg}"));
                }
                // The window was closed. If we don't shut down here, a process with
                // nowhere left to draw stays alive unseen, still holding the listening port.
                Ev::Closed => self.closed = true,
                // The settings page's "close settings" button. Where the tab actually
                // gets torn down (caps, active) isn't touched here — that's left to the loop.
                Ev::CloseSettings => self.close_settings = true,
                Ev::OpenSettings { section, ret } => self.open_settings = Some((section, ret)),
                Ev::VaultSearch { query } => self.vault_queries.push(query),
                ev @ Ev::VaultOpen { .. } => self.vault_opens.push(ev),
                Ev::Branch { from, branch, base, make, carry } => {
                    self.branches.push((from, branch, base, make, carry))
                }
                Ev::FolderColor { folder, color } => self.folder_colors.push((folder, color)),
                Ev::Browse { path, open } => self.browses.push((path, open)),
                Ev::FolderName { folder, name } => self.folder_names.push((folder, name)),
                Ev::FolderClose { folder } => self.folder_closes.push(folder),
                Ev::FolderDiscard { folder } => self.folder_discards.push(folder),
                Ev::RemoteCut => self.remote_cut = true,
                // A Lua quick-action was tapped. Remember its index; the loop looks
                // up the code and runs it (it has the hook engine and config).
                Ev::RunAction { index } => self.run_actions.push(index),
                // Operate-a-target request; the loop has the engine to attach it.
                Ev::Operate { target, goal } => self.operates.push((target, goal)),
                // Save the newest replay.lua to Downloads (the board can't
                // download over HTTP; the loop owns the answer message).
                Ev::ReplaySave => self.replay_saves = true,
                // ✨ suggestion request; the loop owns the assistant AI call.
                Ev::Suggest { text } => self.suggests.push(text),
                // 🔍 survey request; the loop types the probe and captures it.
                Ev::Survey => self.surveys += 1,
                // 📼 / ▶ from the composer, and recorded steps from pages. All
                // resolved by the loop (it knows the shown browser and the engine).
                Ev::Record { on } => self.record_arms.push(on),
                Ev::RunLua { code } => self.run_luas.push(code),
                Ev::Recorded {
                    from: Some(child),
                    act,
                    sel,
                    value,
                    xpath,
                    hint,
                } => self.recorded.push(RecordedStep { child, act, sel, value, xpath, hint }),
                // Composer input while viewing a browser tab. Stash it; the loop
                // injects it into the shown browser via caps.browser_inject — the
                // same call the phone's relay makes, not a desktop-only path.
                Ev::Inject { input, .. } => self.injects.push(input),
                // A file attached in the desktop composer. Save it beside the
                // active tab (the folder its AI runs in) and hand the path back to
                // the page. Same saver the phone's /api/attach route uses.
                Ev::Attach { id, name, data } => {
                    let cwd = active_tab.map(tab_cwd_abs).unwrap_or_default();
                    let result = crate::remote::attach_save(&cwd, &name, &data);
                    let _ = self
                        .win
                        .eval(&format!("window.__attachDone({id}, {result});"));
                }
                // The top bar was pressed. The destination is "whatever page is currently
                // showing", so the loop decides (only one bar is ever displayed).
                Ev::Go { go } => self.gos.push(go),
                Ev::Scroll { by, row, col } => self.scrolls.push((by, row, col)),
                Ev::Say { tab, text } => self.says.push((tab, text)),
                Ev::Where {
                    from: Some(name),
                    url,
                    can_back,
                    can_forward,
                } => self.wheres.push((name, url, can_back, can_forward)),
                // The bar on a placed page was pressed = a human finished their turn.
                // Who pressed it can only be told from the name attached to the report.
                Ev::Button { from: Some(name) } => self.presses.push(name),
                // A placed page took the keyboard. Only pages placed in the
                // window report this; the shell's own presses already say
                // which pane they landed on
                Ev::Touched { from: Some(name) } => self.touches.push(name),
                Ev::Touched { from: None } => {}
                // A tab was asked for from a pane with nothing in it. Note
                // which pane asked, then go on to open the form exactly as the
                // tab bar's + does -- one door, so the two cannot drift
                Ev::AddTab { pane: Some(id) } => {
                    self.add_tab_pane = Some(id);
                    for e in keys_for(&crate::browser::Ev::AddTab { pane: None }) {
                        self.pending.push_back(e);
                    }
                }
                // The pen a placed page drew for itself was pressed
                Ev::Compose { .. } => {
                    let _ = self.win.eval("window.__composer && window.__composer();");
                }
                // The window's page says whether that pen should be showing
                Ev::Pen { on } => self.pen = Some(on),
                // Our own page says it is up. It comes back blank — a reload
                // after an update, a first paint — and everything we send is
                // "what changed since last time", so unless the record of what
                // it already has is torn up here, the board stays empty until
                // something happens to move every part of it.
                Ev::Ready { from: None, .. } => {
                    self.last = None;
                    self.last_screen_rows.clear();
                    self.last_screen_key = None;
                    self.last_cursor = None;
                    self.last_layout.clear();
                    self.last_pane_screens.clear();
                }
                // A placed page finished loading (fires on every navigation)
                Ev::Ready {
                    from: Some(name),
                    url,
                    complete,
                } => self.loads.push((name, url, complete)),
                // A browser started/finished loading. Conversion to the id happens on
                // the loop side — doing it here as a display name would make WinSurface
                // need to know about caps.
                Ev::Loading {
                    from: Some(name),
                    busy,
                } => self.loading.push((name, busy)),
                // Treat the clipboard the same way the terminal side does
                Ev::Copy { text } => {
                    if let Ok(mut c) = arboard::Clipboard::new() {
                        let _ = c.set_text(text);
                    }
                }
                Ev::Paste => {
                    if let Some(t) = active_tab {
                        let _ = paste_clipboard(t);
                    }
                }
                // A relay frame. Decode the base64 into a byte buffer and stash it;
                // the loop delivers it to phones.
                Ev::Frame { data, .. } => {
                    use base64::Engine as _;
                    if let Ok(bytes) =
                        base64::engine::general_purpose::STANDARD.decode(data.as_bytes())
                    {
                        self.frames.push(bytes);
                    }
                }
                // Everything else can be converted into keystrokes. `keys_for` is the
                // single place that knows how.
                other => {
                    for e in keys_for(&other) {
                        self.pending.push_back(e);
                    }
                }
            }
        }
    }
}

/// One recorded step as reported by a page: which pane it came from, what
/// happened, and how the element was addressed (CSS, or a text-anchored
/// XPath). `hint` is the element's visible text, kept as a repair aid.
struct RecordedStep {
    child: String,
    act: String,
    sel: String,
    value: String,
    xpath: bool,
    hint: String,
}

/// One recorded step → one line of the dialect every Lua surface here speaks
/// (the rally, quick actions, ▶ run mode). JSON escaping is used for the
/// strings — Lua's double-quoted literals accept everything the recorder will
/// realistically produce (\n, \t, \", \\). An XPath selector becomes the
/// `{xpath=...}` table form `sel_of` already understands; a click keeps its
/// element's text as a trailing comment so a selector broken by a site change
/// can be repaired (by a person or an AI) without re-recording.
fn recorded_lua(name: &str, step: &RecordedStep) -> Option<String> {
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

/// Converts an intent from the screen into keystrokes the loop already understands.
///
/// The window and the phone use the same page. If there were two separate places
/// doing this conversion, the same press could end up meaning different things
/// depending on which one it came from.
/// Intents that can't be converted to a keystroke (load-complete, resize, etc.) return empty.
fn keys_for(ev: &crate::browser::Ev) -> Vec<Event> {
    use crate::browser::Ev;
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

/// Converts a control key sent by name into the terminal's key type
fn named_key(n: &str) -> Option<KeyCode> {
    Some(match n {
        "enter" => KeyCode::Enter,
        "bs" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        // Both spellings: the page and the phone have always sent "escape"
        // through their own map, and a name that works in one place and is
        // silently ignored in another is the worst kind of half-support
        "esc" | "escape" => KeyCode::Esc,
        "del" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "right" => KeyCode::Right,
        "left" => KeyCode::Left,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" => KeyCode::PageUp,
        "pgdn" => KeyCode::PageDown,
        _ => {
            let f = n.strip_prefix('f')?.parse::<u8>().ok()?;
            (1..=12).contains(&f).then_some(KeyCode::F(f))?
        }
    })
}

/// Opens our own window and runs the same loop on top of it
fn run_in_window() -> Result<()> {
    // Serve the shell page. file:// breaks wry's IPC, so serve it over local HTTP instead.
    let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| {
        anyhow::anyhow!(crate::i18n::tp(
            "err.main.local_server",
            &[("e", &e.to_string())]
        ))
    })?;
    let port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.main.no_port")))?
        .port();
    let page = shell::page();
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            // Only serves the shell page. The QR image rides along inside the state,
            // so there's no separate route for it (works even when the window and
            // the phone get served from different origins).
            let r = tiny_http::Response::from_string(page.clone()).with_header(
                tiny_http::Header::from_bytes(
                    &b"Content-Type"[..],
                    &b"text/html; charset=utf-8"[..],
                )
                .expect("header"),
            );
            let _ = req.respond(r);
        }
    });

    let win = std::rc::Rc::new(browser::Browser::spawn(
        &format!("http://127.0.0.1:{port}/"),
        "SHIKISHA-TERM",
    )?);
    run(WinSurface {
        win,
        rows: 40,
        cols: 120,
        phone: None,
        last: None,
        last_screen_rows: Vec::new(),
        last_screen_key: None,
        last_cursor: None,
        area: (0, 0, 0, 0),
        pane_geom: Vec::new(),
        full: (0, 0, 0, 0),
        focus_panes: Vec::new(),
        close_panes: Vec::new(),
        pane_ratios: Vec::new(),
        last_layout: String::new(),
        last_pane_screens: std::collections::HashMap::new(),
        pending: std::collections::VecDeque::new(),
        presses: Vec::new(),
        loads: Vec::new(),
        scrolls: Vec::new(),
        gos: Vec::new(),
        wheres: Vec::new(),
        loading: Vec::new(),
        frames: Vec::new(),
        closed: false,
        close_settings: false,
        open_settings: None,
        remote_cut: false,
        says: Vec::new(),
        run_actions: Vec::new(),
        vault_queries: Vec::new(),
        vault_opens: Vec::new(),
        branches: Vec::new(),
        folder_colors: Vec::new(),
        browses: Vec::new(),
        folder_names: Vec::new(),
        folder_closes: Vec::new(),
        folder_discards: Vec::new(),
        record_arms: Vec::new(),
        run_luas: Vec::new(),
        pane_splits: Vec::new(),
        restart_panes: Vec::new(),
        font_size: None,
        tab_width: None,
        touches: Vec::new(),
        pen: None,
        add_tab_pane: None,
        recorded: Vec::new(),
        operates: Vec::new(),
        replay_saves: false,
        suggests: Vec::new(),
        surveys: 0,
        injects: Vec::new(),
    })
}

/// Summarizes the current state into a form with no presentation attached
/// What a newly split pane should show.
///
/// The next surface that isn't already on screen, counting on from the one
/// being split — so splitting twice walks down the tab bar instead of asking
/// the same question twice. With nothing spare it falls back to the dashboard,
/// which is never wrong and never a duplicate.
/// What one hook event is worth keeping, once the CLI's JSON has been read.
///
/// Pure, so it can be tested: this is the only place on the hook path that
/// makes a judgment, and it runs inside a child process of the agent that is
/// not allowed to fail loudly.
#[derive(Debug, Default, PartialEq, Eq)]
struct Report {
    /// The conversation this is, when the event says
    id: Option<String>,
    /// What to tell the tab it is doing, in this app's own vocabulary
    state: Option<String>,
}

/// `kind` is what the hook entry asked for: `session`, or `state:<STATE>`.
fn hook_report(kind: &str, v: &serde_json::Value) -> Report {
    // The same fact goes by several names across the CLIs that report it, and a
    // CLI is free to rename it in its next release. Read every spelling anyone
    // is known to use rather than one and a shrug
    let id = ["session_id", "sessionId", "conversation_id", "conversationId"]
        .iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let Some(state) = kind.strip_prefix("state:") else {
        return Report { id: id.filter(|_| kind == "session"), state: None };
    };
    // A subagent's events carry its parent's session id, so its "finished"
    // would put the whole tab back to rest while the real turn runs on. The
    // one thing a subagent has to say that cannot wait is that it is asking
    // for permission — that dialog is in front of the person either way.
    //
    // The id is left to the event that exists to carry it: reporting it from
    // every event would write the same line into the log all day
    let sub = ["agent_id", "agent_type"]
        .iter()
        .any(|k| v.get(*k).is_some_and(|x| !x.is_null()));
    let keep = !sub || state.eq_ignore_ascii_case("QUESTION");
    Report { id: None, state: keep.then(|| state.to_string()) }
}

/// Every tab's identity and state, in the shape the automation engine reads.
///
/// Built in one place because two callers need the same thing. One is the
/// detection tick, so a script can read `shikisha.state`. The other is the
/// external API — and there it is not a convenience: a call arrives naming
/// nothing but its own key, and the engine works out which tab that is by
/// looking the name up in THIS list. An engine that has not been given it
/// attributes the call to nobody, which is how a hook comes to report a
/// conversation id "from tab0: no such tab" and lose it.
fn tab_states(tabs: &[Tab]) -> Vec<(hooks::TabKey, String)> {
    tabs.iter()
        .map(|t| (t.key(), t.state.label().to_string()))
        .collect()
}

/// Carry one hook event from an AI CLI back to the app.
///
/// Runs as a short-lived child of the agent. Reads the agent's JSON from stdin,
/// takes the one thing worth keeping, and hands it over the API pipe. Failure is
/// never reported to the agent — a hook that fails loudly would break the very
/// conversation it exists to preserve — so problems go to the log instead.
fn hook_mode(kind: String) -> Result<()> {
    use std::io::Read as _;
    let mut body = String::new();
    let read = std::io::stdin().read_to_string(&mut body);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    // Stamped where it was said, not where it lands. Hooks are separate
    // processes told not to block, so two of them race and the loser of the
    // race is not the loser of the argument
    let sent = hooks::epoch_ms() as i64;

    let report = hook_report(&kind, &v);
    let mut calls: Vec<(&str, Vec<serde_json::Value>)> = Vec::new();
    if let Some(id) = report.id {
        calls.push(("set_session", vec![id.into()]));
    }
    if let Some(state) = report.state {
        calls.push(("set_state", vec![state.into(), sent.into()]));
    }
    if calls.is_empty() {
        // A hook that quietly does nothing is the worst way for this to fail —
        // the conversation is lost at the next restart and nobody finds out
        // until then. Say what arrived, in enough detail to tell "the CLI sent
        // nothing" from "the CLI sent something we didn't recognise"
        append_hook_log(&format!(
            "hook {kind}: nothing to report — stdin {read:?}, {} bytes, {} fields",
            body.len(),
            v.as_object().map(|o| o.len()).unwrap_or(0)
        ));
        return Ok(());
    }
    // The same CLI is also used on its own, outside this app, and then there
    // is no app to report to. That is not a failure and must not be written
    // down as one: it would be a line in a log file for every turn of every
    // conversation anyone ever has
    if std::env::var(api::ENV_PIPE).is_err() {
        return Ok(());
    }
    let mut client = match api::ApiClient::from_env() {
        Ok(c) => c,
        Err(e) => {
            append_hook_log(&format!("hook {kind} could not report: {e}"));
            return Ok(());
        }
    };
    for (method, params) in calls {
        match client.call(method, params) {
            Ok(answer) if answer["ok"] == serde_json::json!(true) => {}
            Ok(answer) => append_hook_log(&format!("hook {kind} refused: {answer}")),
            Err(e) => append_hook_log(&format!("hook {kind} could not report: {e}")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod hook_report_tests {
    use super::*;

    fn claude(event: &str) -> serde_json::Value {
        serde_json::json!({
            "session_id": "abc123",
            "transcript_path": "/home/u/.claude/projects/x/abc123.jsonl",
            "cwd": "/home/u/project",
            "hook_event_name": event,
        })
    }

    #[test]
    fn the_conversation_is_taken_from_the_event_that_carries_it() {
        assert_eq!(
            hook_report("session", &claude("SessionStart")),
            Report { id: Some("abc123".into()), state: None }
        );
    }

    #[test]
    fn a_state_event_says_the_state_and_nothing_else() {
        assert_eq!(
            hook_report("state:BUSY", &claude("UserPromptSubmit")),
            Report { id: None, state: Some("BUSY".into()) }
        );
    }

    /// A subagent's events arrive under the parent's session id. Its "finished"
    /// is not the turn finishing — believing it puts a tab back to rest, fires
    /// on_done, and hands the ball on in the middle of the work.
    #[test]
    fn a_subagent_cannot_end_the_turn_but_can_still_ask() {
        let mut done = claude("Stop");
        done["agent_id"] = serde_json::json!("agent_42");
        assert_eq!(hook_report("state:DONE", &done).state, None, "サブの完了は本体の完了ではない");

        let mut ask = claude("PermissionRequest");
        ask["agent_id"] = serde_json::json!("agent_42");
        ask["tool_name"] = serde_json::json!("Bash");
        assert_eq!(
            hook_report("state:QUESTION", &ask).state,
            Some("QUESTION".into()),
            "誰が出したダイアログでも人は答えなければならない"
        );
    }

    /// Codex spells the conversation the same way; the ones that don't are
    /// covered by reading every spelling anyone is known to use
    #[test]
    fn another_clis_spelling_of_the_same_fact_is_read_too() {
        let v = serde_json::json!({ "conversationId": "b5f6c1c2", "hook_event_name": "SessionStart" });
        assert_eq!(hook_report("session", &v).id.as_deref(), Some("b5f6c1c2"));
    }

    #[test]
    fn an_event_with_nothing_in_it_reports_nothing() {
        let v = serde_json::json!({});
        assert_eq!(hook_report("session", &v), Report::default());
    }
}

/// What a restart should do about this tab's conversation, and — when it cannot
/// carry it — the reason to put on screen.
///
/// The decision lives here rather than in the tab because it depends on the
/// other tabs: continuing "whatever ran in this folder last" is only safe when
/// nobody else could have been what ran there.
///
/// Resuming the wrong conversation is worse than starting a new one, so every
/// uncertain case ends up at Fresh with something to say for itself.
/// The launch plan for a config tab: resume the id it names, or start fresh.
///
/// A tab reopened from the Vault carries the conversation's id; a plain tab
/// carries nothing and begins a new one. The id is trusted as a Store id --
/// it came from the CLI's own record, which is exactly what Store means
pub(crate) fn resume_plan_of(id: Option<&str>) -> tab::Resume {
    match id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => tab::Resume::Id(tab::Session {
            id: id.to_string(),
            source: tab::SessionSource::Store,
        }),
        None => tab::Resume::Fresh,
    }
}

fn resume_plan(t: &Tab, alone: bool, keep: bool) -> (tab::Resume, Option<&'static str>) {
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
fn restart_surface(
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
fn only_one_here(tabs: &[Tab], index: usize) -> bool {
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
fn restart_tab(t: &mut Tab, alone: bool, keep: bool, rows: u16, cols: u16) -> String {
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
fn split_focused(
    l: &mut crate::layout::Layout,
    dir: crate::layout::Dir,
    surface_count: usize,
    active: usize,
) -> usize {
    let next = free_surface(l, surface_count, active);
    l.split(dir, next);
    l.focused_surface()
}

fn free_surface(l: &crate::layout::Layout, surface_count: usize, from: usize) -> usize {
    (1..=surface_count)
        .map(|n| (from + n) % (surface_count + 1))
        .find(|n| *n != 0 && l.pane_of(*n).is_none())
        .unwrap_or(0)
}

/// The pane tree in the form the page draws it: one rectangle per pane, in
/// fractions of the content area.
///
/// Only geometry and identity travel here. What a pane *shows* — the name, the
/// state dot, whether it is a browser — the page already has from `__state`,
/// looked up by surface number. Sending it twice would let the two copies
/// disagree, and the pane would caption itself with a stale name.
fn panes_json(l: &crate::layout::Layout) -> String {
    #[derive(serde::Serialize)]
    struct Pane {
        id: crate::layout::PaneId,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        surface: usize,
        focused: bool,
    }
    /// One divider, in the same fractions. `i` is its position in
    /// `Layout::dividers()`, which is how a drag names it coming back
    #[derive(serde::Serialize)]
    struct Divider {
        i: usize,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        /// The first half's share, so the page can draw the handle on the line
        ratio: f32,
        /// true = the halves are stacked, so the divider lies across
        down: bool,
    }
    let focus = l.focus();
    let surfaces: std::collections::HashMap<_, _> = l.leaves().into_iter().collect();
    let panes: Vec<Pane> = l
        .rects()
        .into_iter()
        .map(|(id, r)| Pane {
            id,
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            surface: surfaces.get(&id).copied().unwrap_or(0),
            focused: id == focus,
        })
        .collect();
    let dividers: Vec<Divider> = l
        .dividers()
        .into_iter()
        .enumerate()
        .map(|(i, (r, dir, ratio))| Divider {
            i,
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
            ratio,
            down: dir == crate::layout::Dir::Col,
        })
        .collect();
    serde_json::json!({
        "single": l.is_single(),
        "focus": focus,
        "panes": panes,
        "dividers": dividers,
    })
    .to_string()
}

/// What decides whether a tab's picture differs from the one already on screen.
///
/// Every way the contents can move shows up in one of these: bytes arriving
/// from the program (counted by every path that writes to a screen, the app's
/// own notes included), a resize, scrolling back, and which tab is in front.
/// Asking is a mutex and five loads; rendering the grid to compare it against
/// the last one is a millisecond, sixty times a second, for an answer that is
/// almost always "nothing moved".
#[derive(Clone, Copy, PartialEq, Eq)]
struct ScreenKey {
    session: usize,
    bytes: u64,
    rows: u16,
    cols: u16,
    scrollback: usize,
}

/// What the page has to be told about a screen that has just been rendered.
#[derive(Debug, PartialEq, Eq)]
enum ScreenPush {
    /// It already shows exactly this
    Nothing,
    /// These rows, and nothing else, moved
    Rows(Vec<usize>),
    /// Hand it the whole grid again
    Whole,
}

/// Decides which of the three a frame calls for.
///
/// The whole grid goes out when the screen changed shape (the page's rows no
/// longer line up with ours, so a row number would land somewhere else), and
/// when enough of it moved that dozens of separate repairs cost more than one
/// parse of the lot. Everything in between -- which is nearly every frame an
/// AI at work produces -- is a handful of rows.
fn screen_push(had: &[String], now: &[String]) -> ScreenPush {
    if had.len() != now.len() {
        return ScreenPush::Whole;
    }
    let moved: Vec<usize> = (0..now.len()).filter(|&i| now[i] != had[i]).collect();
    match moved.len() {
        0 => ScreenPush::Nothing,
        n if n * 3 > now.len() => ScreenPush::Whole,
        _ => ScreenPush::Rows(moved),
    }
}

#[cfg(test)]
mod screen_push_tests {
    use super::{ScreenPush, screen_push};

    fn rows(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("row {i}")).collect()
    }

    /// An AI at work redraws its spinner and nothing else. That must reach the
    /// page as that one row: rewriting the grid makes the browser build every
    /// element on screen again, and the next keystroke in the composer waits
    /// behind that layout.
    #[test]
    fn a_spinner_turning_over_sends_one_row() {
        let had = rows(40);
        let mut now = had.clone();
        now[39] = "* Flambeing... (13s)".into();
        assert_eq!(screen_push(&had, &now), ScreenPush::Rows(vec![39]));
    }

    /// A screen that did not move is not worth a word.
    #[test]
    fn an_unchanged_screen_says_nothing() {
        let had = rows(40);
        assert_eq!(screen_push(&had, &had.clone()), ScreenPush::Nothing);
    }

    /// Output scrolling past moves nearly every row. Dozens of separate
    /// repairs cost more than handing over one string, so hand it over.
    #[test]
    fn a_scrolling_screen_goes_over_whole() {
        let had = rows(40);
        let now: Vec<String> = (0..40).map(|i| format!("row {}", i + 1)).collect();
        assert_eq!(screen_push(&had, &now), ScreenPush::Whole);
    }

    /// A resize renumbers everything. A row number sent now would land on a
    /// different line over there, so nothing may be said row by row.
    #[test]
    fn a_resized_screen_goes_over_whole() {
        assert_eq!(screen_push(&rows(40), &rows(50)), ScreenPush::Whole);
        // Including the first frame, when the page has nothing at all
        assert_eq!(screen_push(&[], &rows(40)), ScreenPush::Whole);
    }
}

fn screen_key(session: usize, t: &Tab) -> ScreenKey {
    let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
    let s = p.screen();
    let (rows, cols) = s.size();
    ScreenKey { session, bytes: t.output_count(), rows, cols, scrollback: s.scrollback() }
}

fn ui_state_of(tabs: &[Tab], ui: &Ui, flash: Option<&str>) -> crate::uistate::UiState {
    // The folders these tabs are actually in. Worked out here, once, so the
    // window and the phone are looking at the same list
    let groups = crate::uistate::GroupState::all(tabs, &ui.folder_colors);
    crate::uistate::UiState {
        groups: groups.iter().map(|(_, g)| g.clone()).collect(),
        branch: ui.branch.clone(),
        browse: ui.browse.clone(),
        workspace: ui
            .ws_names
            .get(ui.ws_index)
            .cloned()
            .unwrap_or_default(),
        workspaces: ui.ws_names.clone(),
        ws_index: ui.ws_index,
        active: ui.active,
        board: ui.board,
        settings_open: ui.settings,
        auto_enabled: ui.auto.unwrap_or(true),
        remote_on: ui.remote_on,
        remote_conn: ui.remote_conn,
        remote_sticky: ui.remote_sticky,
        aim: ui.aim,
        first_run: ui.first_run,
        // Keep the order exactly as written in the config.
        // Listing sessions and browsers separately would push the browser
        // written first to the back.
        tabs: ui
            .surfaces
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match p {
                Surface::Session(s) => tabs.get(*s).map(|t| {
                    let mut ts = crate::uistate::TabState::of(i + 1, t);
                    ts.group = t
                        .cwd()
                        .and_then(|c| groups.iter().position(|(k, _)| k == c));
                    ts
                }),
                Surface::Browser { key, name } => {
                    Some(crate::uistate::TabState::browser(i + 1, key, name))
                }
            })
            .collect(),
        // The ball moves by session number; what we display is the screen number
        ball: crate::uistate::BallState::of(&ui.ball, ui.max_chain, ui.now_ms),
        flash: flash.map(str::to_string),
        help_open: ui.help_open,
        help_rows: ui.help_rows.clone(),
        vault: ui.vault.clone(),
        self_cost: ui.self_cost.clone(),
        ws_open: ui.ws_open,
        qr: ui.qr.clone(),
        // Build the image just once here. Both the window and the phone read the
        // same state, so the same QR shows up regardless of origin (this ends the
        // link-rot we used to get back when it was served as a separate image).
        qr_svg: ui.qr.as_deref().map(|u| crate::netaddr::qr_svg(u, 6)),
        nav: ui.nav.clone(),
        scrolled: ui.scrolled,
        build: format!("build {}  ({})", env!("BUILD_TIME"), env!("BUILD_REV")),
        restartable: ui.restartable,
        discuss_start: ui.discuss_start,
        discuss_start_name: ui.discuss_start_name.clone(),
        // "At rest" = a discussion workspace where every participant's screen
        // has gone quiet and the automation ring has settled (Idle). We gauge
        // "quiet" from how long the screen has been unchanged rather than the
        // BUSY verdict, because some CLIs (Claude Code) leave a static status
        // footer that keeps the busy-pattern matcher latched — a screen that
        // hasn't changed in a couple of seconds is genuinely done regardless.
        // Requiring the ring to be idle too covers the brief hand-off gap
        // between turns, when the outgoing speaker has stopped but the ring is
        // still in flight — without it the banner would flicker mid-round.
        discuss_idle: ui.discuss_start.is_some() && {
            const QUIET_MS: u64 = 2000;
            let anyone_active = ui.surfaces.iter().any(|p| match p {
                Surface::Session(s) => tabs
                    .get(*s)
                    .map(|t| t.ms_since_change(ui.now_ms) < QUIET_MS)
                    .unwrap_or(false),
                Surface::Browser { .. } => false,
            });
            let ring_idle = matches!(ui.ball.phase(ui.now_ms), crate::ball::Phase::Idle);
            !anyone_active && ring_idle
        },
    }
}

impl WinSurface {
    fn size(&self) -> Result<Size> {
        Ok(Size { width: self.cols, height: self.rows })
    }

    /// Waits for the next operation. Intents from the window arrive already
    /// converted into key operations the loop knows about.
    fn poll(&mut self, timeout: Duration, active_tab: Option<&Tab>) -> Result<Option<Event>> {
        self.take_events(active_tab);
        if self.closed {
            return Ok(None);
        }
        if let Some(e) = self.pending.pop_front() {
            return Ok(Some(e));
        }
        std::thread::sleep(timeout);
        Ok(None)
    }

    /// Where browsers get placed. Placing them inside the window lets the OS handle
    /// position and stacking order for us.
    fn host(&self) -> Option<(std::rc::Rc<crate::browser::Browser>, (i32, i32, i32, i32))> {
        Some((std::rc::Rc::clone(&self.win), self.area))
    }

    /// Asks for a password. Not shown on the phone (the page side doesn't show it there either).
    fn ask_password(&mut self, title: &str, note: &str) -> Result<Option<String>> {
        let _ = self.win.eval(&format!(
            "return window.__password({},{});",
            serde_json::to_string(title).unwrap_or_default(),
            serde_json::to_string(note).unwrap_or_default()
        ));
        // Wait until the human finishes typing. No reason to rush them.
        self.win.wait_password(Duration::from_secs(600))
    }

    /// Hand the page the terminal's contents, saying as little as will do.
    ///
    /// A screen almost never changes all over: an AI at work redraws its
    /// spinner, a build prints a line. Rewriting the whole grid for that makes
    /// the browser throw away and rebuild every element on it — and the next
    /// keystroke in the composer has to wait behind that layout, which is what
    /// made typing to a thinking AI feel like wading through mud. So the rows
    /// that moved are sent on their own, and the whole grid only when the
    /// screen changed shape, or when most of it moved anyway (one parse beats
    /// dozens of separate repairs).
    fn send_screen(&mut self, rows: Vec<String>) {
        match screen_push(&self.last_screen_rows, &rows) {
            ScreenPush::Nothing => return,
            ScreenPush::Rows(moved) => {
                let list: Vec<(usize, &str)> =
                    moved.iter().map(|&i| (i, rows[i].as_str())).collect();
                let _ = self.win.eval(&format!(
                    "return window.__rows({});",
                    serde_json::to_string(&list).unwrap_or_default()
                ));
            }
            ScreenPush::Whole => {
                // The screen was redrawn whole (new shape, or a switched tab),
                // so re-place the cursor once even if its row/col is the same.
                self.last_cursor = None;
                let _ = self.win.eval(&format!(
                    "return window.__screen({});",
                    serde_json::to_string(&rows.join("\n")).unwrap_or_default()
                ));
            }
        }
        self.last_screen_rows = rows;
    }

    fn draw(&mut self, tabs: &[Tab], ui: &Ui, flash: Option<&str>) -> Result<()> {
        {
            {
                let w = &mut *self;
                let state = ui_state_of(tabs, ui, flash);
                if w.last.as_ref() != Some(&state) {
                    let json = serde_json::to_string(&state).unwrap_or_default();
                    if w.last.is_none() {
                        crate::append_hook_log(&format!(
                            "Sending state: {} tabs, workspace \"{}\", {} chars",
                            state.tabs.len(),
                            state.workspace,
                            json.len()
                        ));
                    }
                    let _ = w.win.eval(&format!(
                        "return window.__state({});",
                        serde_json::to_string(&json).unwrap_or_default()
                    ));
                    w.last = Some(state);
                }
                // The division of the content area. The focused pane keeps the
                // full renderer below (cursor, composer, board, browser chrome);
                // this only tells the page where each pane sits.
                let lay = panes_json(&ui.layout);
                if w.last_layout != lay {
                    w.last_layout = lay.clone();
                    let live: std::collections::HashSet<_> =
                        ui.layout.leaves().into_iter().map(|(id, _)| id).collect();
                    w.last_pane_screens.retain(|id, _| live.contains(id));
                    // The page empties the focused pane's read-only copy -- the
                    // full renderer draws over that rectangle instead. Forget
                    // what was last sent there, or the pane would stay empty
                    // after focus moved on: the copy it wants back is the one
                    // already recorded as sent, so nothing would be judged to
                    // have changed and nothing would be sent. The pane you had
                    // just left was the one that went blank
                    w.last_pane_screens.remove(&ui.layout.focus());
                    let _ = w.win.eval(&format!(
                        "return window.__panes({});",
                        serde_json::to_string(&lay).unwrap_or_default()
                    ));
                }
                // Every pane that isn't focused gets a read-only view of its
                // terminal. A browser pane needs nothing here — the page placed
                // in the window covers that rectangle itself.
                for (id, surface) in ui.layout.leaves() {
                    if id == ui.layout.focus() {
                        continue;
                    }
                    let seat = session_at(&ui.surfaces, surface);
                    let Some((i, t)) = seat.and_then(|i| tabs.get(i).map(|t| (i, t))) else {
                        continue;
                    };
                    // A pane nobody is looking at changes as often as one they
                    // are, so it gets the same guard: build the picture only
                    // once something it is made of has moved
                    let key = screen_key(i, t);
                    if w.last_pane_screens.get(&id).map(|(k, _)| *k) == Some(key) {
                        continue;
                    }
                    let html = {
                        let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                        crate::shell::screen_html(p.screen())
                    };
                    if w.last_pane_screens.get(&id).map(|(_, h)| h.as_str()) != Some(html.as_str()) {
                        let _ = w.win.eval(&format!(
                            "return window.__panescreen({},{});",
                            id,
                            serde_json::to_string(&html).unwrap_or_default()
                        ));
                    }
                    w.last_pane_screens.insert(id, (key, html));
                }
                // Only send the terminal contents for the tab currently being
                // viewed, and only once something it is made of has moved
                let seat = session_at(&ui.surfaces, ui.active);
                if let Some((i, t)) = seat.and_then(|i| tabs.get(i).map(|t| (i, t))) {
                    let key = screen_key(i, t);
                    if w.last_screen_key != Some(key) {
                        w.last_screen_key = Some(key);
                        let (rows, cursor) = {
                            let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                            let s = p.screen();
                            let (r, c) = s.cursor_position();
                            (crate::shell::screen_rows(s), (r, c, !s.hide_cursor()))
                        };
                        w.send_screen(rows);
                        // Placing the cursor forces a layout recompute in the
                        // page, so only do it when the cursor actually moved —
                        // not 60x a second onto an unchanged position.
                        if w.last_cursor != Some(cursor) {
                            w.last_cursor = Some(cursor);
                            let (r, c, on) = cursor;
                            let _ = w
                                .win
                                .eval(&format!("return window.__cursor({r},{c},{on});"));
                        }
                    }
                }
                Ok(())
            }
        }
    }
}

fn run(mut surface: WinSurface) -> Result<()> {
    // The mode flag is not a command to launch.
    // Forgetting to filter it out would send us looking for a program named `--window`.
    let cmd_args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !matches!(a.as_str(), "--settings"))
        .collect();
    let start = Instant::now();
    // Width comes from config if given; otherwise it's auto-computed from tab names
    // (finalized once tabs are launched).
    let (mut rows, mut cols) = pty_dims(surface.size()?);

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
    (rows, cols) = pty_dims(surface.size()?);
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
                match surface.ask_password(&i18n::t("prompt.password.title"), &note)? {
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
    // Capabilities granted to automation (empty by default). An advanced feature that
    // can only be enabled by writing it into the config file.
    let caps: hooks::Caps = std::rc::Rc::new(match cfg.as_ref() {
        Some(c) => caps::Capabilities::new(
            c.capabilities.clone(),
            config_file_dir(),
            c.resolve_tokens(password.as_deref()),
        ),
        None => caps::Capabilities::disabled(),
    });
    let mut engines: Vec<Option<HookEngine>> = (0..workspaces.len().max(1)).map(|_| None).collect();
    // If we have a window, put browsers inside it
    caps.set_host(surface.host());
    caps.set_workspace(ws_index);
    if let Some(w) = workspaces.get(ws_index) {
        // Restrict which secrets this workspace is allowed to use (deny-all by default)
        caps.set_secret_allow(w.secrets_allow.clone(), w.secrets_allow_all);
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
    // What the Vault overlay is showing right now: the last search and its
    // hits. Kept across frames so the results stay put until the next search,
    // and dropped from the state entirely while the overlay is closed
    let mut vault_view: Option<crate::uistate::VaultState> = None;
    // What making a branch would do. Answered while the name is being typed,
    // and cleared once the folder exists so the dialog can close itself
    let mut branch_view: Option<crate::uistate::BranchPlan> = None;
    // Branch folders asked to go, waiting for their tabs to finish leaving:
    // where it is, when it was asked for, and when it was last tried
    let mut pending_discards: Vec<(std::path::PathBuf, Instant, Instant)> = Vec::new();
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
    let mut last_remote_screen = String::new();
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
    // Check once in the background for whether a newer version is out (only notifies; doesn't update)
    let update_rx = update::spawn_check();
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
                if let Ok(u) = ensure_web_url(&mut web, &config_file, &remote_info, &web_password) {
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
            (surface.rows, surface.cols),
            surface.phone,
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
                &surface.pane_geom,
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
                max_chain = newcfg.max_chain.unwrap_or(10);
                auto_switch = newcfg.auto_switch.unwrap_or(true);
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
                );
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
                    last_remote_screen = String::new();
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
                surface.push_actions(&crate::shell::actions_json());
                surface.push_theme();
                let (next, errs) = crate::keys::Keys::load(cfg.as_ref());
                keymap = next;
                startup_errors.extend(errs);
            }
        }

        // Update notification. Shown once the screen is free, so it doesn't overwrite other output.
        if flash.is_none() {
            if let Ok(v) = update_rx.try_recv() {
                flash = Some(i18n::tp("msg.update_available", &[("version", &v)]));
            }
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

            // A tab whose launch command changed in settings is flagged for
            // restart, but only actually restarted here once it is idle — so a
            // running AI is never cut off. This makes "swap the AI in settings"
            // take effect on an idle tab on its own, instead of quietly keeping
            // the old process alive. The new session is treated as a fresh
            // launch (started_fired cleared) so its on_start briefing fires again.
            // A staged change to the launch conditions (encoding, scrollback…)
            // is not a reason to lose the conversation
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
                eng.set_states(tab_states(&tabs));
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
                            TabState::Done if old == TabState::Busy && !answering => {
                                append_hook_log(&format!(
                                    "Ignoring done tab{idx} [{}] prompted={} submitting={} answered={}",
                                    tabs[idx - 1].profile_name(),
                                    tabs[idx - 1].was_prompted(),
                                    submitting,
                                    tabs[idx - 1].answered_since_submit()
                                ));
                            }
                            TabState::Done if answering && old == TabState::Busy => {
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
                            if t.state != TabState::Done {
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
                            let msg = i18n::tp(
                                "msg.notify.on_done",
                                &[("name", &tabs[idx - 1].title)],
                            );
                            let status = notifier.send(&dest, &msg);
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
                    ui: surface.last.clone(),
                    screen_html: tabs
                        .get(session_at(&surfaces, active).unwrap_or(usize::MAX))
                        .map(|t| {
                            let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                            shell::screen_html(p.screen())
                        })
                        .unwrap_or_default(),
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
                // whenever it changes; the screen is rate-limited to ~7Hz so a
                // burst of output can't saturate a slow link (idle = nothing sent).
                if r.has_state_clients() {
                    let ui_json = serde_json::to_string(&snap.ui).unwrap_or_default();
                    if last_remote_ui.as_deref() != Some(ui_json.as_str()) {
                        r.push_state(format!("{{\"ui\":{ui_json}}}"));
                        last_remote_ui = Some(ui_json);
                    }
                    if snap.screen_html != last_remote_screen
                        && last_remote_push.elapsed() >= Duration::from_millis(140)
                    {
                        let scr = serde_json::to_string(&snap.screen_html).unwrap_or_default();
                        r.push_state(format!("{{\"screen_html\":{scr}}}"));
                        last_remote_screen = snap.screen_html.clone();
                        last_remote_push = Instant::now();
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
                        eng.call_primitive_as(call.caller.as_deref(), &call.method, &call.params)
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
                    remote::RemoteCmd::Ui(crate::browser::Ev::Inject { input, .. }) => {
                        if let Some(key) = &shown_browser {
                            let _ = caps.browser_inject(key, input);
                        }
                    }
                    // The top bar (back/forward/refresh/URL) doesn't turn into terminal
                    // keystrokes. Just like the window, push it onto `gos` and let the
                    // shared handling below pass it to the browser. Routing it through
                    // `keys_for` used to silently drop `Go` as unmatched.
                    remote::RemoteCmd::Ui(crate::browser::Ev::Go { go }) => {
                        surface.gos.push(go);
                    }
                    // Scrolling back through history isn't a keystroke, so keys_for()
                    // can't carry it — it would be dropped, leaving the phone stuck on
                    // the current screen with no way to review earlier output. Push it
                    // onto the very queue the window's own wheel feeds, so both are
                    // applied identically below (into a full-screen TUI's own scroll,
                    // or our kept scrollback for a plain shell).
                    remote::RemoteCmd::Ui(crate::browser::Ev::Scroll { by, row, col }) => {
                        surface.scrolls.push((by, row, col));
                    }
                    // The phone fits the terminal to its own screen. Its numbers are
                    // kept as the phone's own -- not written over the window's, which
                    // is what the window falls back to the moment nobody is watching
                    // from afar (see `terminal_size`). Its `area` is not taken either:
                    // that positions the window's own browser child view, which the
                    // phone doesn't use (it watches the relay), so the window keeps
                    // the placement it measured for itself.
                    remote::RemoteCmd::Ui(crate::browser::Ev::Resize { rows, cols, .. }) => {
                        surface.phone = Some((rows, cols));
                        surface.pending.push_back(Event::Resize(cols, rows));
                    }
                    // A Lua quick-action fired from the phone. It's not a keystroke,
                    // so route it straight to the same queue the window's ipc path
                    // fills (drained and run against the active tab below).
                    remote::RemoteCmd::Ui(crate::browser::Ev::RunAction { index }) => {
                        surface.run_actions.push(index);
                    }
                    remote::RemoteCmd::Ui(crate::browser::Ev::Operate { target, goal }) => {
                        surface.operates.push((target, goal));
                    }
                    // 📼 / ▶ from the phone's composer: same queues as the window's.
                    remote::RemoteCmd::Ui(crate::browser::Ev::Record { on }) => {
                        surface.record_arms.push(on);
                    }
                    remote::RemoteCmd::Ui(crate::browser::Ev::RunLua { code }) => {
                        surface.run_luas.push(code);
                    }
                    // ✨ a suggestion request from the phone: same queue as the
                    // window's (keys_for would silently drop it, like Go once was)
                    remote::RemoteCmd::Ui(crate::browser::Ev::Suggest { text }) => {
                        surface.suggests.push(text);
                    }
                    remote::RemoteCmd::Ui(crate::browser::Ev::Survey) => {
                        surface.surveys += 1;
                    }
                    // A line the phone finished in the composer. Not a
                    // keystroke -- the recipient may be a model bridge, which
                    // has no keyboard -- so it goes to the same queue the
                    // window's composer fills. Without this it fell through to
                    // keys_for and was dropped, which the loop's own
                    // fall-through guard had been saying all along.
                    remote::RemoteCmd::Ui(crate::browser::Ev::Say { tab, text }) => {
                        surface.says.push((tab, text));
                    }
                    remote::RemoteCmd::Ui(ev @ crate::browser::Ev::VaultSearch { .. })
                    | remote::RemoteCmd::Ui(ev @ crate::browser::Ev::VaultOpen { .. }) => {
                        surface.queue_vault(ev);
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
                            surface.inject(e);
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
            if let Some(jpeg) = surface.take_frames().pop() {
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
        let ui = Ui {
            first_run,
            active,
            board: board_open,
            settings: settings_open,
            auto: engine.as_ref().map(|_| auto_enabled),
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
            browse: browse_view.clone(),
            folder_colors: cfg
                .as_ref()
                .map(|c| c.folder_colors.clone())
                .unwrap_or_default(),
            self_cost: self_cost.clone(),
            qr: if qr_open { remote_ui.as_ref().map(|r| r.url.clone()) } else { None },
            remote_on: remote_ui.is_some(),
            remote_conn: remote_ui.as_ref().is_some_and(|r| r.has_state_clients()),
            remote_sticky: cfg.as_ref().is_some_and(|c| c.remote.sticky_token),
            aim: aim_of(workspaces.get(ws_index), &surfaces, &tabs, active),
            nav,
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
        surface.draw(&tabs, &ui, flash.as_deref())?;
        // The window's size can change. If we don't hand it back over, a placed
        // page stays at its previous size.
        caps.set_area(surface.area);
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
                        let _ = surface.win.focus(None);
                    }
                }
            }
        }
        // Focus follows a click on a pane, the way it follows a click in the
        // tab bar. `active` moves with it so every existing path stays right.
        for id in surface.take_focus_panes() {
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
        if let Some(on) = surface.take_pen() {
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
        if let Some(id) = surface.take_add_tab_pane() {
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
        for child in surface.take_touches() {
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
        for (divider, ratio) in surface.take_pane_ratios() {
            pane_layout.set_divider(divider, ratio);
        }
        // The terminal was zoomed. The page has already redrawn itself; this
        // is only so it opens that size next time. Written on a delay because
        // a wheel sends a notch at a time and a settings file is not a place
        // to write sixty times a second
        if let Some(px) = surface.take_font_size() {
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
        if let Some(px) = surface.take_tab_width() {
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
        for (id, down) in surface.take_pane_splits() {
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
        for (id, keep) in surface.take_restart_panes() {
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
        for id in surface.take_close_panes() {
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
            let geom = &surface.pane_geom;
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
            if settings_open && !covered {
                caps.show_at(&[(SETTINGS_TAB.to_string(), surface.full)]);
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
                        .unwrap_or(surface.area);
                    Some((key.clone(), rect))
                })
                .filter(|_| !covered)
                .collect();
            caps.show_at(&shown);
            }
        }
        // Hand off that a bar button was pressed.
        // Only the main app can receive the window's reports, so it goes through here.
        for child in surface.take_presses() {
            caps.note_press(&child);
            // Convert the name inside the window back to an id. Ones we can't
            // convert belong to a different workspace.
            let Some(name) = caps.name_of_child(&child) else {
                continue;
            };
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
        for (by, row, col) in surface.take_scrolls() {
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
        for (tab, line) in surface.take_says() {
            let now_ms = start.elapsed().as_millis() as u64;
            let to = if tab == 0 { active } else { tab };
            if !hand_line(&mut tabs, &surfaces, to, line, now_ms, &mut pending_send, &mut ball) {
                append_hook_log(&format!("say went nowhere: tab{to} is not a session"));
            }
        }

        // Lua quick-actions tapped in the bar: look up the code (kept server-side)
        // and run it against the active tab. Its commands drain with the hooks'.
        for index in surface.take_run_actions() {
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
        for on in surface.take_record_arms() {
            if let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) {
                let _ = caps.browser_record(key, on);
            } else if !on {
                // "Off" must land even when the browser tab is no longer shown
                // (e.g. the tab switch that caused it) — it names no page.
                let _ = caps.browser_record("", false);
            }
        }

        // ▶ run mode: composer Lua against the shown browser, in the rally's
        // sandbox (browser functions on that one tab, nothing else). The verdict
        // returns as a toast on both surfaces.
        for code in surface.take_run_luas() {
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
            surface.push_lua_done(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"luadone\":{js}}}"));
            }
        }

        // 🩺 environment survey: DRAFT the fixed read-only probe (syntax
        // picked from the tab's launch command / prompt shape) into the
        // composer — the person reviews and sends it themselves, exactly
        // like a ✨ suggestion. Nothing types itself into a terminal. The
        // watcher below waits for the marker-wrapped output to appear
        if surface.take_surveys() > 0 {
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
                    surface.push_surveyed(&js);
                    if let Some(r) = remote_ui.as_ref() {
                        r.push_state(format!("{{\"surveyed\":{js}}}"));
                    }
                }
                _ => {
                    append_hook_log("survey refused: active pane is not a plain terminal");
                    let js = serde_json::json!({"ok": false, "error": i18n::t("msg.suggest.no_tab")})
                        .to_string();
                    surface.push_surveyed(&js);
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
                surface.push_surveyed(js);
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
        for query in surface.take_vault_queries() {
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
        for (folder, name) in surface.take_folder_names() {
            let ws = workspaces.get(ws_index).map(|w| w.name.clone()).unwrap_or_default();
            if let Err(e) = config::rename_group(&ws, std::path::Path::new(&folder), &name) {
                flash = Some(format!("{e:#}"));
            }
        }
        // Thrown away for good. Refused first, while nothing has happened yet,
        // so a folder with work in it is never closed on the way to a no.
        // Then the tabs are ended by taking the folder out of the settings --
        // git will not remove a folder something is still standing in -- and
        // the removal itself waits for them to actually be gone
        for folder in surface.take_folder_discards() {
            let at = std::path::PathBuf::from(&folder);
            if let Err(e) = crate::worktree::ready_to_discard(&at) {
                flash = Some(format!("{e:#}"));
                continue;
            }
            let ws = workspaces.get(ws_index).map(|w| w.name.clone()).unwrap_or_default();
            match config::remove_group(&ws, &at) {
                Ok(()) => pending_discards.push((at, Instant::now(), Instant::now())),
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        // The folders whose tabs are on their way out. Tried again each time
        // round until git can have it, and given up on out loud rather than
        // silently -- a folder that was asked to go and did not is a surprise
        // waiting in the settings
        pending_discards.retain_mut(|(at, since, last)| {
            // Gone is the only thing that counts as done. Git can let go of a
            // folder while Windows still holds the empty shell of it open,
            // and a folder still sitting there is not one that was removed
            if !at.exists() {
                flash = Some(i18n::tp(
                    "msg.folder.discarded",
                    &[("path", &at.display().to_string())],
                ));
                return false;
            }
            // Not every time round: each try asks git how the folder is doing,
            // and a frame is far too often to ask about one that is only
            // waiting for a process to let go of it
            if last.elapsed() < std::time::Duration::from_millis(400) {
                return true;
            }
            *last = Instant::now();
            let trouble = crate::worktree::discard(at).err();
            let waited = since.elapsed() > std::time::Duration::from_secs(10);
            if waited {
                if let Some(e) = trouble {
                    flash = Some(format!("{e:#}"));
                }
            }
            !waited
        });
        for folder in surface.take_folder_closes() {
            let ws = workspaces.get(ws_index).map(|w| w.name.clone()).unwrap_or_default();
            match config::remove_group(&ws, std::path::Path::new(&folder)) {
                // Said out loud, because the folder is still on disk and this
                // is the only sign that it was left there on purpose
                Ok(()) => flash = Some(i18n::tp("msg.folder.closed", &[("path", &folder)])),
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        // Somewhere new to work. Looking hands back what is inside; choosing
        // writes the folder into the settings, and the reload opens it
        for (path, open) in surface.take_browses() {
            if !open {
                browse_view = Some(crate::uistate::BrowseState::of(&path));
                continue;
            }
            let ws = workspaces.get(ws_index).map(|w| w.name.clone()).unwrap_or_default();
            let at = std::path::PathBuf::from(&path);
            match config::append_group(&ws, None, &at, None) {
                Ok(()) => {
                    browse_view = None;
                    flash = Some(i18n::tp("msg.folder.opened", &[("path", &path)]));
                }
                Err(e) => flash = Some(format!("{e:#}")),
            }
        }
        // A colour chosen for a project. Written against the folder git shares
        // between its branches, so all of them change at once
        for (folder, color) in surface.take_folder_colors() {
            let at = std::path::PathBuf::from(&folder);
            if let Some(family) = crate::repo::family_of(&at) {
                if let Err(e) = config::set_folder_color(&family, &color) {
                    flash = Some(format!("{e:#}"));
                }
            }
        }
        // Another branch of a project already open. The same call answers "what
        // would this do" and does it, so the line shown before it happens is
        // the line that happens
        for (from, name, base, make, carry) in surface.take_branches() {
            let from = std::path::PathBuf::from(&from);
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
            let chosen = match base.trim().is_empty() {
                true => repo
                    .as_deref()
                    .map(crate::worktree::default_base)
                    .unwrap_or_default(),
                false => base.clone(),
            };
            // Nothing typed yet: propose one, so the dialog opens with a
            // complete answer and pressing the button is enough
            let wanted = match name.trim().is_empty() {
                true => repo.as_deref().map(crate::worktree::suggest).unwrap_or_default(),
                false => name.clone(),
            };
            branch_view = Some(match crate::worktree::plan(&from, &wanted, Some(&base)) {
                Err(e) => crate::uistate::BranchPlan {
                    from: from.display().to_string(),
                    branch: name.clone(),
                    asked: name,
                    base: chosen,
                    bases,
                    carry: carryable,
                    error: Some(format!("{e:#}")),
                    ..Default::default()
                },
                Ok(plan) => {
                    let mut view = crate::uistate::BranchPlan {
                        from: from.display().to_string(),
                        branch: plan.branch.clone(),
                        asked: name.clone(),
                        folder: plan.folder.display().to_string(),
                        line: plan.line(),
                        base: plan.base.clone(),
                        bases,
                        carry: carryable,
                        error: None,
                        done: false,
                    };
                    if make {
                        // Made first, written down second: settings naming a
                        // folder that does not exist would launch tabs into
                        // nowhere on the next reload
                        let ws = workspaces
                            .get(ws_index)
                            .map(|w| w.name.clone())
                            .unwrap_or_default();
                        let wrote = crate::worktree::create(&plan).and_then(|()| {
                            config::append_group(
                                &ws,
                                Some(&plan.main),
                                &plan.folder,
                                Some(&plan.branch),
                            )
                        });
                        match wrote {
                            Ok(()) => {
                                view.done = true;
                                // What could not be brought along is said out
                                // loud: the folder is made either way, and the
                                // first build is what would otherwise fail
                                let missed = crate::worktree::carry_into(&plan, &carry);
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
                    view
                }
            });
        }
        for ev in surface.take_vault_opens() {
            if let crate::browser::Ev::VaultOpen { program, id, cwd, title } = ev {
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

        for want in surface.take_suggests() {
            let target = session_at(&surfaces, active).and_then(|i| tabs.get(i));
            let Some(t) = target else {
                surface.push_suggested(
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
            surface.push_suggested(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"suggested\":{js}}}"));
            }
        }

        // Recorded steps → one Lua line each, appended to the composer on both
        // surfaces. Each line calls the same primitives the automation uses,
        // addressed by the browser's Lua name, so record → paste → run round-trips.
        for step in surface.take_recorded() {
            let Some(name) = caps.name_of_child(&step.child) else {
                continue;
            };
            let Some(line) = recorded_lua(&name, &step) else {
                continue;
            };
            let js = serde_json::to_string(&line).unwrap_or_default();
            surface.push_recorded(&js);
            if let Some(r) = remote_ui.as_ref() {
                r.push_state(format!("{{\"recorded\":{js}}}"));
            }
        }

        // Composer text/keys typed while viewing a browser tab go straight into that
        // browser — the same caps.browser_inject the phone's relay drives, so the two
        // share one injection path rather than each growing its own.
        let injects = surface.take_injects();
        if !injects.is_empty() {
            if let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) {
                for input in injects {
                    let _ = caps.browser_inject(key, input);
                }
            }
        }

        // The 🎯 panel's replay button: put the newest run's durable script
        // where the user can grab it (the board itself can't download files)
        if std::mem::take(&mut surface.replay_saves) {
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
        for (target, goal) in surface.take_operates() {
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
        if surface.take_close_settings() {
            let _ = caps.browser_close(SETTINGS_TAB);
            settings_open = false;
        }

        // The sidebar gear. Opens settings from any tab (the menu "e" key only
        // fires while INDEX is in view, so the gear needs its own path).
        // The workspace being viewed rides along so its group opens expanded.
        if let Some((section, ret)) = surface.take_open_settings() {
            // The gear passes the workspace being viewed; a deep-link shortcut may
            // also name a section to land on and ask to return once saved.
            let mut query = format!("&ws={ws_index}");
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
        if surface.take_remote_cut() && remote_ui.is_some() {
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
        for go in surface.take_gos() {
            let Some(Surface::Browser { key, .. }) = surfaces.get(active.wrapping_sub(1)) else {
                continue;
            };
            // Reject operations that aren't shown. It would be strange for
            // something not on screen to still work.
            let Some(spec) = caps.nav_of(key) else {
                continue;
            };
            use crate::browser::Go;
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
                Go::To(raw) => match crate::browser::openable(&raw) {
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
        for (child, url, can_back, can_forward) in surface.take_wheres() {
            if let Some(name) = caps.name_of_child(&child) {
                where_now = Some((name, url, can_back, can_forward));
            }
        }
        // Load start/end likewise gets converted to the id before caching.
        // Update the start time, used for the top bar's "in progress" indicator.
        for (child, busy) in surface.take_loading() {
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
        for (child, url, complete) in surface.take_loads() {
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

        let polled = surface.poll(
            Duration::from_millis(16),
            session_at(&surfaces, active).and_then(|i| tabs.get(i)),
        )?;
        // Once the window is gone, fall through to the same place as Ctrl+B q.
        // We want cleanup to live in exactly one place.
        if surface.closed {
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
                        KeyCode::Char('q') => break,
                        // Open the command palette from any tab. It is drawn by
                        // the page, so this only nudges it open
                        KeyCode::Char(':') => surface.open_palette(),
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
                            let query = format!(
                                "&addtab={ws_index}&nonce={}",
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
                            flash =
                                Some(i18n::t("msg.emergency_stop"));
                        }
                        // Ctrl+B c copies the latest captured response to the clipboard
                        KeyCode::Char('c') => {
                            if let Some(t) = session_mut(&mut tabs, &surfaces, active) {
                                flash = Some(match &t.last_response {
                                    Some(r) if !r.trim().is_empty() => copy_to_clipboard(r),
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
                        KeyCode::Char('s') => surface.toggle_tab_bar(),
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
                            let rows = pty_dims(surface.size()?).0;
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
                            if remote_ui.is_some() {
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
                            flash = Some(manage_master_password(
                                &mut surface,
                                cfg.as_ref(),
                                &mut password,
                            )?);
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
                        KeyCode::Char('f') => surface.open_vault(),
                        KeyCode::Char('p') => surface.open_palette(),
                        KeyCode::Char('q') => break,
                        _ => {}
                    }
                    // INDEX-END (a test checks whether keys the board offers are received here)
                } else {
                    let size = surface.size()?;
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

/// Keeps a child process from popping up a window.
///
/// Console apps like cmd.exe show a black window if launched quietly.
/// That would flash briefly every time a browser is opened, so it's suppressed from the start.
pub fn detach_console(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

/// Minimum gap between sending the text body and sending submit (Enter).
/// How long the recipient actually takes to process it depends on device and load,
/// so this is only a floor.
const SUBMIT_FLOOR_MS: u64 = 100;
/// The no-output duration after which paste intake is considered finished.
///
/// This waits for "finished", not "started responding". A long paste keeps
/// redrawing over several round trips, so sending Enter as soon as it starts
/// would arrive mid-intake and get dropped.
/// (measured: around 600 chars goes through fine, around 1900 chars fails)
const SUBMIT_QUIET_MS: u64 = 400;
/// The cap on how long to wait before sending submit anyway, when the recipient
/// keeps responding and never settles
const SUBMIT_GIVE_UP_MS: u64 = 8_000;

/// How much of a paste to hand over at a time.
///
/// The recipient takes a paste in one character at a time — on Windows the
/// console turns the stream into individual key events — and it is far slower at
/// that than we are at writing. Give it the whole thing in one go and it spends
/// seconds working through a backlog nothing on our side can see.
const PASTE_CHUNK: usize = 1024;
/// How long to give the recipient to draw something before handing over the next
/// chunk anyway. Drawing is the only way it has of saying "I've caught up";
/// the timeout is for a recipient that draws nothing at all.
const PASTE_ACK_MS: u64 = 600;

/// What to do with a send on this pass.
enum Step {
    /// Nothing yet: the recipient hasn't caught up, or hasn't settled
    Wait,
    /// Hand over this much more of the paste
    Hand(Vec<u8>),
    /// The body is all in. Press Enter (or, for a draft, stop here)
    Submit { settled: bool },
}

/// A paste on its way to a tab, and the Enter that finishes it.
///
/// Both halves are here because they are one problem. The recipient reads the
/// paste one character at a time and falls behind; the Enter we write next joins
/// the same queue, so it is taken *inside* the paste, where it counts as a
/// newline and not as "send this" — the text sits in the input box, unsent,
/// until the next thing typed carries it in. Waiting longer before pressing
/// Enter cannot fix that: the wait is on our clock, and the queue is on theirs.
///
/// So the paste is handed over a chunk at a time, and the next chunk only goes
/// out once the recipient has drawn something (= caught up). Nothing here is a
/// guess about how fast the recipient is; it sets its own pace, and by the time
/// the last chunk is out it is at most one chunk behind.
///
/// Measured against a real Codex CLI on Windows: 20,000 characters written in
/// one go left the whole thing in the input box (it drew *nothing at all* for
/// two seconds mid-intake, so "output has stopped" looked exactly like
/// "finished"). Handed over in chunks, the same text sends.
struct PendingSend {
    tab: usize,
    /// The paste, already encoded for the recipient, split at character
    /// boundaries. Split before encoding so no character is ever cut in half.
    chunks: Vec<Vec<u8>>,
    /// How many chunks have gone out
    handed: usize,
    /// Whether to press Enter at the end. A draft is placed for a person to
    /// finish, so it stops with the text in the box.
    submit: bool,
    /// The cumulative output amount last seen. A change means the recipient drew.
    seen: u64,
    /// When the last chunk was handed over
    handed_ms: u64,
    /// The point output stopped (None = hasn't stopped yet)
    quiet_since: Option<u64>,
    /// The earliest time submission is allowed, to prevent sending too early
    not_before: u64,
    /// The time to give up and send anyway, if things never settle
    give_up: u64,
}

impl PendingSend {
    fn new(tab: usize, chunks: Vec<Vec<u8>>, submit: bool, seen: u64, now_ms: u64) -> Self {
        Self {
            tab,
            chunks,
            handed: 0,
            submit,
            seen,
            handed_ms: now_ms,
            quiet_since: None,
            not_before: now_ms + SUBMIT_FLOOR_MS,
            give_up: now_ms + SUBMIT_GIVE_UP_MS,
        }
    }

    /// Everything still owed, handed over at once.
    ///
    /// For when something else is about to write to the same tab. A paste that
    /// goes over in pieces owns that tab until the last piece is in: a
    /// keystroke, or another message, arriving in the gaps is typed into the
    /// middle of somebody's sentence. The pacing is what gets given up here,
    /// never the order.
    fn rest(&mut self, now_ms: u64) -> Vec<u8> {
        if self.handed >= self.chunks.len() {
            return Vec::new();
        }
        let out = self.chunks[self.handed..].concat();
        self.handed = self.chunks.len();
        self.quiet_since = None;
        self.not_before = now_ms + SUBMIT_FLOOR_MS;
        self.give_up = now_ms + SUBMIT_GIVE_UP_MS;
        out
    }

    /// The next thing to do for this send.
    ///
    /// While the body is going out, what we wait on is the recipient drawing.
    /// Once it is all out, what we wait on is the drawing *stopping* — the same
    /// "it has taken it in" signal as before, which is now trustworthy because
    /// the recipient was never allowed to fall behind.
    fn step(&mut self, output_count: u64, now_ms: u64) -> Step {
        if self.handed < self.chunks.len() {
            let drew = output_count != self.seen;
            if !drew && now_ms.saturating_sub(self.handed_ms) < PASTE_ACK_MS && self.handed > 0 {
                return Step::Wait;
            }
            let chunk = self.chunks[self.handed].clone();
            self.handed += 1;
            self.seen = output_count;
            self.handed_ms = now_ms;
            // The clock for "has it settled?" starts when the last chunk is out
            if self.handed == self.chunks.len() {
                self.quiet_since = None;
                self.not_before = now_ms + SUBMIT_FLOOR_MS;
                self.give_up = now_ms + SUBMIT_GIVE_UP_MS;
            }
            return Step::Hand(chunk);
        }
        if output_count != self.seen {
            // Still mid-intake. Restart the measurement from when it stops.
            self.seen = output_count;
            self.quiet_since = None;
        } else if self.quiet_since.is_none() {
            self.quiet_since = Some(now_ms);
        }
        if now_ms < self.not_before {
            return Step::Wait;
        }
        let settled = self
            .quiet_since
            .is_some_and(|q| now_ms.saturating_sub(q) >= SUBMIT_QUIET_MS);
        if settled || now_ms >= self.give_up {
            Step::Submit { settled }
        } else {
            Step::Wait
        }
    }
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
fn reload_providers<'a>(
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
fn finish_paste(pending: &mut [PendingSend], t: &Tab, tab: usize, now_ms: u64) {
    for p in pending.iter_mut().filter(|p| p.tab == tab) {
        let rest = p.rest(now_ms);
        if !rest.is_empty() {
            let _ = t.write_passthrough(&rest);
        }
    }
}

/// The paste to hand a tab, cut into chunks small enough for it to swallow one
/// at a time. Cut before encoding, so a character never straddles two writes.
///
/// One place builds this, for every door that pastes into a tab: a person's
/// line, an automated hand-off, and a draft left for someone to finish.
fn paste_chunks(t: &Tab, text: &str) -> Vec<Vec<u8>> {
    let bracketed = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste();
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    // If bracketed paste is supported, multi-line text still arrives as a single input
    let payload = if bracketed {
        format!("\x1b[200~{body}\x1b[201~")
    } else {
        body
    };
    let mut chunks = Vec::new();
    let mut rest = payload.as_str();
    while !rest.is_empty() {
        let mut cut = PASTE_CHUNK.min(rest.len());
        while !rest.is_char_boundary(cut) {
            cut += 1;
        }
        let (head, tail) = rest.split_at(cut);
        chunks.push(t.encode_out(head));
        rest = tail;
    }
    chunks
}

/// The name used when placing the settings page inside the window.
/// If the spelling drifts, it gets treated as a different browser and a second copy opens.
const SETTINGS_TAB: &str = "settings";

/// The name used when placing the result view (finished discussion / review /
/// rally, rendered as a chat) inside the window. Unlike settings it *does* show
/// in the tab strip, and it is reused (re-pointed) on each new result rather
/// than piling up copies.
const RESULT_TAB: &str = "result";

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
fn restartable_page(surfaces: &[Surface], active: usize, caps: &hooks::Caps) -> Option<String> {
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
fn placed_active(surfaces: &[Surface], key_want: &str) -> usize {
    surfaces
        .iter()
        .position(|p| matches!(p, Surface::Browser { key, .. } if key == key_want))
        .map(|i| i + 1)
        .unwrap_or(surfaces.len() + 1)
}

fn settings_active(surfaces: &[Surface]) -> usize {
    placed_active(surfaces, SETTINGS_TAB)
}

/// Writes out the signal for one wheel tick, in terminal convention.
///
/// A full-screen program rewinds its own contents itself, so any history we
/// hold means nothing to it. Reporting the scroll itself is the correct thing
/// to do. Button numbers are fixed by convention: 64 is up, 65 is down.
fn wheel_bytes(up: bool, row: u16, col: u16, enc: vt100::MouseProtocolEncoding) -> Vec<u8> {
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
fn scrolled_to(cur: usize, by: i32) -> usize {
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
fn scroll_by(t: &Tab, by: i32, row: u16, col: u16) {
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
fn to_live(t: &Tab) {
    let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
    if p.screen().scrollback() != 0 {
        p.screen_mut().set_scrollback(0);
    }
}

/// The interval for asking the window "where are you right now".
///
/// The pressable/unpressable appearance lags by this much. It's not worth
/// asking every frame, but it shouldn't lag enough for a human to notice either.
const WHERE_EVERY_MS: u64 = 400;

fn open_browser(url: &str) {
    // cmd's `start` splits on `&` inside the URL, so pass it after an empty title argument
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/c", "start", "", url])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = detach_console(&mut cmd).spawn();
}

/// Sets up a place to talk in text.
///
/// This executable is a windowed app, so Windows doesn't attach a console for
/// it. If the caller is a terminal, borrow that one. Otherwise, open one of
/// our own. If already attached, do nothing (both calls simply fail harmlessly in that case).
fn open_console() {
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole,
    };
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            AllocConsole();
        }
    }
}

/// Loads Lua hooks across 3 tiers (base > workspace > tab).
/// Hook resolution favors "the more specific one wins", so only hooks a tab's
/// script doesn't define fall back to workspace, then base.
/// Lines up per-tab automation by screen number.
///
/// The number matches whatever's shown on screen. If the number a human
/// presses, the number a script points at, and the number the ball flies to
/// were all different, nobody could keep track of any of it.
///
/// Reordering still works, because it's reassigned every time config is reloaded.
/// Nothing is remembered anywhere, so it can never drift out of sync.
/// The automation assignment for a tab: a file or directory of Lua.
///
/// Being aimed at another tab (ð¯) is NOT one of these. An aim is picked on
/// screen and attached when there is a goal, so a tab keeps the automation it
/// was given either way -- the two used to fight, and the aim won silently.
#[derive(Debug, PartialEq)]
enum TabAuto {
    /// An automation path (a directory or .lua file)
    Path(String),
}

/// Returns the screen number (1-based) of the tab in a workspace whose id
/// (or name, if no id) matches. Used to resolve discussion participants/referee
/// from a tab id to a screen number.
fn surface_of_id(ws: &config::Workspace, id: &str) -> Option<usize> {
    let mut pane = 0;
    for t in &ws.tabs {
        if t.cfg.command.argv().is_empty() {
            continue;
        }
        pane += 1;
        if t.cfg.id.as_deref() == Some(id) || t.cfg.name.as_deref() == Some(id) {
            return Some(pane);
        }
    }
    None
}

fn automation_by_pane(ws: &config::Workspace) -> Vec<(usize, TabAuto)> {
    let mut pane = 0;
    let mut out = Vec::new();
    for t in &ws.tabs {
        // A row with an empty command doesn't show up on screen either
        if t.cfg.command.argv().is_empty() {
            continue;
        }
        pane += 1;
        if let Some(p) = t.cfg.automation_path() {
            out.push((pane, TabAuto::Path(p)));
        }
    }
    out
}

/// The page placed in the focused pane, if that is what is there.
///
/// Two things need this and must agree: the pen (which that page draws for
/// itself) and a message raised while it is in front (same reason -- a window
/// of its own cannot be drawn over).
fn focused_page(layout: &crate::layout::Layout, surfaces: &[Surface]) -> Option<String> {
    match surfaces.get(layout.surface_of(layout.focus())?.checked_sub(1)?)? {
        Surface::Browser { key, .. } => Some(key.clone()),
        Surface::Session(_) => None,
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
fn remember_aim(
    ws: Option<&mut config::Workspace>,
    operator: Option<&str>,
    aim: Option<&str>,
) -> bool {
    let Some(name) = operator else { return false };
    if let Some(ws) = ws {
        for t in ws.tabs.iter_mut() {
            let named = t.cfg.id.as_deref() == Some(name) || t.cfg.name.as_deref() == Some(name);
            if named {
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
fn aim_of(
    ws: Option<&config::Workspace>,
    surfaces: &[Surface],
    tabs: &[Tab],
    surface: usize,
) -> Option<usize> {
    let t = session_at(surfaces, surface).and_then(|i| tabs.get(i))?;
    let me = t.id.clone().unwrap_or_else(|| t.title.clone());
    let named = |c: &config::TabConfig| {
        c.id.as_deref() == Some(me.as_str()) || c.name.as_deref() == Some(me.as_str())
    };
    let aim = ws?
        .tabs
        .iter()
        .find(|x| named(&x.cfg))?
        .cfg
        .drives
        .clone()
        .filter(|d| !d.trim().is_empty())?;
    hooks::TabRef::Name(aim).resolve(&surface_keys(surfaces, tabs))
}

fn build_engine(
    cfg: Option<&config::Config>,
    ws: Option<&config::Workspace>,
    errors: &mut Vec<String>,
    caps: &hooks::Caps,
) -> Option<HookEngine> {
    let base = cfg.and_then(|c| c.automation_path());
    let ws_lua = ws.and_then(|w| w.automation.clone());
    let tab_luas: Vec<(usize, TabAuto)> = ws.map(automation_by_pane).unwrap_or_default();
    let has_discuss = ws
        .and_then(|w| w.discuss.as_ref())
        .is_some_and(|d| d.agents.len() >= 2);
    // Keep the engine even with no Lua hooks when a tab wants a completion
    // notification: the on_done detection loop only runs when an engine exists,
    // so without this a notify-only workspace would never fire on_done.
    let wants_notify = ws
        .map(|w| w.tabs.iter().any(|t| t.cfg.notify_on_done.is_some()))
        .unwrap_or(false);
    // A Lua quick-action needs an engine to run in, even when nothing else does.
    let has_lua_actions = cfg
        .map(|c| c.actions.iter().any(|a| a.lua))
        .unwrap_or(false);
    if base.is_none()
        && ws_lua.is_none()
        && tab_luas.is_empty()
        && !has_discuss
        && !wants_notify
        && !has_lua_actions
    {
        return None;
    }

    let mut engine = match HookEngine::with_caps(hooks::Caps::clone(caps)) {
        Ok(e) => e,
        Err(e) => {
            errors.push(format!("Lua: {e:#}"));
            return None;
        }
    };
    let load = |engine: &mut HookEngine, path: &str, errors: &mut Vec<String>| -> Option<usize> {
        match engine.load_path(&resolve_data_path(path)) {
            Ok(id) => Some(id),
            Err(e) => {
                errors.push(format!("Lua({path}): {e:#}"));
                None
            }
        }
    };
    if let Some(p) = &base {
        if let Some(id) = load(&mut engine, p, errors) {
            engine.set_base(id);
        }
    }
    if let Some(p) = &ws_lua {
        if let Some(id) = load(&mut engine, p, errors) {
            engine.set_workspace(id);
        }
    }
    // The referee (stop conditions) is per-workspace. Passed to the built-in commander as a Lua table.
    let stops_lua = ws
        .map(|w| config::stops_to_lua(&w.stops))
        .unwrap_or_else(|| "{}".to_string());
    for (idx, auto) in &tab_luas {
        let id = match auto {
            TabAuto::Path(p) => load(&mut engine, p, errors),
        };
        if let Some(id) = id {
            engine.set_tab(*idx, id);
        }
    }
    // AI-vs-AI discussion: if the workspace has `discuss`, load the built-in discussion commander into each participant tab
    if let Some(w) = ws {
        if let Some(d) = &w.discuss {
            let agents: Vec<String> = d
                .agents
                .iter()
                .filter(|s| !s.trim().is_empty())
                .cloned()
                .collect();
            let n = agents.len();
            if n >= 2 {
                let max_turns = (d.max_rounds.max(1) as usize) * n;
                // Turn the participant list into a Lua list literal to pass to the commander (used by group stops)
                let agents_lua = format!(
                    "{{{}}}",
                    agents
                        .iter()
                        .map(|a| format!("{a:?}"))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                // id -> display name, so the discussion (statements, transcript,
                // hand-offs) refers to participants by their display name while
                // routing still uses the stable id.
                let names_lua = {
                    let mut s = String::from("{");
                    for t in &w.tabs {
                        if t.cfg.command.argv().is_empty() {
                            continue;
                        }
                        let key = t
                            .cfg
                            .id
                            .as_deref()
                            .filter(|x| !x.is_empty())
                            .or_else(|| t.cfg.name.as_deref().filter(|x| !x.is_empty()));
                        let Some(key) = key else { continue };
                        let disp = t
                            .cfg
                            .name
                            .as_deref()
                            .filter(|x| !x.is_empty())
                            .unwrap_or(key);
                        s.push_str(&format!("[{key:?}]={disp:?},"));
                    }
                    s.push('}');
                    s
                };
                let moderator = d.moderator.as_deref().filter(|s| !s.trim().is_empty());
                for (i, id) in agents.iter().enumerate() {
                    let Some(pane) = surface_of_id(w, id) else {
                        errors.push(crate::i18n::tp(
                            "err.ws.discuss_tab_missing",
                            &[("id", id)],
                        ));
                        continue;
                    };
                    let next = &agents[(i + 1) % n];
                    let persona = d.personas.get(id).map(String::as_str).unwrap_or("");
                    match engine.load_discuss_agent(
                        id,
                        next,
                        i == 0,
                        false,
                        d.judge.as_deref(),
                        max_turns,
                        &agents_lua,
                        &names_lua,
                        &stops_lua,
                        &d.verdict,
                        &d.order,
                        moderator,
                        false,
                        persona,
                    ) {
                        Ok(sid) => engine.set_tab(pane, sid),
                        Err(e) => errors.push(crate::i18n::tp(
                            "err.ws.discuss_agent_failed",
                            &[("id", id), ("e", &format!("{e:#}"))],
                        )),
                    }
                }
                if let Some(j) = d.judge.as_deref().filter(|s| !s.trim().is_empty()) {
                    let persona = d.personas.get(j).map(String::as_str).unwrap_or("");
                    match surface_of_id(w, j) {
                        Some(pane) => match engine.load_discuss_agent(
                            j, j, false, true, None, max_turns, &agents_lua, &names_lua,
                            &stops_lua,
                            &d.verdict, &d.order, moderator, false, persona,
                        ) {
                            Ok(sid) => engine.set_tab(pane, sid),
                            Err(e) => errors.push(crate::i18n::tp(
                                "err.ws.discuss_judge_failed",
                                &[("j", j), ("e", &format!("{e:#}"))],
                            )),
                        },
                        None => errors.push(crate::i18n::tp(
                            "err.ws.discuss_judge_missing",
                            &[("j", j)],
                        )),
                    }
                }
                // The moderator tab: nominates the next speaker when order="moderated"
                if let Some(m) = moderator {
                    let persona = d.personas.get(m).map(String::as_str).unwrap_or("");
                    match surface_of_id(w, m) {
                        Some(pane) => match engine.load_discuss_agent(
                            m, m, false, false, d.judge.as_deref(), max_turns, &agents_lua,
                            &names_lua, &stops_lua, &d.verdict, &d.order, moderator, true, persona,
                        ) {
                            Ok(sid) => engine.set_tab(pane, sid),
                            Err(e) => errors.push(crate::i18n::tp(
                                "err.ws.discuss_moderator_failed",
                                &[("m", m), ("e", &format!("{e:#}"))],
                            )),
                        },
                        None => errors.push(crate::i18n::tp(
                            "err.ws.discuss_moderator_missing",
                            &[("m", m)],
                        )),
                    }
                }
            } else if !d.agents.is_empty() {
                errors.push(crate::i18n::t("err.ws.discuss_needs_two"));
            }
        }
    }
    // Keep the engine even with no Lua hooks when a tab wants a completion
    // notification (the on_done detection loop lives behind `Some(engine)`), or
    // when there are Lua quick-actions to run in it.
    (!engine.is_empty() || wants_notify || has_lua_actions).then_some(engine)
}

/// Converts a rebuilt tab config into TabOptions.
/// For a `model <provider>/<model>` tab, loads the resolved connection info into opts.
/// A discussion participant also gets its persona attached (so the stateless
/// bridge doesn't forget its stance). argv is left as-is for identification
/// (the spawn side swaps it for the waiting process). A regular tab passes through untouched.
fn resolve_launch(
    argv: Vec<String>,
    opts: &mut tab::TabOptions,
    ws: Option<&config::Workspace>,
    id: Option<&str>,
) -> Vec<String> {
    if let Some(mut conn) = bridge::launch_for(&argv) {
        if let (Some(d), Some(id)) = (ws.and_then(|w| w.discuss.as_ref()), id) {
            conn.persona = d.personas.get(id).filter(|p| !p.trim().is_empty()).cloned();
        }
        // Whether this model is a browser brain is not decided here. It is
        // decided by what it is aimed at, which can change while it runs
        // (see Tab::set_brain)
        opts.model = Some(conn);
    }
    argv
}

/// Where it runs comes from the tab's group, the only thing that has a folder.
fn tab_options(cfg: &config::TabConfig, group: Option<&config::Group>) -> tab::TabOptions {
    tab::TabOptions {
        cwd: group.and_then(|g| g.cwd.clone()),
        group: group.and_then(|g| g.name.clone()),
        scrollback: cfg.scrollback.unwrap_or(tab::SCROLLBACK_LINES),
        encoding: tab::TabOptions::encoding_from_name(cfg.encoding.as_deref()),
        log: cfg.log,
        model: None,
    }
}

/// Applies a config change to the running set of tabs.
/// Whatever can take effect immediately does; whatever needs the session
/// rebuilt is deferred and flagged instead (so a running AI doesn't get cut
/// off without asking). The return value is the message reported to the user.
fn apply_ws_config(
    tabs: &mut Vec<Tab>,
    ws: &config::Workspace,
    rows: u16,
    cols: u16,
    errors: &mut Vec<String>,
) -> String {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut staged = 0usize;

    // Close tabs no longer in config (removed via the GUI = an explicit instruction)
    let wanted: Vec<String> = ws
        .tabs
        .iter()
        .map(|f| {
            f.cfg
                .name
                .clone()
                .unwrap_or_else(|| title_of(&f.cfg.command.argv()))
        })
        .collect();
    tabs.retain_mut(|t| {
        if wanted.contains(&t.title) {
            true
        } else {
            t.kill();
            removed += 1;
            false
        }
    });

    // Update existing tabs and add new ones
    let mut ordered: Vec<Tab> = Vec::with_capacity(ws.tabs.len());
    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        if argv.is_empty() {
            continue;
        }
        // Browsers aren't child processes, so don't launch them here
        // (open_declared_browsers opens the window)
        if config::browser_url_of(&argv).is_some() {
            continue;
        }
        let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
        let mut opts = tab_options(&ft.cfg, ws.group_of(ft));
        let argv = resolve_launch(
            argv,
            &mut opts,
            Some(ws),
            ft.cfg.id.as_deref(),
        );
        let cwd = opts.cwd.clone();
        match tabs.iter().position(|t| t.title == title) {
            Some(i) => {
                let mut t = tabs.remove(i);
                t.apply_live_config(
                    ft.cfg.profile.clone(),
                    ft.cfg.locked,
                    ft.cfg.auto_restart,
                    ft.depth,
                    ft.cfg.notify_on_done.clone(),
                );
                // Changes to command, encoding, or line count require a rebuild
                if t.signature() != tab::signature_of(&argv, &opts) {
                    t.stage_restart_config(argv.clone(), opts);
                    staged += 1;
                }
                ordered.push(t);
            }
            None => match Tab::spawn_as(
                title.clone(),
                &argv,
                ft.cfg.profile.clone(),
                rows,
                cols,
                opts,
                resume_plan_of(ft.cfg.resume.as_deref()),
            ) {
                Ok(mut t) => {
                    t.locked = ft.cfg.locked;
                    t.auto_restart = ft.cfg.auto_restart;
                    t.depth = ft.depth;
                    t.id = ft.cfg.id.clone();
                    t.notify_on_done = ft.cfg.notify_on_done.clone();
                    ordered.push(t);
                    added += 1;
                }
                Err(e) => errors.push(tab::launch_problem(
                    &title,
                    argv.first().map(String::as_str).unwrap_or(""),
                    cwd.as_deref(),
                    &e.to_string(),
                )),
            },
        }
    }
    // Close whatever's left that isn't in config
    for mut t in tabs.drain(..) {
        t.kill();
        removed += 1;
    }
    *tabs = ordered;

    let mut parts = vec![i18n::t("msg.config_reloaded")];
    if added > 0 {
        parts.push(i18n::tp("msg.config_added", &[("n", &added.to_string())]));
    }
    if removed > 0 {
        parts.push(i18n::tp("msg.config_removed", &[("n", &removed.to_string())]));
    }
    if staged > 0 {
        parts.push(i18n::tp("msg.config_needs_restart", &[("n", &staged.to_string())]));
    }
    parts.join(" / ")
}

/// Launches the tabs for a workspace (called on first activation)
/// Opens the browsers declared in config.
///
/// If one fails to open, the rest still run. A browser failing to launch is
/// never a reason to stop the whole workspace.
fn open_declared_browsers(ws: &config::Workspace, caps: &hooks::Caps, errors: &mut Vec<String>) {
    // Don't touch ones already open. Reopening them would restart the page from
    // scratch, wiping out whatever the user was looking at every time settings are saved.
    let open_now = caps.hosted_names();
    let already = |name: &str| open_now.iter().any(|n| n == name);
    for b in &ws.browsers {
        if already(&b.id) {
            caps.note_declared(&b.id);
            continue;
        }
        let profile = browser::BrowserProfile::new(
            b.browser_profile.as_deref().unwrap_or_default(),
            b.private,
        )
        .calling_itself(b.user_agent.clone());
        match caps.browser_open(&b.id, &b.url, profile) {
            Ok(()) => caps.note_declared(&b.id),
            Err(e) => errors.push(crate::i18n::tp(
                "err.ws.browser_open",
                &[("id", &b.id), ("e", &format!("{e:#}"))],
            )),
        }
    }
    // A tab written as "browser https://..." gets the same treatment.
    // The name automation addresses it by is that tab's ID (or display name if no ID)
    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        let Some(url) = config::browser_url_of(&argv) else {
            continue;
        };
        let name = ft
            .cfg
            .id
            .clone()
            .or_else(|| ft.cfg.name.clone())
            .unwrap_or_else(|| "browser".into());
        if !already(&name) {
            let profile = browser::BrowserProfile::new(
                ft.cfg.browser_profile.as_deref().unwrap_or_default(),
                ft.cfg.private,
            )
            .calling_itself(ft.cfg.user_agent.clone());
            if let Err(e) = caps.browser_open(&name, &url, profile) {
                errors.push(crate::i18n::tp(
                    "err.ws.browser_open",
                    &[("id", &name), ("e", &format!("{e:#}"))],
                ));
                continue;
            }
        }
        caps.note_declared(&name);
    }
    apply_browser_chrome(ws, caps);
}

/// Redraws a page's top bar and bottom band to match config.
///
/// Runs not just on open, but also whenever config is reloaded.
/// Without going through here, checking a box wouldn't show up until a restart.
fn apply_browser_chrome(ws: &config::Workspace, caps: &hooks::Caps) {
    // Close browsers that dropped out of config. Leaving them in place would
    // make them reappear at the back of the list as a "page not in config".
    let declared: Vec<String> = ws
        .browsers
        .iter()
        .map(|b| b.id.clone())
        .chain(ws.tabs.iter().filter_map(|ft| {
            let argv = ft.cfg.command.argv();
            config::browser_url_of(&argv)?;
            Some(
                ft.cfg
                    .id
                    .clone()
                    .or_else(|| ft.cfg.name.clone())
                    .unwrap_or_else(|| "browser".into()),
            )
        }))
        .collect();
    for gone in caps.keep_only_declared(&declared) {
        append_hook_log(&format!("Closed because it dropped out of config: {gone}"));
    }

    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        if config::browser_url_of(&argv).is_none() {
            continue;
        }
        let name = ft
            .cfg
            .id
            .clone()
            .or_else(|| ft.cfg.name.clone())
            .unwrap_or_else(|| "browser".into());
        // Clear it if it was removed. Leaving it around even after reverting
        // config would make it un-fixable. This also brings anything Lua set
        // into line with whatever config specifies at save time.
        match ft.cfg.nav {
            Some(nav) => {
                let _ = caps.browser_nav(&name, nav);
            }
            None => {
                let _ = caps.browser_unnav(&name);
            }
        }
        match &ft.cfg.ask {
            Some(ask) => {
                let label = if ask.label.trim().is_empty() {
                    i18n::t("tui.ask.label")
                } else {
                    ask.label.clone()
                };
                let _ = caps.browser_ask(&name, &ask.text, &label);
            }
            None => {
                let _ = caps.browser_unask(&name);
            }
        }
    }
}

/// The conversation a tab should be launched back into, if there is one.
///
/// Three things all have to hold, and every one of them failing is ordinary
/// rather than exceptional: this tab may have been told to start clean, it may
/// be new since last time, and the conversation may have been deleted since. So
/// there is no message here — a tab that starts fresh is what a tab normally
/// does, and saying so on every launch would be noise.
///
/// The tab is recognised by the same four things `lastsession` writes down, in
/// the same spelling: the program is `argv[0]` and the folder is the resolved
/// `cwd`, exactly as a running tab would report them.
fn carried_conversation(
    carry: Option<&crate::lastsession::Saved>,
    ws: &config::Workspace,
    argv: &[String],
    cfg: &config::TabConfig,
    cwd: &Option<std::path::PathBuf>,
    title: &str,
) -> tab::Resume {
    if cfg.restore_conversation == Some(false) {
        return tab::Resume::Fresh;
    }
    let Some(saved) = carry else {
        return tab::Resume::Fresh;
    };
    let cwd = cwd.as_ref().map(|c| c.display().to_string());
    let Some(session) = saved.conversation_of(
        &ws.name,
        argv.first().map(String::as_str).unwrap_or_default(),
        cwd.as_deref(),
        cfg.id.as_deref(),
        title,
    ) else {
        return tab::Resume::Fresh;
    };
    match tab::resumable(argv, &cfg.profile, &session.id) {
        true => tab::Resume::Id(session),
        false => tab::Resume::Fresh,
    }
}

/// Launch every tab a workspace declares.
///
/// `carry` is what was on screen when the app last closed; whether a given tab
/// actually comes back to it is that tab's own setting. A tab the Vault
/// reopened names its own conversation and outranks both: that id was chosen
/// deliberately, a moment ago, and "what this tab was saying last time" is not
/// an answer to it
fn spawn_workspace(
    ws: &config::Workspace,
    rows: u16,
    cols: u16,
    tabs: &mut Vec<Tab>,
    errors: &mut Vec<String>,
    carry: Option<&crate::lastsession::Saved>,
) {
    // Warn when ids collide, since automation can't tell where to send in that case
    let dups = config::duplicate_keys(ws);
    if !dups.is_empty() {
        errors.push(crate::i18n::tp(
            "err.ws.duplicate_names",
            &[("names", &dups.join(", "))],
        ));
    }
    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        if argv.is_empty() {
            continue;
        }
        // Browsers aren't child processes; they're just pages placed inside the
        // window. Trying to launch one here would produce a baffling "no
        // executable named browser" failure every time, out of nowhere.
        // (open_declared_browsers opens them)
        if config::browser_url_of(&argv).is_some() {
            continue;
        }
        let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
        let mut opts = tab_options(&ft.cfg, ws.group_of(ft));
        let argv = resolve_launch(
            argv,
            &mut opts,
            Some(ws),
            ft.cfg.id.as_deref(),
        );
        let cwd = opts.cwd.clone();
        let plan = match resume_plan_of(ft.cfg.resume.as_deref()) {
            named @ tab::Resume::Id(_) => named,
            _ => carried_conversation(carry, ws, &argv, &ft.cfg, &cwd, &title),
        };
        if let tab::Resume::Id(s) = &plan {
            append_hook_log(&format!("launching \"{title}\" carrying {}", s.short()));
        }
        match Tab::spawn_as(
            title.clone(),
            &argv,
            ft.cfg.profile.clone(),
            rows,
            cols,
            opts,
            plan,
        ) {
            Ok(mut tab) => {
                tab.locked = ft.cfg.locked;
                tab.auto_restart = ft.cfg.auto_restart;
                tab.depth = ft.depth;
                tab.id = ft.cfg.id.clone();
                tab.notify_on_done = ft.cfg.notify_on_done.clone();
                tabs.push(tab);
            }
            Err(e) => errors.push(tab::launch_problem(
                &title,
                argv.first().map(String::as_str).unwrap_or(""),
                cwd.as_deref(),
                &e.to_string(),
            )),
        }
    }
}

/// Switches workspaces (virtual-desktop model).
/// Switching means hiding, not stopping — tabs that go into the background keep running.
/// An unlaunched workspace gets its first launch right here.
#[allow(clippy::too_many_arguments)]
fn switch_workspace(
    to: usize,
    ws_index: &mut usize,
    tabs: &mut Vec<Tab>,
    ws_tabs: &mut [Vec<Tab>],
    workspaces: &[config::Workspace],
    active: &mut usize,
    panes: &mut crate::layout::Layout,
    ws_panes: &mut [crate::layout::Layout],
    rows: u16,
    cols: u16,
    errors: &mut Vec<String>,
    started_fired: &mut Vec<bool>,
    cfg: Option<&config::Config>,
    engine: &mut Option<HookEngine>,
    engines: &mut [Option<HookEngine>],
    caps: &hooks::Caps,
    last: &crate::lastsession::Saved,
) {
    // Guard against every backing array, not just `workspaces`: the per-workspace
    // `engines`/`ws_tabs` caches are resized on config reload, and a mismatch must
    // never index out of bounds (that would crash the whole app on switch).
    if to == *ws_index
        || to >= workspaces.len()
        || to >= engines.len()
        || to >= ws_tabs.len()
        || to >= ws_panes.len()
        || *ws_index >= ws_tabs.len()
        || *ws_index >= ws_panes.len()
    {
        return;
    }
    ws_tabs[*ws_index] = std::mem::take(tabs);
    // How a workspace is divided belongs to that workspace. Carrying one
    // layout across the switch would leave a project split into panes that
    // point at another project's tab numbers — the screen would look
    // deliberate and mean nothing.
    ws_panes[*ws_index] = panes.clone();
    // The Lua environment is kept per workspace (so shared variables survive switching)
    engines[*ws_index] = engine.take();
    *ws_index = to;
    // Ids only mean something within their own workspace.
    // Placed pages also only appear in the tab list for whichever one is currently viewed.
    caps.set_workspace(to);
    caps.set_secret_allow(
        workspaces[to].secrets_allow.clone(),
        workspaces[to].secrets_allow_all,
    );
    config::save_last_workspace(&workspaces[to].name);
    *tabs = std::mem::take(&mut ws_tabs[to]);
    if tabs.is_empty() {
        // First visit this run, so these tabs are being launched for the first
        // time and the same question applies as at startup: come back to what
        // this workspace was saying, or start it clean
        spawn_workspace(&workspaces[to], rows, cols, tabs, errors, Some(last));
        // Whether or not it was carried, the way back is worth holding on to:
        // this is what Ctrl+B r reaches for on a tab nobody has spoken to yet
        for t in tabs.iter_mut() {
            t.previous = last.conversation_for(&workspaces[to].name, t);
        }
        open_declared_browsers(&workspaces[to], caps, errors);
    }
    *engine = match engines[to].take() {
        Some(e) => Some(e),
        None => build_engine(cfg, workspaces.get(to), errors, caps),
    };
    started_fired.clear();
    started_fired.resize(tabs.len(), false);
    *active = if tabs.is_empty() { 0 } else { 1 };
    *panes = std::mem::replace(&mut ws_panes[to], crate::layout::Layout::single(*active));
    panes.show(*active);
}

/// What's laid out on screen, in exactly the order written in config.
///
/// Keeping sessions and browsers as separate variants is purely an internal
/// concern; it has nothing to do with whoever wrote the config.
#[derive(Clone, Debug, PartialEq)]
enum Surface {
    /// Which index into `tabs` (0-based)
    Session(usize),
    /// A page placed inside the window
    Browser {
        /// The name automation addresses it by (ID, or display name if none). Also the name used to place it in the window.
        key: String,
        /// The human-readable name
        name: String,
    },
}

/// Builds what's laid out on screen, in the order written in config.
///
/// Things not in config (a browser automation opened later, a tab launched
/// via arguments) get appended at the end. There's no way to decide a
/// position for something that was never written down.
fn surfaces_of(ws: Option<&config::Workspace>, titles: &[&str], hosted: &[String]) -> Vec<Surface> {
    let mut out: Vec<Surface> = Vec::new();
    let mut used_tabs = vec![false; titles.len()];
    let mut used_web: Vec<&str> = Vec::new();
    if let Some(ws) = ws {
        for ft in &ws.tabs {
            let argv = ft.cfg.command.argv();
            if argv.is_empty() {
                continue;
            }
            if config::browser_url_of(&argv).is_some() {
            let key = ft
                    .cfg
                    .id
                    .clone()
                    .or_else(|| ft.cfg.name.clone())
                    .unwrap_or_else(|| "browser".into());
                // Keeps a position even if it isn't open. If numbering shifted
                // based on open order, whatever a script points to would change
                // on every run.
                if let Some(h) = hosted.iter().find(|h| **h == key) {
                    used_web.push(h);
                }
                let name = ft.cfg.name.clone().unwrap_or_else(|| key.clone());
                out.push(Surface::Browser { key, name });
                continue;
            }
            let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
            // Even with duplicate names, match them one-to-one in written order
            let found = titles
                .iter()
                .enumerate()
                .find(|(i, t)| **t == title && !used_tabs[*i])
                .map(|(i, _)| i);
            if let Some(i) = found {
                used_tabs[i] = true;
                out.push(Surface::Session(i));
            }
        }
    }
    // Things not written in config
    for (i, used) in used_tabs.iter().enumerate() {
        if !used {
            out.push(Surface::Session(i));
        }
    }
    // Things not in config (opened later by automation, the settings screen, etc.) — the name is all there is
    for h in hosted {
        if !used_web.iter().any(|u| u == h) {
            // The result view gets a friendly, localized tab label; every other
            // ad-hoc page is addressed by (and labeled with) its own name.
            let name = if h == RESULT_TAB {
                i18n::t("tui.result.tab")
            } else {
                h.clone()
            };
            out.push(Surface::Browser {
                key: h.clone(),
                name,
            });
        }
    }
    out
}

/// Looks up a session's location from its screen number (1-based)
/// The surface a screen number stands for. Numbers are 1-based; 0 is no
/// surface at all -- a pane with nothing in it yet
fn ui_surface_at(surfaces: &[Surface], n: usize) -> Option<&Surface> {
    surfaces.get(n.checked_sub(1)?)
}

fn session_at(surfaces: &[Surface], active: usize) -> Option<usize> {
    match surfaces.get(active.checked_sub(1)?)? {
        Surface::Session(i) => Some(*i),
        Surface::Browser { .. } => None,
    }
}

/// The shape every terminal is cut to: **a phone that is watching decides it,
/// and the window decides it when none is.**
///
/// The two viewers see the same terminals at wildly different widths, and only
/// one number can be handed to a program. Both of them re-measure and re-report
/// freely -- the pane tree is redrawn on a tab switch and re-reports as part of
/// that -- so "whoever spoke last wins" was never a rule at all: the window
/// spoke on every repaint and took the size back within a frame of the phone
/// getting it. A phone opened onto a tab fitted its screen, then jumped to the
/// window's width the first time a tab was switched, and Claude Code -- which
/// rules a line clean across the terminal -- hung two thirds of itself off the
/// right edge with only a sideways scroll to read it by.
///
/// So the choice is made in one place, from who is looking rather than from who
/// spoke most recently, and the reports themselves become harmless. Watching
/// means a live state socket or a viewer still polling for the state; the
/// heartbeat sent along that socket is what makes a phone that walks away
/// noticed within a few seconds, and the window then has its own shape back
/// without anybody having to ask for it.
fn terminal_size(window: (u16, u16), phone: Option<(u16, u16)>, watched: bool) -> Size {
    match phone {
        Some((rows, cols)) if watched => Size { width: cols, height: rows },
        _ => Size { width: window.1, height: window.0 },
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
fn tab_sizes(
    tabs: usize,
    layout: &crate::layout::Layout,
    surfaces: &[Surface],
    geom: &[crate::browser::PaneGeom],
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
fn surface_at(surfaces: &[Surface], session: usize) -> usize {
    surfaces
        .iter()
        .position(|p| *p == Surface::Session(session.wrapping_sub(1)))
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// The session currently being viewed. None if viewing a browser.
fn session_mut<'a>(tabs: &'a mut [Tab], surfaces: &[Surface], active: usize) -> Option<&'a mut Tab> {
    let i = session_at(surfaces, active)?;
    tabs.get_mut(i)
}

/// Trims the screen text sent to the phone.
/// Trailing blank lines from the terminal would otherwise hide the content, so
/// those are dropped from the end; line count is also capped to save bandwidth.
fn trim_for_phone(s: &str, max_lines: usize) -> String {
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

/// Decides the remote UI's token.
/// Uses the one in secrets if present; otherwise saves one to data\remote-token
/// and reuses it (a token that changes every time would force reconnecting
/// phones each time and make it impossible to show the QR from settings).
/// Shortest fixed token accepted (hex chars of a 64-bit secret; anything
/// shorter is guessable from the open internet a Tailscale-less LAN may be)
pub const FIXED_TOKEN_MIN: usize = 16;

pub fn remote_token(cfg: &config::Config, password: Option<&str>) -> String {
    // A sticky pairing with a written token: the person's own string wins.
    // (A shorter string never reaches here — start_remote_bg refuses to start)
    if cfg.remote.sticky_token && cfg.remote.fixed_token.trim().len() >= FIXED_TOKEN_MIN {
        return cfg.remote.fixed_token.trim().to_string();
    }
    if let Some(t) = cfg.remote_token(password) {
        return t;
    }
    let path = config::state_path("remote-token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim().to_string();
        if t.len() >= 16 {
            return t;
        }
    }
    let t = random_hex(24);
    let _ = crypto::write_atomic(&path, &t);
    t
}

/// Starts the remote UI according to config (None if disabled)
/// Start the remote server WITHOUT making the caller wait for the bind (a
/// lingering earlier instance can hold the port for up to a second, and the
/// caller is the loop that answers every click). Returns None when remote is
/// disabled; otherwise a channel that delivers (the server if it came up,
/// error/note lines for the flash) once the bind settles.
fn start_remote_bg(
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
fn publish_remote(info: &Arc<Mutex<webui::RemoteInfo>>, ui: &Option<remote::RemoteUi>) {
    let mut i = info.lock().unwrap();
    match ui {
        Some(r) => {
            i.running = true;
            i.url = r.url.clone();
            i.note = r.note.clone().unwrap_or_default();
        }
        None => *i = Default::default(),
    }
}

/// A random hex string (for the remote UI's token)
/// A random UUID (version 4), in the spelling CLIs expect.
///
/// Written out here rather than pulled in: it is sixteen random bytes with six
/// bits set to say which kind of UUID it is, and a dependency for that would
/// weigh more than the function.
pub fn random_uuid() -> String {
    let hex = random_hex(16);
    let mut b: Vec<u8> = (0..16)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0))
        .collect();
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 1
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

pub fn random_hex(bytes: usize) -> String {
    use rand::TryRng as _;
    let mut buf = vec![0u8; bytes];
    if rand::rngs::SysRng.try_fill_bytes(&mut buf).is_err() {
        return "shikisha-fallback-token".into();
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// The root of the portable layout (base for relative paths; where the exe and its folders sit side by side)
fn config_file_dir() -> std::path::PathBuf {
    config::root_dir()
}

/// Opens the settings screen inside our own window. Only launched once; from
/// the second time on, it just returns to the same location.
/// `query` is extra instruction appended to the URL (e.g. "&addtab=0"; empty by default)
fn open_settings(
    web: &mut Option<webui::WebUi>,
    config_file: &std::path::Path,
    remote_info: &Arc<Mutex<webui::RemoteInfo>>,
    web_password: &Arc<Mutex<Option<String>>>,
    caps: &hooks::Caps,
    query: &str,
) -> Result<()> {
    let url = ensure_web_url(web, config_file, remote_info, web_password)?;
    // The settings screen is a local UI page. It holds no cookies, so the shared default profile is plenty.
    caps.browser_open(
        SETTINGS_TAB,
        &format!("{url}{query}"),
        browser::BrowserProfile::shared_default(),
    )
}

/// The tab's working folder as an absolute path string, for attachments. Falls
/// back to the app's own working folder when the tab has none configured.
fn tab_cwd_abs(t: &Tab) -> String {
    let abs = match t.cwd().map(std::path::Path::to_path_buf) {
        Some(p) if p.is_absolute() => Some(p),
        Some(p) => std::env::current_dir().ok().map(|c| c.join(p)),
        None => std::env::current_dir().ok(),
    };
    abs.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Ensure the local settings/result web server is running and hand back its
/// base URL (`http://127.0.0.1:<port>/?token=<token>`). Started lazily on first
/// use and kept for the process lifetime.
fn ensure_web_url(
    web: &mut Option<webui::WebUi>,
    config_file: &std::path::Path,
    remote_info: &Arc<Mutex<webui::RemoteInfo>>,
    web_password: &Arc<Mutex<Option<String>>>,
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
fn open_result(
    web: &mut Option<webui::WebUi>,
    config_file: &std::path::Path,
    remote_info: &Arc<Mutex<webui::RemoteInfo>>,
    web_password: &Arc<Mutex<Option<String>>>,
    caps: &hooks::Caps,
    run_id: &str,
) -> Result<()> {
    let base = ensure_web_url(web, config_file, remote_info, web_password)?;
    // base is ".../?token=<t>"; move to the /result page and carry the run id.
    let url = format!(
        "{}&run={}",
        base.replacen("/?token=", "/result?token=", 1),
        run_id
    );
    caps.browser_open(
        RESULT_TAB,
        &url,
        browser::BrowserProfile::shared_default(),
    )
}

/// Resolves a data file's path, preferring the location beside the exe (portable layout).
/// Used to resolve the automation directory. The settings GUI (webui) uses this
/// same resolution too, so "where the main app runs it from" and "where the
/// GUI reads/writes it" never drift apart.
pub(crate) fn resolve_data_path(p: &str) -> std::path::PathBuf {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
    {
        let cand = dir.join(p);
        if cand.exists() {
            return cand;
        }
    }
    std::path::PathBuf::from(p)
}

/// Builds a placed page's context from the screen layout.
/// Returns None for a page that's not in the layout (e.g. after it's closed).
fn page_ctx(
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
fn busy_repeat_due(
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

fn tab_ctx(t: &Tab, index: usize) -> TabCtx {
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
fn browser_ctx(index: usize, key: &str) -> TabCtx {
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
const MANUAL_GUARD_MS: u64 = 5000;

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
fn hand_line(
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
fn starting_workspace(enabled: bool, last: Option<&str>, names: &[String]) -> usize {
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
struct ViewMove {
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
const VIEW_GUARD_MS: u64 = 8_000;

/// Whether a human touched it recently. False if never touched at all.
///
/// Treating time 0 as "touched" here would silently drop every auto-send for
/// the guard period after app startup (this used to be why startup automation
/// didn't run).
fn touched_recently(t: &Tab, now_ms: u64) -> bool {
    t.last_manual_ms
        .is_some_and(|m| now_ms.saturating_sub(m) < MANUAL_GUARD_MS)
}

/// An excerpt collapsed onto a single line, for logging. Full text isn't
/// readable, so keep only the beginning.
fn log_excerpt(text: &str, max: usize) -> String {
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
const POSIX_PROBE: &str = r#"echo "===SHIKISHA ENV==="; uname -a; cat /etc/os-release 2>/dev/null | head -4; sw_vers 2>/dev/null; echo "--- tools ---"; for c in docker kubectl git python3 node java nginx mysql psql redis-cli systemctl apt-get yum dnf; do command -v $c >/dev/null 2>&1 && echo $c; done; echo "--- running ---"; ps -eo comm= 2>/dev/null | sort -u | grep -iE "nginx|httpd|apache|mysqld|mariadb|postgres|redis|php|java|node|docker|containerd|tomcat" | head -15; echo "===ENV END===""#;
const PS_PROBE: &str = r#""===SHIKISHA ENV==="; $PSVersionTable.PSVersion.ToString(); (Get-CimInstance Win32_OperatingSystem).Caption; "--- tools ---"; foreach ($c in "docker","kubectl","git","python","node","java","mysql","psql") { if (Get-Command $c -ErrorAction SilentlyContinue) { $c } }; "--- running ---"; (Get-Service | Where-Object Status -eq "Running" | Select-Object -ExpandProperty Name) -match "sql|nginx|apache|redis|docker|iis|w3svc|tomcat" | Select-Object -First 15; "===ENV END===""#;
const CMD_PROBE: &str =
    "echo ===SHIKISHA ENV=== & ver & echo --- tools --- & where docker git python node java mysql 2>nul & echo ===ENV END===";

/// Pull the survey's output block off a screen. The drafted command itself
/// echoes on screen too (with both markers inside one command line), so the
/// capture insists on the shape only real output has: the start marker ALONE
/// on its line, with content lines underneath, ending at the bare end marker
fn extract_env_block(screen: &str) -> Option<String> {
    // Walk marker lines, not raw indices: the echoed command line contains
    // the marker mid-line and must never match
    let mut start_line: Option<usize> = None;
    let lines: Vec<&str> = screen.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if l.trim() == "===SHIKISHA ENV===" {
            start_line = Some(i);
        }
    }
    let start = start_line?;
    let end = lines[start + 1..]
        .iter()
        .position(|l| l.trim().starts_with("===ENV END==="))?
        + start
        + 1;
    let body = lines[start..end].join("\n");
    (end > start + 1).then(|| body.trim().chars().take(1500).collect())
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
        assert_eq!(survey_probe("powershell", ""), PS_PROBE);
        assert_eq!(survey_probe("C:\\Windows\\System32\\cmd.exe", ""), CMD_PROBE);
        assert_eq!(survey_probe("wsl", ""), POSIX_PROBE);
        assert_eq!(survey_probe("ssh user@host", "user@host:~$ "), POSIX_PROBE);
        assert_eq!(survey_probe("ssh user@host", "PS C:\\Users\\a> "), PS_PROBE);
        assert_eq!(survey_probe("ssh user@host", "C:\\Users\\a> "), CMD_PROBE);
    }
}

/// Pick the probe whose syntax matches the terminal: the launch command for
/// local tabs, the prompt's shape for SSH and other indirections (a POSIX
/// shell being the overwhelming default on the far side)
fn survey_probe(cmdline: &str, screen: &str) -> &'static str {
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
fn save_replay_to_downloads() -> std::io::Result<Option<std::path::PathBuf>> {
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

pub fn append_hook_log(msg: &str) {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    let t = START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config::logs_dir().join("hooks.log"))
    {
        use std::io::Write as _;
        let _ = writeln!(f, "[{t:>8.3}] {msg}");
    }
}

/// The list of ids in the same order they're laid out on screen.
///
/// Targets are counted by screen position. A name and a number both point to
/// the same thing (numbers shift with reordering, so using names when writing is recommended).
fn surface_keys(surfaces: &[Surface], tabs: &[Tab]) -> Vec<hooks::TabKey> {
    surfaces
        .iter()
        .map(|p| match p {
            Surface::Session(i) => tabs.get(*i).map(|t| t.key()).unwrap_or_default(),
            // Browsers give priority to the id too; still lookup-able by display name
            Surface::Browser { key, name } => hooks::TabKey {
                id: Some(key.clone()),
                name: name.clone(),
            },
        })
        .collect()
}

/// A hand-off that can't be delivered yet. Runs once the recipient becomes ready to receive input.
///
/// It's not unusual for the target to still be starting up. Since a dropped
/// hand-off is invisible to everyone, we hold onto it ourselves instead.
struct Waiting {
    cmd: Command,
    /// Give up once this time passes. Holding onto it any longer wouldn't help — eventually nobody remembers it anyway.
    give_up_ms: u64,
}

/// Whether this hand-off is one that can wait for the recipient to become ready.
///
/// Only "delivering something" can wait. Restarts and notifications have
/// nothing to do with whether the recipient is ready.
fn can_wait(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::SendPrompt { .. } | Command::DraftPrompt { .. }
    )
}

/// The destination of that hand-off
fn target_of(cmd: &Command) -> Option<&hooks::TabRef> {
    match cmd {
        Command::SendPrompt { target, .. } | Command::DraftPrompt { target, .. } => Some(target),
        _ => None,
    }
}

/// Whether the recipient is in a state where it can accept input.
/// `now_ms` is the main loop's clock — the same one the readiness gate measures
/// "the screen has held still" against
fn ready_to_receive(t: &Tab, now_ms: u64) -> bool {
    t.ready_for_startup_hook(now_ms)
}

/// How long to hold before giving up. Whoever wrote it isn't watching anymore by the time this long has passed.
const WAIT_FOR_TAB_MS: u64 = 30_000;

/// Executes the operation requests queued by Lua hooks.
/// Auto-sends inherit chain depth (the invisible ball) and stop once the cap is hit.
#[allow(clippy::too_many_arguments)]
fn exec_commands(
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
fn handle_copy_key(
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
            *flash = Some(copy_to_clipboard(&text));
            t.copy = None;
            return Ok(());
        }
        // Copy the whole history
        KeyCode::Char('a') => {
            let text = extract_text(&mut p, 0, usize::MAX / 2, cols_v);
            p.screen_mut().set_scrollback(0);
            drop(p);
            *flash = Some(copy_to_clipboard(&text));
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

/// Bring one line of history into view, with the cursor on it.
///
/// Put near the middle rather than at an edge: a match at the very bottom of
/// the screen shows what came before it and nothing of what came after, which
/// is half the reason for looking
fn show_line<CB: vt100::Callbacks>(
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

/// Mouse handling: click a tab bar entry to switch / wheel scroll / select-to-copy instantly / right-click to paste
#[allow(clippy::too_many_arguments)]












/// UI state needed for drawing
struct Ui {
    /// First-ever run, before config exists (shows onboarding on INDEX)
    first_run: bool,
    /// Whether what's in view can be put back the way it started (see
    /// `restartable_page`). Drives the restart button beside the stop button
    restartable: bool,
    active: usize,
    /// Whether INDEX is covering the window. Not a surface: the panes wait
    /// underneath and come back the moment a running thing is picked
    board: bool,
    /// Whether the settings form is covering the window. A screen the same way
    settings: bool,
    auto: Option<bool>,
    ws_names: Vec<String>,
    ws_index: usize,
    ws_open: bool,
    help_open: bool,
    /// The keys in force, for the help screen to show
    help_rows: Vec<(String, String)>,
    /// The Vault's current search, when its overlay is open
    vault: Option<uistate::VaultState>,
    /// What this whole app is costing the machine, for the board header
    self_cost: Option<String>,
    /// The connection URL, if the QR code is being shown
    qr: Option<String>,
    /// Whether the remote UI is listening (shown at all times so it's never a mystery)
    remote_on: bool,
    /// Whether a phone/browser is connected over the remote link right now
    remote_conn: bool,
    /// Whether the pairing token is the fixed one from settings (it decides
    /// what the disconnect button promises, not what it does)
    remote_sticky: bool,
    /// What the focused tab is aimed at (🎯), as a screen number
    aim: Option<usize>,
    /// Where the auto-chain currently is (the invisible ball, made visible)
    ball: ball::Ball,
    /// The chain cap. Represents how close the ball's color is to that cap.
    max_chain: u32,
    /// Draw timestamp (relative ms). Used to drive the ball's animation.
    now_ms: u64,
    /// The surfaces on screen (one per tab-bar row), in the order written in config
    surfaces: Vec<Surface>,
    /// How the content area is divided, and which pane the keyboard is aimed at.
    /// `active` is always the surface in the focused pane
    layout: crate::layout::Layout,
    /// If the current workspace is a discussion, the opening speaker's session
    /// number (1-based) and display name — for the dashboard's "start" card
    discuss_start: Option<usize>,
    discuss_start_name: Option<String>,
    /// What making a branch would do, while someone is naming one
    branch: Option<crate::uistate::BranchPlan>,
    /// The folders being looked through, while somewhere new is being chosen
    browse: Option<crate::uistate::BrowseState>,
    /// The colours chosen for projects, by the folder git shares
    folder_colors: std::collections::HashMap<String, String>,
    /// The controls shown over the browser being viewed (None = don't show)
    nav: Option<crate::uistate::NavState>,
    /// How many lines back from the current screen we're scrolled (0 = live)
    scrolled: usize,
}






/// INDEX = home screen: session list + menu
/// The block-letter wordmark (3 lines). Per-character width is uneven, so
/// measure actual character width rather than counting including right-edge padding.
const WORDMARK: [&str; 3] = [
    "█▀▀ █ █ █ █ █ █ █▀▀ █ █ █▀█    ▀█▀ █▀▀ █▀█ █▄█",
    "▀▀█ █▀█ █ █▀▄ █ ▀▀█ █▀█ █▀█ ▀▀  █  █▀▀ █▀▄ █ █",
    "▀▀▀ ▀ ▀ ▀ ▀ ▀ ▀ ▀▀▀ ▀ ▀ ▀ ▀     ▀  ▀▀▀ ▀ ▀ ▀ ▀",
];

/// The wording used when collapsed to a single line
const WORDMARK_SMALL: &str = "◢◤ SHIKISHA-TERM";

/// How to show the name. Shrink it if it doesn't fit the screen; if even that
/// doesn't fit, don't show it at all.
///
/// Forcing something that doesn't fit to draw anyway would wrap and break
/// apart, making the screen itself look broken, not just the name. Height is
/// checked too, so the name doesn't push the list off screen when there are many tabs.
pub fn wordmark_lines(width: u16, height: u16) -> Vec<String> {
    let need = WORDMARK
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as u16;
    // Measure with one extra column added for left padding. An exact fit touches the border and looks cramped.
    if width >= need + 2 && height >= 12 {
        return WORDMARK.iter().map(|l| format!(" {l}")).collect();
    }
    if width >= WORDMARK_SMALL.chars().count() as u16 + 2 {
        return vec![format!(" {WORDMARK_SMALL}")];
    }
    Vec::new()
}

fn copy_to_clipboard(text: &str) -> String {
    let lines = text.lines().count();
    match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())) {
        Ok(()) => i18n::tp("msg.copied", &[("lines", &lines.to_string())]),
        Err(e) => i18n::tp("msg.copy_failed", &[("error", &e.to_string())]),
    }
}

/// Pastes clipboard contents into the child process.
/// Wraps it in \x1b[200~ ... \x1b[201~ if the child is in bracketed paste mode
fn paste_clipboard(t: &Tab) -> Result<Option<String>> {
    match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
        Ok(text) => {
            let bracketed = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste();
            let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
            if bracketed {
                let mut bytes = b"\x1b[200~".to_vec();
                bytes.extend_from_slice(normalized.as_bytes());
                bytes.extend_from_slice(b"\x1b[201~");
                t.write_bytes(&bytes)?;
            } else {
                t.write_bytes(normalized.as_bytes())?;
            }
            Ok(None)
        }
        Err(e) => Ok(Some(i18n::tp("msg.paste_failed", &[("error", &e.to_string())]))),
    }
}

/// A modified Enter, Tab, Backspace or Escape, spelled so the program can tell
/// which one it was.
///
/// The old keyboard has no room for these: Enter is one byte, and Shift+Enter
/// is the same byte, so "send this" and "start a new line here" arrive as the
/// same keystroke. Every AI CLI wants both, which is why they ask for the
/// newer keyboard on startup -- and why, until they were answered, the only way
/// to type a newline into one was to know its private workaround.
///
/// Only these four, and only when modified. A program that asked to tell keys
/// apart still gets its ordinary Return as a Return; what it gains is the one
/// distinction it had no way to make.
fn disambiguated_key(key: &KeyEvent) -> Option<Vec<u8>> {
    let code = match key.code {
        KeyCode::Enter => 13u32,
        KeyCode::Tab | KeyCode::BackTab => 9,
        KeyCode::Backspace => 127,
        KeyCode::Esc => 27,
        _ => return None,
    };
    // One bit per modifier, and the count is that plus one -- which is how
    // every escape has counted modifiers since long before this protocol, and
    // why an unmodified key counts 1 and is not sent this way at all
    let mut bits = 0u8;
    if key.modifiers.contains(KeyModifiers::SHIFT) || key.code == KeyCode::BackTab {
        bits |= 1;
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        bits |= 2;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        bits |= 4;
    }
    (bits != 0).then(|| format!("\x1b[{code};{}u", bits + 1).into_bytes())
}

/// crossterm KeyEvent -> the byte sequence sent to the child PTY.
///
/// `flags` is what the program in that tab asked the keyboard to report; 0 is
/// the keyboard every terminal has always had, and what everything not asking
/// for anything gets.
fn key_to_bytes_with(key: &KeyEvent, flags: u8) -> Option<Vec<u8>> {
    if flags & 1 != 0 {
        if let Some(bytes) = disambiguated_key(key) {
            return Some(bytes);
        }
    }
    key_to_bytes(key)
}

/// crossterm KeyEvent -> the byte sequence sent to the child PTY (VT100/xterm-style encoding)
fn key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::with_capacity(8);
    if key.modifiers.contains(KeyModifiers::ALT) {
        buf.push(0x1b);
    }
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let lower = c.to_ascii_lowercase();
                if lower.is_ascii_lowercase() {
                    buf.push((lower as u8) - b'a' + 1);
                } else {
                    return None;
                }
            } else {
                let mut tmp = [0u8; 4];
                buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
            }
        }
        KeyCode::Enter => buf.push(b'\r'),
        KeyCode::Backspace => buf.push(0x7f),
        KeyCode::Tab => buf.push(b'\t'),
        KeyCode::BackTab => buf.extend_from_slice(b"\x1b[Z"),
        KeyCode::Esc => buf.push(0x1b),
        KeyCode::Up => buf.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => buf.extend_from_slice(b"\x1b[B"),
        KeyCode::Right => buf.extend_from_slice(b"\x1b[C"),
        KeyCode::Left => buf.extend_from_slice(b"\x1b[D"),
        KeyCode::Home => buf.extend_from_slice(b"\x1b[H"),
        KeyCode::End => buf.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => buf.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => buf.extend_from_slice(b"\x1b[6~"),
        KeyCode::Insert => buf.extend_from_slice(b"\x1b[2~"),
        KeyCode::Delete => buf.extend_from_slice(b"\x1b[3~"),
        KeyCode::F(n) => match n {
            1 => buf.extend_from_slice(b"\x1bOP"),
            2 => buf.extend_from_slice(b"\x1bOQ"),
            3 => buf.extend_from_slice(b"\x1bOR"),
            4 => buf.extend_from_slice(b"\x1bOS"),
            5 => buf.extend_from_slice(b"\x1b[15~"),
            6 => buf.extend_from_slice(b"\x1b[17~"),
            7 => buf.extend_from_slice(b"\x1b[18~"),
            8 => buf.extend_from_slice(b"\x1b[19~"),
            9 => buf.extend_from_slice(b"\x1b[20~"),
            10 => buf.extend_from_slice(b"\x1b[21~"),
            11 => buf.extend_from_slice(b"\x1b[23~"),
            12 => buf.extend_from_slice(b"\x1b[24~"),
            _ => return None,
        },
        _ => return None,
    }
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let ev = crate::browser::Ev::Key {
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
        use crate::browser::PaneGeom;
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
        let argv = vec!["powershell.exe".to_string()];
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

    fn workspace_from(json: &str) -> config::Workspace {
        let cfg: config::Config = serde_json::from_str(json).unwrap();
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
            r#"{"workspaces":[{"name":"W","tabs":[{"name":"AGENT","command":"claude"}]}]}"#,
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
            r#"{"workspaces":[{"name":"W","tabs":[
                {"name":"AGENT","command":"claude","resume":"picked-from-the-vault"}
            ]}]}"#,
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
        let src = include_str!("main.rs");
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
    #[test]
    fn the_add_tab_button_arrives_prefixed() {
        let evs = super::keys_for(&crate::browser::Ev::AddTab { pane: None });
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
        let evs = super::keys_for(&crate::browser::Ev::OpenWs);
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
    /// Four things distribute this app and each used to carry its own list. They
    /// drifted without a sound: the deploy hook was copying the wording files and
    /// nothing else, so the detection profiles and the automation manual never
    /// reached the test machine at all, and no one could have noticed. A payload
    /// that is named once cannot arrive in some places and not others.
    #[test]
    fn one_list_says_what_gets_handed_out() {
        let list = include_str!("../dist.list");
        let mut patterns: Vec<&str> = Vec::new();
        for line in list.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') || t.starts_with('[') {
                continue;
            }
            patterns.push(t);
        }
        assert!(patterns.len() > 5, "dist.list を読めていない ({} 件)", patterns.len());

        // A pattern matching nothing is a typo that deploys quietly and forever
        for p in &patterns {
            let rel = p.trim_end_matches("/**");
            let (dir, file_pat) = rel.rsplit_once('/').unwrap_or((".", rel));
            // cargo test runs from the crate root, which is where these live
            let hit = std::fs::read_dir(dir).ok().is_some_and(|mut e| {
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
            assert!(hit, "dist.list の `{p}` に当てはまるものが1つも無い (綴り間違い?)");
        }

        // ...and the consumers must go through it rather than keeping their own copy
        let build_rs = include_str!("../build.rs");
        assert!(build_rs.contains("dist.list"), "build.rs が dist.list を読んでいない");
        let release = include_str!("../.github/workflows/release.yml");
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
    fn the_apps_own_screens_are_not_restartable_pages() {
        let caps: crate::hooks::Caps = std::rc::Rc::new(crate::caps::Capabilities::new(
            Default::default(),
            std::path::PathBuf::from("."),
            std::collections::HashMap::new(),
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
        let evs = super::keys_for(&crate::browser::Ev::Restart);
        assert_eq!(evs.len(), 2, "前置キー + 'r' の2打鍵");
        let Event::Key(k) = &evs[0] else { panic!("前置キーが打鍵でない") };
        assert_eq!(k.code, KeyCode::Char('b'));
        assert!(k.modifiers.contains(KeyModifiers::CONTROL));
        let Event::Key(k) = &evs[1] else { panic!("本体が打鍵でない") };
        assert_eq!(k.code, KeyCode::Char('r'));
        assert!(k.modifiers.is_empty());
        // Ctrl+B r has to still be the tab restart on the receiving side
        let body = include_str!("main.rs");
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
            let evs = super::keys_for(&crate::browser::Ev::Menu {
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
        // Also lookup-able by name
        assert_eq!(surface_of_id(&ws, "審判"), Some(3));
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
                    group: 0,
                }
            })
            .collect();
        config::Workspace {
            name: "試験".into(),
            groups: vec![config::Group::default()],
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
        let argv = vec!["cmd.exe".to_string()];
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
        let argv = vec!["cmd.exe".to_string()];
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
        let argv = vec!["cmd.exe".to_string()];
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
        let t = Tab::spawn("cmd".into(), &["cmd.exe".to_string()], None, 24, 80, tab::TabOptions::default())
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
            hooks::TabRef::Name("解析".into()).resolve(&keys),
            Some(1),
            "ブラウザを名前で指せていない"
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
        let argv = vec!["cmd.exe".to_string(), "/c".into(), "cd".into()];
        let mut t = Tab::spawn("cwd".into(), &argv, None, 10, 60, opts).unwrap();
        // cmd's "cd" shows the current folder
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let screen = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents();
        t.kill();
        assert!(
            screen.contains("shikisha-cwd-test"),
            "指定した作業フォルダで起動する: {screen}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_folder_falls_back_instead_of_failing_to_start() {
        let opts = tab::TabOptions {
            cwd: Some(std::path::PathBuf::from("Z:/does/not/exist")),
            ..Default::default()
        };
        let argv = vec!["cmd.exe".to_string()];
        // The session still launches even for a folder that doesn't exist (easier to recover from than a launch failure)
        let mut t = Tab::spawn("fallback".into(), &argv, None, 10, 60, opts)
            .expect("存在しないフォルダでも起動できる");
        t.kill();
    }

    #[test]
    fn hot_reload_applies_changes_without_restarting_untouched_tabs() {
        let ws0 = workspace_from(
            r#"{"workspaces":[{"name":"T","tabs":[
                {"name":"one","command":"cmd.exe"},
                {"name":"two","command":"cmd.exe"}
            ]}]}"#,
        );
        let mut tabs = Vec::new();
        let mut errs = Vec::new();
        spawn_workspace(&ws0, 24, 80, &mut tabs, &mut errs, None);
        assert_eq!(tabs.len(), 2, "{errs:?}");
        let one_before = tabs[0].signature();

        // one: gains a lock (applies immediately) / two: removed / three: added
        let ws1 = workspace_from(
            r#"{"workspaces":[{"name":"T","tabs":[
                {"name":"one","command":"cmd.exe","locked":true},
                {"name":"three","command":"cmd.exe"}
            ]}]}"#,
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
            r#"{"workspaces":[{"name":"T","tabs":[
                {"name":"one","command":"cmd.exe","encoding":"shift_jis"},
                {"name":"three","command":"cmd.exe"}
            ]}]}"#,
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
    /// Must be built as a windowed app.
    ///
    /// Without this, Windows opens a black console alongside it.
    /// Since the UI draws into our own window, that console would show nothing at all.
    #[test]
    fn the_exe_asks_windows_for_no_console() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("#![windows_subsystem = \"windows\"]"),
            "コンソールが付いてくる"
        );
    }

    /// The run must end when the window closes.
    ///
    /// If it kept running after that, a process invisible to everyone would
    /// be left behind. Since it still holds the listening port, the next
    /// launch would fail with "address already in use".
    ///
    /// `keys_for` discards reports that can't be converted into keystrokes,
    /// so a close can't be routed through there. The loop has to see it directly.
    #[test]
    fn closing_the_window_ends_the_run() {
        use crate::browser::Ev;
        assert!(
            super::keys_for(&Ev::Closed).is_empty(),
            "閉じたことを打鍵として扱っている"
        );
        let src = include_str!("main.rs");
        assert!(
            src.contains("Ev::Closed => self.closed = true"),
            "窓が閉じた報告を受けていない"
        );
        // How newlines are written varies by environment, so check line by line
        let mut lines = src.lines().map(str::trim);
        assert!(
            lines.any(|l| l == "if surface.closed {") && lines.next() == Some("break;"),
            "閉じてもループが終わらない"
        );
    }
}



//! SHIKISHA-TERM: portable, multi-session AI orchestration TUI
//!
//! Phase 3: multi-tab + INDEX dashboard + config.json
//!
//! Launch:
//!   SHIKISHA-TERM.exe                 # tabs from config.json (falls back to 1 PowerShell tab)
//!   SHIKISHA-TERM.exe claude          # debug: launch the given command in a single tab
//!
//! Controls (prefix key Ctrl+B):
//!   Ctrl+B q      quit (asks first while an AI is at work) / Ctrl+B 0-9 switch tab (0=INDEX) / Ctrl+B n/p next/prev tab
//!   ✕ on the window puts it away in the notification area (a setting); the icon there opens it again or quits
//!   Ctrl+B [      copy mode / Ctrl+B b send a literal Ctrl+B
//! Mouse: wheel=scroll (copy mode) / left-drag=select & copy instantly / right-click=paste

// The UI draws into our own window. We don't need a black console, so don't let
// Windows allocate one. (Only the terminal-facing --settings mode opens one itself, on demand.)
#![windows_subsystem = "windows"]

use shikisha_core::keymap::{key_to_bytes, key_to_bytes_with};
use shikisha_core::tab::RecordedStep;
use shikisha_core::workspace::{
    TabAuto, apply_ws_config, automation_by_pane, carried_conversation, resolve_launch,
    tab_options, build_engine, extract_env_block, open_declared_browsers, panel_places,
    spawn_workspace, surface_of_id, switch_workspace,
};
use shikisha_core::runtime::{WORDMARK, config_file_dir, WORDMARK_SMALL, keys_for, run, session_at, tab_cwd_abs};
use shikisha_core::view::{
    Size, pty_dims, terminal_size,
    RESULT_TAB, ScreenPush, Surface, Ui, panes_json, remote_floor, screen_push, server_spec,
    surfaces_of, title_of, ui_state_of,
};
use shikisha_core::send::{
    PASTE_ACK_MS, PASTE_CHUNK, PendingSend, SUBMIT_GIVE_UP_MS, SUBMIT_QUIET_MS, Step, paste_chunks,
};
use shikisha_core::{
    FIXED_TOKEN_MIN,
    append_hook_log,
    detach_console,
    random_bytes,
    random_hex,
    random_uuid,
    remote_token,
    resume_plan_of,
    agenthook,
    api,
    attach,
    ball,
    bridge,
    browserstate,
    caps,
    config,
    conpty,
    crypto,
    detect,
    digest,
    discover,
    exchange,
    folders,
    git,
    grants,
    hooks,
    i18n,
    instance,
    job,
    keys,
    lastsession,
    layout,
    limits,
    migrate,
    netaddr,
    notify,
    pr,
    profile,
    push,
    pwa,
    reader,
    remote,
    reply,
    repo,
    session_log,
    sessionfind,
    shell,
    ssh,
    tab,
    tailscale,
    theme,
    toast,
    uistate,
    update,
    usage,
    vault,
    watch,
    webui,
    winpath,
    worktree,
    ws,
    wspack,
};
mod browser;
mod picker;
mod wintoast;
mod tray;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};


use detect::TabState;
use hooks::{Command, HookEngine, TabCtx};
use tab::{CopyState, Tab, extract_text};



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

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Say something where it can be seen when there is no window yet.
///
/// This is a GUI subsystem binary, so it has no console: an error returned
/// from `main` is written to a stderr that nobody owns, and the program simply
/// vanishes. Anything that can fail before the first window exists has to
/// borrow the shell's own dialog or it says nothing at all -- which is what a
/// person who double-clicks the exe and gets no answer is looking at.
fn say_fatally(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let body = wide(text);
    let title = wide(&i18n::t("err.fatal.title"));
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}


fn say_fatally_with_page(text: &str, url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDOK, MB_ICONERROR, MB_OKCANCEL, MessageBoxW, SW_SHOWNORMAL,
    };
    let body = wide(text);
    let title = wide(&i18n::t("err.fatal.title"));
    let answer = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OKCANCEL | MB_ICONERROR,
        )
    };
    if answer == IDOK {
        let open = wide("open");
        let wurl = wide(url);
        let ok = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                open.as_ptr(),
                wurl.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        // Anything at or below 32 is a failure, and a machine with nothing
        // registered to open http with is a real one -- Windows Sandbox is
        // exactly that. Having promised a page, hand over the address rather
        // than doing nothing where a button was pressed.
        if (ok as isize) <= 32 {
            say_fatally(&shikisha_core::i18n::tp("err.webview2.address", &[("url", url)]));
        }
    }
}

fn main() -> Result<()> {
    // This process owns a desktop, so it is the one that can put a banner on it.
    // Told once, before anything has cause to send one
    notify::use_local_banners(Box::new(wintoast::WindowsBanners));
    webui::use_file_picker(Box::new(picker::DesktopPicker));
    let r = boot();
    if let Err(e) = &r {
        // The modes that run as somebody else's subprocess stay silent. A hook
        // must never make a sound the agent could mistake for its own, and a
        // dialog with nobody there to close it would hang the caller instead of
        // failing it.
        let quiet = matches!(
            std::env::args().nth(1).as_deref(),
            Some("--bridge") | Some("--hook")
        );
        if !quiet {
            say_fatally(&format!("{e}"));
        }
    }
    r
}

fn boot() -> Result<()> {
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
    // A copy started to finish an update waits for the copy that started it
    // to leave, so nothing below reads files the old one is still writing
    update::wait_for_handoff();
    // An update that was interrupted mid-swap is put back, and one that
    // finished is tidied, before any of the files it touched is read
    update::finish_last();
    // Move the legacy layout (config.json under the root) into the config folder (once only).
    // This must happen before loading, or we'd start up with the pre-migration empty config.
    config::migrate_legacy_config();
    // The first start of a version over these files: a copy is kept, then
    // they are carried forward one step per version (migrate.rs). Before
    // anything reads them, so what is read is already in this version's shape
    update::set_outcome(migrate::on_start());
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

    // One program per layout (see instance.rs). Started again over the same
    // folders -- a second click on the shortcut while the first is put away in
    // the notification area -- this one asks the first to show its window and
    // leaves, the way any resident program does. A copy on another layout
    // (demo, portable beside a different exe) is not this one and runs
    let _instance = match instance::claim() {
        instance::Standing::First(claim) => claim,
        instance::Standing::Second => {
            append_hook_log("Already running from this layout; asked the running copy to show its window");
            instance::ask_to_show();
            return Ok(());
        }
    };

    // Which pseudo console this run got, written down before the first tab
    // opens. Both ways of ending up on the older one are silent -- a download
    // that arrived without the file, and the file arriving without the program
    // it starts -- so this line is where that silence gets a sentence.
    append_hook_log(&conpty::report().line());

    // Ask what WSL distributions are installed, once, on a thread of its own.
    // A shell inside one announces its folder by the distribution's name, and
    // the only way to tell that name from a machine at the end of an ssh
    // session is to know what is installed here. Asking on the thread that
    // reads a tab's output would stop that tab for as long as wsl.exe takes.
    discover::learn_wsl_distros();

    // Running it pops up the window. Only add a launcher in front when there's a reason to.
    run_in_window()
}

/// `--cast-test <url>`: opens a browser, starts screen relaying, saves the first frame
/// to logs/cast-test.jpg, and exits. For self-checks that don't need a human's eyes.
fn cast_test(url: &str) -> Result<()> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;

    println!("{}", shikisha_core::i18n::tp("cli.cast_test.opening", &[("url", url)]));
    let browser = browser::Browser::spawn(url, "cast-test")?;
    browser.screencast(None, true)?;
    println!("{}", shikisha_core::i18n::t("cli.cast_test.relaying"));

    let status = config::logs_dir().join("cast-test.txt");
    // The very first frame tends to be blank white, before anything's drawn.
    // Collect for a few seconds and save the "last one" instead.
    let settle = Instant::now() + Duration::from_secs(5);
    let mut last: Option<(Vec<u8>, u32, u32)> = None;
    let mut count = 0u32;
    loop {
        for ev in browser.drain() {
            if let shikisha_shared::Ev::Frame { data, w, h, .. } = ev {
                let bytes = b64.decode(data.as_bytes()).map_err(|e| {
                    anyhow::anyhow!(shikisha_core::i18n::tp(
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
            print!("{}", shikisha_core::i18n::tp("cli.cast_test.saved", &[("msg", &msg)]));
        }
        None => {
            let _ = std::fs::write(&status, "TIMEOUT: no frame in 5s\n");
            println!("{}", shikisha_core::i18n::t("cli.cast_test.no_frame"));
        }
    }
    Ok(())
}

















/// The set of things needed to draw into our own window
struct WinSurface {
    /// What a person did, waiting for the loop to act on it. The window only
    /// ever posts into this; what the reports mean is the runtime's business
    mail: shikisha_core::mailbox::Mailbox,
    win: std::rc::Rc<crate::browser::Browser>,
    /// What the window measured for itself, in character cells.
    rows: u16,
    cols: u16,
    /// What a phone measured for its own screen, when one has said. Kept apart
    /// from the window's own numbers because they are two different answers to
    /// two different questions, and `terminal_size` picks between them.
    phone: Option<(u16, u16)>,
    /// The last state we sent. Only send again when it changes.
    last: Option<shikisha_core::uistate::UiState>,
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
    pane_geom: Vec<shikisha_shared::PaneGeom>,
    /// The whole content area. Where a screen that covers the window goes
    full: (i32, i32, i32, i32),
    /// The pane tree as last sent to the page. Only send it again when it changes
    last_layout: String,
    /// The terminal contents last sent for each unfocused pane, and what they
    /// were made from. The focused pane goes through `last_screen_rows`, since
    /// it keeps the full renderer
    last_pane_screens: std::collections::HashMap<u32, (ScreenKey, String)>,
    /// Intents that arrived from the window, converted into the form the loop reads.
    /// The loop only understands terminal key input, so everything gets funneled there.
    pending: std::collections::VecDeque<Event>,
    /// The window is put away. Drawing goes on regardless (the phone reads the
    /// same state), but a notification's click has to bring it back first
    hidden: bool,
}

impl WinSurface {
    // ── What this shell measured, as a runtime asks for it ──────────────
    /// Put the keyboard back where a person expects it after a placed page
    /// had it (the window's own view, not the page's)
    fn take_keyboard_back(&self) {
        use shikisha_shared::BrowserHost;
        let _ = self.win.focus(None);
    }

    fn geom_area(&self) -> (i32, i32, i32, i32) { self.area }
    fn geom_full(&self) -> (i32, i32, i32, i32) { self.full }
    fn geom_rows(&self) -> u16 { self.rows }
    fn geom_cols(&self) -> u16 { self.cols }
    fn geom_panes(&self) -> &[shikisha_shared::PaneGeom] { &self.pane_geom }
    fn phone_size(&self) -> Option<(u16, u16)> { self.phone }
    fn set_phone_size(&mut self, size: Option<(u16, u16)>) { self.phone = size; }
    fn is_hidden(&self) -> bool { self.hidden }
    fn last_drawn(&self) -> Option<&shikisha_core::uistate::UiState> { self.last.as_ref() }
    fn queue_input(&mut self, ev: Event) { self.pending.push_back(ev); }

    /// Puts an externally-arrived operation into the same queue as window keystrokes.
    /// Whether it came from a phone or not, the loop sees no difference.
    fn inject(&mut self, ev: Event) {
        self.pending.push_back(ev);
    }











    /// Put the tab bar away, or bring it back out.
    ///
    /// The page owns the width and answers with the new one, so this asks
    /// rather than decides -- there is one number and one place that holds it
    fn toggle_tab_bar(&self) {
        let _ = self.win.eval("window.__toggleTabBar && window.__toggleTabBar();");
    }


    /// The pending "open settings" request (section, return-on-save, the
    /// working folder to land on), if any, clearing it.
    fn take_open_settings(
        &mut self,
    ) -> Option<(Option<String>, bool, Option<String>, Option<u32>)> {
        self.mail.open_settings.take()
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

















    /// Hand one answer back to the git panel (already JSON-encoded)
    fn push_git(&self, json: &str) {
        let _ = self.win.eval(&format!("window.__git && window.__git({json});"));
    }


    /// Hand one answer back to the file panel (already JSON-encoded)
    fn push_sftp(&self, json: &str) {
        let _ = self.win.eval(&format!("window.__sftp && window.__sftp({json});"));
    }


    /// Deliver one recorded Lua line (already JSON-encoded) to the composer.
    fn push_recorded(&self, line_json: &str) {
        let _ = self.win.eval(&format!("window.__recorded({line_json});"));
    }

    /// Takes the pending ✨ suggestion requests since the last drain.
    /// Route a Vault intent that arrived from the phone into the same queues a
    /// window-origin one uses, so both are drained in one place
    fn queue_vault(&mut self, ev: shikisha_shared::Ev) {
        match ev {
            shikisha_shared::Ev::VaultSearch { query } => self.mail.vault_queries.push(query),
            ev @ shikisha_shared::Ev::VaultOpen { .. } => self.mail.vault_opens.push(ev),
            _ => {}
        }
    }











    /// Deliver a finished ✨ suggestion (JSON: {ok, cmd?/error?}) to the composer.
    fn push_suggested(&self, json: &str) {
        let _ = self.win.eval(&format!("window.__suggested({json});"));
    }


    /// Deliver 🔍 survey progress (JSON: {stage} / {ok, error?}) to the board.
    fn push_surveyed(&self, json: &str) {
        let _ = self.win.eval(&format!("window.__surveyed({json});"));
    }

    /// Deliver the verdict of a ▶ run (JSON: null = clean, string = the error).
    fn push_lua_done(&self, err_json: &str) {
        let _ = self.win.eval(&format!("window.__luaDone({err_json});"));
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
        let look = shikisha_core::config::load().map(|c| c.appearance).unwrap_or_default();
        let scheme = look.scheme();
        let vars = serde_json::to_string(&scheme.css_vars()).unwrap_or_else(|_| "\"\"".into());
        let light = shikisha_core::theme::is_light(&scheme);
        let _ = self
            .win
            .eval(&format!("window.__setTheme({vars}, {light});"));
    }

    /// Puts the window away. Everything else goes on: the tabs, the phone,
    /// automation. The icon in the notification area is the way back
    fn hide(&mut self) {
        let _ = self.win.hide();
        self.hidden = true;
        append_hook_log("Window put away; the program goes on in the notification area");
    }

    /// Brings the window back in front of the person
    fn show(&mut self) {
        let _ = self.win.show();
        if self.hidden {
            append_hook_log("Window brought back from the notification area");
        }
        self.hidden = false;
    }

    /// Says, the first time the window is put away, that the program is still
    /// there and where to find it. Once: after that the icon speaks for itself,
    /// and a banner on every ✕ would be nagging
    fn say_where_it_went(&self) {
        let told = config::state_path("tray-noticed");
        if told.exists() {
            return;
        }
        let _ = crypto::write_atomic(&told, "1");
        let _ = self.win.tray_notice("SHIKISHA-TERM", &i18n::t("msg.tray.resident"));
    }

    fn take_events(&mut self, active_tab: Option<&Tab>) {
        use shikisha_shared::Ev;
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
                Ev::FocusPane { id } => self.mail.focus_panes.push(id),
                Ev::ClosePane { id } => self.mail.close_panes.push(id),
                Ev::PaneRatio { divider, ratio } => self.mail.pane_ratios.push((divider, ratio)),
                Ev::SplitPane { id, down } => self.mail.pane_splits.push((id, down)),
                Ev::RestartPane { id, keep } => self.mail.restart_panes.push((id, keep)),
                Ev::FontSize { px } => self.mail.font_size = Some(px),
                Ev::TabWidth { px } => self.mail.tab_width = Some(px),
                Ev::JsError { msg } => {
                    shikisha_core::append_hook_log(&format!("Screen failure: {msg}"));
                }
                // The window was closed. If we don't shut down here, a process with
                // nowhere left to draw stays alive unseen, still holding the listening port.
                Ev::Closed => self.mail.closed = true,
                Ev::CloseRequested => self.mail.close_requested = true,
                Ev::TrayOpen => self.mail.tray_open = true,
                Ev::TrayQuit => self.mail.tray_quit = true,
                // The settings page's "close settings" button. Where the tab actually
                // gets torn down (caps, active) isn't touched here — that's left to the loop.
                Ev::CloseSettings => self.mail.close_settings = true,
                Ev::OpenSettings { section, ret, folder, tabpos } => {
                    self.mail.open_settings = Some((section, ret, folder, tabpos))
                }
                Ev::VaultSearch { query } => self.mail.vault_queries.push(query),
                ev @ Ev::VaultOpen { .. } => self.mail.vault_opens.push(ev),
                ev @ Ev::Branch { .. } => {
                    self.mail.branches.extend(shikisha_shared::BranchAsk::of(ev));
                }
                Ev::Repair { folder, choose, branch, take } => {
                    self.mail.repairs.push((folder, choose, branch, take))
                }
                Ev::FolderColor { folder, color } => self.mail.folder_colors.push((folder, color)),
                Ev::Browse { path, open } => self.mail.browses.push((path, open)),
                Ev::FolderName { folder, name } => self.mail.folder_names.push((folder, name)),
                Ev::FolderClose { folder } => self.mail.folder_closes.push(folder),
                Ev::FolderDiscard { folder } => self.mail.folder_discards.push(folder),
                Ev::RemoteCut => self.mail.remote_cut = true,
                Ev::Coach { step } => self.mail.coach_done = Some(step),
                Ev::Thanks { open } => self.mail.thanks = Some(open),
                Ev::Update { open } => self.mail.update_card = Some(open),
                Ev::Help => self.mail.help_site = true,
                Ev::LimitAck { tab } => self.mail.limit_acks.push(tab),
                // A Lua quick-action was tapped. Remember its index; the loop looks
                // up the code and runs it (it has the hook engine and config).
                Ev::RunAction { index } => self.mail.run_actions.push(index),
                // Operate-a-target request; the loop has the engine to attach it.
                Ev::Operate { target, goal } => self.mail.operates.push((target, goal)),
                // Save the newest replay.lua to Downloads (the board can't
                // download over HTTP; the loop owns the answer message).
                Ev::ReplaySave => self.mail.replay_saves = true,
                // ✨ suggestion request; the loop owns the assistant AI call.
                Ev::Suggest { text } => self.mail.suggests.push(text),
                // 🔍 survey request; the loop types the probe and captures it.
                Ev::Survey => self.mail.surveys += 1,
                // 📼 / ▶ from the composer, and recorded steps from pages. All
                // resolved by the loop (it knows the shown browser and the engine).
                Ev::Record { on } => self.mail.record_arms.push(on),
                Ev::RunLua { code } => self.mail.run_luas.push(code),
                Ev::Git { panel, act, args } => self.mail.gits.push((panel, act, args)),
                Ev::Sftp { panel, act, args } => self.mail.sftps.push((panel, act, args)),
                Ev::Recorded {
                    from: Some(child),
                    act,
                    sel,
                    value,
                    xpath,
                    hint,
                } => self.mail.recorded.push(RecordedStep { child, act, sel, value, xpath, hint }),
                // Composer input while viewing a browser tab. Stash it; the loop
                // injects it into the shown browser via caps.browser_inject — the
                // same call the phone's relay makes, not a desktop-only path.
                Ev::Inject { input, .. } => self.mail.injects.push(input),
                // A file attached in the desktop composer. Save it beside the
                // active tab (the folder its AI runs in) and hand the path back to
                // the page. Same saver the phone's /api/attach route uses.
                Ev::Attach { id, name, data } => {
                    let cwd = active_tab.map(tab_cwd_abs).unwrap_or_default();
                    let result = shikisha_core::remote::attach_save(&cwd, &name, &data);
                    let _ = self
                        .win
                        .eval(&format!("window.__attachDone({id}, {result});"));
                }
                // The top bar was pressed. The destination is "whatever page is currently
                // showing", so the loop decides (only one bar is ever displayed).
                Ev::Go { go } => self.mail.gos.push(go),
                Ev::Scroll { by, row, col } => self.mail.scrolls.push((by, row, col)),
                Ev::Say { tab, text } => self.mail.says.push((tab, text)),
                Ev::Where {
                    from: Some(name),
                    url,
                    can_back,
                    can_forward,
                } => self.mail.wheres.push((name, url, can_back, can_forward)),
                // The bar on a placed page was pressed = a human finished their turn.
                // Who pressed it can only be told from the name attached to the report.
                Ev::Button { from: Some(name) } => self.mail.presses.push(name),
                // A placed page took the keyboard. Only pages placed in the
                // window report this; the shell's own presses already say
                // which pane they landed on
                Ev::Touched { from: Some(name) } => self.mail.touches.push(name),
                Ev::Touched { from: None } => {}
                // A tab was asked for from a pane with nothing in it. Note
                // which pane asked, then go on to open the form exactly as the
                // tab bar's + does -- one door, so the two cannot drift
                Ev::AddTab { pane, folder } => {
                    self.mail.add_tab_pane = pane.or(self.mail.add_tab_pane);
                    // A folder was named: the form has to be told, so remember
                    // it for the door below. Otherwise this is the tab bar's +,
                    // which goes through the same key the keyboard uses -- one
                    // door, so the two cannot drift
                    if let Some(f) = folder {
                        self.mail.add_tab_folder = Some(f);
                    }
                    for e in keys_for(&shikisha_shared::Ev::AddTab {
                        pane: None,
                        folder: None,
                    }) {
                        self.pending.push_back(e);
                    }
                }
                // The pen a placed page drew for itself was pressed
                Ev::Compose { .. } => {
                    let _ = self.win.eval("window.__composer && window.__composer();");
                }
                // The window's page says whether that pen should be showing
                Ev::Pen { on } => self.mail.pen = Some(on),
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
                } => self.mail.loads.push((name, url, complete)),
                // A browser started/finished loading. Conversion to the id happens on
                // the loop side — doing it here as a display name would make WinSurface
                // need to know about caps.
                Ev::Loading {
                    from: Some(name),
                    busy,
                } => self.mail.loading.push((name, busy)),
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
                        self.mail.frames.push(bytes);
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





/// Opens our own window and runs the same loop on top of it
fn run_in_window() -> Result<()> {
    // Serve the shell page. file:// breaks wry's IPC, so serve it over local HTTP instead.
    let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| {
        anyhow::anyhow!(shikisha_core::i18n::tp(
            "err.main.local_server",
            &[("e", &e.to_string())]
        ))
    })?;
    let port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow::anyhow!(shikisha_core::i18n::t("err.main.no_port")))?
        .port();
    let page = shell::page();
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            // The shell page and the pictures it names. The QR image rides
            // along inside the state, so there's no separate route for it
            // (works even when the window and the phone get served from
            // different origins).
            //
            // The pictures are here because the page asks for them wherever it
            // is served, and a page whose <link> answers with HTML is a page
            // that lies about itself. It is the same drawing the phone gets.
            let path = req.url().split('?').next().unwrap_or("/").to_string();
            if let Some(bytes) = pwa::icon(&path) {
                let r = tiny_http::Response::from_data(bytes).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"image/png"[..])
                        .expect("header"),
                );
                let _ = req.respond(r);
                continue;
            }
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

    // Ask before opening the first window, not through its failure. Without the
    // runtime the window thread dies at once and says so only to the log, while
    // this side waits out its twenty second timeout -- so the machine that
    // cannot run this program at all spends twenty seconds looking like one
    // that is merely slow, and then closes without a word.
    if browser::runtime_version().is_none() {
        // Said here rather than returned, because this is the one failure whose
        // answer is known: the generic path can only repeat an error, and this
        // one can hand over the page that fixes it.
        say_fatally_with_page(
            &shikisha_core::i18n::t("err.webview2.missing"),
            "https://developer.microsoft.com/microsoft-edge/webview2/",
        );
        std::process::exit(1);
    }
    let win = std::rc::Rc::new(browser::Browser::spawn(
        &format!("http://127.0.0.1:{port}/"),
        "SHIKISHA-TERM",
    )?);
    let mut surface = WinSurface {
        mail: Default::default(),
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
        last_layout: String::new(),
        last_pane_screens: std::collections::HashMap::new(),
        pending: std::collections::VecDeque::new(),
        hidden: false,
    };
    run(&mut surface)
}

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






fn screen_key(session: usize, t: &Tab) -> ScreenKey {
    let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
    let s = p.screen();
    let (rows, cols) = s.size();
    ScreenKey { session, bytes: t.output_count(), rows, cols, scrollback: s.scrollback() }
}


impl WinSurface {
    fn size(&self) -> Result<Size> {
        Ok(Size { width: self.cols, height: self.rows })
    }

    /// Waits for the next operation. Intents from the window arrive already
    /// converted into key operations the loop knows about.
    fn poll(&mut self, timeout: Duration, active_tab: Option<&Tab>) -> Result<Option<Event>> {
        self.take_events(active_tab);
        if self.mail.closed {
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
    fn host(&self) -> Option<(std::rc::Rc<dyn shikisha_shared::BrowserHost>, (i32, i32, i32, i32))> {
        Some((std::rc::Rc::clone(&self.win) as std::rc::Rc<dyn shikisha_shared::BrowserHost>, self.area))
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
                        shikisha_core::append_hook_log(&format!(
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
                        shikisha_core::shell::screen_html(p.screen())
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
                            (shikisha_core::shell::screen_rows(s), (r, c, !s.hide_cursor()))
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


































/// Decides the remote UI's token.
/// Uses the one in secrets if present; otherwise saves one to data\remote-token
/// and reuses it (a token that changes every time would force reconnecting
/// phones each time and make it impossible to show the QR from settings).
/// Shortest fixed token accepted (hex chars of a 64-bit secret; anything
/// shorter is guessable from the open internet a Tailscale-less LAN may be)










































/// Mouse handling: click a tab bar entry to switch / wheel scroll / select-to-copy instantly / right-click to paste
#[allow(clippy::too_many_arguments)]




















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



/// How long a burst of output takes to reach the rows the window is handed.
///
/// The published figures for the two pseudo consoles were measured at the PTY
/// mouth: bytes out of the pipe, and nothing after that. Everything after that
/// is ours -- the vt100 parse, the row diff, the JSON that goes to the page --
/// and whatever extra the older console sends goes through every one of those
/// steps. So this measures the last point this program controls.
///
/// Two shapes, because the answer turned out to depend entirely on which:
///
/// The output comes from `vt_writer` (src/bin/vt_writer.rs), which announces
/// itself as a terminal program before it writes. `type` will not do: it writes
/// through a handle whose console mode says nothing about escape sequences, and
/// then both pseudo consoles behave identically because neither is being asked
/// to do the thing they differ at. Measured that way, they looked the same.
///
///   - **poured**: ten thousand lines of Japanese, written once, never
///     revisited. This is a build log, a `cat`.
///   - **redrawn**: the same characters put on screen over and over, each
///     frame beginning by sending the cursor home and colouring as it goes.
///     This is what an AI CLI looks like while it is thinking, and it is the
///     shape the older console re-renders rather than forwards.
///
/// Run by hand, twice, with `vendor\conpty` beside the test binary and then
/// moved aside:
///
///   cargo test -- --ignored a_burst_of_japanese --nocapture
///
/// Both runs print which console they used, so the pair cannot be mixed up.
#[cfg(test)]
mod frame_bench {
    use super::*;
    use shikisha_core::tab::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    const END: &str = "SHIKISHA-BURST-END";
    /// 39 characters, the width the published figures were taken at, and wide
    /// enough that a re-render has real work to do on every line.
    const LINE: &str = "吾輩は猫である。名前はまだ無い。どこで生れたか頓と見当がつかぬ。何でも薄暗いじ";

    /// Where the stand-in terminal program is, beside the test binary or one
    /// folder up from it (`cargo test` puts tests under `deps/`).
    fn writer() -> std::path::PathBuf {
        let exe = std::env::current_exe().expect("current_exe");
        let here = exe.parent().expect("dir");
        for dir in [here, here.parent().unwrap_or(here)] {
            let p = dir.join("vt_writer.exe");
            if p.exists() {
                return p;
            }
        }
        panic!("vt_writer が見つからない (cargo build --bin vt_writer)");
    }

    /// Wait until the tab has stopped saying anything for `quiet`.
    fn settle(tab: &Tab, quiet: Duration, cap: Duration) {
        let start = Instant::now();
        let (mut last, mut still) = (0u64, Instant::now());
        while start.elapsed() < cap {
            std::thread::sleep(Duration::from_millis(50));
            let n = tab.output_count();
            if n != last {
                last = n;
                still = Instant::now();
            } else if last > 0 && still.elapsed() > quiet {
                return;
            }
        }
    }

    fn measure(tab: &Tab, writer: &std::path::Path, kind: &str) {
        let before = tab.output_count();
        let start = Instant::now();
        tab.write_bytes(format!("\"{}\" {kind}\r", writer.display()).as_bytes())
            .expect("write");

        // Exactly what the window does: read the parser at the rate the loop
        // polls at, work out which rows moved, and count what would have been
        // handed over.
        let mut had: Vec<String> = Vec::new();
        let (mut rows_sent, mut whole_sent, mut to_page) = (0usize, 0usize, 0usize);
        let mut arrived = None;
        while start.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(16));
            let now = {
                let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
                shikisha_core::shell::screen_rows(p.screen())
            };
            match screen_push(&had, &now) {
                ScreenPush::Nothing => {}
                ScreenPush::Rows(moved) => {
                    rows_sent += 1;
                    let list: Vec<(usize, &str)> =
                        moved.iter().map(|&i| (i, now[i].as_str())).collect();
                    to_page += serde_json::to_string(&list).unwrap_or_default().len();
                }
                ScreenPush::Whole => {
                    whole_sent += 1;
                    to_page += serde_json::to_string(&now.join("\n")).unwrap_or_default().len();
                }
            }
            had = now;
            // What comes back from screen_rows is the HTML the page is handed,
            // not plain text, so the row is searched rather than compared. The
            // echo of the typed command holds the writer's path and the shape,
            // never this word, so finding it means the burst has landed.
            if had.iter().any(|r| r.contains(END)) {
                arrived = Some(start.elapsed());
                break;
            }
        }

        let Some(took) = arrived else {
            for (i, r) in had.iter().enumerate().rev().take(4).collect::<Vec<_>>().iter().rev() {
                println!("  row {i}: {:?}", r.chars().take(120).collect::<String>());
            }
            panic!("{kind}: 最後の行が画面に出ないまま時間切れ");
        };
        println!(
            "  {kind:>7}: {:>4}ms  {:>9} bytes from the pty  \
             {rows_sent} row updates + {whole_sent} whole redraws  \
             {to_page} bytes to the page",
            took.as_millis(),
            tab.output_count() - before,
        );
    }

    /// The same measurement with the writer as the tab's own program, and no
    /// shell under it.
    ///
    /// `cmd.exe` leans on the console API, which is the thing passthrough
    /// cannot serve -- a pseudo console in passthrough mode stops rendering on
    /// the child's behalf, and a program that expected it to renders nothing.
    /// So for that question the child has to be a program that speaks only VT.
    #[test]
    #[ignore]
    fn a_burst_from_a_vt_only_program() {
        println!("{}", shikisha_core::conpty::report().line());
        let writer = writer();
        for kind in ["poured", "redrawn", "sequences"] {
            let tab = Tab::spawn(
                writer.display().to_string(),
                &[writer.display().to_string(), kind.to_string()],
                None,
                40,
                120,
                TabOptions::default(),
            )
            .expect("起動");
            let start = Instant::now();
            let mut had: Vec<String> = Vec::new();
            let mut arrived = None;
            while start.elapsed() < Duration::from_secs(30) {
                std::thread::sleep(Duration::from_millis(16));
                had = {
                    let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
                    shikisha_core::shell::screen_rows(p.screen())
                };
                if had.iter().any(|r| r.contains(END)) {
                    arrived = Some(start.elapsed());
                    break;
                }
            }
            match arrived {
                Some(took) => println!(
                    "  {kind:>9}: {:>4}ms  {:>9} bytes from the pty",
                    took.as_millis(),
                    tab.output_count()
                ),
                None => println!("  {kind:>9}: 時間切れ ({} bytes)", tab.output_count()),
            }
        }
    }

    /// Does what came out arrive in the order it went in?
    ///
    /// Read off the raw pipe rather than the screen: the sequences at issue --
    /// an image block, a Sixel, a hyperlink -- leave no text behind, so the
    /// only place their position is visible is the byte stream itself.
    #[test]
    #[ignore]
    fn what_came_out_is_still_in_order() {
        use portable_pty::{CommandBuilder, PtySize, native_pty_system};
        use std::io::Read as _;

        println!("{}", shikisha_core::conpty::report().line());
        let pair = native_pty_system()
            .openpty(PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 })
            .expect("openpty");
        let mut cmd = CommandBuilder::new(writer().display().to_string());
        cmd.arg("ordered");
        let mut child = pair.slave.spawn_command(cmd).expect("spawn");
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().expect("reader");
        let mut writer_side = pair.master.take_writer().expect("writer");

        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });

        let mut seen: Vec<u8> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => seen.extend_from_slice(&chunk),
                Err(_) => {
                    if child.try_wait().ok().flatten().is_some() {
                        break;
                    }
                }
            }
            // The console asks who it is talking to before it will go on. Not
            // answering is how a probe stops after forty bytes.
            if seen.windows(4).any(|w| w == b"\x1b[6n") {
                let _ = writer_side.write_all(b"\x1b[40;1R");
            }
        }
        let _ = child.kill();

        let text = String::from_utf8_lossy(&seen).to_string();
        let at = |needle: &str| text.find(needle);
        println!("  {} bytes", seen.len());
        for m in ["MARK1", "MARK2", "MARK3", "MARK4"] {
            println!("    {m}: {:?}", at(m));
        }
        for (name, needle) in [
            ("kitty APC", "\u{1b}_G"),
            ("Sixel DCS", "\u{1b}P"),
            ("OSC 8", "\u{1b}]8;;"),
        ] {
            println!("    {name}: {:?}", at(needle));
        }
        let marks: Vec<usize> = ["MARK1", "MARK2", "MARK3", "MARK4"]
            .iter()
            .filter_map(|m| at(m))
            .collect();
        println!(
            "    markers in order: {}",
            marks.len() == 4 && marks.windows(2).all(|w| w[0] < w[1])
        );
    }

    #[test]
    #[ignore]
    fn a_burst_of_japanese_reaches_the_window() {
        println!("{}", shikisha_core::conpty::report().line());
        let tab = Tab::spawn(
            "cmd.exe".into(),
            &["cmd.exe".into()],
            None,
            40,
            120,
            TabOptions::default(),
        )
        .expect("起動");
        settle(&tab, Duration::from_millis(600), Duration::from_secs(10));
        // The codepage is the one condition worth being able to change.
        //
        // The published figures had the older console inflating Japanese by
        // 35%; measured here with the console told to be UTF-8, the two send
        // byte-identical output. Which raises the obvious question, and
        // SHIKISHA_BENCH_CHCP=0 is how it gets asked: leave the console on
        // whatever codepage the machine boots with, and measure again.
        let utf8 = std::env::var("SHIKISHA_BENCH_CHCP").unwrap_or_else(|_| "1".into()) != "0";
        println!(
            "  codepage: {}",
            if utf8 { "65001, set here" } else { "left as the machine had it" }
        );
        if utf8 {
            tab.write_bytes(b"chcp 65001 >nul\r").expect("chcp");
            settle(&tab, Duration::from_millis(600), Duration::from_secs(10));
        }

        let writer = writer();
        for kind in ["poured", "redrawn", "sequences"] {
            measure(&tab, &writer, kind);
            settle(&tab, Duration::from_millis(400), Duration::from_secs(10));
        }
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
        use shikisha_shared::Ev;
        assert!(
            super::keys_for(&Ev::Closed).is_empty(),
            "閉じたことを打鍵として扱っている"
        );
        let src = include_str!("main.rs");
        assert!(
            src.contains("Ev::Closed => self.mail.closed = true"),
            "窓が閉じた報告を受けていない"
        );
    }
}

/// The window, seen as "a shell the runtime can be driven by".
///
/// Every one of these already existed; the trait is what lets the loop be
/// written once, for this window and for no window at all.
impl shikisha_core::host::Shell for WinSurface {
    fn mail(&mut self) -> &mut shikisha_core::mailbox::Mailbox {
        &mut self.mail
    }

    fn confirm_quit(&mut self, busy: usize) -> bool {
        quit_confirmed(busy)
    }

    fn install_store_update(&mut self, version: &str) -> Result<()> {
        shikisha_core::update::store::install(browser::main_hwnd(), version.to_string());
        Ok(())
    }

    fn take_keyboard_back(&self) { WinSurface::take_keyboard_back(self) }
    fn geom_area(&self) -> (i32, i32, i32, i32) { WinSurface::geom_area(self) }
    fn geom_full(&self) -> (i32, i32, i32, i32) { WinSurface::geom_full(self) }
    fn geom_rows(&self) -> u16 { WinSurface::geom_rows(self) }
    fn geom_cols(&self) -> u16 { WinSurface::geom_cols(self) }
    fn geom_panes(&self) -> &[shikisha_shared::PaneGeom] { WinSurface::geom_panes(self) }
    fn phone_size(&self) -> Option<(u16, u16)> { WinSurface::phone_size(self) }
    fn set_phone_size(&mut self, size: Option<(u16, u16)>) { WinSurface::set_phone_size(self, size) }
    fn is_hidden(&self) -> bool { WinSurface::is_hidden(self) }
    fn last_drawn(&self) -> Option<&shikisha_core::uistate::UiState> { WinSurface::last_drawn(self) }
    fn queue_input(&mut self, ev: Event) { WinSurface::queue_input(self, ev) }
    fn inject(&mut self, ev: Event) { WinSurface::inject(self, ev) }
    fn toggle_tab_bar(&self) { WinSurface::toggle_tab_bar(self) }
    fn take_open_settings( &mut self, ) -> Option<(Option<String>, bool, Option<String>, Option<u32>)> { WinSurface::take_open_settings(self) }
    fn open_vault(&self) { WinSurface::open_vault(self) }
    fn open_palette(&self) { WinSurface::open_palette(self) }
    fn push_git(&self, json: &str) { WinSurface::push_git(self, json) }
    fn push_sftp(&self, json: &str) { WinSurface::push_sftp(self, json) }
    fn push_recorded(&self, line_json: &str) { WinSurface::push_recorded(self, line_json) }
    fn queue_vault(&mut self, ev: shikisha_shared::Ev) { WinSurface::queue_vault(self, ev) }
    fn push_suggested(&self, json: &str) { WinSurface::push_suggested(self, json) }
    fn push_surveyed(&self, json: &str) { WinSurface::push_surveyed(self, json) }
    fn push_lua_done(&self, err_json: &str) { WinSurface::push_lua_done(self, err_json) }
    fn push_actions(&self, actions_json: &str) { WinSurface::push_actions(self, actions_json) }
    fn push_theme(&self) { WinSurface::push_theme(self) }
    fn hide(&mut self) { WinSurface::hide(self) }
    fn show(&mut self) { WinSurface::show(self) }
    fn say_where_it_went(&self) { WinSurface::say_where_it_went(self) }
    fn size(&self) -> Result<Size> { WinSurface::size(self) }
    fn poll(&mut self, timeout: Duration, active_tab: Option<&Tab>) -> Result<Option<Event>> { WinSurface::poll(self, timeout, active_tab) }
    fn host(&self) -> Option<(std::rc::Rc<dyn shikisha_shared::BrowserHost>, (i32, i32, i32, i32))> { WinSurface::host(self) }
    fn ask_password(&mut self, title: &str, note: &str) -> Result<Option<String>> { WinSurface::ask_password(self, title, note) }
    fn draw(&mut self, tabs: &[Tab], ui: &Ui, flash: Option<&str>) -> Result<()> { WinSurface::draw(self, tabs, ui, flash) }
}

/// from the notification area with the window put away
fn quit_confirmed(busy: usize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW,
    };
    if busy == 0 {
        return true;
    }
    let body = wide(&i18n::tp("msg.quit.busy", &[("n", &busy.to_string())]));
    let title = wide("SHIKISHA-TERM");
    let answer = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND | MB_TOPMOST,
        )
    };
    let yes = answer == IDYES;
    append_hook_log(&format!("Quit asked with {busy} tab(s) at work: {}", if yes { "yes" } else { "no" }));
    yes
}


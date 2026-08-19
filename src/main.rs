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

mod ball;
mod bridge;
mod browser;
mod caps;
mod config;
mod crypto;
mod detect;
mod exchange;
mod hooks;
mod i18n;
mod netaddr;
mod notify;
mod profile;
mod remote;
mod session_log;
mod shell;
mod tab;
mod uistate;
mod update;
mod watch;
mod ws;
mod webui;
mod wspack;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};


use detect::TabState;
use hooks::{Command, HookEngine, TabCtx};
use tab::{CopyState, Tab, extract_text};
use unicode_width::UnicodeWidthStr as _;

const TAB_BAR_MIN: u16 = 10;
const TAB_BAR_MAX: u16 = 40;
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
    // Decide where WebView2's user data (cookies, cache, etc.) lives, based on config.
    // Default is local (%LOCALAPPDATA%) — putting it in a Drive-synced folder would cause
    // the cache to sync endlessly and trigger notifications/conflicts. This must be set
    // before the first WebView is created.
    unsafe {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", config::browser_data_dir());
    }
    // Decide the display language (config, then OS; falls back to English if untranslated)
    i18n::init(
        config::load().and_then(|c| c.language).as_deref(),
        &[config_file_dir(), std::path::PathBuf::from(".")],
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
/// the subtraction became a *second* one: every AI was handed ~`tab_w` fewer
/// columns than it had, rendering into only part of the width with a wide blank
/// margin on the right — and on a phone-narrow screen, where the total column
/// count is barely above `tab_w`, it collapsed almost to nothing.
fn pty_dims(size: Size, _tab_w: u16) -> (u16, u16) {
    (size.height.max(3), size.width.max(10))
}

/// Auto-computes the tab bar width to fit the tab names.
/// Finds the width (accounting for full-width chars) needed for "[x] 12. tab-name 🔒" and clamps it to range.
fn auto_tab_width(tabs: &[Tab]) -> u16 {
    let longest = tabs
        .iter()
        .map(|t| {
            // 4-digit indicator + "N. " + name + indent + 2-digit lock + 1-char border
            4 + 4 + t.title.width() as u16 + t.depth + 2 + 1
        })
        .max()
        .unwrap_or(TAB_BAR_MIN);
    longest.clamp(TAB_BAR_MIN, TAB_BAR_MAX)
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

/// The set of things needed to draw into our own window
struct WinSurface {
    win: std::rc::Rc<crate::browser::Browser>,
    rows: u16,
    cols: u16,
    /// The last state we sent. Only send again when it changes.
    last: Option<crate::uistate::UiState>,
    last_screen: String,
    /// The last cursor (row, col, shown) we sent. Placing the cursor forces the
    /// page to recompute layout, so re-sending an unchanged one every frame kept
    /// the WebView busy at ~60Hz for nothing. Only send again when it moves.
    last_cursor: Option<(u16, u16, bool)>,
    /// The content area (x, y, width, height). Where the browser gets placed.
    area: (i32, i32, i32, i32),
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
    /// The sidebar gear was pressed. The loop opens the settings page (from any tab).
    open_settings: bool,
    /// Lines typed into a model tab's chat box, awaiting delivery to the bridge.
    chats: Vec<String>,
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
    fn take_close_settings(&mut self) -> bool {
        std::mem::take(&mut self.close_settings)
    }

    /// True if the settings gear was pressed (and clears the flag if so)
    fn take_open_settings(&mut self) -> bool {
        std::mem::take(&mut self.open_settings)
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
    fn take_chats(&mut self) -> Vec<String> {
        std::mem::take(&mut self.chats)
    }

    fn take_events(&mut self, active_tab: Option<&Tab>) {
        use crate::browser::Ev;
        for ev in self.win.drain() {
            match ev {
                Ev::Resize { rows, cols, area } => {
                    self.rows = rows;
                    self.cols = cols;
                    self.area = area;
                    self.pending.push_back(Event::Resize(cols, rows));
                }
                Ev::JsError { msg } => {
                    crate::append_hook_log(&format!("Screen failure: {msg}"));
                }
                // The window was closed. If we don't shut down here, a process with
                // nowhere left to draw stays alive unseen, still holding the listening port.
                Ev::Closed => self.closed = true,
                // The settings page's "close settings" button. Where the tab actually
                // gets torn down (caps, active) isn't touched here — that's left to the loop.
                Ev::CloseSettings => self.close_settings = true,
                Ev::OpenSettings => self.open_settings = true,
                // The top bar was pressed. The destination is "whatever page is currently
                // showing", so the loop decides (only one bar is ever displayed).
                Ev::Go { go } => self.gos.push(go),
                Ev::Scroll { by, row, col } => self.scrolls.push((by, row, col)),
                Ev::Chat { text } => self.chats.push(text),
                Ev::Where {
                    from: Some(name),
                    url,
                    can_back,
                    can_forward,
                } => self.wheres.push((name, url, can_back, can_forward)),
                // The bar on a placed page was pressed = a human finished their turn.
                // Who pressed it can only be told from the name attached to the report.
                Ev::Button { from: Some(name) } => self.presses.push(name),
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

/// Converts an intent from the screen into keystrokes the loop already understands.
///
/// The window and the phone use the same page. If there were two separate places
/// doing this conversion, the same press could end up meaning different things
/// depending on which one it came from.
/// Intents that can't be converted to a keystroke (load-complete, resize, etc.) return empty.
fn keys_for(ev: &crate::browser::Ev) -> Vec<Event> {
    use crate::browser::Ev;
    let plain = |c: char| Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    let prefixed = |c: char| {
        vec![
            Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            plain(c),
        ]
    };
    match ev {
        // "I want to look at this tab" is the same thing as Ctrl+B <digit>
        Ev::Select { tab } if *tab <= 9 => {
            prefixed(char::from_digit(*tab as u32, 10).unwrap_or('0'))
        }
        // The tab bar's + is prefixed so it works no matter which tab is showing
        Ev::AddTab => prefixed('t'),
        // The board's menu is a plain keystroke while looking at INDEX.
        // Adding the prefix key would mean only characters present on both sides work.
        Ev::Menu { key } => key.chars().next().map(plain).map(|k| vec![k]).unwrap_or_default(),
        // The workspace-switcher button. Prefixed (Ctrl+B w) so it opens the
        // list no matter which tab is showing — a bare 'w' would be typed into
        // the visible session instead (the old Menu "w" bug: "wwww").
        Ev::OpenWs => prefixed('w'),
        Ev::Stop => prefixed('x'),
        Ev::Key { text, named, ctrl } => {
            if let Some(n) = named {
                named_key(n)
                    .map(|code| vec![Event::Key(KeyEvent::new(code, KeyModifiers::NONE))])
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
        _ => Vec::new(),
    }
}

/// Converts a control key sent by name into the terminal's key type
fn named_key(n: &str) -> Option<KeyCode> {
    Some(match n {
        "enter" => KeyCode::Enter,
        "bs" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "esc" => KeyCode::Esc,
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
    let page = shell::page("");
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
        last: None,
        last_screen: String::new(),
        last_cursor: None,
        area: (0, 0, 0, 0),
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
        open_settings: false,
        chats: Vec::new(),
    })
}

/// Summarizes the current state into a form with no presentation attached
fn ui_state_of(tabs: &[Tab], ui: &Ui, flash: Option<&str>) -> crate::uistate::UiState {
    crate::uistate::UiState {
        workspace: ui
            .ws_names
            .get(ui.ws_index)
            .cloned()
            .unwrap_or_default(),
        workspaces: ui.ws_names.clone(),
        ws_index: ui.ws_index,
        active: ui.active,
        auto_enabled: ui.auto.unwrap_or(true),
        remote_on: ui.remote_on,
        first_run: ui.first_run,
        // Keep the order exactly as written in the config.
        // Listing sessions and browsers separately would push the browser
        // written first to the back.
        tabs: ui
            .panes
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match p {
                Pane::Session(s) => tabs.get(*s).map(|t| crate::uistate::TabState::of(i + 1, t)),
                Pane::Browser { key, name } => {
                    Some(crate::uistate::TabState::browser(i + 1, key, name))
                }
            })
            .collect(),
        // The ball moves by session number; what we display is the screen number
        ball: crate::uistate::BallState::of(&ui.ball, ui.max_chain, ui.now_ms),
        flash: flash.map(str::to_string),
        help_open: ui.help_open,
        ws_open: ui.ws_open,
        qr: ui.qr.clone(),
        // Build the image just once here. Both the window and the phone read the
        // same state, so the same QR shows up regardless of origin (this ends the
        // link-rot we used to get back when it was served as a separate image).
        qr_svg: ui.qr.as_deref().map(|u| crate::netaddr::qr_svg(u, 6)),
        nav: ui.nav.clone(),
        scrolled: ui.scrolled,
        build: format!("build {}  ({})", env!("BUILD_TIME"), env!("BUILD_REV")),
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
            let anyone_active = ui.panes.iter().any(|p| match p {
                Pane::Session(s) => tabs
                    .get(*s)
                    .map(|t| t.ms_since_change(ui.now_ms) < QUIET_MS)
                    .unwrap_or(false),
                Pane::Browser { .. } => false,
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
                // Only send the terminal contents for the tab currently being viewed
                if let Some(t) = session_at(&ui.panes, ui.active).and_then(|i| tabs.get(i)) {
                    let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                    let s = p.screen();
                    let html = crate::shell::screen_html(s);
                    if html != w.last_screen {
                        w.last_screen = html.clone();
                        // The screen was redrawn (new content, or a switched tab),
                        // so re-place the cursor once even if its row/col is the same.
                        w.last_cursor = None;
                        let _ = w.win.eval(&format!(
                            "return window.__screen({});",
                            serde_json::to_string(&html).unwrap_or_default()
                        ));
                    }
                    let (r, c) = s.cursor_position();
                    let on = !s.hide_cursor();
                    // Placing the cursor forces a layout recompute in the page,
                    // so only do it when the cursor actually moved — not 60x a
                    // second onto an unchanged position.
                    if w.last_cursor != Some((r, c, on)) {
                        w.last_cursor = Some((r, c, on));
                        let _ = w
                            .win
                            .eval(&format!("return window.__cursor({r},{c},{on});"));
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
    let mut tab_w = 18u16;
    let (mut rows, mut cols) = pty_dims(surface.size()?, tab_w);

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
        spawn_workspace(w, rows, cols, &mut tabs, &mut startup_errors);
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

    // Finalize the width now that all tab names are known, and re-fit the PTY size
    tab_w = match cfg.as_ref().and_then(|c| c.tab_bar_width) {
        Some(w) => w.clamp(TAB_BAR_MIN, TAB_BAR_MAX),
        None => auto_tab_width(&tabs),
    };
    (rows, cols) = pty_dims(surface.size()?, tab_w);
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
    // (encrypted-secret keys get unlocked here too)
    if let Some(c) = &cfg {
        if password.is_some() {
            bridge::set_providers(c, password.as_deref());
            // Tabs already spawned (before the prompt) with keys that couldn't be
            // decrypted yet. Now that the providers hold the real keys, refresh
            // each model tab so it stops sending an empty bearer token (→ 401).
            for t in &mut tabs {
                t.refresh_model_conn();
            }
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
            notify::Notifier::new(dests)
        }
        None => notify::Notifier::new(Default::default()),
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
        open_declared_browsers(w, &caps, &mut startup_errors);
    } else {
        engines[0] = build_engine(cfg.as_ref(), None, &mut startup_errors, &caps);
    }
    let slot = ws_index.min(engines.len().saturating_sub(1));
    let mut engine = engines[slot].take();

    // Remote UI (monitor/control from a phone, etc). Only starts listening when
    // enabled in config. Status is also handed to the settings page so the QR code
    // can be viewed in a browser.
    let remote_info: Arc<Mutex<webui::RemoteInfo>> = Arc::new(Mutex::new(Default::default()));
    let mut remote_ui = start_remote(cfg.as_ref(), password.as_deref(), &mut startup_errors);
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
    let mut started_fired = vec![false; tabs.len()];
    // The "invisible ball" of the automation chain. Used in the display to show
    // which tab currently holds the work.
    let mut ball = ball::Ball::default();
    // Holding area for hand-offs the recipient can't accept yet
    let mut waiting: Vec<Waiting> = Vec::new();
    // A reservation to send submit (Enter) later, for text that's already been sent
    let mut pending_submit: Vec<PendingSubmit> = Vec::new();
    // Tabs that look like they've finished responding, and the time that gets confirmed.
    // We hold off firing until we've verified it stayed quiet, so we don't fire on a
    // mid-response pause for breath.
    let mut pending_done: Vec<(usize, u64)> = Vec::new();
    // Whether to follow the ball by switching screens
    let mut follow_ball = cfg.as_ref().and_then(|c| c.follow_ball).unwrap_or(true);
    // The last time a human touched the screen. Don't auto-follow right after that.
    let mut view_touched_ms: u64 = 0;
    // Where we last auto-followed to. Remembered so we don't jump to the same place repeatedly.
    let mut followed: usize = 0;
    // Clickable spots on INDEX. Rebuilt every frame at draw time.

    // 0 = INDEX, 1.. = sessions. Start on INDEX (the screen with onboarding guidance) at first.
    let mut active: usize = if tabs.is_empty() || first_run { 0 } else { 1 };
    let mut prefix_active = false;
    // The last state drawn. This is what gets handed to the phone (keeps the
    // assembly point to a single spot).
    let mut last_ui_state: Option<crate::uistate::UiState> = None;
    // What we last pushed to remote viewers over the state socket, so we only
    // send on change. The screen is also rate-limited (see below) so a burst of
    // AI output doesn't flood a slow phone link the way pushing every frame would.
    let mut last_remote_ui: Option<String> = None;
    let mut last_remote_screen = String::new();
    let mut last_remote_push = Instant::now() - Duration::from_secs(1);
    // Whether an overlaid browser is currently being shown. Leaving it up would
    // permanently hide the terminal, so it's hidden by default.
    let mut flash: Option<String> = startup_errors
        .first()
        .map(|e| i18n::tp("msg.startup_failed", &[("error", e)]))
        .or_else(|| plaintext_secrets_warning(cfg.as_ref()));
    let mut last_detect = Instant::now() - Duration::from_secs(1);
    // The browser currently being screen-relayed (only streams while someone's watching)
    let mut casting: Option<String> = None;
    // Workspaces use a virtual-desktop model: switching means hiding, not stopping.
    // Each workspace keeps its own set of tabs, launched the first time it's activated.
    // Launched tabs live in `tabs`; the shelf reserves space for the remaining workspaces.
    let mut ws_tabs: Vec<Vec<Tab>> = Vec::new();
    ws_tabs.resize_with(workspaces.len(), Vec::new);
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

    loop {
        // What's laid out on screen, in the order written in config.
        // The upper bound of pressable numbers needs more than just the session count.
        let hosted = caps.hosted_names();
        let titles: Vec<&str> = tabs.iter().map(|t| t.title.as_str()).collect();
        let layout = panes_of(workspaces.get(ws_index), &titles, &hosted);
        let panes = layout.len();
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
                workspaces = new_ws;
                max_chain = newcfg.max_chain.unwrap_or(10);
                follow_ball = newcfg.follow_ball.unwrap_or(true);
                done_confirm_ms = newcfg
                    .done_confirm_ms
                    .unwrap_or(profile::DEFAULT_DONE_CONFIRM_MS);
                if let Some(w) = newcfg.tab_bar_width {
                    let w = w.clamp(TAB_BAR_MIN, TAB_BAR_MAX);
                    if w != tab_w {
                        tab_w = w;
                        (rows, cols) = pty_dims(surface.size()?, tab_w);
                        for t in &tabs {
                            let _ = t.resize(rows, cols);
                        }
                    }
                }
                // Rebuild notification destinations, capabilities, and automation scripts
                let (dests, err) = newcfg.resolve_notify(password.as_deref());
                if let Some(e) = err {
                    startup_errors.push(e);
                }
                notifier = notify::Notifier::new(dests);
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
                if (want.enabled, &want.bind, want.port, want.allow_public)
                    != (now.enabled, &now.bind, now.port, now.allow_public)
                {
                    if let Some(r) = &remote_ui {
                        r.shutdown();
                    }
                    remote_ui = start_remote(Some(&newcfg), password.as_deref(), &mut startup_errors);
                    publish_remote(&remote_info, &remote_ui);
                    // Fresh server = fresh viewers; forget what the old one pushed.
                    last_remote_ui = None;
                    last_remote_screen = String::new();
                    remote_changed = Some(if remote_ui.is_some() {
                        i18n::t("msg.remote_enabled")
                    } else {
                        i18n::t("msg.remote_stopped")
                    });
                }
                cfg = Some(newcfg);
                // Re-resolve the model bridge's connection info (picks up providers/secret changes)
                if let Some(c) = &cfg {
                    bridge::set_providers(c, password.as_deref());
                }
                watcher.retarget(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
                let mut note = remote_changed.unwrap_or(msg);
                if lang_restart {
                    note.push_str(&i18n::t("msg.lang_restart"));
                }
                flash = Some(format!(">> {note}"));
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
            for (i, t) in tabs.iter_mut().enumerate() {
                if t.needs_restart
                    && t.state != TabState::Busy
                    && t.restart(rows, cols).is_ok()
                {
                    if let Some(f) = started_fired.get_mut(i) {
                        *f = false;
                    }
                }
            }

            // Fire hooks -> resume waiting coroutines -> run the queued operations
            if let Some(eng) = engine.as_mut() {
                // Let the loop read the current state (shikisha.state)
                eng.set_states(
                    tabs.iter()
                        .map(|t| (t.key(), t.state.label().to_string()))
                        .collect(),
                );
                // Discard waiting loops belonging to exited tabs (don't leave infinite loops behind)
                for &(idx, old, new) in &transitions {
                    if new == TabState::Exited && old != TabState::Exited {
                        eng.cancel_tab(pane_at(&layout, idx));
                    }
                }
                let now_ms = start.elapsed().as_millis() as u64;
                if auto_enabled {
                    for (i, fired) in started_fired.iter_mut().enumerate() {
                        // Sending right after launch gets dropped, since the AI CLI
                        // hasn't drawn its input box yet. Wait until it's ready before
                        // flushing it in.
                        if !*fired && tabs[i].ready_for_startup_hook() {
                            *fired = true;
                            eng.fire(
                                "on_start",
                                &tab_ctx(&tabs[i], pane_at(&layout, i + 1)),
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
                            "State tab{idx} {}->{} [{}] prompted={} working={} answered={} submit_pending={}",
                            old.label(),
                            new.label(),
                            t.profile_name(),
                            t.was_prompted(),
                            t.saw_working_flag(),
                            t.answered_since_submit(),
                            pending_submit.iter().any(|p| p.tab == idx)
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
                        let ctx = tab_ctx(&tabs[idx - 1], pane_at(&layout, idx));
                        // Even just the startup banner's output makes the screen move
                        // then settle, so every tab is guaranteed to pass through DONE
                        // once with nobody having asked anything. To avoid forwarding
                        // that output as a response, only treat it as one once there's
                        // been input. A tab where submit (Enter) hasn't arrived yet is
                        // merely showing a pasted draft. Going quiet doesn't make that a response.
                        let submitting = pending_submit.iter().any(|p| p.tab == idx);
                        // If nothing came out after submit, it never arrived.
                        // Don't read a screen that's just showing the pasted draft as a response.
                        let answering = tabs[idx - 1].was_prompted()
                            && !submitting
                            && tabs[idx - 1].answered_since_submit();
                        match new {
                            TabState::Busy if answering => eng.fire("on_busy", &ctx, None),
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
                        let ctx = tab_ctx(&tabs[idx - 1], pane_at(&layout, idx));
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

                    // Automation addresses things by screen number; the contents live in sessions
                    eng.tick_pending(&|pane| {
                        session_at(&layout, pane)
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
                        &layout,
                        max_chain,
                        auto_enabled,
                        now_ms,
                        rows,
                        cols,
                        &notifier,
                        &mut flash,
                        &mut ball,
                        &mut pending_submit,
                        &mut waiting,
                        &mut active,
                        settings_open,
                    );
                }
            }

            // Hand the current status to the remote UI and run any operations it sent
            if let Some(r) = remote_ui.as_ref() {
                let snap = remote::Snapshot {
                    // Pass along what was built at draw time. `ui` doesn't exist here yet,
                    // and rebuilding it would create a second place that assembles state.
                    ui: last_ui_state.clone(),
                    screen_html: tabs
                        .get(session_at(&layout, active).unwrap_or(usize::MAX))
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
                    tabs: tabs
                        .iter()
                        .enumerate()
                        .map(|(i, t)| remote::RemoteTab {
                            index: i + 1,
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
                }
                *r.snapshot.lock().unwrap() = snap;
            }

            // auto_restart: automatically bring exited tabs back
            for (i, t) in tabs.iter_mut().enumerate() {
                if t.state == TabState::Exited && t.auto_restart {
                    match t.restart(rows, cols) {
                        Ok(()) => {
                            append_hook_log(&format!("auto-restart tab{}", i + 1));
                            flash = Some(i18n::tp("msg.restarted", &[("name", &t.title)]));
                        }
                        Err(e) => flash = Some(i18n::tp("msg.restart_failed", &[("error", &t.launch_hint(&e.to_string()))])),
                    }
                }
            }
        }

        // Process remote operations and frame delivery every iteration (waiting 200ms
        // would let finger-swipe traces bunch up and arrive all at once, breaking swipe playback)
        if let Some(r) = remote_ui.as_ref() {
            let now_ms = start.elapsed().as_millis() as u64;
            // The browser currently being viewed (target for Inject / relay)
            let shown_browser = match layout.get(active.wrapping_sub(1)) {
                Some(Pane::Browser { key, .. }) => Some(key.clone()),
                _ => None,
            };
            while let Ok(cmd) = r.rx.try_recv() {
                match cmd {
                    // Treat input from remote as a human operation
                    // (resets the auto-chain, and is rejected while locked)
                    remote::RemoteCmd::Send { tab, text } => {
                        if let Some(t) = session_at(&layout, tab).and_then(|i| tabs.get_mut(i)) {
                            if t.locked {
                                continue;
                            }
                            t.chain_depth = 0;
                            t.last_manual_ms = Some(now_ms);
                            let seen = t.output_count();
                            write_prompt(t, &text);
                            pending_submit.push(PendingSubmit::new(tab, seen, now_ms));
                            append_hook_log(&format!(
                                "remote send tab{tab}: {}",
                                log_excerpt(&text, 120)
                            ));
                        }
                    }
                    remote::RemoteCmd::Keys { tab, keys } => {
                        if let Some(t) = session_at(&layout, tab).and_then(|i| tabs.get_mut(i)) {
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
                    // Convert other screen operations into the same keystrokes that come from the window
                    remote::RemoteCmd::Ui(ev) => {
                        for e in keys_for(&ev) {
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
            let keys = pane_keys(&layout, &tabs);
            let mut ready: Vec<Command> = Vec::new();
            let mut keep: Vec<Waiting> = Vec::new();
            for w in std::mem::take(&mut waiting) {
                let can = target_of(&w.cmd)
                    .and_then(|r| r.resolve(&keys))
                    .and_then(|p| session_at(&layout, p))
                    .and_then(|i| tabs.get(i))
                    .map(ready_to_receive)
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
                    &layout,
                    max_chain,
                    auto_enabled,
                    now_ms,
                    rows,
                    cols,
                    &notifier,
                    &mut flash,
                    &mut ball,
                    &mut pending_submit,
                    &mut waiting,
                    &mut active,
                    settings_open,
                );
            }
        }

        // Send the reserved submit (Enter) once the recipient has drawn the pasted text
        if !pending_submit.is_empty() {
            let now_ms = start.elapsed().as_millis() as u64;
            pending_submit.retain_mut(|p| {
                let Some(t) = session_at(&layout, p.tab).and_then(|i| tabs.get(i)) else {
                    return false;
                };
                if !p.ready(t.output_count(), now_ms) {
                    return true;
                }
                let settled = now_ms < p.give_up;
                let _ = t.write_bytes(b"\r");
                append_hook_log(&format!(
                    "submit tab{} ({})",
                    p.tab,
                    if settled { "after intake finished" } else { "sent while still unsettled" }
                ));
                false
            });
        }

        // Move the screen to wherever the ball was passed.
        // Don't follow right after a human touches the screen (so they're not
        // yanked away mid-read).
        {
            let now_ms = start.elapsed().as_millis() as u64;
            if let Some(to) = follow_target(
                follow_ball,
                ball.holder,
                followed,
                // Count against what's laid out on screen. Counting sessions instead
                // would make tabs behind any browsers look like "numbers that don't
                // exist", and the ball passed there would never be followed.
                layout.len(),
                now_ms,
                view_touched_ms,
            ) {
                followed = to;
                // Don't follow while the settings overlay is up — the human is
                // reading settings, not spectating the ball. (followed is still
                // advanced above, so we don't re-jump the instant it closes.)
                if active != to && !settings_open {
                    // Keep this so "it was passed but the screen didn't move" can be
                    // traced. This was removed once during cleanup, and that exact
                    // investigation got stuck because of it.
                    append_hook_log(&format!("Following tab{active} -> tab{to}"));
                    active = to;
                }
            }
        }

        // chain_depth resets to 0 when a human types. Make the ball follow that too
        // (checked from the holder's side, so we don't need to add more places that reset it).
        // Don't clear a ball that's waiting on a human here. Even if the chain has
        // ended, the work still belongs to the holder. It gets cleared on the
        // touched side once a human touches it.
        if ball.holder > 0
            && !ball.awaiting_human
            && !session_at(&layout, ball.holder)
                .and_then(|i| tabs.get(i))
                .map(|t| t.chain_depth > 0)
                .unwrap_or(false)
        {
            ball.reset();
        }
        ball.clamp_to(layout.len());

        // The controls shown over the browser being viewed.
        //
        // Whether to show them is decided by config or Lua; whether they're pressable
        // is answered by the window. The answer arrives with a delay, so show them
        // looking unpressable until it comes in.
        let drawn_ms = start.elapsed().as_millis() as u64;
        let showing = match layout.get(active.wrapping_sub(1)) {
            Some(Pane::Browser { key, .. }) => Some(key.clone()),
            _ => None,
        };
        let nav = showing.as_deref().and_then(|key| {
            let spec = caps.nav_of(key)?;
            let w = where_now.as_ref().filter(|w| w.0 == key);
            Some(crate::uistate::NavState {
                back: spec.back,
                forward: spec.forward,
                reload: spec.reload,
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
                let pane = pane_of_id(w, first)?;
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
            auto: engine.as_ref().map(|_| auto_enabled),
            ws_names: workspaces.iter().map(|w| w.name.clone()).collect(),
            ws_index,
            ws_open,
            help_open,
            qr: if qr_open { remote_ui.as_ref().map(|r| r.url.clone()) } else { None },
            remote_on: remote_ui.is_some(),
            nav,
            scrolled: session_at(&layout, active)
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
            panes: layout.clone(),
            discuss_start,
            discuss_start_name,
        };
        last_ui_state = Some(ui_state_of(&tabs, &ui, flash.as_deref()));
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
            let want = match layout.get(active.wrapping_sub(1)) {
                Some(Pane::Browser { key, .. }) => Some(key.clone()),
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
        caps.show_only(match layout.get(active.wrapping_sub(1)) {
            Some(Pane::Browser { key, .. }) => Some(key.as_str()),
            _ => None,
        });
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
                .zip(page_ctx(&layout, &name, String::new(), true))
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
            if let Some(t) = session_at(&layout, active).and_then(|i| tabs.get(i)) {
                scroll_by(t, by, row, col);
                // If the screen jumps while scrolling back, you lose track of what you were reading
                view_touched_ms = start.elapsed().as_millis() as u64;
            }
        }

        // Lines typed into a model tab's chat box. Deliver each to whichever
        // model tab is in view; a line typed while a non-model tab is up is
        // dropped (the box is only shown over model tabs anyway).
        for line in surface.take_chats() {
            if let Some(t) = session_at(&layout, active).and_then(|i| tabs.get_mut(i)) {
                if t.is_model() {
                    // A line typed by the human is a fresh turn (chain 0), so a
                    // rally brain resets its per-goal budget instead of treating
                    // it as more of the automated chain.
                    t.chain_depth = 0;
                    t.chat_send(line);
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
            active = 0;
        }

        // The sidebar gear. Opens settings from any tab (the menu "e" key only
        // fires while INDEX is in view, so the gear needs its own path).
        if surface.take_open_settings() {
            flash = Some(
                match open_settings(&mut web, &config_file, &remote_info, &web_password, &caps, "") {
                    Ok(()) => {
                        active = settings_active(&layout);
                        settings_open = true;
                        i18n::t("msg.settings_here")
                    }
                    Err(e) => i18n::tp("msg.settings_failed", &[("error", &e.to_string())]),
                },
            );
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
                    Ok(()) => active = placed_active(&layout, RESULT_TAB),
                    Err(e) => append_hook_log(&format!("open_result failed: {e}")),
                }
            }
        }

        // The top bar was pressed. The destination is whatever page is currently
        // viewed (only one bar is ever shown). Don't touch chain depth — that's
        // only counted when work is passed to another tab.
        for go in surface.take_gos() {
            let Some(Pane::Browser { key, .. }) = layout.get(active.wrapping_sub(1)) else {
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
                    (engine.as_mut(), page_ctx(&layout, &name, url, complete))
                {
                    eng.fire_page("on_load", &page);
                }
            }
        }

        let polled = surface.poll(
            Duration::from_millis(16),
            session_at(&layout, active).and_then(|i| tabs.get(i)),
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
                                    rows,
                                    cols,
                                    &mut startup_errors,
                                    &mut started_fired,
                                    cfg.as_ref(),
                                    &mut engine,
                                    &mut engines,
                                    &caps,
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
                if prefix_active {
                    prefix_active = false;
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char(c @ '0'..='9') => {
                            let n = c as usize - '0' as usize;
                            if n <= panes {
                                active = n;
                                view_touched_ms = start.elapsed().as_millis() as u64;
                                // An explicit tab pick is a deliberate exit from settings.
                                settings_open = false;
                            }
                        }
                        KeyCode::Char('n') => {
                            active = if active >= panes { 0 } else { active + 1 };
                            view_touched_ms = start.elapsed().as_millis() as u64;
                        }
                        KeyCode::Char('p') => {
                            active = if active == 0 { panes } else { active - 1 };
                            view_touched_ms = start.elapsed().as_millis() as u64;
                        }
                        // Ctrl+B b sends a literal Ctrl+B through to the child process
                        KeyCode::Char('b') => {
                            if let Some(t) = session_mut(&mut tabs, &layout, active) {
                                t.write_bytes(&[0x02])?;
                            }
                        }
                        // Ctrl+B r restarts this tab (recovers from exit/disconnect)
                        KeyCode::Char('r') => {
                            if let Some(eng) = engine.as_mut() {
                                eng.cancel_tab(active);
                            }
                            if let Some(t) = session_mut(&mut tabs, &layout, active) {
                                flash = Some(match t.restart(rows, cols) {
                                    Ok(()) => i18n::tp("msg.restarted", &[("name", &t.title)]),
                                    Err(e) => i18n::tp("msg.restart_failed", &[("error", &t.launch_hint(&e.to_string()))]),
                                });
                            }
                        }
                        // Ctrl+B l toggles the input lock / w workspace list / ? help
                        KeyCode::Char('l') => {
                            if let Some(t) = session_mut(&mut tabs, &layout, active) {
                                t.locked = !t.locked;
                                flash = Some(i18n::t(if t.locked {
                                    "msg.lock_on"
                                } else {
                                    "msg.lock_off"
                                }));
                            }
                        }
                        KeyCode::Char('w') => {
                            if workspaces.len() > 1 {
                                ws_open = true;
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
                                    rows,
                                    cols,
                                    &mut startup_errors,
                                    &mut started_fired,
                                    cfg.as_ref(),
                                    &mut engine,
                                    &mut engines,
                                    &caps,
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
                                        active = settings_active(&layout);
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
                            // If a submit reservation is left over, only the Enter
                            // would arrive after stopping.
                            pending_submit.clear();
                            // Discard every waiting loop too (don't let them revive on resume)
                            if let Some(eng) = engine.as_mut() {
                                eng.cancel_all();
                            }
                            flash =
                                Some(i18n::t("msg.emergency_stop"));
                        }
                        // Ctrl+B c copies the latest captured response to the clipboard
                        KeyCode::Char('c') => {
                            if let Some(t) = session_mut(&mut tabs, &layout, active) {
                                flash = Some(match &t.last_response {
                                    Some(r) if !r.trim().is_empty() => copy_to_clipboard(r),
                                    _ => i18n::t("msg.no_response"),
                                });
                            }
                        }
                        // Ctrl+B [ enters copy mode (tmux copy-mode style)
                        KeyCode::Char('[') => {
                            let rows = pty_dims(surface.size()?, tab_w).0;
                            if let Some(t) = session_mut(&mut tabs, &layout, active) {
                                t.copy = Some(CopyState {
                                    cursor_row: rows.saturating_sub(1),
                                    anchor: None,
                                });
                            }
                        }
                        _ => {}
                    }
                } else if key.code == KeyCode::Char('b')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    prefix_active = true;
                } else if active == 0 {
                    // INDEX = home screen: digit keys switch tabs, letter keys run menu items.
                    // Characters received here must line up with MENU_KEYS
                    // (prevents a case where the board shows something that does nothing when pressed)
                    match key.code {
                        KeyCode::Char(c @ '0'..='9') => {
                            let n = c as usize - '0' as usize;
                            if n <= panes {
                                active = n;
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
                            if workspaces.len() > 1 {
                                ws_open = true;
                            }
                        }
                        KeyCode::Char('r') => {
                            let mut msgs = Vec::new();
                            for t in tabs.iter_mut().filter(|t| t.state == TabState::Exited) {
                                match t.restart(rows, cols) {
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
                        KeyCode::Char('e') => {
                            flash = Some(
                                match open_settings(&mut web, &config_file, &remote_info, &web_password, &caps, "")
                                {
                                    Ok(()) => {
                                        // Once opened, switch to that tab.
                                        // Don't leave it opened but invisible.
                                        // If already open, switch to its existing location.
                                        active = settings_active(&layout);
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
                        KeyCode::Char('q') => break,
                        _ => {}
                    }
                    // INDEX-END (a test checks whether keys the board offers are received here)
                } else {
                    let size = surface.size()?;
                    let now_ms = start.elapsed().as_millis() as u64;
                    let mut locked_hit = false;
                    if let Some(t) = session_mut(&mut tabs, &layout, active) {
                        if t.copy.is_some() {
                            handle_copy_key(t, &key, size, tab_w, &mut flash)?;
                        } else if t.locked {
                            // Soft lock: viewing and copying still work, but input is ignored
                            locked_hit = true;
                        } else if let Some(bytes) = key_to_bytes(&key) {
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
                if let Some(t) = session_mut(&mut tabs, &layout, active) {
                    if !t.locked {
                        t.chain_depth = 0;
                        t.last_manual_ms = Some(now_ms);
                        to_live(t);
                        t.write_bytes(text.as_bytes())?;
                    }
                }
            }
            Event::Resize(width, height) => {
                (rows, cols) = pty_dims(Size { width, height }, tab_w);
                for t in &tabs {
                    let _ = t.resize(rows, cols);
                }
            }
            _ => {}
        }
    }

    if let Some(w) = &web {
        w.shutdown();
    }
    if let Some(r) = &remote_ui {
        r.shutdown();
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

/// A reservation to send submit (Enter) after the text body has been sent.
///
/// Writing it all in one go means the Enter arrives before the AI CLI has
/// finished taking in the paste and gets dropped. But using a fixed wait time
/// instead is just guessing "how many seconds the recipient takes to process
/// it", and breaks the moment device, load, or body length changes.
/// The signal to use instead is the recipient having drawn the paste
/// (= having produced output).
struct PendingSubmit {
    tab: usize,
    /// The cumulative output amount last seen. While it keeps increasing, intake is still in progress.
    seen: u64,
    /// The point output stopped (None = hasn't stopped yet)
    quiet_since: Option<u64>,
    /// The earliest time submission is allowed, to prevent sending too early
    not_before: u64,
    /// The time to give up and send anyway, if things never settle
    give_up: u64,
}

impl PendingSubmit {
    fn new(tab: usize, seen: u64, now_ms: u64) -> Self {
        Self {
            tab,
            seen,
            quiet_since: None,
            not_before: now_ms + SUBMIT_FLOOR_MS,
            give_up: now_ms + SUBMIT_GIVE_UP_MS,
        }
    }

    /// Whether it's okay to send submit (Enter) to this tab right now.
    ///
    /// What we wait for is "intake finished", not "response started". A long
    /// paste keeps redrawing over several round trips, so sending as soon as it
    /// starts arrives mid-intake and gets dropped (measured: around 600 chars
    /// goes through, around 1900 chars fails).
    fn ready(&mut self, output_count: u64, now_ms: u64) -> bool {
        if output_count != self.seen {
            // Still mid-intake. Restart the measurement from when it stops.
            self.seen = output_count;
            self.quiet_since = None;
        } else if self.quiet_since.is_none() {
            self.quiet_since = Some(now_ms);
        }
        if now_ms < self.not_before {
            return false;
        }
        let settled = self
            .quiet_since
            .is_some_and(|q| now_ms.saturating_sub(q) >= SUBMIT_QUIET_MS);
        settled || now_ms >= self.give_up
    }
}

/// Sends just the text body to the prompt. The caller sends submit a little later.
fn write_prompt(t: &Tab, text: &str) {
    let bracketed = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste();
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    let mut bytes = Vec::new();
    if bracketed {
        // If bracketed paste is supported, multi-line text still arrives as a single input
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(body.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
    } else {
        bytes.extend_from_slice(body.as_bytes());
    }
    let _ = t.write_bytes(&bytes);
}

/// The name used when placing the settings page inside the window.
/// If the spelling drifts, it gets treated as a different browser and a second copy opens.
const SETTINGS_TAB: &str = "settings";

/// The name used when placing the result view (finished discussion / review /
/// rally, rendered as a chat) inside the window. Unlike settings it *does* show
/// in the tab strip, and it is reused (re-pointed) on each new result rather
/// than piling up copies.
const RESULT_TAB: &str = "result";

/// The screen number (1-based) to switch to for a placed local page (settings
/// or result). If already open, its own slot; otherwise the slot right after
/// the end (`layout.len() + 1`). Using `len()+1` while it is already in the
/// layout would point one slot too far and paint the screen solid black.
fn placed_active(layout: &[Pane], key_want: &str) -> usize {
    layout
        .iter()
        .position(|p| matches!(p, Pane::Browser { key, .. } if key == key_want))
        .map(|i| i + 1)
        .unwrap_or(layout.len() + 1)
}

fn settings_active(layout: &[Pane]) -> usize {
    placed_active(layout, SETTINGS_TAB)
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
/// The automation assignment for a tab. Either a file/directory, or the built-in browser-driving agent.
#[derive(Debug, PartialEq)]
enum TabAuto {
    /// An automation path (a directory or .lua file)
    Path(String),
    /// Browser-driving mode. Built in; the value is the id of the browser being driven.
    Agent(String),
}

/// Returns the screen number (1-based) of the tab in a workspace whose id
/// (or name, if no id) matches. Used to resolve discussion participants/referee
/// from a tab id to a screen number.
fn pane_of_id(ws: &config::Workspace, id: &str) -> Option<usize> {
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
        // `drives` (browser-driving mode) takes priority over `automation`; the built-in commander runs
        if let Some(br) = t.cfg.drives.clone().filter(|s| !s.trim().is_empty()) {
            out.push((pane, TabAuto::Agent(br)));
        } else if let Some(p) = t.cfg.automation_path() {
            out.push((pane, TabAuto::Path(p)));
        }
    }
    out
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
    if base.is_none() && ws_lua.is_none() && tab_luas.is_empty() && !has_discuss && !wants_notify {
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
    // `drives` and `discuss` can't coexist on the same tab (each one claims that
    // tab's automation). If both are written, `discuss` takes priority and
    // browser-driving mode is disabled with a warning.
    let discuss_panes: std::collections::HashSet<usize> = ws
        .and_then(|w| {
            w.discuss.as_ref().map(|d| {
                d.agents
                    .iter()
                    .chain(d.judge.iter())
                    .chain(d.moderator.iter())
                    .filter(|s| !s.trim().is_empty())
                    .filter_map(|id| pane_of_id(w, id))
                    .collect()
            })
        })
        .unwrap_or_default();
    for (idx, auto) in &tab_luas {
        let id = match auto {
            TabAuto::Path(p) => load(&mut engine, p, errors),
            // Browser-driving mode: loads the built-in commander for the target browser.
            // But if the same tab is also a discussion participant, discuss wins and this is disabled.
            TabAuto::Agent(br) if discuss_panes.contains(idx) => {
                errors.push(crate::i18n::tp(
                    "err.ws.agent_mode_and_discuss",
                    &[("idx", &idx.to_string())],
                ));
                None
            }
            TabAuto::Agent(br) => match engine.load_browser_agent(br, &stops_lua) {
                Ok(id) => Some(id),
                Err(e) => {
                    errors.push(crate::i18n::tp(
                        "err.ws.agent_mode_failed",
                        &[("br", br), ("e", &format!("{e:#}"))],
                    ));
                    None
                }
            },
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
                    let Some(pane) = pane_of_id(w, id) else {
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
                    match pane_of_id(w, j) {
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
                    match pane_of_id(w, m) {
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
    // notification: the on_done detection loop lives behind `Some(engine)`, so
    // without this a notify-only workspace would never detect "done" at all.
    (!engine.is_empty() || wants_notify).then_some(engine)
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
    drives: Option<&str>,
) -> Vec<String> {
    if let Some(mut conn) = bridge::launch_for(&argv) {
        if let (Some(d), Some(id)) = (ws.and_then(|w| w.discuss.as_ref()), id) {
            conn.persona = d.personas.get(id).filter(|p| !p.trim().is_empty()).cloned();
        }
        // A model tab that `drives` a browser is a rally brain: it steers the
        // browser by emitting Lua in its reply instead of chatting.
        conn.drives = drives
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        opts.model = Some(conn);
    }
    argv
}

fn tab_options(cfg: &config::TabConfig) -> tab::TabOptions {
    tab::TabOptions {
        // Relative paths are resolved against the config file's location (so the whole folder is portable)
        cwd: cfg.cwd.as_ref().map(|c| {
            let p = std::path::PathBuf::from(c);
            if p.is_absolute() {
                p
            } else {
                config_file_dir().join(p)
            }
        }),
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
        let mut opts = tab_options(&ft.cfg);
        let argv = resolve_launch(
            argv,
            &mut opts,
            Some(ws),
            ft.cfg.id.as_deref(),
            ft.cfg.drives.as_deref(),
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
            None => match Tab::spawn(title.clone(), &argv, ft.cfg.profile.clone(), rows, cols, opts) {
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
        );
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
            );
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

fn spawn_workspace(
    ws: &config::Workspace,
    rows: u16,
    cols: u16,
    tabs: &mut Vec<Tab>,
    errors: &mut Vec<String>,
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
        let mut opts = tab_options(&ft.cfg);
        let argv = resolve_launch(
            argv,
            &mut opts,
            Some(ws),
            ft.cfg.id.as_deref(),
            ft.cfg.drives.as_deref(),
        );
        let cwd = opts.cwd.clone();
        match Tab::spawn(
            title.clone(),
            &argv,
            ft.cfg.profile.clone(),
            rows,
            cols,
            opts,
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
    rows: u16,
    cols: u16,
    errors: &mut Vec<String>,
    started_fired: &mut Vec<bool>,
    cfg: Option<&config::Config>,
    engine: &mut Option<HookEngine>,
    engines: &mut [Option<HookEngine>],
    caps: &hooks::Caps,
) {
    // Guard against every backing array, not just `workspaces`: the per-workspace
    // `engines`/`ws_tabs` caches are resized on config reload, and a mismatch must
    // never index out of bounds (that would crash the whole app on switch).
    if to == *ws_index
        || to >= workspaces.len()
        || to >= engines.len()
        || to >= ws_tabs.len()
        || *ws_index >= ws_tabs.len()
    {
        return;
    }
    ws_tabs[*ws_index] = std::mem::take(tabs);
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
        spawn_workspace(&workspaces[to], rows, cols, tabs, errors);
        open_declared_browsers(&workspaces[to], caps, errors);
    }
    *engine = match engines[to].take() {
        Some(e) => Some(e),
        None => build_engine(cfg, workspaces.get(to), errors, caps),
    };
    started_fired.clear();
    started_fired.resize(tabs.len(), false);
    *active = if tabs.is_empty() { 0 } else { 1 };
}

/// What's laid out on screen, in exactly the order written in config.
///
/// Keeping sessions and browsers as separate variants is purely an internal
/// concern; it has nothing to do with whoever wrote the config.
#[derive(Clone, Debug, PartialEq)]
enum Pane {
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
fn panes_of(ws: Option<&config::Workspace>, titles: &[&str], hosted: &[String]) -> Vec<Pane> {
    let mut out: Vec<Pane> = Vec::new();
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
                out.push(Pane::Browser { key, name });
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
                out.push(Pane::Session(i));
            }
        }
    }
    // Things not written in config
    for (i, used) in used_tabs.iter().enumerate() {
        if !used {
            out.push(Pane::Session(i));
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
            out.push(Pane::Browser {
                key: h.clone(),
                name,
            });
        }
    }
    out
}

/// Looks up a session's location from its screen number (1-based)
fn session_at(panes: &[Pane], active: usize) -> Option<usize> {
    match panes.get(active.checked_sub(1)?)? {
        Pane::Session(i) => Some(*i),
        Pane::Browser { .. } => None,
    }
}

/// Looks up the screen number (1-based) from a session's location.
/// The ball moves by session number, so route it through here when displaying it.
fn pane_at(panes: &[Pane], session: usize) -> usize {
    panes
        .iter()
        .position(|p| *p == Pane::Session(session.wrapping_sub(1)))
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// The session currently being viewed. None if viewing a browser.
fn session_mut<'a>(tabs: &'a mut [Tab], panes: &[Pane], active: usize) -> Option<&'a mut Tab> {
    let i = session_at(panes, active)?;
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
pub fn remote_token(cfg: &config::Config, password: Option<&str>) -> String {
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
fn start_remote(
    cfg: Option<&config::Config>,
    password: Option<&str>,
    errors: &mut Vec<String>,
) -> Option<remote::RemoteUi> {
    let c = cfg.filter(|c| c.remote.enabled)?;
    match netaddr::resolve_bind(&c.remote.bind, c.remote.allow_public) {
        Ok((ip, note)) => {
            let token = remote_token(c, password);
            match remote::RemoteUi::start(ip, c.remote.port, token) {
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
            }
        }
        Err(e) => {
            errors.push(crate::i18n::tp(
                "err.ws.remote_ui",
                &[("e", &e.to_string())],
            ));
            None
        }
    }
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
fn random_hex(bytes: usize) -> String {
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
    panes: &[Pane],
    key: &str,
    url: String,
    complete: bool,
) -> Option<hooks::PageCtx> {
    panes.iter().enumerate().find_map(|(i, p)| match p {
        Pane::Browser { key: k, name } if k == key => Some(hooks::PageCtx {
            index: i + 1,
            id: k.clone(),
            name: name.clone(),
            url: url.clone(),
            complete,
        }),
        _ => None,
    })
}

fn tab_ctx(t: &Tab, index: usize) -> TabCtx {
    TabCtx {
        index,
        name: t.title.clone(),
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

/// Grace period holding off auto-submit right after manual input (avoids keystroke cross-talk)
const MANUAL_GUARD_MS: u64 = 5000;

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

fn follow_target(
    enabled: bool,
    holder: usize,
    already: usize,
    tab_count: usize,
    now_ms: u64,
    view_touched_ms: u64,
) -> Option<usize> {
    if !enabled || holder == 0 || holder == already || holder > tab_count {
        return None;
    }
    (now_ms.saturating_sub(view_touched_ms) >= FOLLOW_GUARD_MS).then_some(holder)
}

/// The delay after a human touches the screen before auto-follow resumes.
///
/// Getting yanked away mid-read is the worst outcome, so once someone touches
/// it, stay quiet and follow along for a while.
const FOLLOW_GUARD_MS: u64 = 8_000;

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
fn pane_keys(panes: &[Pane], tabs: &[Tab]) -> Vec<hooks::TabKey> {
    panes
        .iter()
        .map(|p| match p {
            Pane::Session(i) => tabs.get(*i).map(|t| t.key()).unwrap_or_default(),
            // Browsers give priority to the id too; still lookup-able by display name
            Pane::Browser { key, name } => hooks::TabKey {
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

/// Whether the recipient is in a state where it can accept input
fn ready_to_receive(t: &Tab) -> bool {
    t.ready_for_startup_hook()
}

/// How long to hold before giving up. Whoever wrote it isn't watching anymore by the time this long has passed.
const WAIT_FOR_TAB_MS: u64 = 30_000;

/// Executes the operation requests queued by Lua hooks.
/// Auto-sends inherit chain depth (the invisible ball) and stop once the cap is hit.
#[allow(clippy::too_many_arguments)]
fn exec_commands(
    cmds: Vec<Command>,
    tabs: &mut [Tab],
    panes: &[Pane],
    max_chain: u32,
    auto_enabled: bool,
    now_ms: u64,
    rows: u16,
    cols: u16,
    notifier: &notify::Notifier,
    flash: &mut Option<String>,
    ball: &mut ball::Ball,
    pending_submit: &mut Vec<PendingSubmit>,
    waiting: &mut Vec<Waiting>,
    active: &mut usize,
    // When true, the settings overlay is showing; ShowTab is ignored so
    // automation can't pull the screen off settings.
    settings_open: bool,
) {
    let keys = pane_keys(panes, tabs);
    let index_of = |r: &hooks::TabRef| r.resolve(&keys);
    // From a screen number to its location in the tabs array. None for a browser.
    let session_of = |pane: usize| session_at(panes, pane);
    for cmd in cmds {
        // If the recipient can't accept input yet, hold onto it and deliver it later.
        // Sending it now would be silently dropped, invisible to whoever wrote it.
        if can_wait(&cmd) {
            let not_yet = target_of(&cmd)
                .and_then(index_of)
                .and_then(session_of)
                .and_then(|i| tabs.get(i))
                .map(|t| !ready_to_receive(t))
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
                if settings_open {
                    // The human is in settings; don't yank them out of it.
                    append_hook_log(&format!(
                        "ShowTab {target:?} ignored: settings overlay is open"
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
            Command::Restart { target } => {
                let Some(target) = index_of(&target) else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                    continue;
                };
                if let Some(t) = session_of(target).and_then(|i| tabs.get_mut(i)) {
                    match t.restart(rows, cols) {
                        Ok(()) => {
                            append_hook_log(&format!("restart tab{target} (lua)"));
                            *flash = Some(i18n::tp("msg.restarted", &[("name", &t.title)]));
                        }
                        Err(e) => *flash = Some(i18n::tp("msg.restart_failed", &[("error", &t.launch_hint(&e.to_string()))])),
                    }
                }
            }
            Command::Notify { dest, text } => {
                append_hook_log(&format!("NOTIFY[{dest}] {text}"));
                *flash = Some(notifier.send(&dest, &text));
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
                    let mut bytes = Vec::with_capacity(text.len() + 12);
                    bytes.extend_from_slice(b"\x1b[200~");
                    bytes.extend_from_slice(text.as_bytes());
                    bytes.extend_from_slice(b"\x1b[201~");
                    let _ = t.write_bytes(&bytes);
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
                    write_prompt(t, &text);
                    pending_submit.push(PendingSubmit::new(target, seen, now_ms));
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
    tab_w: u16,
    flash: &mut Option<String>,
) -> Result<()> {
    let (rows_v, cols_v) = pty_dims(size, tab_w);
    let Some(mut cs) = t.copy.take() else {
        return Ok(());
    };
    let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
    let cur = p.screen().scrollback();
    let mut keep = true;
    match key.code {
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

/// Mouse handling: click a tab bar entry to switch / wheel scroll / select-to-copy instantly / right-click to paste
#[allow(clippy::too_many_arguments)]












/// UI state needed for drawing
struct Ui {
    /// First-ever run, before config exists (shows onboarding on INDEX)
    first_run: bool,
    active: usize,
    auto: Option<bool>,
    ws_names: Vec<String>,
    ws_index: usize,
    ws_open: bool,
    help_open: bool,
    /// The connection URL, if the QR code is being shown
    qr: Option<String>,
    /// Whether the remote UI is listening (shown at all times so it's never a mystery)
    remote_on: bool,
    /// Where the auto-chain currently is (the invisible ball, made visible)
    ball: ball::Ball,
    /// The chain cap. Represents how close the ball's color is to that cap.
    max_chain: u32,
    /// Draw timestamp (relative ms). Used to drive the ball's animation.
    now_ms: u64,
    /// What's laid out on screen, in the order written in config
    panes: Vec<Pane>,
    /// If the current workspace is a discussion, the opening speaker's session
    /// number (1-based) and display name — for the dashboard's "start" card
    discuss_start: Option<usize>,
    discuss_start_name: Option<String>,
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
        let evs = super::keys_for(&crate::browser::Ev::AddTab);
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
            Command::Restart { target: TabRef::Index(1) },
            Command::Notify { dest: "slack".into(), text: "x".into() },
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
        let layout = vec![
            Pane::Browser { key: "html".into(), name: "HTML解析".into() },
            Pane::Session(0),
        ];
        let page = page_ctx(&layout, "html", "https://example.com/".into(), true)
            .expect("並びにあるのに見つからない");
        assert_eq!(page.index, 1, "画面の番号と違う");
        assert_eq!(page.id, "html", "自動化から指す呼び名が違う");
        assert_eq!(page.name, "HTML解析", "人が読む名前が出ていない");
        assert!(page.complete);

        // Nothing is passed for a page not in the layout (e.g. after it's closed)
        assert!(page_ctx(&layout, "shop", String::new(), true).is_none());
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
        assert_eq!(pane_of_id(&ws, "ai1"), Some(1));
        assert_eq!(pane_of_id(&ws, "ai2"), Some(2));
        assert_eq!(pane_of_id(&ws, "ref"), Some(3));
        assert_eq!(pane_of_id(&ws, "いない"), None);
        // Also lookup-able by name
        assert_eq!(pane_of_id(&ws, "審判"), Some(3));
    }

    /// A tab with `drives` (browser-driving mode) written on it must be assigned the built-in agent
    #[test]
    fn a_tab_with_drives_uses_the_builtin_browser_agent() {
        let mut ws = ws_from(&[
            ("エージェント", "ai", "claude"),
            ("ページ", "br", "browser https://example.com/"),
        ]);
        ws.tabs[0].cfg.drives = Some("br".into());

        let got = automation_by_pane(&ws);
        assert_eq!(
            got,
            vec![(1, TabAuto::Agent("br".to_string()))],
            "drives のタブが内蔵エージェントに割り当たっていない"
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

        let panes = panes_of(Some(&ws), &tabs, &hosted);
        assert_eq!(
            panes,
            vec![Pane::Browser { key: "html".into(), name: "HTML解析".into() }, Pane::Session(0)],
            "設定の順に並んでいない"
        );
        // A session must be resolvable from its screen number
        assert_eq!(session_at(&panes, 1), None, "1番はブラウザのはず");
        assert_eq!(session_at(&panes, 2), Some(0));
        // The ball moves by session number; what's displayed is the screen number
        assert_eq!(pane_at(&panes, 1), 2);
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
                }
            })
            .collect();
        config::Workspace {
            name: "試験".into(),
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
        let panes = panes_of(Some(&ws), &tabs, &[]);
        assert_eq!(
            panes,
            vec![Pane::Browser { key: "html".into(), name: "HTML解析".into() }, Pane::Session(0)],
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
        let panes = panes_of(Some(&ws), &tabs, &hosted);
        assert_eq!(
            panes,
            vec![
                Pane::Session(0),
                Pane::Session(1),
                Pane::Browser { key: "settings".into(), name: "settings".into() }
            ]
        );
    }

    /// The number that switches to the settings tab must point at its
    /// existing location if it's already open.
    ///
    /// Using `layout.len() + 1` points one slot too far, since settings is
    /// already in the layout — this used to leave the screen solid black when pressed
    /// (this happens when pressing "add tab" while settings is already open).
    #[test]
    fn settings_active_points_at_the_open_settings_tab() {
        // Not open yet: points to the slot right after the end
        let before = vec![Pane::Session(0), Pane::Session(1)];
        assert_eq!(settings_active(&before), 3, "開く前は末尾の次");

        // Already open: points to its existing location (the end). Not one slot further.
        let after = vec![
            Pane::Session(0),
            Pane::Session(1),
            Pane::Browser { key: "settings".into(), name: "settings".into() },
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

        write_prompt(&t, "echo shikisha-ok");
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

    /// Submit (Enter) must wait until paste intake has "finished".
    ///
    /// Sending it the moment it "starts" gets a long paste dropped mid-intake.
    /// Measured: around 600 chars goes through, around 1900 chars fails.
    #[test]
    fn the_enter_waits_for_the_paste_to_finish_being_taken_in() {
        let mut p = PendingSubmit::new(1, 100, 1_000);

        // Never send while output is still moving, no matter how long it's been
        assert!(!p.ready(100, 1_000), "送った瞬間");
        assert!(!p.ready(200, 1_100), "反応が始まっただけでは送らない");
        assert!(!p.ready(300, 2_000), "まだ増えている");
        assert!(!p.ready(400, 3_000), "まだ増えている");

        // Only send once it's stopped and stayed quiet for a while
        assert!(!p.ready(400, 3_100), "止まった直後はまだ");
        assert!(!p.ready(400, 3_100 + SUBMIT_QUIET_MS - 1), "静かな時間が足りない");
        assert!(p.ready(400, 3_100 + SUBMIT_QUIET_MS), "落ち着いたら送る");

        // Restart the measurement if activity resumes partway through.
        // The quiet period starts not at "the moment it stopped" but "the first time we noticed it had stopped".
        let mut p = PendingSubmit::new(1, 0, 0);
        assert!(!p.ready(0, 100), "静かだがまだ足りない");
        assert!(!p.ready(50, 200), "再開したので測り直す");
        assert!(!p.ready(50, 300), "ここで改めて静止を観測");
        assert!(!p.ready(50, 300 + SUBMIT_QUIET_MS - 1), "測り直し中");
        assert!(p.ready(50, 300 + SUBMIT_QUIET_MS), "改めて落ち着いた");

        // Send anyway once the cap is hit, even if it never settles
        let mut p = PendingSubmit::new(1, 0, 0);
        let mut out = 0;
        for t in (100..SUBMIT_GIVE_UP_MS).step_by(100) {
            out += 1;
            assert!(!p.ready(out, t), "増え続けている間は待つ ({t}ms)");
        }
        out += 1;
        assert!(p.ready(out, SUBMIT_GIVE_UP_MS), "上限に達したら送る");
    }

    /// The view must follow the ball, but yield to a human operating it.
    #[test]
    fn the_view_follows_the_ball_but_yields_to_the_person() {
        let g = FOLLOW_GUARD_MS;

        // Moves to wherever the ball was passed
        assert_eq!(follow_target(true, 2, 1, 3, g, 0), Some(2));
        // Doesn't jump to the same place repeatedly
        assert_eq!(follow_target(true, 2, 2, 3, g, 0), None);
        // Doesn't move if nobody holds it
        assert_eq!(follow_target(true, 0, 1, 3, g, 0), None);
        // Doesn't go to a tab that doesn't exist (e.g. right after a workspace switch)
        assert_eq!(follow_target(true, 5, 1, 3, g, 0), None);
        // Doesn't move if disabled in config
        assert_eq!(follow_target(false, 2, 1, 3, g, 0), None);

        // Doesn't follow right after a human touches the screen (don't yank them away mid-read)
        assert_eq!(follow_target(true, 2, 1, 3, 1_000, 1_000), None);
        assert_eq!(follow_target(true, 2, 1, 3, 1_000 + g - 1, 1_000), None);
        // Follows again once enough time has passed
        assert_eq!(follow_target(true, 2, 1, 3, 1_000 + g, 1_000), Some(2));
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

    /// The ball must still be followable even in a layout with a browser mixed in.
    ///
    /// The ball moves by screen number. If the count used the session number
    /// instead, tabs behind however many browsers there are would look like
    /// "numbers that don't exist", and a ball passed there would never be
    /// followed again.
    /// (With the layout Analysis=1 browser / AI=2 session, passing to AI didn't move the screen.)
    #[test]
    fn a_browser_in_the_row_does_not_hide_the_tabs_behind_it() {
        let panes = vec![
            Pane::Browser { key: "html".into(), name: "解析".into() },
            Pane::Session(0),
        ];
        // Only one session. Counting against the wrong thing gets it rejected by 2 > 1.
        assert_eq!(
            follow_target(true, 2, 0, panes.len(), FOLLOW_GUARD_MS, 0),
            Some(2),
            "ブラウザの後ろのタブへ追従できていない"
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
        spawn_workspace(&ws0, 24, 80, &mut tabs, &mut errs);
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



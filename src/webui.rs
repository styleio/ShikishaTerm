//! Local web GUI for settings. DESIGN.md sections 5.5 / 10.2.
//!
//! Security:
//!   - Binds to 127.0.0.1 only (unreachable from outside, no firewall warning either)
//!   - Random port + one-time token per launch
//!   - The token is verified both in the URL and the request header, preventing
//!     other processes on the same PC or a malicious web page (CSRF / DNS rebinding) from operating the config API
//!   - Host header is restricted to the 127.0.0.1 family (DNS rebinding countermeasure)
//!   - The master password is not handled here (kept entirely inside the TUI)

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use rand::TryRng as _;
use tiny_http::{Header, Response, Server};

/// Remote UI status passed to the settings screen (updated by the main app)
#[derive(Default, Clone)]
pub struct RemoteInfo {
    pub running: bool,
    pub url: String,
    /// Explanation for when it can't be enabled, or a note that needs attention
    pub note: String,
}

pub struct WebUi {
    pub url: String,
    stop: Arc<AtomicBool>,
}

impl WebUi {
    /// Starts a local server that edits the config file.
    /// config_path is the target being edited (usually config.json).
    /// password is a share of the master password held by the main app (used to encrypt secrets).
    /// **Never exposed to the page or the network** — used only on the server side, within the same process
    pub fn start_with(
        config_path: std::path::PathBuf,
        remote: Arc<std::sync::Mutex<RemoteInfo>>,
        password: Arc<std::sync::Mutex<Option<String>>>,
    ) -> Result<Self> {
        let token = random_token()?;
        let server = Server::http("127.0.0.1:0")
            .map_err(|e| anyhow::anyhow!(crate::i18n::tp("webui.err.server_start", &[("e", &e.to_string())])))?;
        let port = server
            .server_addr()
            .to_ip()
            .context(crate::i18n::t("webui.err.port"))?
            .port();
        let url = format!("http://127.0.0.1:{port}/?token={token}");
        let stop = Arc::new(AtomicBool::new(false));

        {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Err(e) = handle(req, &token, &config_path, &remote, &password) {
                        crate::append_hook_log(&format!("WebUI: {e}"));
                    }
                }
            });
        }
        Ok(Self { url, stop })
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 24];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|e| anyhow::anyhow!(crate::i18n::tp("webui.err.token", &[("e", &e.to_string())])))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Marks a request that reached this loopback server through the remote proxy —
/// i.e. the person operating it is on a phone, not at this PC. The proxy builds
/// a fresh request and never forwards the phone's own headers, so this can only
/// be set by us. Anything that would put a window on the PC's screen has to know
/// (see `NATIVE_DIALOG_PATHS`).
pub const REMOTE_CLIENT_HEADER: &str = "X-Remote-Client";

/// Endpoints whose whole job is to open a native dialog on this PC. From a phone
/// there is nobody standing at that screen to answer it, so the request would sit
/// there until it timed out — the app looking frozen from the phone's side. These
/// are refused outright when the caller is remote, and the buttons that call them
/// are left off the page.
const NATIVE_DIALOG_PATHS: [&str; 3] = [
    "/api/pick",
    "/api/workspace/export",
    "/api/workspace/import",
];

fn header_value(req: &tiny_http::Request, name: &'static str) -> String {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default()
}

/// Maximum accepted request-body size. Big enough for any real config/secrets
/// payload, small enough that a giant or slow body can't exhaust memory.
const MAX_BODY: usize = 1 << 20; // 1 MiB

/// Read a request body, capped at `max` bytes. Returns None if it would exceed
/// the cap (the caller answers 413), so an oversized body is never buffered.
fn read_body(req: &mut tiny_http::Request, max: usize) -> std::io::Result<Option<String>> {
    use std::io::Read as _;
    let mut body = String::new();
    req.as_reader().take(max as u64 + 1).read_to_string(&mut body)?;
    Ok((body.len() <= max).then_some(body))
}

fn query_token(url: &str) -> String {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("token="))
        .unwrap_or("")
        .to_string()
}

/// Reads one raw query-string value. Callers use known ASCII keys.
fn query_param(url: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    url.split_once('?')?
        .1
        .split('&')
        .find_map(|kv| kv.strip_prefix(&prefix).map(|v| v.to_string()))
}

/// Minimal percent-encoding for a query-string value.
fn pct(s: &str) -> String {
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

/// Opens a URL in the user's default browser instead of the in-app WebView.
/// Uses ShellExecuteW so the whole URL — query string, '&' and percent-escapes
/// included — is handed to the shell verbatim (explorer.exe mis-parses those
/// and falls back to opening a file window instead).
fn open_external(url: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let op: Vec<u16> = "open\0".encode_utf16().collect();
    let file: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// Best-effort Windows version string for a bug report (the field is optional).
fn windows_version() -> String {
    std::process::Command::new("cmd")
        .args(["/C", "ver"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// The GitHub "new bug report" URL, pre-filled with the build and OS so the
/// reporter does not have to dig those out by hand.
fn bug_report_url() -> String {
    let version = format!(
        "v{} (build {}, {})",
        env!("CARGO_PKG_VERSION"),
        env!("BUILD_TIME"),
        env!("BUILD_REV")
    );
    format!(
        "https://github.com/styleio/ShikishaTerm/issues/new?template=bug_report.yml&version={}&windows={}",
        pct(&version),
        pct(&windows_version())
    )
}

/// Runs `<prog> --help` and returns its output, for the settings "Show flags"
/// button. Asking the tool itself keeps the list accurate for whatever version
/// is installed, with no per-CLI knowledge to maintain. Bounded by a timeout so
/// a command that treats --help as "start" can't hang the settings page.
fn cli_help(prog: &str) -> Result<String, String> {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;
    if prog.trim().is_empty() {
        return Err(String::new());
    }
    // Resolve exactly like a tab launch: search PATH + .exe/.cmd/.bat and route
    // a .cmd/.bat shim (how npm installs claude/gemini/…) through cmd.exe. A bare
    // Command::new("claude") only looks for claude.exe and reports "not found".
    // An empty error string tells the page to show its own "is it installed?" note.
    let Some(path) = crate::tab::resolve_command(prog) else {
        return Err(String::new());
    };
    let is_script = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            e == "cmd" || e == "bat"
        })
        .unwrap_or(false);
    let mut cmd = if is_script {
        let mut c = Command::new("cmd.exe");
        c.arg("/c").arg(&path).arg("--help");
        c
    } else {
        let mut c = Command::new(&path);
        c.arg("--help");
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(cmd.output());
    });
    match rx.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(out)) => {
            let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
            // Some tools print their help to stderr instead.
            if s.trim().is_empty() {
                s = String::from_utf8_lossy(&out.stderr).into_owned();
            }
            Ok(s)
        }
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("timed out".into()),
    }
}

/// Safely resolves the ?file=... path.
/// Only allows .json files under the same directory as the config file,
/// and rejects absolute paths or parent-directory references (..) (path traversal countermeasure)
fn safe_workspace_path(
    url: &str,
    config_path: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let raw = url
        .split_once('?')?
        .1
        .split('&')
        .find_map(|kv| kv.strip_prefix("file="))?;
    let decoded = percent_decode(raw);
    let rel = std::path::Path::new(&decoded);
    if rel.is_absolute() {
        return None;
    }
    if rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::Prefix(_)))
    {
        return None;
    }
    if rel.extension().and_then(|e| e.to_str()) != Some("json") {
        return None;
    }
    let base = config_path.parent()?;
    Some(base.join(rel))
}

/// Safely resolves the automation folder path (version without extension checking)
fn safe_dir_path(url: &str, _config_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let raw = url
        .split_once('?')?
        .1
        .split('&')
        .find_map(|kv| kv.strip_prefix("dir="))?;
    let decoded = percent_decode(raw);
    let rel = std::path::Path::new(&decoded);
    if rel.is_absolute() || decoded.is_empty() {
        return None;
    }
    if rel.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::Prefix(_)
        )
    }) {
        return None;
    }
    // Uses the same resolution as the main app's automation loader. Prefers next to the exe over next to config.
    // If this drifts, you get "the GUI says it's unconfigured but it actually runs" / "editing in the GUI has no effect"
    Some(crate::resolve_data_path(&decoded))
}

/// The manual is embedded in the exe. It can always be referenced regardless of where it's
/// launched from, and won't break even if the docs are forgotten in the distribution.
/// To bundle a translation into the exe, add one line here (it's still read if placed in docs/ instead)
const EMBEDDED_MANUALS: &[(&str, &str)] = &[
    ("en", include_str!("../docs/AUTOMATION.md")),
    ("ja", include_str!("../docs/AUTOMATION.ja.md")),
];

/// Prefers a file placed alongside it, if any (so the user can add to it).
/// Looks in order: localized version (AUTOMATION.<code>.md) → English → embedded
fn load_manual(config_path: &std::path::Path) -> String {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(d) = config_path.parent() {
        dirs.push(d.to_path_buf());
    }
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
    {
        dirs.push(d);
    }
    dirs.push(std::path::PathBuf::from("."));

    let lang = crate::i18n::lang();
    let mut names = Vec::new();
    if lang != "en" {
        names.push(format!("AUTOMATION.{lang}.md"));
    }
    names.push("AUTOMATION.md".to_string());

    for name in &names {
        for d in &dirs {
            for rel in [d.join("docs").join(name), d.join(name)] {
                if let Ok(s) = std::fs::read_to_string(rel) {
                    if !s.trim().is_empty() {
                        return s;
                    }
                }
            }
        }
    }
    let embedded = |code: &str| EMBEDDED_MANUALS.iter().find(|(c, _)| *c == code).map(|(_, m)| *m);
    embedded(&lang).or_else(|| embedded("en")).unwrap_or_default().to_string()
}

/// Computes the remote UI status that should be displayed.
/// If the main app is listening, uses that info; otherwise builds the connection
/// info from config (so a QR code can still be shown when only settings is open, or right after enabling it)
fn effective_remote(shared: &Arc<std::sync::Mutex<RemoteInfo>>) -> RemoteInfo {
    let info = shared.lock().unwrap().clone();
    if info.running && !info.url.is_empty() {
        return info;
    }
    let Some(c) = crate::config::load().filter(|c| c.remote.enabled) else {
        return RemoteInfo::default();
    };
    match crate::netaddr::resolve_bind(&c.remote.bind, c.remote.allow_public) {
        Ok((ip, _)) => RemoteInfo {
            running: true,
            url: format!(
                "http://{ip}:{}/?t={}",
                c.remote.port,
                crate::remote_token(&c, None)
            ),
            note: crate::i18n::t("settings.phone.only_while_running"),
        },
        Err(e) => RemoteInfo {
            running: false,
            url: String::new(),
            note: e,
        },
    }
}

/// The connection info the phone card is drawn from: the stand-in laid out for
/// a promotional shot (`netaddr::demo_link`) when there is one, otherwise the
/// real thing. The card is drawn as running in that case — there is a link to
/// show, which is the only question this screen asks.
fn remote_for_display(shared: &Arc<std::sync::Mutex<RemoteInfo>>) -> (RemoteInfo, bool) {
    match crate::netaddr::demo_link() {
        Some(url) => (
            RemoteInfo {
                running: true,
                url,
                note: String::new(),
            },
            true,
        ),
        None => (effective_remote(shared), false),
    }
}


/// Brings our own process's dialog to the front when it appears.
/// Windows forbids background processes from popping themselves to the front on their own,
/// so we set the topmost attribute to keep it from hiding behind the browser
#[cfg(windows)]
fn raise_own_dialog() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    type BOOL = i32;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
        SetForegroundWindow, SetWindowPos, SwitchToThisWindow, HWND_TOPMOST, SWP_NOMOVE,
        SWP_NOSIZE, SWP_SHOWWINDOW,
    };

    struct Found {
        pid: u32,
        hwnd: HWND,
    }

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let found = unsafe { &mut *(lparam as *mut Found) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != found.pid || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        // Standard dialogs have the class name "#32770". Consoles are excluded
        let mut buf = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        if class == "#32770" {
            found.hwnd = hwnd;
            return 0;
        }
        1
    }

    let pid = std::process::id();
    std::thread::spawn(move || {
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut found = Found {
                pid,
                hwnd: std::ptr::null_mut(),
            };
            unsafe { EnumWindows(Some(cb), &mut found as *mut Found as LPARAM) };
            if !found.hwnd.is_null() {
                unsafe {
                    // Leave the topmost attribute set. It only lasts until the dialog closes,
                    // and removing it would let it hide behind the browser
                    SetWindowPos(
                        found.hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                    );
                    BringWindowToTop(found.hwnd);
                    // Foregrounding can be refused for a background process,
                    // so also use an Alt+Tab-equivalent switch
                    SwitchToThisWindow(found.hwnd, 1);
                    SetForegroundWindow(found.hwnd);
                }
                break;
            }
        }
    });
}

#[cfg(not(windows))]
fn raise_own_dialog() {}

/// Turns the chosen path into the form written to config.
/// If it's under the config folder, makes it relative so the whole folder stays portable
fn display_path(path: &std::path::Path, config_path: &std::path::Path) -> String {
    config_path
        .parent()
        .and_then(|base| path.strip_prefix(base).ok())
        .map(|rel| rel.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
        .replace('\\', "/")
}

/// The place to open first. ~/.ssh for a key, the config's location for a folder
fn default_pick_dir(kind: &str, config_path: &std::path::Path) -> Option<std::path::PathBuf> {
    if kind == "key" {
        if let Some(home) = std::env::var_os("USERPROFILE") {
            let ssh = std::path::PathBuf::from(&home).join(".ssh");
            if ssh.is_dir() {
                return Some(ssh);
            }
            return Some(std::path::PathBuf::from(home));
        }
    }
    config_path.parent().map(std::path::Path::to_path_buf)
}

/// Event files read/written from the settings screen.
/// Both the session's and the browser's
const EVENT_FILES: [&str; 8] = [
    "on_start",
    "on_done",
    "on_question",
    "on_exit",
    "on_busy",
    "on_load",
    "on_press",
    "_shared",
];

/// Runs a locally installed AI CLI one-shot to generate Lua code.
/// No API key needed (uses the user's subscription auth as-is).
/// The generated result is always shown on screen and never saved until the user approves it
/// Supported AI CLIs (name, non-interactive execution args, display name)
const AI_ENGINES: [(&str, &[&str], &str); 3] = [
    ("claude", &["-p"], "Claude Code"),
    ("codex", &["exec"], "Codex CLI"),
    ("gemini", &["-p"], "Gemini CLI"),
];

/// Turns the tab layout into descriptive text. Passed so the AI knows the destination number
/// (since the user should be able to give instructions by tab name)
fn describe_tabs(parsed: &serde_json::Value) -> String {
    let Some(tabs) = parsed.get("tabs").and_then(|v| v.as_array()) else {
        return String::new();
    };
    if tabs.is_empty() {
        return String::new();
    }
    let me = parsed.get("self").and_then(|v| v.as_u64()).unwrap_or(0);
    let mut s = crate::i18n::t("ai.tabs.header");
    s.push('\n');
    for t in tabs {
        let i = t.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            s.push_str(&format!("{i}. {name}"));
        } else {
            // If there's an ID, use it for addressing instead (survives tab renames)
            s.push_str(&format!(
                "{i}. {name}{}",
                crate::i18n::tp("ai.tabs.id", &[("id", id)])
            ));
        }
        if i == me {
            s.push_str(&crate::i18n::t("ai.tabs.self"));
        }
        s.push('\n');
    }
    s
}

fn generate_with_local_ai(
    event: &str,
    want: &str,
    layout: &str,
    engine: Option<&str>,
    config_path: &std::path::Path,
) -> Result<String> {
    if want.trim().is_empty() {
        anyhow::bail!("{}", crate::i18n::t("automation.editor.want"));
    }
    // Pass the manual to the AI as a spec (the custom API isn't in its training data)
    let manual = load_manual(config_path);

    // Fix the output format with markers so it doesn't just reply with conversational text
    let prompt = crate::i18n::tp(
        "ai.prompt",
        &[
            ("event", event),
            ("want", want),
            ("layout", layout),
            ("manual", &manual),
        ],
    );

    let (cmd, args) = pick_local_ai(engine)?;
    let mut spawner = std::process::Command::new(&cmd);
    spawner
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Inheriting the console here too would kill the mouse (same reason as open_browser)
    let mut child = crate::detach_console(&mut spawner)
        .spawn()
        .with_context(|| crate::i18n::tp("ai.err.cannot_run", &[("cmd", &cmd)]))?;
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().context(crate::i18n::t("webui.err.stdin"))?;
        stdin.write_all(prompt.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        anyhow::bail!(
            "{}",
            crate::i18n::tp(
                "ai.err.failed",
                &[("cmd", &cmd), ("error", String::from_utf8_lossy(&out.stderr).trim())]
            )
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    extract_lua(&text)
}

/// Have the assistant AI untangle one conflicted file.
///
/// It is handed the file exactly as git left it -- both sides and the markers --
/// and gives back what the file should be. Nothing here decides that the answer
/// is right: it is written to the working tree, where the person reads it as a
/// diff and stages it, or throws it away. Proposing is the AI's half.
pub fn resolve_conflict(name: &str, body: &str, engine: Option<&str>) -> Result<String> {
    if !body.contains("<<<<<<<") {
        anyhow::bail!("{}", crate::i18n::tp("err.git.no_markers", &[("file", name)]));
    }
    // A file with a conflict in it is usually ordinary in size; one that is not
    // is not something to send half of, because half a file written back is
    // worse than the conflict
    const ROOM: usize = 60_000;
    if body.chars().count() > ROOM {
        anyhow::bail!("{}", crate::i18n::tp("err.git.too_big", &[("file", name)]));
    }
    let prompt = crate::i18n::tp("ai.resolve.prompt", &[("file", name), ("body", body)]);
    let said = ask_local_ai(&prompt, engine)?;
    let text = strip_fence(&said);
    if text.trim().is_empty() {
        anyhow::bail!("{}", crate::i18n::tp("err.git.no_answer", &[("file", name)]));
    }
    if text.contains("<<<<<<<") || text.contains(">>>>>>>") {
        anyhow::bail!("{}", crate::i18n::tp("err.git.markers_left", &[("file", name)]));
    }
    Ok(text)
}

/// What a model says, minus the way models like to wrap it. A fenced block is
/// taken as the answer; anything outside one is dropped, because a file that
/// begins "Here is the resolved version:" does not compile
fn strip_fence(said: &str) -> String {
    let text = said.trim();
    let Some(start) = text.find("```") else {
        return text.to_string();
    };
    let after = &text[start + 3..];
    // The rest of the fence line is the language, not content
    let body = match after.find('\n') {
        Some(nl) => &after[nl + 1..],
        None => "",
    };
    match body.rfind("```") {
        Some(end) => body[..end].trim_end_matches('\n').to_string(),
        None => body.to_string(),
    }
}

/// Run the assistant AI once with this prompt on its standard input.
///
/// The three things that ask an AI something -- write me Lua, suggest me a
/// command, write me a commit message -- differ in what they ask and in
/// nothing else. Spawning it lived in each of them until there were three
pub fn ask_local_ai(prompt: &str, engine: Option<&str>) -> Result<String> {
    let (cmd, args) = pick_local_ai(engine)?;
    let mut spawner = std::process::Command::new(&cmd);
    spawner
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Inheriting the console here would kill the mouse (same reason as open_browser)
    let mut child = crate::detach_console(&mut spawner)
        .spawn()
        .with_context(|| crate::i18n::tp("ai.err.cannot_run", &[("cmd", &cmd)]))?;
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().context(crate::i18n::t("webui.err.stdin"))?;
        stdin.write_all(prompt.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        anyhow::bail!(
            "{}",
            crate::i18n::tp(
                "ai.err.failed",
                &[("cmd", &cmd), ("error", String::from_utf8_lossy(&out.stderr).trim())]
            )
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// One-shot "natural language → one shell command" via the assistant AI
/// (config's ai_engine, auto-detected when unset). The terminal's own launch
/// command and recent screen ride along as the environment fingerprint: the
/// prompt string, login banners, and recent output tell the model whether
/// it's cmd / PowerShell / bash and which OS or distro sits behind an SSH —
/// no probing protocol required
pub fn suggest_with_local_ai(
    want: &str,
    shell: &str,
    screen: &str,
    env: &str,
    engine: Option<&str>,
) -> Result<String> {
    if want.trim().is_empty() {
        anyhow::bail!("{}", crate::i18n::t("ai.suggest.want"));
    }
    // The environment card (🩺's captured survey) outranks screen guesswork
    let env_block = if env.trim().is_empty() {
        crate::i18n::t("ai.suggest.env_none")
    } else {
        env.to_string()
    };
    let prompt = crate::i18n::tp(
        "ai.suggest.prompt",
        &[("want", want), ("shell", shell), ("screen", screen), ("env", &env_block)],
    );
    let (cmd, args) = pick_local_ai(engine)?;
    let mut spawner = std::process::Command::new(&cmd);
    spawner
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = crate::detach_console(&mut spawner)
        .spawn()
        .with_context(|| crate::i18n::tp("ai.err.cannot_run", &[("cmd", &cmd)]))?;
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().context(crate::i18n::t("webui.err.stdin"))?;
        stdin.write_all(prompt.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        anyhow::bail!(
            "{}",
            crate::i18n::tp(
                "ai.err.failed",
                &[("cmd", &cmd), ("error", String::from_utf8_lossy(&out.stderr).trim())]
            )
        );
    }
    extract_cmd(&String::from_utf8_lossy(&out.stdout))
}

/// Extracts the contents of <<<CMD ... >>> (falling back to a lone code
/// fence). Conversational text must never be typed into a terminal as-is
fn extract_cmd(text: &str) -> Result<String> {
    if let Some((_, rest)) = text.split_once("<<<CMD") {
        let body = rest.split_once(">>>").map(|(b, _)| b).unwrap_or(rest);
        let cmd = body.trim();
        if !cmd.is_empty() {
            return Ok(cmd.to_string());
        }
    }
    if let Some((_, rest)) = text.split_once("```") {
        if let Some((body, _)) = rest.split_once("```") {
            let body = body.trim_start_matches(|c: char| c.is_ascii_alphanumeric());
            let cmd = body.trim();
            if !cmd.is_empty() {
                return Ok(cmd.to_string());
            }
        }
    }
    anyhow::bail!("{}", crate::i18n::t("ai.suggest.no_cmd"))
}

/// Extracts the contents of <<<LUA ... >>> from the AI's output.
/// If there's no marker, strips a code fence and returns that instead; if there's no fence either, errors out
/// (so conversational text doesn't get saved as code as-is)
fn extract_lua(text: &str) -> Result<String> {
    if let Some((_, rest)) = text.split_once("<<<LUA") {
        let body = rest.split_once(">>>").map(|(b, _)| b).unwrap_or(rest);
        return Ok(body.trim().to_string());
    }
    let stripped = strip_code_fence(text);
    // Minimum bar for "looks like code": contains shikisha.* or tab.
    if stripped.contains("shikisha.") || stripped.contains("tab.") {
        return Ok(stripped);
    }
    anyhow::bail!(
        "{}",
        crate::i18n::tp(
            "ai.err.no_code",
            &[("reply", &text.trim().chars().take(120).collect::<String>())]
        )
    )
}

/// Decides which AI CLI to use. If none is specified, the first one found in order claude → codex → gemini
fn pick_local_ai(want: Option<&str>) -> Result<(String, Vec<String>)> {
    for (name, args, _) in AI_ENGINES {
        if want.is_some_and(|w| w != name) {
            continue;
        }
        if let Some(path) = crate::tab::resolve_command(name) {
            let p = path.to_string_lossy().to_string();
            // .cmd/.bat can only be launched via cmd.exe
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            return Ok(if matches!(ext.as_deref(), Some("cmd") | Some("bat")) {
                let mut a = vec!["/c".to_string(), p];
                a.extend(args.iter().map(|s| s.to_string()));
                ("cmd.exe".to_string(), a)
            } else {
                (p, args.iter().map(|s| s.to_string()).collect())
            });
        }
    }
    match want {
        Some(w) => anyhow::bail!("{}", crate::i18n::tp("webui.err.ai_not_found", &[("name", w)])),
        None => anyhow::bail!("{}", crate::i18n::t("webui.err.ai_missing")),
    }
}

/// Strips the code fence AIs tend to add
fn strip_code_fence(s: &str) -> String {
    let t = s.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t.to_string();
    };
    let rest = rest.strip_prefix("lua").unwrap_or(rest);
    rest.trim_start_matches('\n')
        .rsplit_once("```")
        .map(|(body, _)| body)
        .unwrap_or(rest)
        .trim()
        .to_string()
}

/// Whether a replay.lua holds anything actually replayable (not just the
/// header comments an empty run leaves behind)
fn has_replay_code(text: &str) -> bool {
    text.lines()
        .any(|l| !l.trim().is_empty() && !l.trim_start().starts_with("--"))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Responds with JSON
/// Add privacy headers: keep the URL token out of the Referer header on any
/// outbound request, and out of any on-disk cache (shared-computer hygiene).
fn secure<R: std::io::Read>(resp: Response<R>) -> Response<R> {
    resp.with_header(Header::from_bytes(&b"Referrer-Policy"[..], &b"no-referrer"[..]).unwrap())
        .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap())
}

fn json_resp(v: serde_json::Value) -> Response<Cursor<Vec<u8>>> {
    secure(Response::from_string(v.to_string()).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap(),
    ))
}

/// What one of the user's JSON files amounts to right now: usable text, or a
/// refusal to hand it over with the reason attached.
///
/// Split out from the responding so the decision can be tested on its own —
/// what counts as "missing" versus "broken" is the whole point of this.
enum UserJson {
    /// The file's text (or `empty` when it simply isn't there yet).
    Text(String),
    /// It exists but can't be used. Carries the HTTP status and the body that
    /// says why, so the settings page can point at the mistake.
    Refused(u16, serde_json::Value),
}

fn read_user_json(path: &std::path::Path, empty: &str) -> UserJson {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        // Not there yet is not a refusal: that's a fresh install, and `empty` is
        // what it should look like.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return UserJson::Text(empty.into()),
        Err(e) => {
            return UserJson::Refused(
                500,
                serde_json::json!({
                    "ok": false,
                    "path": path.to_string_lossy(),
                    "error": e.to_string(),
                }),
            );
        }
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(_) => UserJson::Text(text),
        // The text rides along so the page can show the offending line itself,
        // rather than making someone count to line 24 in an editor.
        Err(e) => UserJson::Refused(
            409,
            serde_json::json!({
                "ok": false,
                "path": path.to_string_lossy(),
                "error": e.to_string(),
                "line": e.line(),
                "column": e.column(),
                "text": text,
            }),
        ),
    }
}

/// Hands one of the user's JSON files to the settings page — and refuses plainly
/// when the file is there but unusable.
///
/// A file that doesn't exist yet is not a refusal: that's a fresh install, and
/// `empty` is what it should look like. A file that exists but won't parse used
/// to be answered with 200 and its broken text, which left the page with a
/// thrown parse, an empty form and no explanation — and Save would then write
/// that emptiness over the real thing. So say what's wrong and where, and let
/// the page hold Save until it's fixed.
fn serve_user_json(req: tiny_http::Request, path: &std::path::Path, empty: &str) -> Result<()> {
    match read_user_json(path, empty) {
        UserJson::Text(text) => {
            let resp = secure(Response::from_string(text).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                    .unwrap(),
            ));
            req.respond(resp).map_err(Into::into)
        }
        UserJson::Refused(status, body) => req
            .respond(json_resp(body).with_status_code(status))
            .map_err(Into::into),
    }
}

/// Path to the secrets file. Uses config's setting if present, otherwise secrets.json next to config.json
fn secrets_file(config_path: &std::path::Path) -> std::path::PathBuf {
    crate::config::load()
        .and_then(|c| c.secrets_path())
        .unwrap_or_else(|| {
            let mut p = config_path.to_path_buf();
            p.set_file_name("secrets.json");
            p
        })
}

fn handle(
    req: tiny_http::Request,
    token: &str,
    config_path: &std::path::Path,
    remote: &Arc<std::sync::Mutex<RemoteInfo>>,
    password: &Arc<std::sync::Mutex<Option<String>>>,
) -> Result<()> {
    // DNS rebinding countermeasure: Host must always be loopback
    let host = header_value(&req, "Host");
    let host_ok = host.starts_with("127.0.0.1:") || host.starts_with("localhost:");
    // The token only needs to match in either the URL or the X-Token header
    let supplied = {
        let h = header_value(&req, "X-Token");
        if h.is_empty() {
            query_token(req.url())
        } else {
            h
        }
    };
    if !host_ok || !crate::crypto::token_eq(&supplied, token) {
        return req
            .respond(Response::from_string("forbidden").with_status_code(403))
            .map_err(Into::into);
    }

    let method = req.method().as_str().to_string();
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    // Whoever is asking: this PC's own window, or a phone coming in over the proxy
    let remote_client = header_value(&req, REMOTE_CLIENT_HEADER) == "1";
    // Say no before a dialog can be opened at a screen nobody is looking at. The
    // page hides these buttons for a remote caller, so reaching here means a page
    // that was already open, or something calling the API directly
    if remote_client && NATIVE_DIALOG_PATHS.contains(&path.as_str()) {
        let mut req = req;
        // Drain the body first so the response isn't written over an unread request
        let _ = read_body(&mut req, MAX_BODY)?;
        return req
            .respond(json_resp(serde_json::json!({
                "ok": false,
                "error": crate::i18n::t("settings.pick.no_remote"),
            })))
            .map_err(Into::into);
    }
    match (method.as_str(), path.as_str()) {
        ("GET", "/") => {
            let html = crate::i18n::render(&themed(PAGE.to_string()))
                .replace("__TOKEN__", token)
                .replace("__REMOTE__", if remote_client { "true" } else { "false" })
                .replace("__GRANTS__", &crate::grants::catalog_json())
                .replace(
                    "__GITLUA__",
                    &serde_json::to_string(crate::hooks::COMMIT_MESSAGE_LUA)
                        .unwrap_or_else(|_| "\"\"".into()),
                )
                .replace(
                    "__PROTECT__",
                    &serde_json::to_string(&crate::git::DEFAULT_PROTECTED)
                        .unwrap_or_else(|_| "[]".into()),
                )
                .replace("__DICT__", &crate::i18n::dict_json());
            let resp = secure(Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            ));
            req.respond(resp)?;
        }
        // The result view: a finished run's transcript.md rendered as a chat
        // (AI-vs-AI discussion / code review / browser rally). Same token gate
        // as the settings page; the run id rides in the query string.
        ("GET", "/result") => {
            let html = crate::i18n::render(&themed(RESULT_PAGE.to_string()))
                .replace("__TOKEN__", token)
                .replace("__DICT__", &crate::i18n::dict_json());
            let resp = secure(Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            ));
            req.respond(resp)?;
        }
        // How-to-write documentation (openable from the GUI, so the user doesn't have to hunt for the file)
        ("GET", "/help") => {
            let md = load_manual(config_path);
            let html = crate::i18n::render(&themed(HELP_PAGE.to_string()))
                .replace("__MD__", &serde_json::to_string(&md)?);
            let resp = secure(Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            ));
            req.respond(resp)?;
        }
        ("GET", "/api/config") => serve_user_json(req, config_path, "{}")?,
        // Opens an external help/report page in the user's real browser (not
        // the in-app WebView). Destinations are whitelisted, so the page can
        // never be talked into acting as an open redirect.
        ("GET", "/api/open") => {
            let url = match query_param(req.url(), "dest").as_deref() {
                Some("bug") => Some(bug_report_url()),
                Some("discussions") => {
                    Some("https://github.com/styleio/ShikishaTerm/discussions".to_string())
                }
                _ => None,
            };
            match url {
                Some(u) => {
                    open_external(&u);
                    req.respond(json_resp(serde_json::json!({ "ok": true })))?;
                }
                None => {
                    req.respond(Response::from_string("bad dest").with_status_code(400))?;
                }
            }
        }
        // Runs `<cmd> --help` so the settings page can show a CLI's real flags.
        // The program is the first token of the tab's command.
        // Which project a folder belongs to, and whether its folder is one the
        // app made. The settings screen cannot work either out: both mean
        // looking at what git keeps behind the folder
        ("GET", "/api/family") => {
            let at = query_param(req.url(), "path")
                .map(|c| percent_decode(&c))
                .unwrap_or_default();
            let at = std::path::Path::new(at.trim());
            let resp = match at.as_os_str().is_empty() {
                true => serde_json::json!({ "family": null, "cut": false }),
                false => serde_json::json!({
                    "family": crate::repo::family_of(at).map(|f| f.display().to_string()),
                    "cut": crate::repo::is_linked(at),
                }),
            };
            req.respond(json_resp(resp))?;
        }
        // Throw a branch's folder away for good. Refused while anything in it
        // is uncommitted -- said before the settings let go of it, so a no
        // costs nothing. The removal itself waits for the tabs to leave
        ("POST", "/api/folder/discard") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let at = std::path::PathBuf::from(
                p.get("path").and_then(|v| v.as_str()).unwrap_or_default().trim(),
            );
            let resp = match crate::worktree::ready_to_discard(&at) {
                Ok(()) => {
                    crate::worktree::discard_soon(at);
                    serde_json::json!({ "ok": true })
                }
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
            };
            req.respond(json_resp(resp))?;
        }
        ("GET", "/api/cli-help") => {
            let prog = query_param(req.url(), "cmd")
                .map(|c| percent_decode(&c))
                .and_then(|c| c.split_whitespace().next().map(str::to_string))
                .unwrap_or_default();
            let resp = match cli_help(&prog) {
                Ok(help) => serde_json::json!({ "ok": true, "help": help }),
                Err(e) => serde_json::json!({ "ok": false, "error": e }),
            };
            req.respond(json_resp(resp))?;
        }
        // Sends a test notification to one destination described in the body
        // ({"type":"slack","webhook":…} etc). "@name" fields are expanded from
        // the secret store, so a saved destination can be tested too.
        ("POST", "/api/notify/test") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let resp = match serde_json::from_str::<crate::notify::Destination>(&body) {
                Ok(mut dest) => {
                    let pw = password.lock().unwrap().clone();
                    let tokens = crate::config::load()
                        .map(|c| c.resolve_tokens(pw.as_deref()))
                        .unwrap_or_default();
                    let deref = |v: &str| -> String {
                        match v.strip_prefix('@') {
                            Some(k) => tokens.get(k).cloned().unwrap_or_default(),
                            None => v.to_string(),
                        }
                    };
                    match &mut dest {
                        crate::notify::Destination::Slack { webhook } => *webhook = deref(webhook),
                        crate::notify::Destination::Telegram { token, chat_id } => {
                            *token = deref(token);
                            *chat_id = deref(chat_id);
                        }
                    }
                    match crate::notify::send_blocking(
                        &dest,
                        &crate::i18n::t("err.main.test_notify_body"),
                    ) {
                        Ok(()) => serde_json::json!({ "ok": true }),
                        Err(e) => serde_json::json!({ "ok": false, "error": e }),
                    }
                }
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            };
            req.respond(json_resp(resp))?;
        }
        // Syntax-check a Lua snippet ({"code":"…"}) so the settings UI can refuse
        // to save a quick action whose Lua is broken. Compile-only, never runs it.
        ("POST", "/api/lint") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let code = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(str::to_string))
                .unwrap_or_default();
            let resp = match crate::hooks::lint_lua(&code) {
                None => serde_json::json!({ "ok": true }),
                Some(e) => serde_json::json!({ "ok": false, "error": e }),
            };
            req.respond(json_resp(resp))?;
        }
        // The command line a tab will really be launched with. The settings
        // screen shows what it is given here rather than working it out for
        // itself: a second implementation in the page would be a second answer
        // to "what runs", and the two would drift the first time either moved
        ("POST", "/api/launch-line") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let str_of = |k: &str| {
                v.get(k)
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
                    .filter(|s| !s.trim().is_empty())
            };
            let argv = crate::config::CommandSpec::Line(str_of("command").unwrap_or_default()).argv();
            let line = crate::tab::launch_line(
                &argv,
                &str_of("profile"),
                crate::resume_plan_of(str_of("resume").as_deref()),
                &crate::i18n::t("settings.tab.command.newid"),
            );
            req.respond(json_resp(serde_json::json!({
                "argv": line.argv, "added": line.added
            })))?;
        }
        // Recent rally history (newest first). Returns the id plus an excerpt to help a human tell them apart
        ("GET", "/api/rally/list") => {
            let mut arr: Vec<serde_json::Value> = Vec::new();
            for dir in crate::exchange::recent_runs(60) {
                if arr.len() >= 20 {
                    break;
                }
                let id = dir
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                let t = std::fs::read_to_string(dir.join("transcript.md")).unwrap_or_default();
                let record = std::fs::read_to_string(dir.join("record.lua")).unwrap_or_default();
                // Use the first line that isn't a heading (#) or blank as an identifying excerpt
                let title = t
                    .lines()
                    .find(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
                    .map(|l| l.chars().take(60).collect::<String>());
                // Don't show empty runs (ones on_start merely created) in the history
                let has_record = record.lines().any(|l| !l.trim_start().starts_with("--") && !l.trim().is_empty());
                match &title {
                    Some(tt) => arr.push(serde_json::json!({ "id": id, "title": tt })),
                    None if has_record => {
                        arr.push(serde_json::json!({ "id": id, "title": crate::i18n::t("rally.md.actions_only") }))
                    }
                    None => {}
                }
            }
            let resp = Response::from_string(serde_json::Value::Array(arr).to_string()).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                    .unwrap(),
            );
            req.respond(resp)?;
        }
        // Raw transcript for the chat-style result view. Returns the run's
        // transcript.md verbatim plus a `kind` hint (discuss vs rally, told
        // apart by whether the run recorded executed Lua). The page parses the
        // Markdown itself; the untouched download stays available separately.
        ("GET", "/api/rally/transcript") => {
            let picked = req
                .url()
                .split_once('?')
                .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("run=")))
                .map(percent_decode)
                .and_then(|id| crate::exchange::run_by_id(&id));
            match picked.or_else(crate::exchange::latest_run) {
                Some(dir) => {
                    let md = std::fs::read_to_string(dir.join("transcript.md")).unwrap_or_default();
                    let record = std::fs::read_to_string(dir.join("record.lua")).unwrap_or_default();
                    // A rally records the Lua it executed; a discussion never
                    // does. That presence is the reliable tell, independent of
                    // the (localized) transcript headings.
                    let kind = if record
                        .lines()
                        .any(|l| !l.trim_start().starts_with("--") && !l.trim().is_empty())
                    {
                        "rally"
                    } else {
                        "discuss"
                    };
                    let id = dir.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    // Whether a durable replay exists (beyond its header comments) —
                    // the view shows its download button only when there is one
                    let replay = std::fs::read_to_string(dir.join("replay.lua"))
                        .map(|s| has_replay_code(&s))
                        .unwrap_or(false);
                    let body =
                        serde_json::json!({ "md": md, "kind": kind, "id": id, "replay": replay })
                            .to_string();
                    let resp = Response::from_string(body).with_header(
                        Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json; charset=utf-8"[..],
                        )
                        .unwrap(),
                    );
                    req.respond(resp)?;
                }
                None => {
                    req.respond(Response::from_string("{}").with_header(
                        Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ))?;
                }
            }
        }
        // The durable replay script: css/xpath anchors only, no digest/ref
        // dependency — paste into ▶ run mode or an automation, on any PC.
        // ?run=<id> for a specific run, otherwise the latest. 404 when the
        // run recorded nothing replayable
        ("GET", "/api/rally/replay") => {
            let picked = req
                .url()
                .split_once('?')
                .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("run=")))
                .map(percent_decode)
                .and_then(|id| crate::exchange::run_by_id(&id));
            let found = picked.or_else(crate::exchange::latest_run).and_then(|dir| {
                let text = std::fs::read_to_string(dir.join("replay.lua")).unwrap_or_default();
                let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("run").to_string();
                has_replay_code(&text).then_some((text, name))
            });
            match found {
                Some((text, name)) => {
                    let cd = format!("attachment; filename=\"shikisha-macro-{name}.lua\"");
                    let resp = Response::from_string(text)
                        .with_header(
                            Header::from_bytes(
                                &b"Content-Type"[..],
                                &b"text/plain; charset=utf-8"[..],
                            )
                            .unwrap(),
                        )
                        .with_header(
                            Header::from_bytes(&b"Content-Disposition"[..], cd.as_bytes())
                                .unwrap(),
                        );
                    req.respond(resp)?;
                }
                None => {
                    req.respond(Response::from_string("no replay").with_status_code(404))?;
                }
            }
        }
        // Lets the rally result be downloaded as a single Markdown file. ?run=<id> for a specific run,
        // otherwise the latest. Contents: the human-readable flow (transcript) + the verdict + the executed Lua (record, paste it to reproduce).
        // Whatever the orchestrator (AI+AI, etc.), leaving transcript.md/record.lua in the run folder lets it be downloaded through the same path
        ("GET", "/api/rally/download") => {
            let picked = req
                .url()
                .split_once('?')
                .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("run=")))
                .map(percent_decode)
                .and_then(|id| crate::exchange::run_by_id(&id));
            match picked.or_else(crate::exchange::latest_run) {
            Some(dir) => {
                let transcript =
                    std::fs::read_to_string(dir.join("transcript.md")).unwrap_or_default();
                let record = std::fs::read_to_string(dir.join("record.lua")).unwrap_or_default();
                let mut md = String::new();
                if transcript.trim().is_empty() {
                    md.push_str(&crate::i18n::t("rally.md.empty"));
                } else {
                    md.push_str(&transcript);
                }
                if !record.trim().is_empty() {
                    md.push_str(&crate::i18n::t("rally.md.lua_heading"));
                    md.push_str(&record.replace('\n', "\n    "));
                    md.push('\n');
                }
                let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("rally");
                let cd = format!("attachment; filename=\"rally-{name}.md\"");
                let resp = Response::from_string(md)
                    .with_header(
                        Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/markdown; charset=utf-8"[..],
                        )
                        .unwrap(),
                    )
                    .with_header(
                        Header::from_bytes(&b"Content-Disposition"[..], cd.as_bytes()).unwrap(),
                    );
                req.respond(resp)?;
            }
            None => {
                req.respond(Response::from_string("no rally").with_status_code(404))?;
            }
            }
        }
        // Read/write a workspace definition file (external file reference)
        ("GET", "/api/workspace") => {
            let Some(p) = safe_workspace_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            serve_user_json(req, &p, r#"{"tabs":[]}"#)?;
        }
        ("POST", "/api/workspace") => {
            let Some(p) = safe_workspace_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(_) => {
                    if let Some(dir) = p.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    crate::crypto::write_atomic(&p, &body)?;
                    req.respond(Response::from_string(r#"{"ok":true}"#))?;
                }
                Err(e) => {
                    let msg = serde_json::json!({ "ok": false, "error": e.to_string() });
                    req.respond(Response::from_string(msg.to_string()).with_status_code(400))?;
                }
            }
        }
        // ── Secrets (equivalent to GitHub Secrets) ────────────────────
        // The master password is never exposed to the page or the network.
        // The listing returns only keys and descriptions; values are never returned
        ("GET", "/api/secrets") => {
            let path = secrets_file(config_path);
            let pw = password.lock().unwrap().clone();
            let encrypted = std::fs::read_to_string(&path)
                .map(|t| crate::crypto::is_encrypted(&t))
                .unwrap_or(false);
            let (mode, items): (&str, Vec<serde_json::Value>) = if !path.exists() {
                ("empty", Vec::new())
            } else if encrypted && pw.is_none() {
                // If it's encrypted and there's no password, we can't even show the listing
                ("locked", Vec::new())
            } else {
                match crate::config::list_secrets(&path, pw.as_deref()) {
                    Ok(list) => (
                        if encrypted { "encrypted" } else { "plaintext" },
                        list.into_iter()
                            .map(|(k, d)| serde_json::json!({ "key": k, "description": d }))
                            .collect(),
                    ),
                    Err(_) => ("locked", Vec::new()),
                }
            };
            req.respond(json_resp(serde_json::json!({
                "mode": mode,
                "has_password": pw.is_some(),
                "secrets": items,
            })))?;
        }
        ("POST", "/api/secrets/set") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let s = |k| p.get(k).and_then(|v| v.as_str()).unwrap_or("");
            let (key, desc, value) = (s("key").trim(), s("description"), s("value"));
            let path = secrets_file(config_path);
            let pw = password.lock().unwrap().clone();
            let resp = if value.is_empty() {
                serde_json::json!({ "ok": false, "error": crate::i18n::t("webui.err.empty_value") })
            } else {
                match crate::config::upsert_secret(&path, pw.as_deref(), key, desc, value) {
                    Ok(()) => serde_json::json!({ "ok": true }),
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                }
            };
            req.respond(json_resp(resp))?;
        }
        ("POST", "/api/secrets/delete") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let key = p.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let path = secrets_file(config_path);
            let pw = password.lock().unwrap().clone();
            let resp = match crate::config::delete_secret(&path, pw.as_deref(), key) {
                Ok(()) => serde_json::json!({ "ok": true }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            };
            req.respond(json_resp(resp))?;
        }
        // List the available models for a provider (the "candidates" button).
        // Hits the OpenAI-compatible {base_url}/models so the user can pick a
        // real model name instead of guessing. Resolves an @secret api_key and
        // any custom headers just like resolve_provider does.
        ("POST", "/api/provider/models") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let s = |k| p.get(k).and_then(|v| v.as_str()).unwrap_or("");
            let base_url = s("base_url").trim();
            let api_key = s("api_key").trim();
            let pw = password.lock().unwrap().clone();
            let tokens = crate::config::load()
                .map(|c| c.resolve_tokens(pw.as_deref()))
                .unwrap_or_default();
            let deref = |v: &str| -> String {
                match v.strip_prefix('@') {
                    Some(k) => tokens.get(k).cloned().unwrap_or_default(),
                    None => v.to_string(),
                }
            };
            let mut headers = std::collections::HashMap::new();
            if let Some(obj) = p.get("headers").and_then(|h| h.as_object()) {
                for (k, v) in obj {
                    if let Some(vs) = v.as_str() {
                        headers.insert(k.clone(), deref(vs));
                    }
                }
            }
            if headers.is_empty() && !api_key.is_empty() {
                headers.insert("Authorization".into(), format!("Bearer {}", deref(api_key)));
            }
            let resp = if base_url.is_empty() {
                serde_json::json!({ "ok": false, "error": crate::i18n::t("webui.err.empty_base_url") })
            } else {
                match crate::bridge::list_models(base_url, &headers) {
                    Ok(models) => serde_json::json!({ "ok": true, "models": models }),
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                }
            };
            req.respond(json_resp(resp))?;
        }
        // Export a single workspace, scripts and all, as one file.
        // Addressed by index into the saved config — not the screen's in-progress edits
        ("POST", "/api/workspace/export") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let index = p.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let resp = match crate::wspack::pack(config_path, index) {
                Ok((name, text)) => {
                    raise_own_dialog();
                    let picked = rfd::FileDialog::new()
                        .set_title(crate::i18n::t("settings.ws.export.title"))
                        .set_file_name(&name)
                        .add_filter(crate::i18n::t("settings.ws.file_kind"), &["json"])
                        .set_directory(
                            config_path.parent().unwrap_or(std::path::Path::new(".")),
                        )
                        .save_file();
                    match picked {
                        // Write to the chosen location. This is fine to be outside the config folder, since the user picked it
                        Some(path) => match crate::crypto::write_atomic(&path, &text) {
                            Ok(()) => serde_json::json!({
                                "ok": true,
                                "path": path.display().to_string(),
                            }),
                            Err(e) => {
                                serde_json::json!({ "ok": false, "error": e.to_string() })
                            }
                        },
                        None => serde_json::json!({ "ok": false, "cancelled": true }),
                    }
                }
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
            };
            req.respond(
                Response::from_string(resp.to_string()).with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            )?;
        }
        // Import an exported file. Adds one workspace to the config
        ("POST", "/api/workspace/import") => {
            raise_own_dialog();
            let picked = rfd::FileDialog::new()
                .set_title(crate::i18n::t("settings.ws.import.title"))
                .add_filter(crate::i18n::t("settings.ws.file_kind"), &["json"])
                .set_directory(config_path.parent().unwrap_or(std::path::Path::new(".")))
                .pick_file();
            let resp = match picked {
                Some(path) => match std::fs::read_to_string(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|t| crate::wspack::unpack(config_path, &t))
                {
                    Ok(placed) => serde_json::json!({
                        "ok": true,
                        "name": placed.name,
                        "files": placed.files,
                        "moved": placed.moved.iter()
                            .map(|(f, t)| serde_json::json!([f, t]))
                            .collect::<Vec<_>>(),
                    }),
                    Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
                },
                None => serde_json::json!({ "ok": false, "cancelled": true }),
            };
            req.respond(
                Response::from_string(resp.to_string()).with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            )?;
        }
        // Read/write automation (per-event files)
        ("GET", "/api/automation") => {
            let Some(dir) = safe_dir_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            let mut map = serde_json::Map::new();
            for name in EVENT_FILES {
                let f = dir.join(format!("{name}.lua"));
                let body = std::fs::read_to_string(&f).unwrap_or_default();
                map.insert(name.to_string(), serde_json::Value::String(body));
            }
            let resp = Response::from_string(serde_json::Value::Object(map).to_string())
                .with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                );
            req.respond(resp)?;
        }
        ("POST", "/api/automation") => {
            let Some(dir) = safe_dir_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let parsed: serde_json::Value = serde_json::from_str(&body)?;
            std::fs::create_dir_all(&dir)?;
            for name in EVENT_FILES {
                let Some(code) = parsed.get(name).and_then(|v| v.as_str()) else {
                    continue;
                };
                let f = dir.join(format!("{name}.lua"));
                if code.trim().is_empty() {
                    // Emptying it means "do nothing for this event" = delete the whole file
                    let _ = std::fs::remove_file(&f);
                } else {
                    crate::crypto::write_atomic(&f, code)?;
                }
            }
            req.respond(Response::from_string(r#"{"ok":true}"#))?;
        }
        // Opens the standard Windows file-picker dialog.
        // The browser can't hand over a real file path for safety reasons, so we open it on this side
        ("POST", "/api/pick") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let kind = p.get("kind").and_then(|v| v.as_str()).unwrap_or("file");
            let fallback = crate::i18n::t("settings.pick.title");
            let title = p.get("title").and_then(|v| v.as_str()).unwrap_or(&fallback);
            let start = p
                .get("start")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(std::path::PathBuf::from)
                .filter(|p| p.exists())
                .or_else(|| default_pick_dir(kind, config_path));

            // Bring the dialog to the front right after opening it
            // (a background process can't foreground itself on its own)
            raise_own_dialog();
            let mut dlg = rfd::FileDialog::new().set_title(title);
            if let Some(d) = start {
                dlg = dlg.set_directory(d);
            }
            let picked = if kind == "dir" {
                dlg.pick_folder()
            } else {
                dlg.pick_file()
            };
            let resp = match picked {
                Some(path) => {
                    serde_json::json!({ "ok": true, "path": display_path(&path, config_path) })
                }
                None => serde_json::json!({ "ok": false }),
            };
            req.respond(
                Response::from_string(resp.to_string()).with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            )?;
        }
        // Status of the phone-usable feature (also returns which network is available)
        // What each AI CLI can do about carrying its conversation across a
        // restart, and — where it needs one — whether its hook is installed.
        // The person asked "will my conversation survive?", and this answers
        // that per CLI rather than describing a mechanism
        // The saved browser logins, to manage. Names and counts only -- the
        // cookies themselves never come back out of here
        ("GET", "/api/logins") => {
            let rows: Vec<serde_json::Value> = crate::browserstate::list()
                .into_iter()
                .map(|e| serde_json::json!({ "label": e.label, "count": e.count }))
                .collect();
            req.respond(json_resp(serde_json::json!(rows)))?;
        }
        ("POST", "/api/logins/delete") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let label = v.get("label").and_then(|x| x.as_str()).unwrap_or_default();
            let ok = crate::browserstate::delete(label).is_ok();
            req.respond(json_resp(serde_json::json!({ "ok": ok })))?;
        }
        // The saved page snapshots, newest first, each as a data URL so the
        // card can show it without a second authenticated image request. A
        // handful is the normal case; the newest are enough to glance at
        ("GET", "/api/snapshots") => {
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD;
            let rows: Vec<serde_json::Value> = crate::browserstate::snapshots()
                .into_iter()
                .take(24)
                .filter_map(|(label, path)| {
                    let bytes = std::fs::read(&path).ok()?;
                    let data = format!("data:image/png;base64,{}", b64.encode(&bytes));
                    Some(serde_json::json!({ "label": label, "data": data }))
                })
                .collect();
            req.respond(json_resp(serde_json::json!(rows)))?;
        }
        ("POST", "/api/snapshots/delete") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let label = v.get("label").and_then(|x| x.as_str()).unwrap_or_default();
            let ok = crate::browserstate::delete_snapshot(label).is_ok();
            req.respond(json_resp(serde_json::json!({ "ok": ok })))?;
        }
        // Whether this machine has a GitHub sign-in, so the settings can say
        // why a tab shows a branch but no pull request number. Whether, never
        // what: nothing here hands a token back out
        ("GET", "/api/github") => {
            req.respond(json_resp(serde_json::json!({
                "signed_in": crate::pr::signed_in(),
            })))?;
        }
        // What this machine already offers to open a tab on: the installed WSL
        // distributions and the hosts in the person's own ssh config. Both were
        // things the settings screen asked people to type from memory
        ("GET", "/api/discover") => {
            req.respond(json_resp(serde_json::json!({
                "wsl": crate::discover::wsl_distros(),
                "ssh": crate::discover::ssh_hosts(),
            })))?;
        }
        // Which pseudo console the terminals are running on. There is nothing
        // to set: what decides it is whether the file shipped, so this reports
        // and the download fixes. It is here at all because both ways of
        // falling back to Windows' older one are completely silent
        ("GET", "/api/conpty") => {
            let r = crate::conpty::report();
            req.respond(json_resp(serde_json::json!({
                "bundled": r.bundled,
                "version": r.version,
                "path": r.path.display().to_string(),
                "missing": r.missing.map(|m| m.id()),
            })))?;
        }
        // Every action the window has, with the key it answers to right now.
        // The names are the app's own, so the settings screen never has its
        // own idea of what this program can do
        ("GET", "/api/keys") => {
            let (map, errs) = crate::keys::Keys::load(crate::config::load().as_ref());
            let shown: std::collections::HashMap<&str, String> = crate::keys::ACTIONS
                .iter()
                .map(|a| (a.name, String::new()))
                .chain(
                    map.help_rows()
                        .into_iter()
                        .filter_map(|(k, d)| {
                            let name = crate::keys::ACTIONS.iter().find(|a| a.desc == d)?.name;
                            Some((name, k))
                        }),
                )
                .collect();
            let rows: Vec<serde_json::Value> = crate::keys::ACTIONS
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "name": a.name,
                        "desc": crate::i18n::t(a.desc),
                        "now": shown.get(a.name).cloned().unwrap_or_default(),
                    })
                })
                .collect();
            req.respond(json_resp(serde_json::json!({
                "prefix": map.prefix_shown(),
                "rows": rows,
                "problems": errs,
            })))?;
        }
        // Every colour scheme this machine can name, in the order they are
        // found. The names come from what the person already has, so the list
        // is the point: a scheme they cannot see the name of is one they will
        // never type
        ("GET", "/api/themes") => {
            let rows: Vec<serde_json::Value> = crate::theme::available()
                .into_iter()
                .map(|s| serde_json::json!({ "name": s.name, "colors": s.swatch() }))
                .collect();
            // What is on screen right now comes back too, because a scheme
            // written out in the settings by hand is in no list and would
            // otherwise be a blank where the person's own colours are
            let look = crate::config::load().map(|c| c.appearance).unwrap_or_default();
            let now = look.scheme();
            req.respond(json_resp(serde_json::json!({
                "default": crate::theme::DEFAULT_NAME,
                "current": { "name": now.name, "colors": now.swatch() },
                "list": rows,
            })))?;
        }
        ("GET", "/api/resume") => {
            let rows: Vec<serde_json::Value> = crate::profile::all()
                .into_iter()
                .filter_map(|p| {
                    let r = p.resume.as_ref()?;
                    // In the order the app itself tries them, so what is shown
                    // is what will actually happen
                    let how = if !r.new_id.is_empty() {
                        "minted"
                    } else if r.record.is_some() {
                        "record"
                    } else if r.hook.is_some() {
                        "hook"
                    } else if !r.newest_here.is_empty() {
                        "newest"
                    } else {
                        "none"
                    };
                    let hook = crate::agenthook::targets()
                        .into_iter()
                        .find(|t| t.name == p.name)
                        .map(|t| {
                            serde_json::json!({
                                "file": t.file.display().to_string(),
                                "status": format!("{:?}", crate::agenthook::status(&t))
                                    .split('(').next().unwrap_or("").to_string(),
                                "preview": crate::agenthook::preview(&t),
                            })
                        });
                    Some(serde_json::json!({ "name": p.name, "how": how, "hook": hook }))
                })
                .collect();
            req.respond(json_resp(serde_json::json!(rows)))?;
        }
        // Put one CLI's hook in, or take it out. Named by profile, so the page
        // never hands over a path to write to
        ("POST", "/api/resume/hook") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or_default();
            let on = v.get("on").and_then(|x| x.as_bool()).unwrap_or(false);
            let found = crate::agenthook::targets().into_iter().find(|t| t.name == name);
            let resp = match found {
                None => serde_json::json!({ "ok": false, "error": "no such CLI" }),
                Some(t) => {
                    let done = if on {
                        crate::agenthook::install(&t)
                    } else {
                        crate::agenthook::uninstall(&t)
                    };
                    match done {
                        Ok(()) => serde_json::json!({
                            "ok": true,
                            "status": format!("{:?}", crate::agenthook::status(&t))
                                .split('(').next().unwrap_or("").to_string(),
                        }),
                        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                    }
                }
            };
            req.respond(json_resp(resp))?;
        }
        // Where the external API is listening, so the settings screen can show
        // the pipe by name — a person writing a script against it needs the
        // exact string, and it carries the process id, so it is not guessable
        // from the docs alone. No token is ever handed out here: in `user` mode
        // it sits in data\api-token, and in `children` mode it exists only in
        // the environment of what this app started
        ("GET", "/api/external") => {
            let path = crate::api::listening_on();
            let resp = serde_json::json!({
                "running": path.is_some(),
                "path": path,
                "token_file": crate::config::state_path("api-token").display().to_string(),
            });
            req.respond(json_resp(resp))?;
        }
        ("GET", "/api/remote") => {
            let (info, demo) = remote_for_display(remote);
            // A stand-in address stands in for the whole card: it is presented
            // as the safe case, so the picture is of the feature rather than of
            // a warning about a network nobody is on
            let (ts, lan) = if demo {
                (Some(crate::netaddr::url_host(&info.url)), None)
            } else {
                (
                    crate::netaddr::tailscale_ip().map(|i| i.to_string()),
                    crate::netaddr::lan_ip().map(|i| i.to_string()),
                )
            };
            // The full URL embeds the access token (= full-machine control), so
            // it is never drawn on the page. The origin is all the page gets;
            // the token reaches the phone inside the QR image, or the clipboard
            // by way of /api/remote/url when the copy button is pressed
            let origin = info.url.split("/?").next().unwrap_or("").to_string();
            let resp = serde_json::json!({
                "running": info.running,
                "origin": origin,
                "note": info.note,
                "tailscale": ts,
                "lan": lan,
                // What the link leads to, said in one word so the page can put
                // a colour on it
                "kind": crate::netaddr::shown_link(&info.url).1,
            });
            req.respond(
                Response::from_string(resp.to_string()).with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            )?;
        }
        // The connection link as text, for the clipboard and nowhere else.
        // It carries the token — the whole machine — so the page asks for it at
        // the moment the copy button is pressed and hands it straight to the
        // clipboard, never to the screen. Behind the same token gate as the
        // rest of this server, so this hands out nothing the QR did not already
        ("GET", "/api/remote/url") => {
            let (info, _) = remote_for_display(remote);
            req.respond(json_resp(serde_json::json!({ "url": info.url })))?;
        }
        // Connection QR code (avoids having to hand-type the URL and token)
        ("GET", "/api/remote/qr") => {
            let url = remote_for_display(remote).0.url;
            let svg = if url.is_empty() {
                String::new()
            } else {
                crate::netaddr::qr_svg(&url, 6)
            };
            req.respond(Response::from_string(svg).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"image/svg+xml; charset=utf-8"[..])
                    .unwrap(),
            ))?;
        }
        // Checks which AI CLIs are available (determined before rendering, so the feature is hidden entirely if none)
        ("GET", "/api/ai") => {
            let list: Vec<serde_json::Value> = AI_ENGINES
                .iter()
                .filter(|(name, _, _)| crate::tab::resolve_command(name).is_some())
                .map(|(name, _, label)| serde_json::json!({ "id": name, "label": label }))
                .collect();
            let resp = Response::from_string(serde_json::json!({ "engines": list }).to_string())
                .with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                );
            req.respond(resp)?;
        }
        // Generates Lua from natural language (one-shot run of a local AI CLI)
        ("POST", "/api/generate") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let event = parsed.get("event").and_then(|v| v.as_str()).unwrap_or("on_done");
            let want = parsed.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            // If none is specified, use config's ai_engine
            let from_cfg = std::fs::read_to_string(config_path)
                .ok()
                .and_then(|t| serde_json::from_str::<crate::config::Config>(&t).ok())
                .and_then(|c| c.ai_engine)
                .filter(|s| !s.is_empty());
            let engine = parsed
                .get("engine")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or(from_cfg);
            let layout = describe_tabs(&parsed);
            let resp = match generate_with_local_ai(event, want, &layout, engine.as_deref(), config_path)
            {
                Ok(code) => serde_json::json!({ "ok": true, "code": code }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            };
            req.respond(
                Response::from_string(resp.to_string()).with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            )?;
        }
        ("POST", "/api/config") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            // Always validate before saving, so broken JSON never wipes out the config
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(_) => {
                    crate::crypto::write_atomic(config_path, &body)?;
                    req.respond(Response::from_string(r#"{"ok":true}"#))?;
                }
                Err(e) => {
                    let msg = serde_json::json!({ "ok": false, "error": e.to_string() });
                    req.respond(
                        Response::from_string(msg.to_string()).with_status_code(400),
                    )?;
                }
            }
        }
        _ => {
            req.respond(Response::new(
                404.into(),
                vec![],
                Cursor::new(b"not found".to_vec()),
                None,
                None,
            ))?;
        }
    }
    Ok(())
}

// The settings screen. Unlike the main app's cyber look, it's a quiet UI that prioritizes readability
// (sidebar + detail pane. The list shows only "what exists"; editing stays focused on one item at a time)
/// The colours and the toast, poured into a page.
///
/// Runs **before** the words are: an unknown `{{key}}` is replaced with the key
/// itself, so a template left to that step would end up with the word THEME
/// sitting in its stylesheet and no colours at all.
///
/// Every page this server serves gets the same blocks, because they are all the
/// same app: the settings screen, the transcript view and the manual are not
/// three products with three looks, and a message means the same thing and
/// behaves the same way on each of them (src/toast.rs).
fn themed(html: String) -> String {
    let look = crate::config::load().map(|c| c.appearance).unwrap_or_default();
    let scheme = look.scheme();
    crate::toast::render(html)
        .replace("{{THEME}}", &scheme.css_vars())
        .replace(
            "{{SCHEME}}",
            if crate::theme::is_light(&scheme) { "light" } else { "dark" },
        )
}

const PAGE: &str = r##"<!doctype html>
<html lang="{{__lang__}}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{settings.title}}</title>
<style>
 :root {
   /* The same colours the window is drawn in, written out by the app from the
      chosen scheme. A settings screen that stayed dark while the window went
      light would be the same app disagreeing with itself */
   {{THEME}}
   /* Measured from the real header (it wraps at some widths, so a fixed number
      would leave the sidebar tucked under it). Everything that has to start
      below the sticky header reads it from here. */
   --headh:53px;
   color-scheme: {{SCHEME}};
 }
 * { box-sizing:border-box; }
 body { margin:0; background:var(--bg); color:var(--text); font-size:14px; line-height:1.6;
   font-family:system-ui,"Segoe UI","Yu Gothic UI","Hiragino Sans",sans-serif; }
 code, .mono, input.mono { font-family:ui-monospace,Consolas,"Courier New",monospace; }

 header { position:sticky; top:0; z-index:5; display:flex; align-items:center; gap:12px;
   padding:12px 20px; background:color-mix(in srgb, var(--bg) 90%, transparent); backdrop-filter:blur(8px);
   border-bottom:1px solid var(--line); }
 header h1 { font-size:15px; font-weight:600; margin:0; letter-spacing:.02em; }
 header .spacer { flex:1; }
 /* The secondary links live in one element so a narrow screen can MOVE them into
    the drawer instead of a second copy being written for the phone. */
 .headlinks { display:flex; align-items:center; gap:12px; }
 /* Drawer handle and current-section label: phone only (see the narrow block). */
 .navtoggle { display:none; font-size:17px; line-height:1; padding:6px 10px; }
 /* Where you are, in two parts: the workspace gives way first (it ellipsises),
    the thing actually being edited always stays whole. */
 #crumb { display:none; align-items:baseline; gap:6px; min-width:0;
   font-weight:600; font-size:14px; white-space:nowrap; }
 #crumb .up { flex:0 1 auto; min-width:0; overflow:hidden; text-overflow:ellipsis;
   font-weight:400; color:var(--muted); }
 #crumb .cur { flex:0 0 auto; overflow:hidden; text-overflow:ellipsis; }
 #navscrim { display:none; }
 /* Label on a desktop, icon on a phone — one button, two skins. */
 .atnarrow { display:none; }
 #msg { color:var(--muted); font-size:13px; border-radius:6px; padding:4px 10px; }
 #msg.warn { color:var(--danger); }
 /* A setting that could not be used. Said in the place it was set, not in a
    log nobody opens */
 .warn { color:var(--danger); font-size:13px; margin:0 0 8px; }
 /* Replay the animation every time, so a click still registers even if the message text repeats */
 #msg.flash { animation:msgflash 1.1s ease-out; }
 @keyframes msgflash {
   0%   { background:var(--accent); color:#04121c; }
   60%  { background:var(--accent); color:#04121c; }
   100% { background:transparent; color:var(--muted); }
 }
 button.primary:disabled { opacity:.55; cursor:default; }
 /* Small text in the header doesn't catch the eye for a save result, so the
    shared toast (src/toast.rs) says it again at the bottom of the screen */
{{TOAST_CSS}}
 /* Mark the save button while there are unsaved changes. It turns amber and
    pulses a glow ring so an unsaved edit is impossible to miss and you remember
    to press Save at the end (it goes back to the normal blue once saved). */
 #savebtn.dirty { background:#ffb020; border-color:#ffb020; color:#1a1205;
   animation:savepulse 1.1s ease-in-out infinite; }
 #savebtn.dirty::before { content:"● "; }
 @keyframes savepulse {
   0%   { box-shadow:0 0 0 0 rgba(255,176,32,.6); }
   70%  { box-shadow:0 0 0 8px rgba(255,176,32,0); }
   100% { box-shadow:0 0 0 0 rgba(255,176,32,0); }
 }
 @media (prefers-reduced-motion: reduce) { #savebtn.dirty { animation:none; } }

 .layout { display:flex; align-items:flex-start; }
 /* Wider than it was, because the settings have the window now: four levels of
    indentation and a folder's whole path both need somewhere to go, and the
    column on the right is capped anyway */
 nav { width:min(340px, 32vw); flex:none; border-right:1px solid var(--line);
   min-height:calc(100vh - var(--headh)); padding:12px 10px; position:sticky;
   top:var(--headh); max-height:calc(100vh - var(--headh)); overflow:auto; }
 main { flex:1; min-width:0; padding:24px 28px; max-width:820px; }

 .navitem { display:block; width:100%; text-align:left; border:0; background:none; color:var(--text);
   padding:7px 10px; border-radius:7px; cursor:pointer; font-size:13.5px; font-family:inherit; }
 .navitem:hover { background:var(--panel); }
 .navitem.sel { background:var(--panel2); }
 .navitem .sub { display:block; color:var(--muted); font-size:11.5px; margin-top:1px;
   white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
 .navgroup { color:var(--muted); font-size:11px; letter-spacing:.08em; text-transform:uppercase;
   margin:14px 10px 4px; }
 .navgrouphead { color:var(--muted); font-size:11px; letter-spacing:.08em; text-transform:uppercase;
   margin:12px 0 2px; display:flex; align-items:center; gap:6px; }
 .navgrouphead.sel { color:var(--text); }
 .navgrouphead .caret { font-size:10px; width:14px; display:inline-block; text-align:center;
   border-radius:4px; }
 .navgrouphead .caret:hover { background:var(--panel2); }
 /* Four levels: everything, a workspace, one of its working folders, a tab in
    that folder. Depth is where the row starts, the way a file list does it --
    elbows drawn between every row would be three times the ink for the same
    three steps. The faint line is there to sight along when the names are long
    enough to fill the width */
 .navitem { position:relative; }
 .lvl1 { padding-left:22px; }
 .lvl2 { padding-left:38px; }
 .lvl3 { padding-left:54px; }
 .lvl1::before, .lvl2::before, .lvl3::before { content:""; position:absolute;
   top:0; bottom:0; width:1px; background:var(--line); }
 .lvl1::before { left:11px; }
 .lvl2::before { left:27px; }
 .lvl3::before { left:43px; }
 /* A folder is the level people are looking for, so it keeps its own weight
    while the tabs under it stay quiet */
 .navfolder { color:var(--text); font-size:12.5px; }
 .navfolder .sub { font-size:11px; }
 .navtab.child .nm { opacity:.9; }
 .navadd { color:var(--muted); font-size:12.5px; }

 .card { background:var(--panel); border:1px solid var(--line); border-radius:10px;
   padding:6px 18px 14px; margin-bottom:18px; }
 .card h2 { font-size:12px; color:var(--muted); font-weight:600; letter-spacing:.06em;
   margin:14px 0 10px; text-transform:uppercase; }
 /* The colours a project can be given. Squares rather than a list of names:
    the thing being chosen is the colour itself */
 .swatches { display:flex; flex-wrap:wrap; gap:8px; align-items:center; padding:4px 0 2px; }
 .swatches i { width:22px; height:22px; border-radius:6px; cursor:pointer; display:block;
   border:1px solid #0004; }
 .swatches i.on { outline:2px solid var(--text); outline-offset:2px; }
 .swatches i.any { background:conic-gradient(red,yellow,lime,aqua,blue,magenta,red); }
 .swatches input[type="color"] { position:absolute; width:0; height:0; opacity:0; padding:0;
   border:0; }
 .row { display:flex; align-items:center; gap:12px; padding:7px 0; flex-wrap:wrap; }
 /* Automation permissions. Two narrow columns on the right, everything else
    on the left, so the eye runs down a column instead of hunting across a row */
 .grantcols { display:flex; align-items:flex-end; gap:0; justify-content:flex-end;
   position:sticky; top:0; background:var(--panel); padding:6px 0 4px; z-index:1; }
 .grantcols span { width:104px; text-align:center; color:var(--muted); font-size:11.5px; }
 .grantcols .grow { width:auto; flex:1; }
 .granthead { display:flex; align-items:center; gap:8px; padding:10px 0 4px;
   border-top:1px solid var(--line); margin-top:4px; }
 .granthead b { font-size:12.5px; font-weight:600; }
 /* Who counts as what, and the one case where the answer surprises people */
 .grantwho { font-size:12.5px; line-height:1.65; margin:2px 0 10px; }
 .grantwho div + div { margin-top:4px; }
 .grantwarn { font-size:12.5px; line-height:1.65; margin:0 0 12px; padding:8px 12px;
   border-left:3px solid var(--danger); background:var(--panel2); border-radius:0 6px 6px 0; }
 .granthead .foldable { cursor:pointer; display:flex; align-items:center; gap:8px; }
 .granthead .caret { font-size:11px; width:14px; text-align:center; color:var(--muted); }
 .grantrow { display:flex; align-items:center; gap:8px; padding:4px 0; }
 .grantrow .nm { font-size:12.5px; }
 /* The name is a link, but a quiet one: the eye is here to find a row, not to
    be sold a destination */
 .grantrow a.nm { color:var(--text); text-decoration:none;
   border-bottom:1px dotted var(--muted); }
 .grantrow a.nm:hover { color:var(--accent); border-bottom-color:var(--accent); }
 .grantrow .sub { display:block; color:var(--muted); font-size:11.5px; margin-top:1px; }
 /* The two columns are one width, wherever they appear -- a heading's pair has
    to stand in the same place as the rows it answers for */
 .grantrow .cell, .granthead .cell { width:104px; flex:0 0 104px; display:flex;
   justify-content:center; }
 .grantrow .grow { min-width:0; }
 .grantrow.off .nm { color:var(--muted); }
 .grantmark { color:var(--muted); font-size:11px; margin-left:6px; }
 .row > label:first-child { width:150px; flex:none; color:var(--muted); font-size:13px; }
 /* A second (or third) label inside one row — "port", "user" next to a host.
    It names the field that follows it, so it sits tight against it rather than
    claiming the row's label column. */
 .row > label.beside { width:auto; }
 .hint { color:var(--muted); font-size:12px; }
 /* The line a tab will really be launched with. It wraps rather than scrolls:
    an argument pushed off the right edge is exactly the argument nobody would
    have seen otherwise */
 .realcmd { margin:6px 0 2px 0; }
 .realcmd code { display:block; margin:3px 0; padding:7px 9px; border-radius:6px;
   background:var(--raise); border:1px solid var(--line);
   white-space:pre-wrap; word-break:break-all; font-size:12px; }
 .realcmd .added { color:var(--brand); font-weight:600; }
 /* One row per entry in an editable list (quick actions, providers, notify
    targets, secrets): its name, its fields, then its buttons, divided by a
    hairline. It wraps, so a narrow screen stacks the parts instead of pushing
    them off the edge — which is also why this is a class and not four copies
    of the same inline style. */
 .listrow { display:flex; align-items:center; flex-wrap:wrap; gap:10px;
   padding:7px 0; border-bottom:1px solid var(--line); }
 /* For entries whose fields are taller than their buttons (a quick action's
    body box), so the buttons sit at the top rather than floating mid-height. */
 .listrow.tall { align-items:flex-start; gap:8px; padding:8px 0; }
 .grow { flex:1; min-width:180px; }
 .stoprow { display:flex; align-items:center; gap:8px; flex-wrap:wrap; padding:8px;
   margin:6px 0; border:1px solid var(--line); border-radius:8px; }
 .stoprow input { width:auto; }
 .stoprow input[type=number] { width:80px; }
 .stoprow .arrow { color:var(--muted); }
 #wsstopslist select { width:auto; }

 /* Which network the phone's connection link leads to. The tone names are its
    own (not the page-wide .warn, which is a paragraph of danger text) so that
    a badge stays a badge whatever else those words come to mean. */
 .netbadge { display:inline-flex; align-items:center; gap:5px; font-size:12px; font-weight:600;
   line-height:1.5; white-space:nowrap; border-radius:999px; padding:2px 10px; border:1px solid; }
 .netbadge.ok   { color:var(--live);   border-color:var(--live);
   background:color-mix(in srgb, var(--live) 14%, transparent); }
 .netbadge.care { color:var(--warn);   border-color:var(--warn);
   background:color-mix(in srgb, var(--warn) 14%, transparent); }
 .netbadge.risk { color:var(--danger); border-color:var(--danger);
   background:color-mix(in srgb, var(--danger) 14%, transparent); }
 .netbadge.mute { color:var(--muted);  border-color:var(--line); }

 input[type=text], input[type=number], select, textarea {
   background:var(--panel2); color:var(--text); border:1px solid var(--line); border-radius:7px;
   padding:7px 10px; font-size:13.5px; font-family:inherit; outline:none; }
 input:focus, select:focus, textarea:focus { border-color:var(--accent); }
 input[type=text]::placeholder, textarea::placeholder { color:#5d6773; }
 input[type=checkbox] { width:16px; height:16px; accent-color:var(--accent); margin:0; }
 label.check { display:flex; align-items:center; gap:8px; width:auto; color:var(--text);
   font-size:13.5px; cursor:pointer; }
 textarea { width:100%; min-height:220px; line-height:1.55; resize:vertical; }

 button { font-family:inherit; font-size:13px; border-radius:7px; cursor:pointer; padding:7px 14px;
   border:1px solid var(--line); background:var(--panel2); color:var(--text); }
 button:hover { border-color:#39424f; }
 button.primary { background:var(--accent); border-color:var(--accent); color:#04121c; font-weight:600; }
 button.quiet { background:none; border-color:transparent; color:var(--muted); padding:6px 8px; }
 button.quiet:hover { color:var(--text); background:var(--panel2); }
 a.quiet { font-size:13px; border-radius:7px; padding:6px 8px; color:var(--muted); text-decoration:none; align-self:center; white-space:nowrap; }
 a.quiet:hover { color:var(--text); background:var(--panel2); }
 button.danger { color:var(--danger); background:none; border-color:transparent; }
 button.danger:hover { background:rgba(229,83,75,.1); }

 details { border-top:1px solid var(--line); margin-top:6px; }
 details > summary { cursor:pointer; color:var(--muted); font-size:13px; padding:12px 0 4px;
   list-style:none; }
 details > summary::before { content:"▸ "; }
 details[open] > summary::before { content:"▾ "; }

 .events { display:flex; flex-direction:column; }
 .event { display:flex; align-items:center; gap:12px; padding:9px 0; border-bottom:1px solid var(--line); }
 .event:last-child { border-bottom:0; }
 .event .name { flex:1; }
 .event .state { color:var(--muted); font-size:12px; }
 .event .state.on { color:var(--accent); }

 .empty { color:var(--muted); text-align:center; padding:40px 20px; }
 .empty .big { font-size:15px; color:var(--text); margin-bottom:6px; }

 .modal { position:fixed; inset:0; background:rgba(0,0,0,.6); display:flex; align-items:center;
   justify-content:center; z-index:20; }
 .modal-inner { background:var(--panel); border:1px solid var(--line); border-radius:12px;
   width:min(880px,92vw); max-height:88vh; overflow:auto; padding:20px 24px; }
 .modal-inner h2 { text-transform:none; font-size:15px; color:var(--text); margin:0 0 4px; }
 /* The exact character a parser stopped at, inside an excerpt. */
 pre .at { background:var(--danger); color:#fff; border-radius:2px; padding:0 1px; }
 pre { background:var(--panel2); border:1px solid var(--line); border-radius:8px; padding:12px;
   overflow:auto; max-height:240px; font-size:12.5px; }
 a { color:var(--accent); }
 /* AI generation can take tens of seconds. Line up a spinner, a growing bar, and a
    progressing number so it's obvious at a glance that it hasn't stalled */
 #aibusy { flex-direction:column; gap:9px; margin-top:10px; padding:12px 14px;
   background:var(--panel2); border:1px solid var(--accent); border-radius:9px; }
 #aibusy .head { display:flex; align-items:center; gap:10px; }
 #aibusytext { color:var(--accent); font-weight:600; }
 .spin { width:16px; height:16px; flex:none; border-radius:50%;
   border:2px solid var(--line); border-top-color:var(--accent);
   animation:spin .8s linear infinite; }
 @keyframes spin { to { transform:rotate(360deg); } }
 .bar { height:4px; border-radius:2px; background:var(--line); overflow:hidden; }
 .bar > i { display:block; height:100%; width:35%; border-radius:2px;
   background:var(--accent); animation:slide 1.3s ease-in-out infinite; }
 @keyframes slide { from { margin-left:-35%; } to { margin-left:100%; } }

 /* ── Narrow screens (a phone reaching the settings over the remote proxy) ──
    The desktop layout is a fixed 260px sidebar next to the content. A phone has
    room for exactly one of the two, so below 760px the sidebar becomes a drawer
    behind the header's ☰ and the content gets the whole width. The header keeps
    only what a thumb needs there — ☰, where you are, Close, Save — and the
    secondary links MOVE into the drawer (the same element, not a second copy).

    align-items:stretch is what actually keeps the page inside the screen: the
    column layout's cross axis is horizontal, so flex-start would size `main` to
    its widest child and push the text off the edge instead of wrapping it. */
 @media (max-width: 760px) {
   header { padding:10px 12px; gap:10px; flex-wrap:nowrap; }
   header h1 { display:none; }
   /* Where you are beats what the app is called when the screen is this narrow. */
   #crumb { display:flex; flex:1; }
   .navtoggle { display:inline-flex; flex:none; }
   .atwide { display:none; }
   .atnarrow { display:inline; }
   /* The toast already announces every result, so the header line stays clear. */
   #msg { display:none; }
   header .spacer { display:none; }
   #backbtn, #savebtn { flex:none; }

   .layout { flex-direction:column; align-items:stretch; }
   /* The drawer: off-canvas, slid in over the content, dismissed by the scrim,
      by Escape, or by picking a section (a group's ▸ caret keeps it open). */
   nav { position:fixed; z-index:30; left:0; top:var(--headh); bottom:0;
     width:min(300px,84vw); min-height:0; max-height:none; overflow:auto;
     padding:12px 10px 28px; background:var(--panel);
     border-right:1px solid var(--line); border-bottom:none;
     transform:translateX(-102%); transition:transform .18s ease;
     box-shadow:0 0 30px rgba(0,0,0,.5); }
   body.navopen nav { transform:none; }
   /* Hold the page still while the drawer is over it. */
   body.navopen { overflow:hidden; }
   #navscrim { display:block; position:fixed; inset:0; z-index:25;
     background:rgba(0,0,0,.5); opacity:0; pointer-events:none;
     transition:opacity .18s ease; }
   body.navopen #navscrim { opacity:1; pointer-events:auto; }
   @media (prefers-reduced-motion: reduce) {
     nav, #navscrim { transition:none; }
   }
   /* In the drawer the links stack at the bottom, under a divider. */
   .headlinks { flex-direction:column; align-items:stretch; gap:2px;
     margin-top:14px; padding-top:10px; border-top:1px solid var(--line); }
   /* a.quiet centres itself in the header row; in the drawer it lines up with
      the nav items above it instead. */
   .headlinks > * { align-self:stretch; text-align:left; }

   main { padding:14px 14px 40px; max-width:100%; }
   .card { padding:6px 12px 12px; }
   /* One field per line: the label above, then the control, then its hint.
      A 150px label column next to a control leaves neither enough room. Every
      label breaks the line, not just the row's first — a row with several
      fields (host / port / user) would otherwise leave each label stranded at
      the end of the previous field's line, reading as if it named that one.
      A checkbox's own label is the exception: it belongs beside its box. */
   .row { flex-wrap:wrap; gap:6px 10px; }
   .row > label:not(.check), .row > label.beside { width:100%; }
   .hint { flex-basis:100%; }
   /* Fixed pixel widths on inputs/selects overflow a phone; cap them all, and
      let flex children shrink below their content (min-width:auto is what turns
      a long value into a page that scrolls sideways). */
   input, select, textarea, .row > input, .row > select { max-width:100%; }
   .row > *, .listrow > *, .stoprow > * { min-width:0; }
   input.mono, .grow { width:100%; }
   textarea { min-height:150px; }
   /* Paths, URLs and ids have no spaces to break at — break them anyway. */
   .hint, .event .name, code { overflow-wrap:anywhere; }
   .modal-inner { width:96vw; max-height:92vh; padding:16px 14px; }
 }
 /* Never let the page itself scroll sideways, whatever a stray wide child does. */
 @media (max-width: 760px) { body { overflow-x:hidden; } }
</style></head><body>

<header>
  <button class="quiet navtoggle" id="navtoggle" onclick="toggleNav()"
          aria-controls="nav" aria-expanded="false" title="{{settings.nav.open}}">☰</button>
  <h1>{{settings.title}}</h1>
  <span id="crumb"></span>
  <div class="spacer"></div>
  <span id="msg"></span>
  <div class="headlinks" id="headlinks">
    <a class="quiet" href="#" onclick="openExt('bug');return false" title="{{settings.report_bug}}">{{settings.report_bug}}</a>
    <a class="quiet" href="#" onclick="openExt('discussions');return false" title="{{settings.discussions}}">{{settings.discussions}}</a>
    <button class="quiet" onclick="load()">{{common.reload}}</button>
  </div>
  <button class="quiet" id="backbtn" onclick="closeSettings()" title="{{settings.close}}"
          aria-label="{{settings.close}}"><span class="atwide">{{settings.close}}</span><span
          class="atnarrow">✕</span></button>
  <button class="primary" id="savebtn" onclick="save()">{{common.save}}</button>
</header>

<div id="navscrim" onclick="closeNav()"></div>
<div class="layout">
  <nav id="nav"></nav>
  <main id="detail"></main>
</div>

{{TOAST_HTML}}

<datalist id="cmdlist"></datalist>

<div id="autobox" class="modal" style="display:none">
  <div class="modal-inner">
    <h2 id="autotitle">{{settings.tab.automation}}</h2>
    <div class="hint" id="autopath"></div>
    <div class="row" style="margin-top:12px">
      <label>{{automation.editor.when}}</label>
      <select id="autoevent" onchange="switchEvent()"></select>
      <span class="hint" id="autohint"></span>
    </div>
    <textarea id="autocode" spellcheck="false"
      placeholder="{{automation.editor.code.ph}}"></textarea>
    <div class="row" id="airow">
      <label>{{automation.editor.ask}}</label>
      <input type="text" id="autoask" class="grow"
             placeholder="{{automation.editor.ask.ph}}">
      <button onclick="askAi()" id="aibtn">{{automation.editor.generate}}</button>
    </div>
    <div id="aibusy" style="display:none">
      <div class="head"><span class="spin"></span><span id="aibusytext"></span></div>
      <div class="bar"><i></i></div>
    </div>
    <div class="row" id="ainone" style="display:none">
      <span class="hint">{{automation.editor.no_ai}}</span>
    </div>
    <div id="aipreview" style="display:none">
      <div class="hint">{{automation.editor.generated}}</div>
      <pre id="aicode"></pre>
      <button class="primary" onclick="applyAi()">{{automation.editor.apply}}</button>
      <button class="quiet" onclick="document.getElementById('aipreview').style.display='none'">{{automation.editor.discard}}</button>
    </div>
    <div class="row" style="border-top:1px solid var(--line); margin-top:12px; padding-top:14px">
      <button class="primary" onclick="saveAuto()">{{automation.editor.save}}</button>
      <button class="quiet" onclick="closeAuto()">{{common.cancel}}</button>
      <span class="spacer" style="flex:1"></span>
      <a href="/help?token=__TOKEN__" target="_blank">{{automation.editor.help}}</a>
      <span id="automsg" class="hint"></span>
    </div>
  </div>
</div>

<script>
const TOKEN = "__TOKEN__";
// True when this page is being read on a phone, over the remote proxy. Native
// dialogs (folder/file pickers, export/import) open a window on the PC instead
// of here, so the buttons that would summon one are left out entirely
const REMOTE = __REMOTE__;
const T = __DICT__;
// Every command there is, grouped, with the answer it has when nobody has said
// otherwise. Comes from the same list the app enforces, so the screen cannot
// show a command that does not exist or miss one that does
const GRANTS = __GRANTS__;
// The commit-message template the app ships with. Poured in rather than
// written out again here: "put the built-in one back" has to put back the one
// that actually runs
const GIT_MESSAGE_LUA = __GITLUA__;
// The branches guarded until somebody says otherwise. Poured in from the app
// so the box shows what is really running, not a copy of it kept here
const PROTECT_DEFAULT = __PROTECT__;
// A list of branch names as it is typed and as it is stored. Space or comma
// between them, because both are what people reach for
const protectList = text => (text || "").split(/[\s,]+/).filter(Boolean);
const protectText = list => (list || []).join(" ");
// What the app guards where a folder has not said anything of its own
const protectApp = () => {
  const g = current.git || {};
  return Array.isArray(g.protect) ? g.protect : PROTECT_DEFAULT;
};
// {name} substitution (same rule as tp on the Rust side)
const fill = (s, args) => Object.entries(args)
  .reduce((acc, [k, v]) => acc.replaceAll("{" + k + "}", v), s || "");
const api = (m, b) => fetch("/api/config", {
   method: m, headers: {"X-Token": TOKEN, "Content-Type":"application/json"}, body: b });
const wsApi = (m, file, b) => fetch("/api/workspace?file=" + encodeURIComponent(file), {
   method: m, headers: {"X-Token": TOKEN, "Content-Type":"application/json"}, body: b });

let current = {};        // Contents of config.json (holds the base settings)
let wss = [];            // Workspaces and tabs
let sel = {ws:0, tab:null, global:true, section:"basic"};
// Which workspaces are expanded in the sidebar. Collapsed by default so
// the nav stays tidy; the workspace you're editing auto-expands.
const navOpen = new Set();
// Workspaces somebody has folded on purpose. Kept apart from the ones opened
// on purpose, because "what you are looking at counts as open" would otherwise
// hold a workspace open for good -- selecting one is exactly what clicking its
// row does now, so the caret could never win
const navShut = new Set();
// The global-settings group is expanded by default (the page opens onto it).
let navGlobalOpen = true;
// When opened via a deep-link shortcut (?ret=1), returning to the board after a
// successful save is the natural finish, so the caller doesn't have to close it.
let returnOnSave = false;
// Set when one of the user's files came back unusable (broken JSON, unreadable).
// While it's held, the screen shows what's wrong and Save is off: the form has
// nothing in it, and writing it out would put that emptiness where the real
// configuration used to be.
let loadFailure = null;
let aiEngines = [];
// The language setting as of when the page was opened. Used at save time to check "did it change = is a restart needed?"
let loadedLanguage = "";

const el = (tag, attrs = {}, ...kids) => {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") n.className = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else if (v !== null && v !== undefined) n.setAttribute(k, v);
  }
  for (const c of kids) if (c !== null && c !== undefined) n.append(c);
  return n;
};
const msg = (t, warn) => {
  const m = document.getElementById("msg");
  m.textContent = t;
  m.classList.toggle("warn", !!warn);
  // Just re-adding the class doesn't replay it, so remove it and force a reflow first
  m.classList.remove("flash");
  void m.offsetWidth;
  if (!warn) m.classList.add("flash");
};

// Reads one of the user's JSON files. The server answers plainly when a file is
// there but unusable, so a broken file arrives as something to show rather than
// as a thrown parse and a blank screen.
async function readUserJson(res) {
  if (res.ok) {
    try { return {value: await res.json()}; }
    catch (e) { return {failure: {error: String(e)}}; }
  }
  const info = await res.json().catch(() => ({}));
  return {failure: {
    path: info.path || "",
    error: info.error || (res.status + " " + res.statusText),
    line: info.line || 0, column: info.column || 0, text: info.text || "",
  }};
}

{{TOAST_JS}}
// This screen reports results, and a result is either good news or bad — say
// which. Declared as a function so it exists from the moment the script starts,
// whatever order the pieces end up in
function toastText(text, warn) { return (warn ? "⚠ " : "✓ ") + text; }

// Keep the result in the header too, but also always make it noticeable via a toast
const result = (text, warn) => { msg(text, warn); toast(text, warn); };

// Determines whether there are unsaved changes. Remembers the content as of save/load and compares against it
let savedSnapshot = "";
// Compares "what would actually get written on save", not the raw input state.
// Input fields turn numbers into strings, so 10 and "10" would otherwise look different
const snapshot = () => JSON.stringify(payload());
function markClean() { savedSnapshot = snapshot(); refreshSave(); }
function refreshSave() {
  const b = document.getElementById("savebtn");
  // There's nothing to compare against until loading finishes. Without this bail-out, the unsaved mark would show the instant the page opens
  if (!b || b.disabled || savedSnapshot === "") return;
  const dirty = snapshot() !== savedSnapshot;
  b.classList.toggle("dirty", dirty);
  b.title = dirty ? T["settings.unsaved"] : "";
}
// Adding, removing, or reordering tabs doesn't fire an input event,
// so listening for events alone would miss it. Comparing the content directly is more reliable
setInterval(refreshSave, 600);

// ── Widgets ─────────────────────────────────────────────
// Stored internally in milliseconds, but shown to people in seconds.
// Letting someone read "10" rather than write "10000" makes for a more natural setting
function secondsField(obj, key, placeholderSec) {
  const i = el("input", {type:"number", step:"1", min:"0",
                         placeholder:String(placeholderSec), class:"grow"});
  i.style.maxWidth = "110px";
  const ms = obj[key];
  i.value = (ms === "" || ms === null || ms === undefined) ? "" : String(Number(ms) / 1000);
  i.oninput = () => {
    const v = i.value.trim();
    if (v === "") delete obj[key];
    else obj[key] = Math.round(Number(v) * 1000);
  };
  return i;
}
function field(obj, key, ph, opts = {}) {
  const i = el("input", {type: opts.type || "text", placeholder: ph,
                         class: (opts.mono ? "mono " : "") + (opts.grow === false ? "" : "grow")});
  if (opts.width) i.style.width = opts.width + "px";
  i.value = obj[key] ?? "";
  i.addEventListener("input", () => { obj[key] = i.value; if (opts.onInput) opts.onInput(i.value); });
  return i;
}
// Names this machine already knows: the installed WSL distributions, the hosts
// in the person's own ssh config.
//
// Asked for once and shared, because the answer costs a child process and does
// not change while a settings screen is open.
let SUGGEST = null;
function suggestions() {
  if (!SUGGEST) {
    SUGGEST = fetch("/api/discover", {headers:{"X-Token":TOKEN}})
      .then(r => r.json()).catch(() => ({}));
  }
  return SUGGEST;
}
// Hang a suggestion list off a text field.
//
// A list and not a menu, deliberately. Everything the machine knows about is
// one keystroke away instead of being typed from memory -- which is the whole
// point, since a distribution name recalled wrongly is a tab that fails to
// start and a command line that looks right. But a menu would also be a claim
// that the list is complete, and it never is: the distribution installed a
// minute ago is not in it, and neither is the host that lives in somebody's
// head. So typing still works, and typing something not on the list is not an
// error.
function suggest(input, key) {
  let list = document.getElementById("sug-" + key);
  if (!list) {
    // One per kind, kept on the page: the cards are rebuilt as a person walks
    // through them, and a new list per rebuild would pile up unseen.
    list = el("datalist", {id:"sug-" + key});
    document.body.append(list);
    suggestions().then(j => {
      for (const v of (j[key] || [])) list.append(el("option", {value:v}));
    });
  }
  input.setAttribute("list", list.id);
  return input;
}
function check(obj, key, label) {
  const i = el("input", {type:"checkbox"});
  i.checked = !!obj[key];
  i.addEventListener("change", () => { obj[key] = i.checked; });
  const l = el("label", {class:"check"}); l.append(i, document.createTextNode(label));
  return l;
}
// An item that's on by default. Doesn't distinguish "unset" from "on"; only holds false once unchecked
function checkDefaultOn(obj, key, label) {
  const i = el("input", {type:"checkbox"});
  i.checked = obj[key] !== false;
  i.addEventListener("change", () => {
    if (i.checked) delete obj[key];
    else obj[key] = false;
  });
  const l = el("label", {class:"check"}); l.append(i, document.createTextNode(label));
  return l;
}
function choose(obj, key, opts, onChange) {
  const s = el("select");
  for (const [v, label] of opts) s.append(el("option", {value:v}, label));
  s.value = obj[key] || "";
  s.addEventListener("change", () => { obj[key] = s.value; if (onChange) onChange(s.value); });
  return s;
}
// Writing the command field from a button (an AI picked from the list, the
// bypass-flag checkbox, a browser's URL) has to look exactly like typing it:
// the value, the tab-bar preview and the real-command line below all follow
// the field's own "input" event, so a write that skipped the event would
// leave one of the three showing a command that is no longer there.
function setCommand(t, input, value) {
  t.command = value;
  input.value = value;
  input.dispatchEvent(new Event("input", {bubbles: true}));
}

// The line a tab will really be launched with, shown under the command field.
//
// A command is read one character at a time and acted on immediately, so an
// argument nobody typed has no business being invisible: it was an invisible
// "--session-id" beside a hand-written "--resume" that once killed a tab on
// every restart, with nothing on screen to connect the two. The app answers
// this -- the page never assembles it -- because a second implementation here
// would be a second answer to "what runs", and the two would drift.
function launchLine(t) {
  const line = el("code", {class:"mono"});
  const note = el("div", {class:"hint"});
  const box = el("div", {class:"realcmd"},
    el("div", {class:"hint"}, T["settings.tab.command.real"]), line, note);
  let seq = 0, timer = null;
  const refresh = async () => {
    const mine = ++seq;
    let r = null;
    try {
      r = await fetch("/api/launch-line", {method:"POST",
        headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
        body: JSON.stringify({command: t.command || "", resume: t.resume || "",
                              profile: t.profile || ""})}).then(x => x.json());
    } catch (e) { r = null; }
    // A later keystroke has already asked; its answer is the current one
    if (mine !== seq) return;
    line.textContent = ""; note.textContent = "";
    if (!r || !r.argv || !r.argv.length) { box.hidden = true; return; }
    box.hidden = false;
    r.argv.forEach((a, i) => {
      if (i) line.append(document.createTextNode(" "));
      line.append(el("span", {class: (i >= 1 && i <= r.added) ? "added" : ""}, a));
    });
    // The same arguments mean two different things depending on the tab's own
    // answer to "come back to this conversation", and the line cannot show
    // both. `--session-id` is added either way -- it is what lets this app know
    // which conversation a tab is having at all, which Ctrl+B r and the Vault
    // both need -- so what changes here is what it is FOR
    if (r.added)
      note.textContent = t.restore_conversation === false
        ? T["settings.tab.command.real_note.off"]
        : T["settings.tab.command.real_note"];
  };
  // Typing is not a reason to ask on every keystroke, and the answer to a
  // half-typed command is not worth showing
  const schedule = () => { clearTimeout(timer); timer = setTimeout(refresh, 250); };
  refresh();
  return { box, schedule };
}
function row(label, ...kids) { return el("div", {class:"row"}, el("label", {}, label), ...kids); }
function card(title, ...kids) { return el("div", {class:"card"}, el("h2", {}, title), ...kids); }

// ── Tab id handling ────────────────────────────────────
// Collects existing tab ids within a workspace (falls back to the display name if there's no id).
// Used as candidates for reference fields (discussion participants/judge/moderator, stop-condition target tab)
function tabIds(ws) {
  return [...new Set((ws.tabs || [])
    .map(t => (t.id || t.name || "").trim())
    .filter(Boolean))];
}
// String → stable 5-char hash (FNV-1a 32-bit in base36). Used as id material
// for names with no ASCII alphanumerics at all, such as Japanese-only names
function hash5(s) {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 0x01000193); }
  return (h >>> 0).toString(36).padStart(5, "0").slice(-5);
}
// Infers an automation id from the display name.
// - Has ASCII alphanumerics (e.g. an English name) → slugify it (lowercase, join [a-z0-9]+ runs with "-")
// - Doesn't (Japanese-only, etc.) → 5-char hash. If the name is also empty, "" (leave as-is)
function slugId(name) {
  const parts = (name || "").toLowerCase().match(/[a-z0-9]+/g);
  if (parts && parts.length) return parts.join("-").slice(0, 24);
  const src = (name || "").trim();
  return src ? hash5(src) : "";
}
// Turns base into an id that doesn't collide within ws (-2, -3, ... if already used). self excludes itself
function uniqueId(ws, base, self) {
  if (!base) return "";
  const used = new Set((ws.tabs || [])
    .filter(t => t !== self).map(t => (t.id || "").trim()).filter(Boolean));
  if (!used.has(base)) return base;
  for (let n = 2; ; n++) { const c = base + "-" + n; if (!used.has(c)) return c; }
}
// Fills in an id derived from the name for tabs with an empty id (a safety net at save time). Leaves it alone if the name is also empty
function ensureIds(ws) {
  const tabs = ws.tabs || [];
  const used = new Set(tabs.map(t => (t.id || "").trim()).filter(Boolean));
  for (const t of tabs) {
    if ((t.id || "").trim()) continue;
    const base = slugId(t.name);
    if (!base) continue;
    let id = base, n = 2;
    while (used.has(id)) id = base + "-" + (n++);
    t.id = id; used.add(id);
  }
}
// A dropdown for picking a tab id (candidates = existing tab ids). emptyLabel is the label for the empty option.
// Pass exclude(t)=>bool when tabs that are aimed at something should be excluded
function idSelect(ws, val, emptyLabel, onChange, exclude) {
  const s = el("select");
  s.append(el("option", {value:""}, emptyLabel));
  const ids = [...new Set((ws.tabs || [])
    .filter(t => !(exclude && exclude(t)))
    .map(t => (t.id || t.name || "").trim()).filter(Boolean))];
  for (const id of ids) s.append(el("option", {value:id}, id));
  // Also allow selecting an existing value that isn't among the candidates (don't erase a hand-typed value)
  if (val && !ids.includes(val)) s.append(el("option", {value:val}, fill(T["settings.tab.missing_option"], {name: val})));
  s.value = val || "";
  s.addEventListener("change", () => { onChange(s.value || null); refreshSave(); });
  return s;
}

async function pickPath(kind, title, start) {
  try {
    const r = await fetch("/api/pick", {method:"POST",
        headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
        body: JSON.stringify({kind, title, start: start || ""})});
    const j = await r.json();
    return j.ok ? j.path : null;
  } catch (e) { return null; }
}
function pathField(obj, key, ph, kind, title) {
  const i = field(obj, key, ph, {mono:true});
  // On a phone the path is typed in; the picker would open on the PC
  if (REMOTE) return [i];
  const b = el("button", {class:"quiet", onclick: async () => {
    const p = await pickPath(kind, title, obj[key]);
    if (p !== null) { obj[key] = p; i.value = p; }
  }}, T["common.browse"]);
  return [i, b];
}

// ── Command assembly (SSH / Docker / WSL) ───────────
function parseSsh(cmd) {
  const t = (cmd || "").trim().split(/\s+/);
  if (t[0] !== "ssh") return null;
  const o = {host:"", port:"", user:"", key:"", agent:false, x11:false,
             forwards:[], jump:"", keepalive:"", extra:[]};
  for (let i = 1; i < t.length; i++) {
    const a = t[i];
    if (a === "-p") o.port = t[++i] || "";
    else if (a === "-i") o.key = t[++i] || "";
    else if (a === "-J") o.jump = t[++i] || "";
    else if (a === "-A") o.agent = true;
    else if (a === "-X") o.x11 = true;
    else if (a === "-L" || a === "-R" || a === "-D") o.forwards.push(a + " " + (t[++i] || ""));
    else if (a === "-o") {
      const v = t[++i] || "";
      const m = v.match(/^ServerAliveInterval=(\d+)$/);
      if (m) o.keepalive = m[1]; else o.extra.push("-o " + v);
    }
    else if (a.startsWith("-")) o.extra.push(a);
    else if (!o.host) {
      const at = a.split("@");
      if (at.length === 2) { o.user = at[0]; o.host = at[1]; } else o.host = a;
    }
    else o.extra.push(a);
  }
  return o;
}
function buildSsh(o) {
  const p = ["ssh"];
  if (o.port) p.push("-p", o.port);
  if (o.key) p.push("-i", o.key);
  if (o.jump) p.push("-J", o.jump);
  if (o.agent) p.push("-A");
  if (o.x11) p.push("-X");
  if (o.keepalive) p.push("-o", "ServerAliveInterval=" + o.keepalive);
  for (const f of o.forwards) if (f.trim()) p.push(f.trim());
  p.push(...o.extra);
  if (o.host) p.push((o.user ? o.user + "@" : "") + o.host);
  return p.join(" ");
}
function parseDocker(cmd) {
  const t = (cmd || "").trim().split(/\s+/);
  if (t[0] !== "docker" || t[1] !== "exec") return null;
  const o = {container:"", dir:"", shell:""}; const rest = [];
  for (let i = 2; i < t.length; i++) {
    const a = t[i];
    if (a === "-w") o.dir = t[++i] || "";
    else if (a === "-it" || a === "-i" || a === "-t") continue;
    else if (a.startsWith("-")) rest.push(a);
    else if (!o.container) o.container = a;
    else rest.push(a);
  }
  o.shell = rest.join(" ");
  return o;
}
const buildDocker = o => ["docker exec -it", o.dir ? "-w " + o.dir : "", o.container,
  o.shell || "bash"].filter(Boolean).join(" ");
function parseWsl(cmd) {
  const t = (cmd || "").trim().split(/\s+/);
  if (t[0] !== "wsl") return null;
  const o = {distro:"", dir:"", shell:""}; const rest = [];
  for (let i = 1; i < t.length; i++) {
    const a = t[i];
    if (a === "-d" || a === "--distribution") o.distro = t[++i] || "";
    else if (a === "--cd") o.dir = t[++i] || "";
    else if (a === "--") { rest.push(...t.slice(i + 1)); i = t.length; }
    else rest.push(a);
  }
  o.shell = rest.join(" ");
  return o;
}
const buildWsl = o => ["wsl", o.distro ? "-d " + o.distro : "", o.dir ? "--cd " + o.dir : "",
  o.shell ? "-- " + o.shell : ""].filter(Boolean).join(" ");

// Parses a browser command. Its only payload is a single URL.
// The leading word (browser / web) belongs to whoever wrote it, so keep it as-is
function parseBrowser(c) {
  const m = /^\s*(browser|web)\s+(\S.*)$/i.exec(cmdToText(c));
  return m ? {head: m[1], url: m[2].trim()} : null;
}
const buildBrowser = o => (o.head || "browser") + " " + (o.url || "");
/// Whether the URL can be embedded in the window. file: and data: can't be opened
const openableUrl = u => /^https?:\/\/\S/i.test((u || "").trim());

// Model (API connection). The primitive is a single "model provider/model-name" line.
// The provider is everything up to the first "/"; the rest is the whole model name (split only
// on the first "/" because some names, like ollama's huihui_ai/qwen..., contain their own "/")
function parseModel(c) {
  // Match a bare "model" too (provider/name still empty) so switching the Kind
  // to Model shows the model panel before anything is filled in.
  const m = /^\s*model(?:\s+(\S.*?))?\s*$/i.exec(cmdToText(c));
  if (!m) return null;
  const rest = (m[1] || "").trim();
  const i = rest.indexOf("/");
  return {provider: i >= 0 ? rest.slice(0, i) : rest, model: i >= 0 ? rest.slice(i + 1) : ""};
}
const buildModel = o => "model " + (o.provider || "") + (o.model ? "/" + o.model : "");
// Default model name per provider (a convenience value that lets input be skipped; empty if none)
const DEFAULT_MODEL = {deepseek: "deepseek-chat"};

// The git panel is the word on its own -- `git status` in a tab is somebody
// who wants a terminal that runs git, and it stays one
const isGitPanel = c => cmdToText(c).trim().toLowerCase() === "git";
const kindOf = c => isGitPanel(c) ? "git" : parseBrowser(c) ? "browser" : parseModel(c) ? "model"
  : parseSsh(c) ? "ssh" : parseDocker(c) ? "docker" : parseWsl(c) ? "wsl" : "cmd";
// CLI-type AIs (external programs the user installs). check = engine id to
// look up in aiEngines for the "(not installed)" note.
const AI_CLIS = [
  {label:"Claude Code", cmd:"claude", check:"claude"},
  {label:"Codex CLI",   cmd:"codex",  check:"codex"},
  {label:"Gemini CLI",  cmd:"gemini", check:"gemini"},
  {label:"Aider",       cmd:"aider",  check:null},
];
// Plain shells (the "Command" category).
const SHELL_CMDS = [
  {label:"PowerShell",  cmd:"powershell.exe", check:null},
  {label:T["settings.tab.kind.cmdprompt"], cmd:"cmd.exe", check:null},
];
// All launchers, for the command field's datalist.
const COMMON_COMMANDS = AI_CLIS.concat(SHELL_CMDS);
// A "cmd" tab whose head is one of these is an AI CLI, so it groups under the
// AI category rather than the plain-shell one.
const AI_CLI_HEADS = ["claude", "codex", "gemini", "aider", "kimi"];
const headOf = c => (cmdToText(c).trim().split(/\s+/)[0] || "").toLowerCase().replace(/\.exe$/, "");
const isAiCli = c => AI_CLI_HEADS.includes(headOf(c));

// Only an interactive AI CLI or a model (API connection) tab can join a discussion (AI vs AI).
// Browser, shell (cmd/PowerShell/SSH/Docker/WSL), and Aider are excluded from discussions
const DISCUSS_HEADS = ["claude", "codex", "gemini", "kimi"];
function isDiscussable(t) {
  const c = cmdToText(t.command).trim();
  const k = kindOf(c);
  if (k === "model") return true;
  if (k !== "cmd") return false;
  const head = c.split(/\s+/)[0].toLowerCase().replace(/\.exe$/, "");
  return DISCUSS_HEADS.includes(head);
}
const cmdToText = c => Array.isArray(c) ? c.join(" ") : (c || "");

// Top-level category for the tab-kind selector. The coarse "cmd" kind splits
// into an AI CLI (grouped under AI) vs a plain shell; a model tab is AI (API).
// Others map 1:1. CLI AIs and API providers then sit side by side inside the AI panel.
const catOf = c => { const k = kindOf(cmdToText(c));
  return k === "model" ? "ai" : k === "cmd" ? (isAiCli(c) ? "ai" : "cmd") : k; };
const CAT_START = {ai:"claude", cmd:"", ssh:"ssh ", docker:"docker exec -it ", wsl:"wsl ",
  browser:"browser https://", git:"git"};
const CAT_LIST = [
  ["ai",      T["settings.tab.cat.ai"]],
  ["cmd",     T["settings.tab.cat.cmd"]],
  ["ssh",     "SSH"],
  ["docker",  "Docker"],
  ["wsl",     "WSL"],
  ["browser", T["settings.tab.kind.browser"]],
  ["git",     T["settings.tab.kind.git"]],
];

// ── Sidebar ───────────────────────────────────────
// What a folder is called in a list: what someone typed, else the folder itself
function folderLabel(g, i) {
  const name = (g.name || "").trim();
  if (name) return name;
  const cwd = (g.cwd || "").trim();
  if (!cwd) return T["settings.group.folder.ph"];
  return cwd.split(/[\\/]/).filter(Boolean).pop() || cwd;
}

function renderNav() {
  const nav = document.getElementById("nav");
  nav.textContent = "";
  // Global settings: a collapsible group whose children are the flat, self-named
  // cards. Its ▸/▾ caret toggles it freely (unlike a workspace group, it is NOT
  // forced open while a section is selected) — so it can be collapsed even while
  // you're viewing a section, and it starts collapsed when you arrive via the gear.
  const gOpen = navGlobalOpen;
  nav.append(el("button", {class:"navitem navgrouphead",
    onclick:() => { navGlobalOpen = !navGlobalOpen; render(); }},
    el("span", {class:"caret"}, gOpen ? "▾" : "▸"),
    el("span", {}, T["settings.global"])));
  if (gOpen) globalSections().forEach(s => {
    const b = el("button", {class:"navitem lvl1" + (sel.global && sel.section === s.id ? " sel" : ""),
      onclick:() => { navGlobalOpen = true; sel = {ws:sel.ws, tab:null, global:true, section:s.id}; render();
                      const cur = document.querySelector(".navitem.sel"); if (cur) cur.scrollIntoView({block:"nearest"}); }});
    b.append(el("span", {}, s.label));
    if (s.sub) b.append(el("span", {class:"sub"}, s.sub));
    nav.append(b);
  });

  wss.forEach((ws, wi) => {
    // The group you're editing counts as open even without an explicit toggle.
    const open = !navShut.has(wi) && (navOpen.has(wi) || (!sel.global && sel.ws === wi));
    // `?? null` because a selection made elsewhere (the gear, a deep link) may
    // simply not mention a folder, and "no folder" has to match "no folder"
    const here = (g, t) => !sel.global && sel.ws === wi && (sel.grp ?? null) === g
      && (sel.tab ?? null) === t;
    // The name is the workspace's own page and the caret is the fold, the way
    // a folder's row works everywhere else. A separate "workspace settings"
    // row underneath made the folders look like its equals rather than its
    // contents, which is the one thing this list has to get across
    nav.append(el("button", {class:"navitem navgrouphead" + (here(null, null) ? " sel" : ""),
      onclick:() => { sel = {ws:wi, grp:null, tab:null, global:false}; render(); }},
      el("span", {class:"caret", onclick:e => {
        e.stopPropagation();
        if (open) { navShut.add(wi); navOpen.delete(wi); }
        else { navShut.delete(wi); navOpen.add(wi); }
        render();
      }}, open ? "▾" : "▸"),
      el("span", {}, ws.name || T["settings.tab.unnamed"])));
    if (!open) return;
    // Every folder, always -- the one a workspace starts with is a folder like
    // any other, and hiding it is how "where does this actually run" became
    // impossible to find
    (ws.folders || []).forEach((g, gi) => {
      nav.append(el("button", {class:"navitem navfolder lvl1" + (here(gi, null) ? " sel" : ""),
        onclick:() => { sel = {ws:wi, grp:gi, tab:null, global:false}; render(); }},
        el("span", {}, folderLabel(g, gi)),
        el("span", {class:"sub"}, g.cwd || T["settings.group.folder.ph"])));
      (ws.tabs || []).forEach((t, ti) => {
        if ((t.group || 0) !== gi) return;
        const b = el("button", {class:"navitem navtab lvl" + (t.depth ? 3 : 2) +
          (here(gi, ti) ? " sel" : ""),
          onclick:() => { sel = {ws:wi, grp:gi, tab:ti, global:false}; render(); }});
        b.append(el("span", {class:"nm"}, t.name || T["settings.tab.unnamed"]));
        b.append(el("span", {class:"sub"}, cmdToText(t.command) || T["automation.unset"]));
        nav.append(b);
      });
      nav.append(el("button", {class:"navitem navadd lvl2",
        onclick:() => {
          sel = {ws:wi, grp:gi, tab:addTabTo(ws, gi), global:false};
          render();
        }},
        T["settings.tab.add"]));
    });
    nav.append(el("button", {class:"navitem navadd lvl1",
      onclick:() => {
        (ws.folders = ws.folders || []).push({name:"", id:"", cwd:""});
        sel = {ws:wi, grp:ws.folders.length - 1, tab:null, global:false};
        render(); refreshSave();
      }},
      T["settings.group.add"]));
  });
  nav.append(el("div", {class:"navgroup"}, ""));
  nav.append(el("button", {class:"navitem navadd", onclick:addWs}, T["settings.workspace.add"]));
}

const newTab = (o = {}) => Object.assign(
  {name:"", id:"", command:"", profile:"", automation:"", locked:false, auto_restart:false,
   browser_profile:"", private:false,
   encoding:"", scrollback:"", log:false, depth:0, group:0}, o);

// Index of a tab with both an empty name and empty command (still in progress). -1 if there is none.
// Even if "Add tab" is clicked repeatedly, if an empty, unwritten tab already exists,
// this just jumps to it instead — so empty tabs don't pile up
function firstEmptyTab(ws) {
  return (ws.tabs || []).findIndex(t =>
    !(t.name || "").trim() && !(t.command || "").trim() && !(t.id || "").trim());
}

// Adds one tab. But if there's already an in-progress empty tab, just selects that instead.
// Returns the index of the added (or found) tab
function addTabTo(ws, group) {
  ws.tabs = ws.tabs || [];
  // Which folder it will run in. Named by the caller, else the one being
  // looked at, else the first: a tab has to be somewhere, and "somewhere"
  // was the part nobody could answer when the button was at the bottom of
  // the whole list
  if (group === undefined || group === null) {
    const at = ws.tabs[sel.tab];
    group = at ? (at.group || 0) : (sel.grp || 0);
  }
  let i = ws.tabs.findIndex(t => (t.group || 0) === group
    && !(t.name || "").trim() && !(t.command || "").trim() && !(t.id || "").trim());
  if (i < 0) {
    // Beside the others in the same folder, so the list stays in folder order
    let j = ws.tabs.length;
    while (j > 0 && (ws.tabs[j - 1].group || 0) > group) j--;
    ws.tabs.splice(j, 0, newTab({group}));
    i = j;
  }
  return i;
}

// ── New-workspace wizard ───────────────────────
// The from-scratch flow: just pick a purpose (a template), then pick AIs from a dropdown,
// and tabs, discuss blocks, stop conditions, and personas are auto-generated behind the scenes.
// The primitives (model x/y, discuss, etc.) stay as they are. Only a thin GUI is added on top.

// A dynamic modal. Pass in the content's DOM and it's shown centered. Clicking the background closes it.
function openModal(...kids) {
  const inner = el("div", {class:"modal-inner"}, ...kids);
  const back = el("div", {class:"modal"}, inner);
  // Closed by a press on the backdrop, not by a click on it. A click belongs to
  // the nearest ancestor of where the button went DOWN and where it came UP, and
  // the backdrop covers the whole screen -- so selecting text in a field and
  // letting go past the dialog's edge (a hurried drag to the end of a line) was
  // a "click on the backdrop", and the form vanished with everything typed into
  // it. Where the press landed is the only thing that says what was meant.
  back.addEventListener("mousedown", e => { if (e.target === back) back.remove(); });
  document.body.append(back);
  return back;
}

// AIs selectable as a participant = interactive AI CLIs + registered model connections
function aiChoices() {
  const cli = [
    {key:"claude", label:"Claude Code"},
    {key:"codex",  label:"Codex CLI"},
    {key:"gemini", label:"Gemini CLI"},
  ];
  const models = Object.keys(current.providers || {}).map(n =>
    ({key:"model:" + n, label: fill(T["wizard.discuss.model_suffix"], {name: n}), isModel:true, provider:n}));
  return cli.concat(models);
}
const choiceOf = key => aiChoices().find(c => c.key === key) || null;
const aiLabelOf = key => { const c = choiceOf(key); return c ? c.label : (key || "AI"); };
// Builds the launch command from the selection key (+ model name). Wizard-made AIs run
// autonomously (a discussion saves a statement every turn, code review commits, browser-op
// writes step files), so each CLI gets its "skip confirmation prompts" flag. It is not hidden:
// the flag also shows in the tab's Command field afterwards, so it stays editable and visible.
// Model-API participants need no flag (the bridge writes the reply itself, in-process).
function aiCommandOf(key, model) {
  if (key && key.startsWith("model:")) {
    const p = key.slice(6);
    return "model " + p + "/" + ((model || "").trim() || DEFAULT_MODEL[p] || "");
  }
  if (key === "claude") return "claude " + cliFlagOf("claude");
  if (key === "codex")  return "codex " + cliFlagOf("codex");
  if (key === "gemini") return "gemini " + cliFlagOf("gemini");
  return key || "";  // kimi and anything else: bare command (no known bypass flag)
}
// The "act without asking" flag each CLI needs to run autonomously (a
// discussion / automation stalls without it). Surfaced explicitly in the tab
// editor as a checkbox with a risk note — never injected silently.
function cliFlagOf(head) {
  if (head === "claude") return "--dangerously-skip-permissions";
  if (head === "codex")  return "--dangerously-bypass-approvals-and-sandbox";
  if (head === "gemini") return "--yolo";
  return "";  // aider / kimi / others: no known bypass flag
}
// AI selection + (only for a model API) a model name. Writes back to st={key,model}
// "Candidates" button: lists a provider's real models via {base_url}/models so
// the user picks an existing model name instead of guessing. getProv() returns
// the provider spec {base_url, api_key, headers?}; onPick(id) fills the model.
// Returns { btn, chips } — put btn inline and chips just below.
function modelCandidates(getProv, onPick) {
  const chips = el("div", {style:"display:flex;gap:6px;flex-wrap:wrap;margin-top:6px"});
  // Fetch the provider's real models, render them as chips, and return the list
  // (empty on failure). Callers can auto-select the first when nothing is set.
  async function load() {
    const prov = getProv() || {};
    chips.textContent = "";
    chips.append(el("span", {class:"hint"}, T["settings.model.candidates_loading"]));
    let r;
    try {
      r = await fetch("/api/provider/models", {method:"POST",
        headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
        body: JSON.stringify({base_url: prov.base_url || "", api_key: prov.api_key || "", headers: prov.headers || {}})})
        .then(x => x.json());
    } catch (e) { r = {ok:false, error:String(e)}; }
    chips.textContent = "";
    if (!r || !r.ok) {
      chips.append(el("span", {class:"hint"}, fill(T["settings.model.candidates_failed"], {e: (r && r.error) || ""})));
      return [];
    }
    const models = r.models || [];
    if (!models.length) { chips.append(el("span", {class:"hint"}, T["settings.model.candidates_none"])); return []; }
    for (const id of models) chips.append(el("button", {class:"quiet", type:"button",
      style:"font-size:12px;padding:2px 8px", onclick:() => onPick(id)}, id));
    return models;
  }
  const btn = el("button", {class:"quiet", type:"button", onclick: load}, T["settings.model.candidates"]);
  return {btn, chips, load};
}

function aiPick(st) {
  const row = el("span", {style:"display:inline-flex;gap:8px;align-items:center;flex-wrap:wrap"});
  const sel = el("select");
  for (const c of aiChoices()) sel.append(el("option", {value:c.key}, c.label));
  sel.value = st.key || "claude"; st.key = sel.value;
  const modelIn = el("input", {type:"text", class:"mono", style:"width:180px"});
  modelIn.value = st.model || "";
  const cand = modelCandidates(
    () => current.providers[(choiceOf(st.key) || {}).provider] || {},
    id => { st.model = id; modelIn.value = id; });
  const sync = () => {
    const c = choiceOf(st.key), isM = !!(c && c.isModel);
    modelIn.style.display = isM ? "" : "none";
    cand.btn.style.display = isM ? "" : "none";
    cand.chips.style.display = isM ? "" : "none";
    if (!isM) cand.chips.textContent = "";
    if (isM) modelIn.placeholder = DEFAULT_MODEL[c.provider] || T["wizard.discuss.model_ph"];
  };
  sel.addEventListener("change", () => { st.key = sel.value; sync(); });
  modelIn.addEventListener("input", () => { st.model = modelIn.value.trim(); });
  sync();
  row.append(sel, modelIn, cand.btn);
  return el("span", {style:"display:inline-block"}, row, cand.chips);
}
// A model-API participant needs a model name (except for a provider that has a default value)
function partsValid(parts) {
  for (const p of parts) {
    const c = choiceOf(p.key);
    if (c && c.isModel && !(p.model || "").trim() && !DEFAULT_MODEL[c.provider])
      return T["wizard.discuss.model_required"];
  }
  return null;
}
function landOnWs(ws) {
  // Made by a wizard, a template or from nothing -- all of them arrive here, so
  // this is the one place that has to make sure a workspace has its folder
  if (!(ws.folders || []).length) ws.folders = [{name:"", id:"", cwd:""}];
  (ws.tabs || []).forEach(t => { if (t.group === undefined) t.group = 0; });
  wss.push(ws); sel = {ws:wss.length - 1, tab:null, global:false}; render(); refreshSave();
}

// "+ Add workspace" → first, have the user pick a purpose
function addWs() {
  const m = openModal();
  const pick = fn => { m.remove(); fn(); };
  const opt = (emoji, title, desc, fn) => el("button",
    {class:"quiet", style:"display:flex;gap:12px;align-items:flex-start;text-align:left;" +
      "width:100%;padding:14px;border:1px solid var(--line);border-radius:10px;margin:8px 0",
     onclick:() => pick(fn)},
    el("span", {style:"font-size:22px;line-height:1"}, emoji),
    el("span", {}, el("div", {style:"color:var(--text);font-weight:600;margin-bottom:2px"}, title),
      el("div", {class:"hint"}, desc)));
  m.firstChild.append(
    el("h2", {}, T["wizard.pick.title"]),
    el("div", {class:"hint"}, T["wizard.pick.hint"]),
    el("div", {style:"margin-top:8px"},
      opt("🗣", T["wizard.pick.discuss.title"], T["wizard.pick.discuss.desc"], wizardDiscuss),
      opt("🌐", T["wizard.pick.browser.title"], T["wizard.pick.browser.desc"], wizardBrowser),
      opt("👨\u200d💻", T["wizard.pick.review.title"], T["wizard.pick.review.desc"], wizardReview),
      opt("🖥", T["wizard.pick.blank.title"], T["wizard.pick.blank.desc"], createBlankWs),
      // A file dialog is the only way in, and a phone has none to open
      REMOTE ? null
             : opt("📂", T["wizard.pick.import.title"], T["wizard.pick.import.desc"], importWs)),
    el("div", {class:"row", style:"margin-top:6px"},
      el("button", {class:"quiet", onclick:() => m.remove()}, T["common.cancel"])));
}
function createBlankWs() {
  landOnWs({name: T["settings.workspace"], automation:"", tabs:[]});
}

// 🗣 Discussion wizard
function wizardDiscuss() {
  const parts = [{key:"claude", model:"", name:"", persona:""},
                 {key:"claude", model:"", name:"", persona:""}];
  const st = {verdict:"winner", judge:""};
  const nameIn = el("input", {value:T["wizard.discuss.default_name"], style:"width:220px"});
  const list = el("div");
  const draw = () => {
    list.textContent = "";
    parts.forEach((p, i) => {
      const nm = el("input", {value:p.name || "", placeholder:T["wizard.discuss.name_ph"], style:"width:160px"});
      nm.addEventListener("input", () => p.name = nm.value);
      const persona = el("textarea", {rows:2, style:"width:100%;box-sizing:border-box;margin-top:6px",
        placeholder:T["wizard.discuss.persona_ph"]});
      persona.value = p.persona || "";
      persona.addEventListener("input", () => p.persona = persona.value);
      const del = el("button", {class:"quiet", onclick:() => {
        if (parts.length > 2) { parts.splice(i, 1); draw(); } else toast(T["wizard.discuss.min_participants"], true);
      }}, T["common.delete"]);
      list.append(el("div", {style:"border:1px solid var(--line);border-radius:9px;padding:9px;margin:7px 0"},
        el("div", {class:"row", style:"align-items:center;gap:8px"},
          el("span", {class:"mono", style:"color:var(--muted)"}, "#" + (i + 1)), nm, aiPick(p), del),
        persona));
    });
  };
  draw();
  const addBtn = el("button", {class:"quiet", onclick:() => {
    parts.push({key:"claude", model:"", name:"", persona:""}); draw();
  }}, T["wizard.discuss.add_participant"]);
  const judgeSel = el("select");
  judgeSel.append(el("option", {value:""}, T["wizard.discuss.judge_none"]));
  for (const c of aiChoices().filter(c => !c.isModel))
    judgeSel.append(el("option", {value:c.key}, c.label));
  judgeSel.addEventListener("change", () => st.judge = judgeSel.value);
  const verdictSel = el("select");
  for (const [v, l] of [["winner",T["wizard.discuss.verdict.winner"]],["synthesis",T["wizard.discuss.verdict.synthesis"]]])
    verdictSel.append(el("option", {value:v}, l));
  verdictSel.addEventListener("change", () => st.verdict = verdictSel.value);
  const m = openModal(
    el("h2", {}, T["wizard.discuss.title"]),
    el("div", {class:"hint"}, T["wizard.discuss.hint"]),
    row(T["settings.workspace.name"], nameIn),
    el("div", {style:"margin-top:10px;color:var(--text);font-size:13px"}, T["wizard.discuss.participants_label"]), list, addBtn,
    el("div", {class:"row", style:"margin-top:12px"}, el("label", {}, T["wizard.discuss.judge_label"]), judgeSel,
      el("label", {class:"beside"}, T["wizard.discuss.verdict_label"]), verdictSel),
    el("div", {class:"row"}, el("label", {}, ""),
      el("span", {class:"hint"}, T["wizard.discuss.note"])),
    el("div", {class:"row", style:"border-top:1px solid var(--line);margin-top:12px;padding-top:14px"},
      el("button", {class:"primary", onclick:() => {
        const err = partsValid(parts); if (err) { toast(err, true); return; }
        const tabs = [], personas = {}, agents = [];
        parts.forEach((p, i) => {
          const id = "p" + (i + 1);
          tabs.push(newTab({name: p.name.trim() || aiLabelOf(p.key), id, command: aiCommandOf(p.key, p.model)}));
          if ((p.persona || "").trim()) personas[id] = p.persona.trim();
          agents.push(id);
        });
        const discuss = {agents, order:"round-robin", max_rounds:6, verdict: st.verdict, personas};
        if (st.judge) { tabs.push(newTab({name:T["wizard.discuss.judge_tab_name"], id:"ref", command: aiCommandOf(st.judge, "")})); discuss.judge = "ref"; }
        m.remove();
        landOnWs({name: nameIn.value.trim() || T["wizard.discuss.default_name"], automation:"", tabs, discuss});
      }}, T["wizard.discuss.create"]),
      el("button", {class:"quiet", onclick:() => m.remove()}, T["common.cancel"])));
}

// 🌐 Browser-control wizard
function wizardBrowser() {
  const nameIn = el("input", {value:T["wizard.browser.default_name"], style:"width:220px"});
  const urlIn = el("input", {class:"mono", placeholder:"https://example.com/",
    style:"width:100%;box-sizing:border-box"});
  const ai = {key:"claude", model:""};
  const m = openModal(
    el("h2", {}, T["wizard.browser.title"]),
    el("div", {class:"hint"}, T["wizard.browser.hint"]),
    row(T["settings.workspace.name"], nameIn),
    el("div", {class:"row", style:"margin-top:8px"}, el("label", {}, T["wizard.browser.url_label"]), urlIn),
    el("div", {class:"row"}, el("label", {}, T["wizard.browser.ai_label"]), aiPick(ai)),
    el("div", {class:"row", style:"border-top:1px solid var(--line);margin-top:12px;padding-top:14px"},
      el("button", {class:"primary", onclick:() => {
        const url = urlIn.value.trim();
        if (!openableUrl(url)) { toast(T["wizard.browser.url_required"], true); return; }
        const err = partsValid([ai]); if (err) { toast(err, true); return; }
        const page = newTab({name:T["wizard.browser.page_tab_name"], id:"page", command:"browser " + url,
          nav:{back:true, forward:true, reload:true, url:true}});
        const aiTab = newTab({name:"AI", id:"ai", command: aiCommandOf(ai.key, ai.model), drives:"page"});
        m.remove();
        landOnWs({name: nameIn.value.trim() || T["wizard.browser.default_name"], automation:"", tabs:[page, aiTab]});
      }}, T["wizard.browser.create"]),
      el("button", {class:"quiet", onclick:() => m.remove()}, T["common.cancel"])));
}

// 👨‍💻 Code review (Git integration) wizard
const CODER_PERSONA = T["wizard.review.persona.coder"];
const REVIEW_ROLES = [
  {label:T["wizard.review.role.ui"], id:"ui", persona:T["wizard.review.persona.ui"]},
  {label:T["wizard.review.role.security"], id:"security", persona:T["wizard.review.persona.security"]},
  {label:T["wizard.review.role.perf"], id:"perf", persona:T["wizard.review.persona.perf"]},
  {label:T["wizard.review.role.test"], id:"test", persona:T["wizard.review.persona.test"]},
  {label:T["wizard.review.role.custom"], id:"", persona:""},
];
function wizardReview() {
  const nameIn = el("input", {value:T["wizard.review.default_name"], style:"width:220px"});
  const repo = {dir:""};
  const coder = {key:"claude", model:""};
  const revs = [{role:T["wizard.review.role.ui"], key:"claude", model:""},
                {role:T["wizard.review.role.security"], key:"claude", model:""}];
  const repoIn = el("input", {class:"mono", placeholder:T["wizard.review.repo_ph"], style:"width:100%;box-sizing:border-box"});
  repoIn.addEventListener("input", () => repo.dir = repoIn.value.trim());
  const repoBtn = REMOTE ? null : el("button", {class:"quiet", onclick: async () => {
    const p = await pickPath("dir", T["wizard.review.pick_repo_title"], repo.dir);
    if (p !== null) { repo.dir = p; repoIn.value = p; }
  }}, T["common.browse"]);
  const list = el("div");
  const draw = () => {
    list.textContent = "";
    revs.forEach((r, i) => {
      const roleSel = el("select");
      for (const rr of REVIEW_ROLES) roleSel.append(el("option", {value:rr.label}, rr.label));
      roleSel.value = r.role; roleSel.addEventListener("change", () => r.role = roleSel.value);
      const del = el("button", {class:"quiet", onclick:() => {
        if (revs.length > 1) { revs.splice(i, 1); draw(); } else toast(T["wizard.review.min_reviewers"], true);
      }}, T["common.delete"]);
      list.append(el("div", {class:"row", style:"align-items:center;gap:8px;border:1px solid var(--line);border-radius:9px;padding:9px;margin:7px 0"},
        el("span", {class:"mono", style:"color:var(--muted)"}, "#" + (i + 1)), roleSel, aiPick(r), del));
    });
  };
  draw();
  const addBtn = el("button", {class:"quiet", onclick:() => {
    revs.push({role:T["wizard.review.role.custom"], key:"claude", model:""}); draw();
  }}, T["wizard.review.add_reviewer"]);
  const m = openModal(
    el("h2", {}, T["wizard.review.title"]),
    el("div", {class:"hint"}, T["wizard.review.hint"]),
    row(T["settings.workspace.name"], nameIn),
    el("div", {class:"row", style:"margin-top:8px"}, el("label", {}, T["wizard.review.repo_label"]), repoIn, repoBtn),
    el("div", {class:"row"}, el("label", {}, T["wizard.review.coder_label"]), aiPick(coder)),
    el("div", {style:"margin-top:10px;color:var(--text);font-size:13px"}, T["wizard.review.reviewers_label"]), list, addBtn,
    el("div", {class:"row", style:"border-top:1px solid var(--line);margin-top:12px;padding-top:14px"},
      el("button", {class:"primary", onclick:() => {
        if (!repo.dir.trim()) { toast(T["wizard.review.repo_required"], true); return; }
        const err = partsValid([coder].concat(revs)); if (err) { toast(err, true); return; }
        const tabs = [], personas = {}, agents = [], used = new Set();
        tabs.push(newTab({name:T["wizard.review.coder_tab_name"], id:"coder", command: aiCommandOf(coder.key, coder.model)}));
        personas["coder"] = CODER_PERSONA; agents.push("coder"); used.add("coder");
        revs.forEach((r, i) => {
          const rr = REVIEW_ROLES.find(x => x.label === r.role);
          let base = (rr && rr.id) || slugId(r.role) || ("rev" + (i + 1)), id = base, n = 2;
          while (used.has(id)) id = base + "-" + (n++);
          used.add(id);
          tabs.push(newTab({name: r.role, id, command: aiCommandOf(r.key, r.model)}));
          personas[id] = (rr && rr.persona) || T["wizard.review.persona.custom"];
          agents.push(id);
        });
        m.remove();
        landOnWs({name: nameIn.value.trim() || T["wizard.review.default_name"], automation:"", tabs,
          folders:[{name:"", id:"", cwd: repo.dir.trim()}],
          discuss:{agents, order:"round-robin", max_rounds:4, personas},
          stops:[{when:"console", agents:"all", pattern:"LGTM", outcome:"success", code:0, reason:T["wizard.review.stop_reason"]}]});
      }}, T["wizard.review.create"]),
      el("button", {class:"quiet", onclick:() => m.remove()}, T["common.cancel"])));
}

// ── Narrow screens: the sidebar as a drawer ─────────────────────
// One phone-width layout, driven from here so the CSS and the DOM never disagree
// about where things are. The same nav, the same links, the same header — moved,
// never duplicated, so a card added later needs no phone-specific counterpart.
const narrow = window.matchMedia("(max-width: 760px)");
// Held as a node, not looked up by id: renderNav() empties the drawer on every
// render, which detaches this element from the document.
const headLinks = document.getElementById("headlinks");

function setNav(open) {
  document.body.classList.toggle("navopen", !!open);
  document.getElementById("navtoggle").setAttribute("aria-expanded", open ? "true" : "false");
}
const closeNav = () => setNav(false);
const toggleNav = () => setNav(!document.body.classList.contains("navopen"));

// The secondary header links belong beside the title on a desktop and at the
// foot of the drawer on a phone. Re-homed after every render (which clears the
// drawer) and on every breakpoint change.
function placeHeadLinks() {
  if (narrow.matches) document.getElementById("nav").append(headLinks);
  else document.getElementById("backbtn").before(headLinks);
}

// What the phone header shows instead of the app's name: where you actually are.
// Returns [what encloses it, what's open] — the first half is the one that gets
// squeezed when the name is long, so the section or tab you're editing survives.
function crumbParts() {
  if (loadFailure) return ["", T["settings.broken.title"]];
  if (sel.global) {
    const s = globalSections().find(x => x.id === sel.section);
    return ["", s ? s.label : T["settings.global"]];
  }
  const ws = wss[sel.ws];
  if (!ws) return ["", ""];
  const name = ws.name || T["settings.tab.unnamed"];
  const g = (ws.folders || [])[sel.grp];
  if (sel.tab === null) {
    if (!g) return ["", name];
    return [name, folderLabel(g, sel.grp)];
  }
  const t = (ws.tabs || [])[sel.tab];
  const where = g ? name + " › " + folderLabel(g, sel.grp) : name;
  return [where, (t && t.name) || T["settings.tab.unnamed"]];
}

function renderCrumb() {
  const [up, cur] = crumbParts();
  const box = document.getElementById("crumb");
  box.textContent = "";
  if (up) box.append(el("span", {class:"up"}, up + " /"));
  box.append(el("span", {class:"cur"}, cur));
  box.setAttribute("title", up ? up + " / " + cur : cur);
}

// The sticky header wraps at some widths, so its height is measured rather than
// assumed — the sidebar and the drawer both start immediately below it.
const headerEl = document.querySelector("header");
const measureHeader = () =>
  document.documentElement.style.setProperty("--headh", headerEl.offsetHeight + "px");
try { new ResizeObserver(measureHeader).observe(headerEl); } catch (e) { measureHeader(); }
window.addEventListener("resize", measureHeader);

// Picking a destination closes the drawer; expanding a group (the ▸ caret) does
// not — that is still choosing. One delegated listener, so every nav item added
// later behaves the same without remembering to wire it up.
document.getElementById("nav").addEventListener("click", e => {
  const item = e.target.closest(".navitem");
  if (item && !item.classList.contains("navgrouphead")) closeNav();
});
document.addEventListener("keydown", e => { if (e.key === "Escape") closeNav(); });
narrow.addEventListener("change", () => { placeHeadLinks(); closeNav(); measureHeader(); });

// ── A file we can't use ───────────────────────────────
// The settings screen is the editor for these files, so it's the right place to
// say what's wrong with one. It shows the path, what the parser objected to, and
// the offending line itself — then holds Save until the file is fixed and
// reloaded, because everything the form would write is missing.
function showLoadFailure(f) {
  loadFailure = f;
  const btn = document.getElementById("savebtn");
  btn.disabled = true;
  btn.classList.remove("dirty");
  btn.title = T["settings.broken.save_blocked"];
  document.getElementById("nav").textContent = "";
  const d = document.getElementById("detail");
  d.textContent = "";
  d.append(card(T["settings.broken.title"],
    f.path ? el("div", {class:"hint mono"}, f.path) : null,
    el("div", {style:"color:var(--danger);margin:8px 0"}, f.error),
    brokenExcerpt(f),
    el("div", {class:"hint"}, T["settings.broken.body"]),
    el("div", {class:"row", style:"margin-top:10px"},
      el("button", {class:"primary", onclick:() => load()}, T["common.reload"]))));
  renderCrumb();
  placeHeadLinks();
  result(T["settings.broken.title"], true);
}

// Back to a working screen: whatever held Save is gone, so give it back.
function clearLoadFailure() {
  if (!loadFailure) return;
  loadFailure = null;
  const btn = document.getElementById("savebtn");
  btn.disabled = false;
  btn.title = "";
}

// The offending line, with its neighbours for bearings and a caret under the
// column the parser stopped at.
function brokenExcerpt(f) {
  if (!f.line || !f.text) return null;
  const lines = f.text.split(/\r?\n/);
  const from = Math.max(0, f.line - 3), to = Math.min(lines.length, f.line + 2);
  const width = String(to).length;
  const pre = el("pre");
  for (let i = from; i < to; i++) {
    const gutter = String(i + 1).padStart(width, " ") + " | ";
    if (i + 1 !== f.line) { pre.append(gutter + lines[i] + "\n"); continue; }
    // The parser counts the column in BYTES, so the line is cut by bytes and the
    // character itself is marked. A caret placed by counting columns would drift
    // the moment the line holds a tab name in Japanese — and a pointer that lies
    // is worse than none. Marking it lets the browser do the placing.
    const bytes = new TextEncoder().encode(lines[i]);
    const at = Math.max(0, f.column - 1);
    const head = new TextDecoder().decode(bytes.slice(0, at));
    const rest = new TextDecoder().decode(bytes.slice(at));
    const bad = [...rest][0] || " ";
    pre.append(gutter, head, el("span", {class:"at"}, bad), rest.slice(bad.length), "\n");
  }
  return pre;
}

// ── Detail pane ───────────────────────────────────────
function render() {
  // Nothing loaded, so there is nothing true to draw. Guarding here rather than
  // at each caller means a deep link, a nav click or a later entry point can't
  // paint an empty form over the explanation.
  if (loadFailure) return showLoadFailure(loadFailure);
  if (sel.global && !sel.section) sel.section = globalSections()[0].id;
  renderNav();
  renderDetail();
  renderCrumb();
  placeHeadLinks();
}

function renderDetail() {
  const d = document.getElementById("detail");
  d.textContent = "";
  if (sel.global) {
    const secs = globalSections();
    const sec = secs.find(s => s.id === sel.section) || secs[0];
    sel.section = sec.id;
    return d.append(sec.build());
  }
  const ws = wss[sel.ws];
  if (!ws) return;
  if (sel.tab === null) {
    if (sel.grp === null || sel.grp === undefined) return d.append(wsPane(ws));
    const g = (ws.folders || [])[sel.grp];
    if (!g) { sel.grp = null; return renderDetail(); }
    return d.append(folderPane(ws, g, sel.grp));
  }
  const t = ws.tabs[sel.tab];
  if (!t) { sel.tab = null; return renderDetail(); }
  d.append(tabPane(ws, t));
}

// The global settings are a FLAT list of self-named cards, not a themed
// hierarchy: each card is its own nav item, so nothing has to be hunted across
// categories, and adding a feature just adds one more named card (no "which
// bucket does this go in?"). Each nav item carries a one-line subtitle so the
// stumble-onto-it discovery a single long scroll used to give isn't lost.
function basicCard() {
  return card(T["settings.tab.basic"],
    row(T["settings.tabbar_width"], field(current, "tab_bar_width", T["settings.tab.automation_dir.ph"], {type:"number", width:110, grow:false}),
        el("span", {class:"hint"}, T["settings.tabbar_width.hint"])),
    row(T["settings.max_chain"], field(current, "max_chain", "10", {type:"number", width:110, grow:false}),
        el("span", {class:"hint"}, T["settings.max_chain.hint"])),
    row(T["settings.done_confirm"], secondsField(current, "done_confirm_ms", 10),
        el("span", {class:"hint"}, T["settings.done_confirm.hint"])),
    row(T["settings.busy_repeat"], field(current, "busy_repeat_sec", "0", {type:"number", width:110, grow:false}),
        el("span", {class:"hint"}, T["settings.busy_repeat.hint"])),
    row(T["settings.auto_switch"], checkDefaultOn(current, "auto_switch", T["settings.auto_switch.label"]),
        el("span", {class:"hint"}, T["settings.auto_switch.hint"])),
    row(T["settings.restore_ws"], checkDefaultOn(current, "restore_workspace", T["settings.restore_ws.label"]),
        el("span", {class:"hint"}, T["settings.restore_ws.hint"])),
    row(T["settings.tui_clipboard"], checkDefaultOn(current, "tui_clipboard", T["settings.tui_clipboard.label"]),
        el("span", {class:"hint"}, T["settings.tui_clipboard.hint"])),
    row(T["settings.conpty"], conptyState(),
        el("span", {class:"hint"}, T["settings.conpty.hint"])),
    row(T["settings.ai_engine"], aiSelect(),
        el("span", {class:"hint", id:"aihint"}, "")),
    row(T["settings.browser_data"],
        choose(current, "browser_data", [
          ["", T["settings.browser_data.local"] || "This PC only (recommended)"],
          ["portable", T["settings.browser_data.portable"] || "Share across PCs (Drive sync)"],
        ]),
        el("span", {class:"hint"}, T["settings.browser_data.hint"] || "")),
    row(T["settings.user_agent"],
        field(current, "user_agent", T["settings.user_agent.ph"], {grow:true}),
        el("span", {class:"hint"}, T["settings.user_agent.hint"] || "")),
    row(T["settings.font"],
        (() => {
          current.appearance = current.appearance || {};
          const a = current.appearance;
          const wrap = el("div", {style:"display:flex;gap:8px;min-width:0;flex:1"});
          const fam = field(a, "font", T["settings.font.ph"], {grow:true});
          const size = el("input", {type:"number", style:"width:80px", min:"8", max:"32"});
          size.value = a.font_size || 14;
          size.addEventListener("input", () => {
            const n = Number(size.value);
            if (n >= 8 && n <= 32) a.font_size = n;
          });
          wrap.append(fam, size);
          return wrap;
        })(),
        el("span", {class:"hint"}, T["settings.font.hint"])),
    row(T["settings.theme"], themePicker(),
        el("span", {class:"hint"}, T["settings.theme.hint"])),
    row(T["settings.pr"], githubState(),
        el("span", {class:"hint"}, T["settings.pr.hint"])),
    row(T["settings.language"],
        choose(current, "language", [
          ["", T["settings.language.auto"]],
          ["ja", "日本語"],
          ["en", "English"],
        ]),
        el("span", {class:"hint"}, T["settings.language.hint"])));
}
// Which pseudo console the terminals are running on.
//
// Read-only, like the sign-in below it: there is no setting behind this. What
// decides it is whether the file travelled with the program, so the only thing
// a person can do about it is get the program again -- and the only reason
// this row exists is that being on the older one looks exactly like being on
// the newer one, right up until a program's output arrives wrong.
function conptyState() {
  const out = el("span", {class:"hint"}, "…");
  (async () => {
    let j;
    try { j = await (await fetch("/api/conpty", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    if (j.bundled) {
      out.textContent = T["settings.conpty.on"] + (j.version ? " (" + j.version + ")" : "");
      out.classList.remove("warn");
      return;
    }
    out.textContent = T["settings.conpty.off." + j.missing] || T["settings.conpty.off"];
    out.classList.add("warn");
  })();
  return out;
}
// Whether pull request numbers can be shown, and why not when they cannot.
//
// Read-only on purpose. There is nothing to set here: the sign-in belongs to
// the person's own GitHub tool, and offering a second place to paste a token
// would be offering them a second place to have one go stale
function githubState() {
  const out = el("span", {class:"hint"}, "…");
  (async () => {
    let j;
    try { j = await (await fetch("/api/github", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    out.textContent = j.signed_in ? T["settings.pr.on"] : T["settings.pr.off"];
    out.classList.toggle("warn", !j.signed_in);
  })();
  return out;
}

// The colour scheme, chosen by name from the ones this machine already has.
//
// The list is fetched rather than built in because most of it is not ours: it
// is whatever schemes the platform's terminal is carrying plus whatever the
// person dropped in their config folder. Each one shows its own colours, since
// nobody remembers what "Nord" looks like from the word.
function themePicker() {
  current.appearance = current.appearance || {};
  const a = current.appearance;
  const wrap = el("div", {style:"display:flex;gap:10px;align-items:center;min-width:0;flex:1;flex-wrap:wrap"});
  const sel = el("select", {style:"min-width:190px"});
  const strip = el("div", {style:"display:flex;gap:3px"});
  wrap.append(sel, strip);
  // A scheme written out in the settings by hand is not in any list, and
  // picking from the list is how someone would replace it -- so it is offered
  // as the current choice rather than silently dropped
  const inline = a.theme && typeof a.theme === "object";
  let known = [], mine = null;
  const paint = () => {
    const found = sel.value ? known.find(t => t.name === sel.value) : mine;
    strip.textContent = "";
    for (const c of (found ? found.colors : [])) {
      strip.append(el("span", {style:"width:14px;height:14px;border-radius:3px;" +
        "border:1px solid var(--line);background:" + c}));
    }
  };
  sel.addEventListener("change", () => {
    // Picking a name replaces whatever was there. Landing back on "written
    // here" leaves the colours the person wrote exactly as they wrote them
    if (sel.value) a.theme = sel.value;
    else if (!inline) delete a.theme;
    paint();
  });
  (async () => {
    let j;
    try { j = await (await fetch("/api/themes", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    known = j.list || [];
    mine = j.current || null;
    if (inline) sel.append(el("option", {value:""}, T["settings.theme.custom"]));
    for (const t of known) sel.append(el("option", {value:t.name}, t.name));
    // An unset theme is the app's own, not whatever happens to sort first
    sel.value = inline ? "" : (a.theme || j.default || "");
    paint();
  })();
  return wrap;
}
// Which key does what.
//
// The rows come from the app: it knows what it can do and what each action
// answers to right now, so this screen never keeps its own copy of that list.
// A key is typed the way people write keys to each other -- ctrl+shift+d -- and
// a bare character means "after the prefix key", which is what the prefix is
// for. Empty gives the key back.
function keysCard() {
  current.keys = current.keys || {};
  const k = current.keys;
  const list = el("div", {}, el("div", {class:"hint"}, "…"));
  const problems = el("div", {});
  const box = card(T["settings.sec.keys"],
    el("div", {class:"hint", style:"margin-bottom:10px"}, T["settings.keys.intro"]),
    problems,
    row(T["settings.keys.prefix"], field(k, "prefix", "ctrl+b", {width:150, grow:false}),
        el("span", {class:"hint"}, T["settings.keys.prefix.hint"])),
    list);
  load();
  async function load() {
    let j;
    try { j = await (await fetch("/api/keys", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    problems.textContent = "";
    for (const p of (j.problems || [])) {
      problems.append(el("div", {class:"warn"}, p));
    }
    list.textContent = "";
    for (const r of (j.rows || [])) {
      // What it does on the left, what it answers to on the right. The box is
      // empty unless this person changed it: showing the default inside the
      // box would make every row look edited
      const inp = el("input", {type:"text", placeholder:r.now || T["settings.keys.off"],
        style:"width:160px"});
      inp.value = k[r.name] || "";
      inp.addEventListener("input", () => {
        const v = inp.value.trim();
        if (v) k[r.name] = v; else delete k[r.name];
      });
      list.append(el("div", {class:"row"},
        el("label", {}, r.desc),
        inp,
        el("span", {class:"hint"}, r.now ? T["settings.keys.now"] + " " + r.now
                                         : T["settings.keys.off"])));
    }
  }
  return box;
}
// Saved browser logins.
//
// Managed here, never made here: a login is saved by signing in once in a
// browser tab and calling browser_state_save (a rally does this). This screen
// is where you see what is kept and throw one away. The cookies never leave
// the machine and never come back through this page -- only the name and how
// much is inside
function loginsCard() {
  const list = el("div", {}, el("div", {class:"hint"}, "…"));
  const box = card(T["settings.sec.logins"],
    el("div", {class:"hint", style:"margin-bottom:10px"}, T["settings.logins.intro"]),
    list);
  load();
  async function load() {
    let rows = [];
    try { rows = await (await fetch("/api/logins", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    list.textContent = "";
    if (!rows.length) { list.append(el("div", {class:"hint"}, T["settings.logins.none"])); return; }
    for (const r of rows) {
      const del = el("button", {class:"btn"}, T["settings.logins.forget"]);
      del.addEventListener("click", async () => {
        del.disabled = true;
        try {
          await fetch("/api/logins/delete", {method:"POST",
            headers:{"X-Token":TOKEN, "Content-Type":"application/json"},
            body: JSON.stringify({label: r.label})});
        } catch (e) {}
        load();
      });
      list.append(el("div", {class:"row"},
        el("label", {}, r.label),
        el("span", {class:"hint"}, T["settings.logins.count"].replace("{n}", r.count)),
        del));
    }
  }
  return box;
}
// Saved page snapshots.
//
// Pictures a rally (or you) took of a browser page with browser_snapshot. Here
// to glance back at what an agent was looking at, and to throw them away. The
// image rides in as a data URL, so the card needs no second request that would
// have to carry the token an <img> tag cannot.
function snapshotsCard() {
  const grid = el("div", {style:"display:flex;flex-wrap:wrap;gap:12px"}, el("div", {class:"hint"}, "…"));
  const box = card(T["settings.sec.snapshots"],
    el("div", {class:"hint", style:"margin-bottom:10px"}, T["settings.snapshots.intro"]),
    grid);
  load();
  async function load() {
    let rows = [];
    try { rows = await (await fetch("/api/snapshots", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    grid.textContent = "";
    if (!rows.length) { grid.append(el("div", {class:"hint"}, T["settings.snapshots.none"])); return; }
    for (const r of rows) {
      const cell = el("div", {style:"width:220px"});
      const img = el("img", {src:r.data, alt:r.label,
        style:"width:220px;height:140px;object-fit:cover;object-position:top;border:1px solid var(--line);border-radius:8px;background:var(--bg)"});
      const row = el("div", {style:"display:flex;align-items:center;gap:6px;margin-top:4px"});
      const del = el("button", {class:"btn"}, T["settings.logins.forget"]);
      del.addEventListener("click", async () => {
        del.disabled = true;
        try { await fetch("/api/snapshots/delete", {method:"POST",
          headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
          body: JSON.stringify({label:r.label})}); } catch (e) {}
        load();
      });
      row.append(el("span", {class:"hint", style:"flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap"}, r.label), del);
      cell.append(img, row);
      grid.append(cell);
    }
  }
  return box;
}
function filesCard() {
  return card(T["settings.section.files"],
    row(T["settings.automation_global"], ...pathField(current, "automation", "scripts/common", "dir",
        T["settings.tab.automation_dir.pick"]),
        el("span", {class:"hint"}, T["settings.automation_global.hint"])),
    row("secrets", ...pathField(current, "secrets", "secrets.json", "file",
        T["settings.secrets"]),
        el("span", {class:"hint"}, T["settings.secrets.hint"])));
}
// Ordered flat list of the global cards. `id` is the stable deep-link handle
// (the sub-input bar's ⚙ opens ?section=actions, for instance).
function globalSections() {
  return [
    {id:"basic",     label:T["settings.sec.basic"],     sub:T["settings.sec.basic.sub"],     build:basicCard},
    {id:"keys",      label:T["settings.sec.keys"],      sub:T["settings.sec.keys.sub"],      build:keysCard},
    {id:"logins",    label:T["settings.sec.logins"],    sub:T["settings.sec.logins.sub"],    build:loginsCard},
    {id:"snapshots", label:T["settings.sec.snapshots"], sub:T["settings.sec.snapshots.sub"], build:snapshotsCard},
    {id:"actions",   label:T["settings.sec.actions"],   sub:T["settings.sec.actions.sub"],   build:actionsCard},
    {id:"permissions", label:T["settings.sec.permissions"], sub:T["settings.sec.permissions.sub"], build:permissionsCard},
    {id:"git",       label:T["settings.sec.git"],       sub:T["settings.sec.git.sub"],       build:gitCard},
    {id:"protect",   label:T["settings.sec.protect"],   sub:T["settings.sec.protect.sub"],   build:protectCard},
    {id:"operate",   label:T["settings.sec.operate"],   sub:T["settings.sec.operate.sub"],   build:operateCard},
    {id:"providers", label:T["settings.sec.providers"], sub:T["settings.sec.providers.sub"], build:providersCard},
    {id:"notify",    label:T["settings.sec.notify"],    sub:T["settings.sec.notify.sub"],    build:notifyCard},
    {id:"remote",    label:T["settings.sec.remote"],    sub:T["settings.sec.remote.sub"],    build:remoteCard},
    {id:"api",       label:T["settings.sec.api"],       sub:T["settings.sec.api.sub"],       build:apiCard},
    {id:"resume",    label:T["settings.sec.resume"],    sub:T["settings.sec.resume.sub"],    build:resumeCard},
    {id:"files",     label:T["settings.sec.files"],     sub:T["settings.sec.files.sub"],     build:filesCard},
    {id:"secrets",   label:T["settings.sec.secrets"],   sub:T["settings.sec.secrets.sub"],   build:secretsCard},
    {id:"results",   label:T["settings.sec.results"],   sub:T["settings.sec.results.sub"],   build:rallyResultCard},
  ];
}

// Downloads the latest rally result.
// Contents: the human-readable flow (transcript) + the verdict + the executed Lua (paste it to reproduce).
// An AI+AI discussion's flow and outcome also stay in this one file, so a human can check it later
function rallyResultCard() {
  const list = el("div", {id:"rallylist"}, el("div", {class:"hint"}, "…"));
  setTimeout(loadRallyList, 0);
  return card(T["settings.rally.title"],
    el("div", {class:"hint"},
      T["settings.rally.hint"]),
    list);
}

async function loadRallyList() {
  const box = document.getElementById("rallylist");
  if (!box) return;
  let runs = [];
  try { runs = await (await fetch("/api/rally/list", {headers:{"X-Token":TOKEN}})).json(); } catch (e) {}
  box.textContent = "";
  if (!runs.length) { box.append(el("div", {class:"hint"}, T["settings.rally.empty"])); return; }
  runs.forEach((r, i) => {
    const label = (i === 0 ? T["settings.rally.latest_prefix"] : "") + (r.title || r.id);
    box.append(el("div", {class:"row", style:"gap:10px"},
      el("span", {class:"grow", style:"min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis",
        title:r.title || r.id}, label),
      el("button", {class: i === 0 ? "primary" : "", onclick:() => downloadRally(r.id)}, T["settings.rally.download"])));
  });
}

async function downloadRally(runId) {
  try {
    const url = "/api/rally/download" + (runId ? ("?run=" + encodeURIComponent(runId)) : "");
    const r = await fetch(url, {headers:{"X-Token":TOKEN}});
    if (!r.ok) { result(T["settings.rally.no_record"], true); return; }
    const blob = await r.blob();
    const u = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = u; a.download = "rally-" + (runId || "latest") + ".md"; document.body.append(a); a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(u), 1000);
    result(T["settings.rally.downloaded"]);
  } catch (e) {
    result(fill(T["settings.rally.download_failed"], {e: e.message || e}), true);
  }
}

// Secrets (equivalent to GitHub Secrets). Referenced by key; once saved, the value is never shown again.
// Encrypted if a master password is set, plaintext otherwise (at the user's own risk) — both handled through the same UI
function secretsCard() {
  const status = el("div", {class:"hint", id:"secretsmode"});
  // Where the master password is set. The password itself is only ever typed
  // into the native app (never this page), so point the user at [k] on INDEX.
  const pwhint = el("div", {class:"hint"}, T["settings.secrets.master_hint"]);
  const head = el("div", {}, status, pwhint);
  const listBox = el("div", {id:"secretslist"}, el("div", {class:"hint"}, T["common.reload"] ? "…" : "…"));
  const keyIn = el("input", {class:"mono", placeholder:T["settings.secrets.key_ph"], style:"width:200px"});
  const descIn = el("input", {placeholder:T["settings.secrets.desc_ph"], style:"width:220px"});
  const valIn = el("input", {type:"password", placeholder:T["settings.secrets.value_ph"], style:"width:220px"});
  const addBtn = el("button", {class:"primary", onclick: async () => {
    const key = keyIn.value.trim();
    if (!key) { toast(T["settings.secrets.key_required"], true); return; }
    if (!valIn.value) { toast(T["settings.secrets.value_required"], true); return; }
    const r = await fetch("/api/secrets/set", {method:"POST",
      headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
      body: JSON.stringify({key, description: descIn.value, value: valIn.value})}).then(r=>r.json());
    if (r.ok) { toast(fill(T["settings.secrets.saved_key"], {key})); keyIn.value=""; descIn.value=""; valIn.value=""; loadSecrets(); }
    else toast(r.error || T["settings.secrets.save_failed"], true);
  }}, T["settings.secrets.add_update"]);
  // Label each field explicitly with a heading. A placeholder alone got truncated and was hard to read
  const labeled = (title, hint, input) => el("div", {style:"display:flex;flex-direction:column;gap:3px"},
    el("span", {style:"font-size:12px;color:var(--text)"}, title),
    input,
    el("span", {class:"hint", style:"font-size:11px"}, hint));
  const form = el("div", {class:"row", style:"flex-wrap:wrap;gap:14px;margin-top:12px;align-items:flex-end"},
    labeled(T["settings.secrets.key_label"], T["settings.secrets.key_hint"], keyIn),
    labeled(T["settings.secrets.desc_label"], T["settings.secrets.desc_hint"], descIn),
    labeled(T["settings.secrets.value_label"], T["settings.secrets.value_hint"], valIn),
    addBtn);
  const c = card(T["settings.secrets.title"], head, listBox, form);
  // Load only after the card is in the DOM (so getElementById works)
  setTimeout(loadSecrets, 0);
  return c;
}

// Model bridge connections (Providers). Registers OpenAI-compatible APIs by name.
// A directly typed key is saved behind the scenes into an encrypted secret, and only an @reference is put in config
// (the user doesn't need to know about the "secret store" or the @name)
function providersCard() {
  current.providers = current.providers || {};
  const listBox = el("div", {id:"providerslist"});
  const draw = () => {
    listBox.textContent = "";
    const names = Object.keys(current.providers);
    if (!names.length) {
      listBox.append(el("div", {class:"hint"},
        T["settings.providers.empty"]));
    }
    for (const name of names) {
      const p = (current.providers[name] = current.providers[name] || {});
      const urlIn = el("input", {class:"mono", value: p.base_url || "",
        placeholder:"https://api.deepseek.com/v1", style:"flex:1 1 0;min-width:120px"});
      urlIn.addEventListener("input", () => { p.base_url = urlIn.value; refreshSave(); });
      const hasKey = (p.api_key || "").startsWith("@");
      const keyIn = el("input", {type:"password", style:"flex:1 1 0;min-width:110px",
        placeholder: hasKey ? T["settings.providers.key_set_ph"] : T["settings.providers.key_ph"]});
      const keyBtn = el("button", {class:"quiet", onclick: async () => {
        const v = keyIn.value.trim();
        if (!v) { toast(T["settings.secrets.key_required"], true); return; }
        // Secret keys allow only [A-Za-z0-9_-.], so namespace with "_" not ":".
        const sk = "provider_" + name;
        const r = await fetch("/api/secrets/set", {method:"POST",
          headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
          body: JSON.stringify({key: sk, description: "model provider "+name, value: v})})
          .then(r=>r.json());
        if (r.ok) { p.api_key = "@" + sk; keyIn.value = ""; toast(fill(T["settings.providers.key_saved"], {name})); refreshSave(); draw(); }
        else toast(r.error || T["settings.secrets.save_failed"], true);
      }}, T["settings.providers.save_key"]);
      // How long to wait for a whole reply. Blank means the app's own 180, and
      // 0 means "as long as it takes" -- which is the only workable answer for
      // a thinking model on the machine next door.
      const waitIn = el("input", {type:"number", min:"0", step:"1", class:"mono",
        style:"flex:none;width:80px", placeholder:T["settings.providers.timeout_ph"],
        title:T["settings.providers.timeout_title"],
        value: (p.timeout_sec === undefined || p.timeout_sec === null) ? "" : String(p.timeout_sec)});
      waitIn.addEventListener("input", () => {
        const v = waitIn.value.trim();
        if (v === "") delete p.timeout_sec; else p.timeout_sec = Math.max(0, Math.floor(Number(v) || 0));
        refreshSave();
      });
      const del = el("button", {class:"quiet", style:"flex:none", onclick: () => {
        if (confirm(fill(T["settings.providers.delete_confirm"], {name}))) { delete current.providers[name]; refreshSave(); draw(); }
      }}, T["common.delete"]);
      listBox.append(el("div", {class:"listrow"},
        el("span", {class:"mono", style:"flex:none;min-width:70px;color:var(--text)"}, name),
        urlIn, keyIn,
        el("span", {class:"mono", style:"flex:none;width:14px"}, hasKey ? "🔑" : ""),
        waitIn, keyBtn, del));
    }
  };
  const nameIn = el("input", {class:"mono", placeholder:T["settings.providers.name_ph"], style:"width:130px"});
  const addBtn = el("button", {class:"primary", onclick: () => {
    const n = nameIn.value.trim().toLowerCase().replace(/[^a-z0-9_.-]/g, "");
    if (!n) { toast(T["settings.providers.name_required"], true); return; }
    if (current.providers[n]) { toast(T["settings.providers.name_dup"], true); return; }
    current.providers[n] = { base_url: "" };
    nameIn.value = ""; refreshSave(); draw();
  }}, T["settings.providers.add"]);
  const c = card(T["settings.providers.title"],
    el("div", {class:"hint"},
      T["settings.providers.hint"]),
    listBox,
    el("div", {class:"row", style:"gap:10px;margin-top:12px;align-items:flex-end"}, nameIn, addBtn));
  setTimeout(draw, 0);
  return c;
}

// Notification destinations (Slack / Telegram). The sensitive webhook/token is
// stored straight into the secret store (like a provider's api_key) — the user
// never has to register a secret by hand first — and config keeps only "@name".
// Broken-Lua guard for quick actions. `actionErrors` maps an action object to its
// last lint message; the settings server compiles the Lua (/api/lint), and a save
// is refused until every Lua action parses. Keyed by object so reordering is safe.
const actionErrors = new Map();
async function lintLuaCode(code) {
  try {
    const r = await fetch("/api/lint", {method:"POST",
      headers:{"X-Token":TOKEN, "Content-Type":"application/json"},
      body: JSON.stringify({code})});
    const j = await r.json();
    return j.ok ? null : (j.error || "Lua error");
  } catch (e) { return null; }  // a network hiccup shouldn't block saving
}
async function lintAction(a) {
  if (!a || !a.lua || !(a.body || "").trim()) { actionErrors.delete(a); return null; }
  const err = await lintLuaCode(a.body);
  if (err) actionErrors.set(a, err); else actionErrors.delete(a);
  return err;
}
// Lint every Lua action; resolves true only when all of them parse. Gates saving.
async function actionsLintClean() {
  await Promise.all((current.actions || []).map(lintAction));
  return (current.actions || []).every(a => !actionErrors.has(a));
}

// Quick actions for the sub-input bar. An editable list saved into config.actions
// (the main save() already writes `current` wholesale). Each action inserts its
// text into the composer, or — with the Lua toggle — runs Lua on tap; that Lua is
// syntax-checked before it can be saved. Empty-label rows are dropped on save.
function actionsCard() {
  current.actions = current.actions || [];
  const listBox = el("div", {id:"actionslist"});
  const draw = () => {
    listBox.textContent = "";
    if (!current.actions.length) listBox.append(el("div", {class:"hint"}, T["settings.actions.empty"]));
    current.actions.forEach((a, i) => {
      // Read, never write: filling in defaults here would count as an edit, and
      // merely opening this card would light up "unsaved" and then write those
      // defaults into config.json. Same rule as payload() — no side effects.
      const label = a.label || "", body = a.body || "", isLua = !!a.lua;
      const labelIn = el("input", {value:label, placeholder:T["settings.actions.label_ph"], style:"width:130px;flex:none"});
      labelIn.addEventListener("input", () => { a.label = labelIn.value; refreshSave(); });
      const bodyIn = el("textarea", {rows:isLua ? 4 : 2,
        placeholder: isLua ? T["settings.actions.lua_ph"] : T["settings.actions.text_ph"],
        class: isLua ? "mono" : "",
        style:"flex:1 1 0;min-width:200px;resize:vertical"});
      bodyIn.value = body;
      bodyIn.addEventListener("input", () => { a.body = bodyIn.value; refreshSave(); });
      // Advanced, per action: the body is Lua run on tap, not text to insert.
      const luaChk = el("input", {type:"checkbox"}); luaChk.checked = isLua;
      luaChk.addEventListener("change", () => { a.lua = luaChk.checked; refreshSave(); draw(); });
      const luaLbl = el("label", {class:"hint", style:"display:flex;align-items:center;gap:4px;flex:none"},
        luaChk, T["settings.actions.lua"]);
      const up = el("button", {class:"quiet", style:"flex:none", title:T["settings.actions.up"], onclick:() => {
        if (i > 0) { const t = current.actions[i-1]; current.actions[i-1] = current.actions[i]; current.actions[i] = t; refreshSave(); draw(); } }}, "↑");
      const del = el("button", {class:"quiet", style:"flex:none", onclick:() => {
        current.actions.splice(i, 1); actionErrors.delete(a); refreshSave(); draw(); }}, T["common.delete"]);
      // A full-width line under the row shows this action's Lua syntax error, if any.
      const errEl = el("div", {class:"hint",
        style:"flex-basis:100%;color:var(--danger);white-space:pre-wrap;font-family:ui-monospace,monospace"});
      const showErr = () => { errEl.textContent = actionErrors.get(a) || ""; };
      showErr();
      if (isLua) {
        lintAction(a).then(showErr);   // lint on render so an existing break shows at once
        bodyIn.addEventListener("blur", () => lintAction(a).then(() => { showErr(); refreshSave(); }));
      }
      listBox.append(el("div", {class:"listrow tall"},
        labelIn, bodyIn, luaLbl, up, del, errEl));
    });
  };
  const addBtn = el("button", {class:"primary", onclick:() => {
    current.actions.push({label:"", body:""}); refreshSave(); draw(); }}, T["settings.actions.add"]);
  const c = card(T["settings.actions.title"],
    el("div", {class:"hint"}, T["settings.actions.hint"]),
    listBox, el("div", {class:"row", style:"margin-top:10px"}, addBtn));
  draw();
  return c;
}

// The built-in starter actions (Continue / Explain / Review / Fix), mirrored from
// the sub-input bar's fallback so settings can show them as editable rows when the
// config has none. Kept as data (not persisted) unless the user edits them.
function defaultActions() {
  // Full literal keys (not built by concatenation) so the key-existence test can
  // see them, and so a missing translation fails loudly rather than silently.
  return [
    { label: T["actions.default.continue.label"], body: T["actions.default.continue.body"] },
    { label: T["actions.default.explain.label"],  body: T["actions.default.explain.body"] },
    { label: T["actions.default.review.label"],   body: T["actions.default.review.body"] },
    { label: T["actions.default.fix.label"],      body: T["actions.default.fix.body"] },
  ].map(a => ({ label: a.label || "", body: a.body || "", lua: false }));
}

// Operate (🎯) runaway limits + stall policy, saved into config.operate. The three
// limits are a safety net (0 = no limit); on_limit picks "stop" (halt and hand back
// to the human) or "continue" (reset the budget and keep going, trusting the
// operator to judge DONE — so it never stops on the user mid-task).
function operateCard() {
  const o = current.operate = current.operate || {};
  const num = (key, def) => {
    const e = el("input", {type:"number", min:"0", step:"1", style:"width:110px"});
    e.value = (o[key] ?? def);
    e.addEventListener("input", () => { o[key] = e.value; refreshSave(); });
    return e;
  };
  const pol = el("select", {style:"width:220px"});
  for (const [v, label] of [["stop", T["settings.operate.on_limit.stop"]],
                            ["continue", T["settings.operate.on_limit.continue"]]]) {
    const opt = el("option", {value:v}, label); if ((o.on_limit || "stop") === v) opt.selected = true;
    pol.append(opt);
  }
  pol.addEventListener("change", () => { o.on_limit = pol.value; refreshSave(); });
  // The brake: hold before the operator acts (off / only sending steps / every step).
  const conf = el("select", {style:"width:220px"});
  for (const [v, label] of [["off", T["settings.operate.confirm.off"]],
                            ["sends", T["settings.operate.confirm.sends"]],
                            ["all", T["settings.operate.confirm.all"]]]) {
    const opt = el("option", {value:v}, label); if ((o.confirm || "off") === v) opt.selected = true;
    conf.append(opt);
  }
  conf.addEventListener("change", () => { o.confirm = conf.value; refreshSave(); });
  return card(T["settings.operate.title"],
    el("div", {class:"hint"}, T["settings.operate.hint"]),
    row(T["settings.operate.max_rounds"], num("max_rounds", 40), el("span", {class:"hint"}, T["settings.operate.zero_hint"])),
    row(T["settings.operate.max_seconds"], num("max_seconds", 900), el("span", {class:"hint"}, T["settings.operate.zero_hint"])),
    row(T["settings.operate.max_tokens"], num("max_tokens", 400000), el("span", {class:"hint"}, T["settings.operate.zero_hint"])),
    row(T["settings.operate.on_limit"], pol, el("span", {class:"hint"}, T["settings.operate.on_limit.hint"])),
    row(T["settings.operate.settle"], num("settle_ms", 1800), el("span", {class:"hint"}, T["settings.operate.settle.hint"])),
    row(T["settings.operate.confirm"], conf, el("span", {class:"hint"}, T["settings.operate.confirm.hint"])));
}

// Who may run which command: the person's own automation in one column, an AI
// in the other. The list is not written here -- it is poured in from the same
// catalog the app enforces (GRANTS), so a command that gained or lost its place
// in the app cannot go missing from this screen.
//
// Only what somebody changed is written to the config file. Untick a box back
// to the standard answer and the row leaves the file again, which is what lets
// a command added next month arrive with the answer its author chose.
const grantFolded = {};
// The manual, at one command if a name is given. `#cmd-<name>` is not an id in
// the page -- the manual is markdown and grows headings of its own -- it is a
// request the help page answers by finding the row that command is written on
const manualHref = name => "/help?token=" + encodeURIComponent(TOKEN)
  + (name ? "#cmd-" + name : "");

function permissionsCard() {
  // Read without writing: merely opening this card must not make the settings
  // look edited. The key appears in the file the first time a box disagrees
  // with the standard answer, and leaves again when it agrees once more
  const saved = () => current.automation_permissions || {};
  const answerOf = (cmd, col) => {
    const rule = saved()[cmd.name] || {};
    return rule[col] === undefined ? cmd[col] : rule[col];
  };
  const changed = cmd => answerOf(cmd, "human") !== cmd.human || answerOf(cmd, "ai") !== cmd.ai;
  const decide = (cmd, col, on) => {
    const all = current.automation_permissions || {};
    const rule = all[cmd.name] || {};
    if (on === cmd[col]) delete rule[col]; else rule[col] = on;
    if (Object.keys(rule).length) all[cmd.name] = rule; else delete all[cmd.name];
    if (Object.keys(all).length) current.automation_permissions = all;
    else delete current.automation_permissions;
    refreshSave();
  };

  const body = el("div", {});
  const draw = () => {
    body.textContent = "";
    body.append(el("div", {class:"grantcols"},
      el("span", {class:"grow"}, ""),
      el("span", {}, T["settings.permissions.col.human"]),
      el("span", {}, T["settings.permissions.col.ai"])));
    for (const sec of GRANTS) {
      const open = !grantFolded[sec.group];
      // The whole heading folds, not just the caret: a 10px triangle is a
      // target nobody hits on the first try
      const fold = () => { grantFolded[sec.group] = open; draw(); };
      const head = el("div", {class:"granthead"},
        el("span", {class:"foldable grow", onclick:fold},
          el("span", {class:"caret"}, open ? "▾" : "▸"),
          el("b", {}, T[sec.label])));
      // One box per column that answers for the whole group. Half-set shows as
      // half-set rather than guessing which way the person meant it
      for (const col of ["human", "ai"]) {
        const on = sec.commands.filter(c => answerOf(c, col)).length;
        const box = el("input", {type:"checkbox"});
        box.checked = on === sec.commands.length;
        box.indeterminate = on > 0 && on < sec.commands.length;
        box.addEventListener("change", () => {
          sec.commands.forEach(c => decide(c, col, box.checked));
          draw();
        });
        head.append(el("span", {class:"cell"}, box));
      }
      body.append(head);
      if (!open) continue;
      for (const cmd of sec.commands) {
        const line = el("div", {class:"grantrow" + (answerOf(cmd, "ai") ? "" : " off")});
        // The name is the way in: the manual is the reference, and this is
        // the row you were already looking at when you wanted it
        const name = el("a", {class:"nm mono", href:manualHref(cmd.name), target:"_blank",
          title:fill(T["settings.permissions.manual.one"], {name: cmd.name})}, cmd.name);
        const label = el("span", {class:"grow"}, name,
          el("span", {class:"sub"}, T[cmd.text] || ""));
        if (changed(cmd)) name.after(el("span", {class:"grantmark"}, T["settings.permissions.changed"]));
        line.append(label);
        for (const col of ["human", "ai"]) {
          const box = el("input", {type:"checkbox"});
          box.checked = answerOf(cmd, col);
          box.addEventListener("change", () => { decide(cmd, col, box.checked); draw(); });
          line.append(el("span", {class:"cell"}, box));
        }
        body.append(line);
      }
    }
  };
  draw();
  const reset = el("button", {onclick:() => {
    delete current.automation_permissions;
    refreshSave();
    draw();
  }}, T["settings.permissions.reset"]);
  // What counts as an AI is the first thing on the card, spelled out rather
  // than left to be assumed. The mistake this prevents is a person unticking
  // the AI column, walking away, and the AI they started by hand in a terminal
  // tab carrying on -- it holds that tab's key, and that tab is a terminal
  return card(T["settings.sec.permissions"],
    el("div", {class:"hint"}, T["settings.permissions.hint"]),
    el("div", {class:"grantwho"},
      el("div", {}, T["settings.permissions.who.ai"]),
      el("div", {}, T["settings.permissions.who.human"])),
    el("div", {class:"grantwarn"}, T["settings.permissions.caution"]),
    el("div", {class:"row"}, reset,
      el("a", {href:manualHref(""), target:"_blank"}, T["settings.permissions.manual"])),
    body);
}

// The branches the panel will not commit straight onto.
//
// It began as two names written into the app, which is right up until somebody
// is working alone on their own repository -- there, "make a branch first" is a
// rule with nobody on the other side of it. So the names are a question, asked
// here for every folder and again on the folder itself for the one project that
// wants something else.
function protectCard() {
  const box = el("input", {class:"mono grow", placeholder:T["settings.protect.ph"]});
  box.value = protectText(protectApp());
  // The settings are touched when somebody types, never by looking: a card
  // that wrote itself into the config on the way in would light the save
  // button for a change nobody made
  box.addEventListener("input", () => {
    (current.git = current.git || {}).protect = protectList(box.value);
    refreshSave();
  });
  return card(T["settings.sec.protect"],
    el("div", {class:"hint"}, T["settings.protect.hint"]),
    el("div", {class:"row"}, box),
    el("div", {class:"hint"}, T["settings.protect.wild"]));
}

// The commit-message button, in two levels. The instruction is ADDED to the
// built-in prompt -- the rules and the diff still go, and this is the extra
// thing to obey. Replacing the prompt outright would leave the AI describing a
// change nobody showed it, so the field that replaces things is the Lua one.
function gitCard() {
  const g = current.git = current.git || {};
  const hint = el("textarea", {rows:"3", class:"mono", style:"width:100%",
    placeholder:T["settings.git.hint.ph"]});
  hint.value = g.message_hint || "";
  hint.addEventListener("input", () => {
    if (hint.value.trim()) g.message_hint = hint.value; else delete g.message_hint;
    refreshSave();
  });

  const useLua = el("input", {type:"checkbox"});
  useLua.checked = typeof g.message_lua === "string";
  const lua = el("textarea", {rows:"12", class:"mono", style:"width:100%",
    placeholder:T["settings.git.lua.ph"]});
  lua.value = g.message_lua || "";
  lua.addEventListener("input", () => { g.message_lua = lua.value; refreshSave(); });
  const luaBox = el("div", {});
  const drawLua = () => {
    luaBox.textContent = "";
    if (!useLua.checked) return;
    luaBox.append(el("div", {class:"hint"}, T["settings.git.lua.hint"]));
    luaBox.append(lua);
    luaBox.append(el("div", {style:"margin-top:6px"},
      el("button", {class:"quiet", onclick:() => {
        // The built-in one, as a starting point rather than a blank sheet
        lua.value = GIT_MESSAGE_LUA;
        g.message_lua = lua.value;
        refreshSave();
      }}, T["settings.git.lua.default"]),
      el("a", {href:manualHref("ai_ask"), target:"_blank", style:"margin-left:10px"},
        T["settings.git.lua.manual"])));
  };
  useLua.addEventListener("change", () => {
    if (useLua.checked) { g.message_lua = lua.value || GIT_MESSAGE_LUA; lua.value = g.message_lua; }
    else delete g.message_lua;
    drawLua();
    refreshSave();
  });
  drawLua();

  return card(T["settings.sec.git"],
    el("div", {class:"hint"}, T["settings.git.hint.about"]),
    el("div", {style:"margin:6px 0 14px"}, hint),
    el("label", {class:"row", style:"cursor:pointer;gap:8px"}, useLua,
      el("span", {}, T["settings.git.lua.label"])),
    luaBox);
}

function notifyCard() {
  current.notify = current.notify || {};
  const listBox = el("div", {id:"notifylist"});
  const draw = () => {
    listBox.textContent = "";
    const names = Object.keys(current.notify);
    if (!names.length) listBox.append(el("div", {class:"hint"}, T["settings.notify.empty"]));
    for (const name of names) {
      const d = (current.notify[name] = current.notify[name] || {type:"slack"});
      const fields = el("div", {class:"row", style:"flex:1 1 0;gap:8px;flex-wrap:wrap;align-items:center;min-width:180px"});
      let saveSecret, testPayload;
      if (d.type === "telegram") {
        const hasTok = (d.token || "").startsWith("@");
        const tokIn = el("input", {type:"password", style:"flex:1 1 0;min-width:120px",
          placeholder: hasTok ? T["settings.providers.key_set_ph"] : T["settings.notify.token_ph"]});
        const chatIn = el("input", {class:"mono", style:"width:120px",
          value: (d.chat_id || "").startsWith("@") ? "" : (d.chat_id || ""), placeholder:T["settings.notify.chat_ph"]});
        chatIn.addEventListener("input", () => { d.chat_id = chatIn.value.trim(); refreshSave(); });
        fields.append(tokIn, chatIn);
        // Test what's typed now if present, otherwise the saved "@ref".
        testPayload = () => ({type:"telegram", token: tokIn.value.trim() || d.token || "", chat_id: chatIn.value.trim() || d.chat_id || ""});
        saveSecret = async () => {
          const v = tokIn.value.trim();
          if (!v) { toast(T["settings.secrets.value_required"], true); return false; }
          const sk = "notify_" + slugId(name) + "_token";
          const r = await fetch("/api/secrets/set", {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
            body: JSON.stringify({key: sk, description: "notify " + name, value: v})}).then(r=>r.json());
          if (r.ok) { d.token = "@" + sk; tokIn.value = ""; refreshSave(); return true; }
          toast(r.error || T["settings.secrets.save_failed"], true); return false;
        };
      } else {
        const hasHook = (d.webhook || "").startsWith("@");
        const hookIn = el("input", {type:"password", style:"flex:1 1 0;min-width:180px",
          placeholder: hasHook ? T["settings.providers.key_set_ph"] : T["settings.notify.webhook_ph"]});
        fields.append(hookIn);
        testPayload = () => ({type:"slack", webhook: hookIn.value.trim() || d.webhook || ""});
        saveSecret = async () => {
          const v = hookIn.value.trim();
          if (!v) { toast(T["settings.secrets.value_required"], true); return false; }
          const sk = "notify_" + slugId(name);
          const r = await fetch("/api/secrets/set", {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
            body: JSON.stringify({key: sk, description: "notify " + name, value: v})}).then(r=>r.json());
          if (r.ok) { d.webhook = "@" + sk; hookIn.value = ""; refreshSave(); return true; }
          toast(r.error || T["settings.secrets.save_failed"], true); return false;
        };
      }
      const saveBtn = el("button", {class:"quiet", onclick: async () => {
        if (await saveSecret()) { toast(T["settings.notify.saved"]); draw(); } }}, T["settings.notify.save"]);
      const testBtn = el("button", {class:"quiet", onclick: async () => {
        const r = await fetch("/api/notify/test", {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
          body: JSON.stringify(testPayload())}).then(r=>r.json()).catch(()=>null);
        toast((r && r.ok) ? T["settings.notify.test_ok"] : ((r && r.error) || T["settings.notify.test_failed"]), !(r && r.ok));
      }}, T["settings.notify.test"]);
      const del = el("button", {class:"quiet", style:"flex:none", onclick: () => {
        if (confirm(fill(T["settings.notify.delete_confirm"], {name}))) { delete current.notify[name]; refreshSave(); draw(); } }}, T["common.delete"]);
      // The primary: where an unnamed shikisha.notify(text) — e.g. the
      // "a human is needed" ring from an operate rally — gets delivered
      const prim = el("input", {type:"radio", name:"notifyprimary"});
      prim.checked = current.primary_notify === name
        || (!current.primary_notify && names.length === 1);
      prim.addEventListener("change", () => {
        if (prim.checked) { current.primary_notify = name; refreshSave(); }
      });
      const primLabel = el("label", {class:"check", style:"flex:none", title:T["settings.notify.primary_hint"]});
      primLabel.append(prim, document.createTextNode(T["settings.notify.primary"]));
      listBox.append(el("div", {class:"listrow"},
        el("span", {class:"mono", style:"flex:none;min-width:64px;color:var(--text)"}, name),
        el("span", {class:"hint", style:"flex:none;text-transform:uppercase"}, d.type),
        primLabel, fields, saveBtn, testBtn, del));
    }
    // A deleted destination must not linger as the primary
    if (current.primary_notify && !current.notify[current.primary_notify]) {
      delete current.primary_notify;
    }
  };
  const nameIn = el("input", {class:"mono", placeholder:T["settings.notify.name_ph"], style:"width:120px"});
  const typeSel = el("select", {style:"width:120px"});
  typeSel.append(el("option", {value:"slack"}, "Slack"), el("option", {value:"telegram"}, "Telegram"));
  const addBtn = el("button", {class:"primary", onclick: () => {
    // The display name may be anything (Japanese included); it's only the
    // derived secret key that has to be ASCII (see slugId below).
    const n = nameIn.value.trim();
    if (!n) { toast(T["settings.notify.name_required"], true); return; }
    if (current.notify[n]) { toast(T["settings.notify.name_dup"], true); return; }
    current.notify[n] = { type: typeSel.value };
    nameIn.value = ""; refreshSave(); draw();
  }}, T["settings.notify.add"]);
  const c = card(T["settings.notify.title"],
    el("div", {class:"hint"}, T["settings.notify.hint"]),
    listBox,
    el("div", {class:"row", style:"gap:10px;margin-top:12px;align-items:flex-end"}, nameIn, typeSel, addBtn));
  setTimeout(draw, 0);
  return c;
}

async function loadSecrets() {
  const listBox = document.getElementById("secretslist");
  const status = document.getElementById("secretsmode");
  if (!listBox) return;
  let j;
  try { j = await fetch("/api/secrets", {headers:{"X-Token":TOKEN}}).then(r=>r.json()); }
  catch (e) { listBox.textContent=""; listBox.append(el("div",{class:"hint warn"},T["settings.secrets.load_failed"])); return; }
  const modes = {
    plaintext: T["settings.secrets.mode.plaintext"],
    encrypted: T["settings.secrets.mode.encrypted"],
    locked: T["settings.secrets.mode.locked"],
    empty: T["settings.secrets.mode.empty"],
  };
  status.textContent = modes[j.mode] || "";
  status.classList.toggle("warn", j.mode === "locked");
  listBox.textContent = "";
  if (!j.secrets || !j.secrets.length) {
    if (j.mode !== "empty" && j.mode !== "locked")
      listBox.append(el("div", {class:"hint"}, T["settings.secrets.none"]));
    return;
  }
  for (const s of j.secrets) {
    const del = el("button", {class:"quiet", onclick: async () => {
      if (!confirm(fill(T["settings.secrets.delete_confirm"], {key: s.key}))) return;
      const r = await fetch("/api/secrets/delete", {method:"POST",
        headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
        body: JSON.stringify({key: s.key})}).then(r=>r.json());
      if (r.ok) { toast(fill(T["settings.secrets.deleted"], {key: s.key})); loadSecrets(); }
      else toast(r.error || T["settings.secrets.delete_failed"], true);
    }}, T["common.delete"]);
    listBox.append(el("div", {class:"listrow"},
      el("span", {class:"mono", style:"min-width:180px;color:var(--text)"}, s.key),
      el("span", {class:"hint", style:"flex:1"}, s.description || T["settings.secrets.no_desc"]),
      el("span", {class:"hint mono", title:T["settings.secrets.value_hidden"]}, "••••"),
      del));
  }
}
// The phone-usage setting. Explains the risk plainly, but still lets it be enabled with one click
// Carrying conversations. Per CLI rather than per mechanism: the question a
// person has is "will my conversation survive a restart", and the answer
// differs by which CLI is in the tab. Where one needs its own settings file
// touched, exactly what would be written is shown before anything is.
function resumeCard() {
  // Spelled out rather than built from the value: the page's strings are
  // checked against the language file, and a key assembled at run time cannot be
  const HOW = {
    minted: T["settings.resume.minted"],
    record: T["settings.resume.record"],
    hook: T["settings.resume.hook"],
    newest: T["settings.resume.newest"],
    none: T["settings.resume.none"],
  };
  const HOOK_STATE = {
    Installed: T["settings.resume.status.Installed"],
    Absent: T["settings.resume.status.Absent"],
    NoConfig: T["settings.resume.status.NoConfig"],
    Stale: T["settings.resume.status.Stale"],
    Unreadable: T["settings.resume.status.Unreadable"],
  };
  const list = el("div", {}, el("div", {class:"hint"}, "…"));
  const box = card(T["settings.section.resume"],
    el("div", {class:"hint", style:"margin-bottom:10px"}, T["settings.resume.intro"]),
    // This page is what somebody reads when they ask whether their conversation
    // comes back, so it has to say where the switch for that is. The switch
    // itself stays in one place -- two controls for one setting is how they
    // start disagreeing
    el("div", {class:"hint", style:"margin-bottom:10px"}, T["settings.resume.where"]),
    list,
    el("div", {class:"hint", style:"margin-top:12px"}, T["settings.resume.note"]));
  load();
  async function load() {
    let rows = [];
    try { rows = await (await fetch("/api/resume", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    list.textContent = "";
    if (!rows.length) { list.append(el("div", {class:"hint"}, "—")); return; }
    for (const r of rows) list.append(row_for(r));
  }
  function row_for(r) {
    const wrap = el("div", {class:"row", style:"align-items:flex-start"});
    const right = el("div", {style:"display:flex;flex-direction:column;gap:6px;min-width:0;flex:1"});
    right.append(el("span", {class:"hint"}, HOW[r.how] || ""));
    if (r.hook) {
      const state = el("span", {class:"hint"},
        (HOOK_STATE[r.hook.status] || r.hook.status) + " — " + r.hook.file);
      const on = r.hook.status === "Installed";
      const btn = el("button", {class:"btn"}, on ? T["settings.resume.remove"] : T["settings.resume.install"]);
      btn.addEventListener("click", async () => {
        btn.disabled = true;
        try {
          const j = await (await fetch("/api/resume/hook", {
            method:"POST", headers:{"X-Token":TOKEN, "Content-Type":"application/json"},
            body: JSON.stringify({name: r.name, on: !on}),
          })).json();
          if (!j.ok) result(j.error || "", true);
        } catch (e) {}
        load();
      });
      // What would be written, before agreeing to it — not a description of it
      const pre = el("pre", {class:"mono", style:"display:none;white-space:pre-wrap;margin:4px 0;" +
        "padding:8px;background:var(--panel);border:1px solid var(--line);border-radius:6px;font-size:11px"},
        r.hook.preview);
      const show = el("a", {href:"#"}, T["settings.resume.show"]);
      show.addEventListener("click", (e) => {
        e.preventDefault();
        pre.style.display = pre.style.display === "none" ? "block" : "none";
      });
      right.append(state, el("div", {style:"display:flex;gap:8px;align-items:center"}, btn, show), pre);
    }
    wrap.append(el("label", {}, r.name), right);
    return wrap;
  }
  return box;
}

// External control. One setting with three values, and the two things a person
// needs in order to actually use it: the exact pipe name (it carries the process
// id, so no document can print it) and, in `user` mode, where the key is kept.
// The key itself is never shown here — reading it is the point of the file.
function apiCard() {
  current.external_api = current.external_api || {};
  const a = current.external_api;
  // Show what is actually in force. An unset value means the default is
  // running, and a blank dropdown would say "nothing is chosen" about a door
  // that is currently open
  if (!a.access) a.access = "children";
  const status = el("div", {class:"hint"}, "…");
  const where = el("div", {class:"row"});
  const keyfile = el("div", {class:"hint", style:"margin-top:6px"});

  const box = card(T["settings.section.api"],
    el("div", {class:"hint", style:"margin-bottom:10px"}, T["settings.api.intro"]),
    row(T["settings.api.access"],
        choose(a, "access", [
          ["children", T["settings.api.access.children"]],
          ["user",     T["settings.api.access.user"]],
          ["off",      T["settings.api.access.off"]],
        ], async () => { await save(); setTimeout(refreshApi, 600); }),
        el("span", {class:"hint"}, T["settings.api.access.hint"])),
    el("div", {class:"row"}, status),
    where,
    keyfile,
    el("div", {class:"hint", style:"margin-top:10px"}, T["settings.api.note"]),
    el("div", {style:"margin-top:8px"},
      el("a", {href:"/help?token=" + encodeURIComponent(TOKEN), target:"_blank"},
        T["settings.api.help"])));

  refreshApi();
  async function refreshApi() {
    let j = {};
    try { j = await (await fetch("/api/external", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    status.textContent = j.running ? T["settings.api.listening"] : T["settings.api.stopped"];
    status.style.color = j.running ? "var(--accent)" : "var(--muted)";
    where.textContent = "";
    if (j.running && j.path) {
      where.append(el("label", {}, T["settings.api.where"]),
                   el("code", {class:"mono", style:"user-select:all"}, j.path));
    }
    // Only said when the key is actually written down. In the default mode
    // there is no file to warn about
    keyfile.textContent = (j.running && (a.access || "children") === "user")
      ? fill(T["settings.api.tokenfile"], {path: j.token_file}) : "";
  }
  return box;
}

function remoteCard() {
  current.remote = current.remote || {};
  const r = current.remote;
  const box = el("div", {class:"card"}, el("h2", {}, T["settings.section.phone"]));
  const status = el("div", {class:"hint"}, T["settings.phone.checking"]);
  const qrbox = el("div", {style:"margin:10px 0"});

  const onoff = el("input", {type:"checkbox"});
  onoff.checked = !!r.enabled;
  onoff.addEventListener("change", async () => {
    r.enabled = onoff.checked;
    if (r.enabled) { if (!r.bind) r.bind = "auto"; if (!r.port) r.port = 8787; }
    await save();          // Saving makes the main app immediately start/stop listening
    setTimeout(refreshRemote, 1200);
  });
  const l = el("label", {class:"check"});
  l.append(onoff, document.createTextNode(T["settings.phone.enable.label"]));

  box.append(el("div", {class:"row"}, el("label", {}, T["settings.phone.enable"]), l));
  box.append(el("div", {class:"row"}, el("label", {}, T["settings.phone.port"]),
    (() => {
      const i = el("input", {type:"number", style:"width:110px"});
      i.value = r.port || 8787;
      i.addEventListener("input", () => { r.port = Number(i.value) || 8787; });
      return i;
    })(),
    el("span", {class:"hint"}, T["settings.phone.port.hint"])));
  // Optional second factor: the URL token travels through notification
  // channels in plain text, so the security-conscious can require a
  // password that the phone enters once per app run. Empty = off
  box.append(el("div", {class:"row"}, el("label", {}, T["settings.phone.password"]),
    (() => {
      const i = el("input", {type:"password", style:"width:180px", value: r.password || ""});
      i.addEventListener("input", () => { r.password = i.value; });
      return i;
    })(),
    el("span", {class:"hint"}, T["settings.phone.password.hint"])));
  // Sticky pairing: the token is a string the person writes (or we generate
  // into the field so it is never blank while on). The phone then keeps it in
  // its URL and storage — bookmarkable, survives a discarded tab — and the
  // disconnect control no longer rotates it. Changing the string is the way
  // to shut a phone out. Plain text in config.json: the trade they accept
  box.append(el("div", {class:"row"}, el("label", {}, T["settings.phone.sticky"]),
    (() => {
      const wrap = el("div", {style:"display:flex;flex-direction:column;gap:6px;min-width:0"});
      const on = el("input", {type:"checkbox"});
      on.checked = !!r.sticky_token;
      const tok = el("input", {type:"text", style:"width:340px;max-width:100%;font-family:monospace",
        value: r.fixed_token || "", spellcheck: "false", autocomplete: "off"});
      tok.disabled = !on.checked;
      const gen = () => Array.from(crypto.getRandomValues(new Uint8Array(24)), b => b.toString(16).padStart(2, "0")).join("");
      on.addEventListener("change", () => {
        r.sticky_token = on.checked;
        tok.disabled = !on.checked;
        if (on.checked && !tok.value.trim()) { tok.value = gen(); r.fixed_token = tok.value; }
      });
      const bad = el("span", {class:"hint", style:"color:var(--bad,#e5534b)"});
      const check = () => {
        const short = on.checked && tok.value.trim().length < 16;
        bad.textContent = short ? fill(T["settings.phone.sticky.short"], {n: 16}) : "";
        tok.style.borderColor = short ? "var(--bad,#e5534b)" : "";
      };
      tok.addEventListener("input", () => { r.fixed_token = tok.value.trim(); check(); });
      on.addEventListener("change", check);
      const l = el("label", {class:"check"});
      l.append(on, document.createTextNode(T["settings.phone.sticky.label"]));
      wrap.append(l, tok, bad, el("span", {class:"hint"}, T["settings.phone.sticky.hint"]));
      check();
      return wrap;
    })()));
  box.append(el("div", {class:"row"}, status));
  box.append(qrbox);
  box.append(el("div", {class:"hint", style:"margin-top:6px"},
    T["settings.phone.note"]));

  refreshRemote();
  async function refreshRemote() {
    let j = {};
    try { j = await (await fetch("/api/remote", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    const net = j.tailscale ? fill(T["settings.phone.tailscale"], {ip: j.tailscale})
              : j.lan ? fill(T["settings.phone.lan"], {ip: j.lan})
              : T["settings.phone.none"];
    const head = j.running ? T["settings.phone.listening"] : T["settings.phone.stopped"];
    status.textContent = head + " — " + net + (j.note ? " / " + j.note : "");
    status.style.color = j.running ? "var(--accent)" : "var(--muted)";
    qrbox.textContent = "";
    if (j.running && j.origin) {
      // The image is loaded directly rather than via fetch, so pass auth as the token in the URL
      const img = el("img", {src:"/api/remote/qr?token=" + encodeURIComponent(TOKEN),
        style:"width:200px;height:200px;border-radius:8px;background:#fff;padding:6px"});
      qrbox.append(el("div", {class:"hint"}, T["settings.phone.scan"]), img, linkRow(j.kind));
    }
  }

  // Under the QR, the two things a person can act on. The link itself is not
  // among them: printed without its token it opens nothing, and printed with
  // one it is a password on a screen someone can photograph over your shoulder.
  // So it goes to the clipboard on a press, and what stays on screen is which
  // network it leads to.
  function linkRow(kind) {
    const copy = el("button", {class:"quiet", style:"font-size:16px;line-height:1",
      title: T["settings.phone.copy"], onclick: () => copyUrl(copy)}, "📋");
    const row = el("div", {style:"display:flex;align-items:center;gap:8px;margin-top:8px;flex-wrap:wrap"},
      copy, netBadge(kind));
    return row;
  }

  // Colour is the message: a Tailscale address is reachable by your own
  // machines only, a LAN one by whoever else is on that Wi-Fi. The words are
  // written out rather than built from the kind, so a translation that goes
  // missing is caught before it ships.
  function netBadge(kind) {
    const nets = {
      tailscale: ["ok",   "🔒", T["settings.phone.badge.tailscale"], T["settings.phone.badge.tailscale.hint"]],
      lan:       ["care", "⚠",  T["settings.phone.badge.lan"],       T["settings.phone.badge.lan.hint"]],
      local:     ["mute", "",   T["settings.phone.badge.local"],     T["settings.phone.badge.local.hint"]],
      public:    ["risk", "⚠",  T["settings.phone.badge.public"],    T["settings.phone.badge.public.hint"]],
    };
    const skin = nets[kind];
    if (!skin) return el("span");
    return el("span", {class:"netbadge " + skin[0], title: skin[3]},
      (skin[1] ? skin[1] + " " : "") + skin[2]);
  }

  // The link is fetched at the moment of the press, so it is never sitting in
  // the page waiting to be read. copyText() is the toast's — one way onto the
  // clipboard for every screen, including a phone on plain http, where
  // navigator.clipboard does not exist at all.
  async function copyUrl(btn) {
    let url = "";
    try { url = ((await (await fetch("/api/remote/url", {headers:{"X-Token":TOKEN}})).json()) || {}).url || ""; }
    catch (e) {}
    if (!url) { toast(T["settings.phone.copy_failed"], true); return; }
    copyText(url).then(() => {
      toast(T["settings.phone.copied"]);
      btn.textContent = "✓";
      setTimeout(() => { btn.textContent = "📋"; }, 1400);
    });
  }
  return box;
}

function aiSelect() {
  const s = el("select", {id:"aiengine"});
  const hint = () => document.getElementById("aihint");
  if (!aiEngines.length) {
    s.append(el("option", {value:""}, T["settings.ai_engine.none"])); s.disabled = true;
  } else {
    s.append(el("option", {value:""}, T["settings.ai_engine.auto"]));
    for (const e of aiEngines) s.append(el("option", {value:e.id}, e.label));
  }
  s.value = current.ai_engine || "";
  s.addEventListener("change", () => { current.ai_engine = s.value; });
  setTimeout(() => { const h = hint(); if (h) h.textContent = aiEngines.length
    ? "" : T["settings.ai_engine.missing"]; }, 0);
  return s;
}

function wsPane(ws) {
  const box = el("div");
  box.append(card(T["settings.workspace"],
    row(T["settings.workspace.name"], field(ws, "name", T["settings.workspace.name"], {grow:false, width:280,
        onInput:() => renderNav()})),
    ws.file ? row(T["settings.workspace.file"], el("span", {class:"hint mono"}, ws.file)) : null,
    row(T["settings.tab.automation"], ...pathField(ws, "automation", T["settings.workspace.automation.hint"], "dir",
        T["settings.tab.automation_dir.pick"]),
        el("span", {class:"hint"}, T["settings.workspace.automation.hint"]))));

  if (!(ws.tabs || []).length) {
    const e = el("div", {class:"empty"},
      el("div", {class:"big"}, T["settings.template.empty"]),
      el("div", {}, T["settings.template.hint"]));
    const bar = el("div", {class:"row", style:"justify-content:center"});
    for (const [k, label] of [["single",T["settings.template.single"]],["review",T["settings.template.review"]],
                              ["ssh",T["settings.template.ssh"]],["docker",T["settings.template.docker"]],["wsl",T["settings.template.wsl"]]])
      bar.append(el("button", {onclick:() => addTemplate(k)}, label));
    e.append(bar);
    box.append(e);
  }
  box.append(wsDiscussCard(ws));
  box.append(wsStopsCard(ws));
  box.append(wsSecretsCard(ws));

  // Writing it out is about this workspace. Reading one in makes a different
  // one, so it is asked for where another workspace is asked for -- not on the
  // page of the workspace it would have nothing to do with. It ends in a file
  // dialog either way, which a phone has no way to open
  if (!REMOTE) box.append(card(T["settings.ws.share"],
    el("div", {class:"row"},
      el("button", {onclick:() => exportWs(sel.ws)}, T["settings.ws.export"])),
    el("div", {class:"hint"}, T["settings.ws.share.hint"])));

  box.append(el("div", {class:"row"},
    el("button", {class:"danger", onclick:() => {
      if (confirm(fill(T["settings.workspace.delete_confirm"], {name: ws.name}))) {
        wss.splice(sel.ws, 1); sel = {ws:0, tab:null, global:true}; render();
      }
    }}, T["settings.workspace.delete"])));
  return box;
}

// Where the work happens. One folder per group, and a tab has none of its own:
// a reviewer pointed somewhere other than the tab it reviews reviews nothing.
// With a single folder this is one field and the word "group" never appears --
// which is the state anyone who has not asked for a second one stays in.
// A folder, and everything about it. One page per folder, reached the same way
// it is reached in the tab list, because "where does this run" is a fact about
// the folder rather than about the workspace it happens to sit in.
function folderPane(ws, g, gi) {
  const box = el("div");
  const tabsHere = () => (ws.tabs || []).filter(t => (t.group || 0) === gi);

  // This folder's own answer about which branches refuse a direct commit.
  // Unticked it follows the app's, which is what the box shows greyed out
  const ownProtect = el("input", {type:"checkbox"});
  ownProtect.checked = Array.isArray(g.protect);
  const protectBox = el("input", {class:"mono grow"});
  const drawProtect = () => {
    protectBox.disabled = !ownProtect.checked;
    protectBox.placeholder = ownProtect.checked
      ? T["settings.protect.ph"]
      : protectText(protectApp());
    protectBox.value = Array.isArray(g.protect) ? protectText(g.protect) : "";
  };
  ownProtect.addEventListener("change", () => {
    if (ownProtect.checked) g.protect = protectApp().slice();
    else delete g.protect;
    drawProtect();
    refreshSave();
  });
  protectBox.addEventListener("input", () => { g.protect = protectList(protectBox.value); refreshSave(); });
  drawProtect();
  const ownLabel = el("label", {class:"check"});
  ownLabel.append(ownProtect, document.createTextNode(T["settings.group.protect.own"]));

  box.append(card(T["settings.group.title"],
    row(T["settings.group.name"], field(g, "name", folderLabel(g, gi), {grow:false, width:280,
        onInput:() => renderNav()}),
        el("span", {class:"hint"}, T["settings.group.name.hint"])),
    row(T["settings.group.folder"],
        ...pathField(g, "cwd", T["settings.group.folder.ph"], "dir", T["settings.group.folder.pick"]),
        el("span", {class:"hint"}, T["settings.group.folder.hint"])),
    row(T["settings.protect.label"], protectBox, ownLabel),
    el("div", {class:"hint"}, T["settings.group.protect.hint"])));

  // The colour is the project's, not this folder's: every branch of one
  // repository shares it, which is the whole reason it is there. Which project
  // that is has to be asked of the app -- the settings screen has no way to
  // look at a folder and see what git shares behind it
  const colours = el("div", {class:"swatches"});
  const paint = family => {
    colours.textContent = "";
    if (!family) {
      colours.append(el("span", {class:"hint"}, T["settings.group.color.none"]));
      return;
    }
    const now = (current.folder_colors || {})[family] || "";
    const put = c => {
      current.folder_colors = current.folder_colors || {};
      if (c) current.folder_colors[family] = c;
      else delete current.folder_colors[family];
      refreshSave();
      paint(family);
    };
    for (const c of ["#d97757","#19c37d","#4285f4","#a06bff",
                     "#e0a80a","#12b3a8","#e5644d","#7f8cff"]) {
      const sw = el("i", {class:(now.toLowerCase() === c ? "on" : ""), onclick:() => put(c)});
      sw.style.background = c;
      colours.append(sw);
    }
    const any = el("input", {type:"color", value:now || "#888888"});
    any.addEventListener("input", () => put(any.value));
    const opener = el("i", {class:"any", onclick:() => any.click()});
    colours.append(opener, any);
    if (now) colours.append(el("button", {class:"quiet", onclick:() => put("")},
      T["settings.group.color.auto"]));
  };
  paint(null);
  box.append(card(T["settings.group.color"], colours,
    el("div", {class:"hint"}, T["settings.group.color.hint"])));

  // Taking it out of the list, and -- for a folder the app made for a branch --
  // getting rid of the folder itself. Two different acts: one can be undone by
  // opening it again, and the other cannot
  const drop = () => {
    ws.folders.splice(gi, 1);
    (ws.tabs || []).forEach(t => { if ((t.group || 0) > gi) t.group--; });
    sel = {ws:sel.ws, grp:null, tab:null, global:false};
    render(); refreshSave();
  };
  const guard = () => {
    if (tabsHere().length) { toast(T["settings.group.in_use"], true); return false; }
    if ((ws.folders || []).length <= 1) { toast(T["settings.group.last"], true); return false; }
    return true;
  };
  const buttons = el("div", {class:"row"},
    el("button", {class:"danger", onclick:() => { if (guard()) drop(); }},
      T["settings.group.delete"]),
    el("span", {class:"hint"}, T["settings.group.delete.hint"]));
  box.append(buttons);

  // Only for a branch's own folder: the project's own is never on the table
  familyOf(g.cwd).then(where => {
    paint(where && where.family);
    if (!where || !where.cut) return;
    buttons.append(el("button", {class:"danger", onclick: async () => {
      if (!guard()) return;
      if (!confirm(fill(T["settings.group.discard.sure"], {name: folderLabel(g, gi)}))) return;
      const r = await fetch("/api/folder/discard",
        {method:"POST", headers:{"X-Token":TOKEN}, body:JSON.stringify({path: g.cwd})})
        .then(r => r.json()).catch(() => ({ok:false, error:""}));
      if (!r.ok) { toast(r.error || T["settings.group.discard.failed"], true); return; }
      drop();
      toast(T["settings.group.discard.done"]);
    }}, T["settings.group.discard"]));
    buttons.append(el("span", {class:"hint"}, T["settings.group.discard.hint"]));
  });
  return box;
}

// Which project a folder belongs to, as the app sees it. Answered by the app
// because it means looking at what git shares behind the folder
async function familyOf(cwd) {
  if (!(cwd || "").trim()) return null;
  try {
    return await fetch("/api/family?path=" + encodeURIComponent(cwd),
                       {headers:{"X-Token":TOKEN}}).then(r => r.json());
  } catch (e) { return null; }
}

// AI vs AI discussion. Lines up participant tab ids and cycles them round-robin or moderated (moderator picks the next speaker).
// At the round cap, the judge renders a verdict (winner/synthesis). The goal (topic) is typed into an input field
function wsDiscussCard(ws) {
  const body = el("div", {id:"wsdiscussbody"});
  const ensure = () => {
    ws.discuss = ws.discuss || { agents:[], order:"round-robin", max_rounds:6, verdict:"winner" };
    ws.discuss.personas = ws.discuss.personas || {};
    return ws.discuss;
  };
  const on = el("input", {type:"checkbox"});
  on.checked = !!ws.discuss;
  body.style.display = ws.discuss ? "" : "none";
  on.addEventListener("change", () => {
    if (on.checked) { ensure(); body.style.display=""; } else { ws.discuss = null; body.style.display="none"; }
    render(); refreshSave();
  });
  const onLabel = el("label", {class:"check"});
  onLabel.append(on, document.createTextNode(T["settings.discuss.enable"]));

  const d = ws.discuss || {};
  const txt = (val, ph, save) => { const e = el("input", {value:val||"", placeholder:ph||""});
    e.addEventListener("input", () => { save(e.value); refreshSave(); }); return e; };
  const numf = (val, save) => { const e = el("input", {type:"number", value:(val ?? 6), style:"width:90px"});
    e.addEventListener("input", () => { save(parseInt(e.value,10)||1); refreshSave(); }); return e; };
  const self = (val, opts, save) => { const e = el("select", {});
    for (const [v,l] of opts) { const o=el("option",{value:v},l); if(val===v)o.selected=true; e.append(o); }
    e.addEventListener("change", () => { save(e.value); refreshSave(); }); return e; };

  // Persona editing. Shows a stance/personality field for each participant/judge/moderator id
  const personaBox = el("div", {id:"discusspersonas"});
  const drawPersonas = () => {
    personaBox.textContent = "";
    const dd = ws.discuss; if (!dd) return;
    dd.personas = dd.personas || {};
    const ids = [...new Set((dd.agents||[])
      .concat(dd.judge ? [dd.judge] : [])
      .concat(dd.moderator ? [dd.moderator] : [])
      .filter(Boolean))];
    if (!ids.length) { personaBox.append(el("div", {class:"hint"}, T["settings.discuss.persona_hint"])); return; }
    ids.forEach(id => {
      const ta = el("textarea", {rows:2, style:"width:100%;box-sizing:border-box",
        placeholder:T["settings.discuss.persona_ph"]});
      ta.value = dd.personas[id] || "";
      ta.addEventListener("input", () => { dd.personas[id] = ta.value; refreshSave(); });
      personaBox.append(el("div", {style:"margin:6px 0"},
        el("div", {class:"mono", style:"font-size:12px;color:var(--text);margin-bottom:3px"}, id), ta));
    });
  };

  const agentsIn = txt((d.agents||[]).join(", "), T["settings.discuss.agents_ph"],
    v => { ensure().agents = v.split(",").map(s=>s.trim()).filter(Boolean); drawChips(); drawPersonas(); });
  // Participant-candidate chips: one click appends an existing tab id to the end of the turn order.
  // A tab aimed at another tab (🎯) already has a turn to keep, so it can't also be a discussion participant
  const chipBox = el("div", {class:"hint", style:"display:flex;gap:6px;flex-wrap:wrap;align-items:center;margin-top:4px"});
  const drawChips = () => {
    chipBox.textContent = "";
    const cur = (ws.discuss && ws.discuss.agents) || [];
    // Only candidate tabs that are a discussable AI (CLI/model API), not already a participant, and not aimed at anything
    const cand = (ws.tabs || [])
      .filter(t => isDiscussable(t) && !(t.drives||"").trim())
      .map(t => (t.id || t.name || "").trim())
      .filter(id => id && !cur.includes(id));
    if (!cand.length) { chipBox.append(document.createTextNode(T["settings.discuss.no_candidates"])); return; }
    chipBox.append(document.createTextNode(T["settings.discuss.candidates_label"]));
    for (const id of cand) chipBox.append(el("button", {class:"quiet", onclick:() => {
      const a = ensure().agents; if (!a.includes(id)) a.push(id);
      agentsIn.value = a.join(", "); drawChips(); drawPersonas(); refreshSave();
    }}, "＋" + id));
  };
  const orderSel = self(d.order || "round-robin",
    [["round-robin",T["settings.discuss.order.round_robin"]],["moderated",T["settings.discuss.order.moderated"]]], v => ensure().order = v);
  const roundsIn = numf(d.max_rounds, v => ensure().max_rounds = v);
  // Judge and moderator are likewise restricted to discussable AIs (aimed tabs, shells, and Aider are excluded)
  const notDiscuss = t => (t.drives||"").trim() || !isDiscussable(t);
  const judgeIn = idSelect(ws, d.judge, T["wizard.discuss.judge_none"],
    v => { ensure().judge = v; drawPersonas(); }, notDiscuss);
  const modIn = idSelect(ws, d.moderator, T["wizard.discuss.judge_none"],
    v => { ensure().moderator = v; drawPersonas(); }, notDiscuss);
  const verdictSel = self(d.verdict || "winner",
    [["winner",T["wizard.discuss.verdict.winner"]],["synthesis",T["wizard.discuss.verdict.synthesis"]]], v => ensure().verdict = v);

  body.append(
    row(T["settings.discuss.agents_label"], agentsIn, el("span", {class:"hint"}, T["settings.discuss.agents_hint"])),
    row("", chipBox),
    row(T["settings.discuss.order_label"], orderSel),
    row(T["settings.discuss.max_rounds_label"], roundsIn, el("span", {class:"hint"}, T["settings.discuss.max_rounds_hint"])),
    row(T["settings.discuss.judge_field_label"], judgeIn, el("span", {class:"hint"}, T["settings.discuss.judge_hint"])),
    row(T["settings.discuss.moderator_label"], modIn, el("span", {class:"hint"}, T["settings.discuss.moderator_hint"])),
    row(T["wizard.discuss.verdict_label"], verdictSel),
    el("div", {style:"margin-top:8px"},
      el("div", {style:"font-size:12px;color:var(--text)"}, T["settings.discuss.persona_section_label"]),
      el("div", {class:"hint"}, T["settings.discuss.persona_section_hint"]),
      personaBox));
  drawChips();
  drawPersonas();

  return card(T["settings.discuss.title"],
    el("div", {class:"hint"},
      T["settings.discuss.card_hint"]),
    el("div", {class:"row", style:"margin-top:6px"}, onLabel),
    body);
}

// Stop conditions (judge). Per-workspace. Evaluated top to bottom; the first one satisfied wins.
// Defines "when does this collaborative task end (success/failure)"
function wsStopsCard(ws) {
  ws.stops = ws.stops || [];
  const list = el("div", {id:"wsstopslist"});
  const redraw = () => {
    list.textContent = "";
    if (!ws.stops.length) list.append(el("div", {class:"hint"}, T["settings.stops.empty"]));
    ws.stops.forEach((s, i) => list.append(stopRow(ws, s, i, redraw)));
  };
  const add = el("button", {onclick:() => {
    ws.stops.push({ when:"screen", outcome:"success", code:0 }); redraw(); refreshSave();
  }}, T["settings.stops.add"]);
  const c = card(T["settings.stops.title"],
    el("div", {class:"hint"},
      T["settings.stops.hint"]),
    list,
    el("div", {class:"row", style:"margin-top:10px"}, add));
  redraw();
  return c;
}

function stopRow(ws, s, i, redraw) {
  const set = (k, v) => { s[k] = v; refreshSave(); };
  const sel = (val, opts, on) => {
    const e = el("select", {});
    for (const [v, label] of opts) { const o = el("option", {value:v}, label); if (val === v) o.selected = true; e.append(o); }
    e.addEventListener("change", () => on(e.value));
    return e;
  };
  const inp = (val, ph, type, on) => {
    const e = el("input", type === "number" ? {type:"number", value:(val ?? "")} : {value:(val ?? ""), placeholder:ph||""});
    if (type !== "number") e.style.minWidth = "150px";
    e.addEventListener("input", () => on(type === "number" ? (parseInt(e.value, 10) || 0) : e.value));
    return e;
  };
  const when = sel(s.when || "screen", [
    ["screen",T["settings.stops.when.screen"]],["css",T["settings.stops.when.css"]],["xpath",T["settings.stops.when.xpath"]],
    ["console",T["settings.stops.when.console"]],["rounds",T["settings.stops.when.rounds"]],["time",T["settings.stops.when.time"]],["tokens",T["settings.stops.when.tokens"]],
  ], v => { s.when = v; redraw(); refreshSave(); });

  // Switch which inputs are shown depending on the type
  const dyn = [];
  if (s.when === "screen" || s.when === "css" || s.when === "xpath" || s.when === "console") {
    if (s.when !== "console")
      dyn.push(idSelect(ws, s.tab, T["settings.stops.target_tab"], v => set("tab", v)));
    if (s.when === "css" || s.when === "xpath")
      dyn.push(inp(s.sel, s.when === "xpath" ? "//button[...]" : "#id", "text", v => set("sel", v)));
    else
      dyn.push(inp(s.pattern, T["settings.stops.pattern_ph"], "text", v => set("pattern", v)));
  } else if (s.when === "rounds" || s.when === "tokens") {
    dyn.push(inp(s.max, T["settings.stops.threshold_ph"], "number", v => set("max", v)));
  } else if (s.when === "time") {
    dyn.push(inp(s.sec, T["settings.stops.seconds_ph"], "number", v => set("sec", v)));
  }

  const outcome = sel(s.outcome || "success", [["success",T["settings.stops.outcome.success"]],["fail",T["settings.stops.outcome.fail"]]], v => set("outcome", v));
  const code = inp(s.code || 0, "code", "number", v => set("code", v));
  const reason = inp(s.reason, T["settings.stops.reason_ph"], "text", v => set("reason", v || null));
  const rm = el("button", {class:"quiet", title:T["common.delete"], onclick:() => { ws.stops.splice(i, 1); redraw(); refreshSave(); }}, "×");

  const row = el("div", {class:"stoprow"}, when, ...dyn,
    el("span", {class:"arrow"}, "→"), outcome, code, reason, rm);
  return row;
}

// Restricts which secrets this workspace's rally (AI) is allowed to use. Denied by default.
// Prevents a key meant for another purpose from being reused. Only key-name permissions are handled here, never values
function wsSecretsCard(ws) {
  ws.secrets_allow = ws.secrets_allow || [];
  const listBox = el("div", {id:"wssecretslist"}, el("div", {class:"hint"}, "…"));
  const allOn = el("input", {type:"checkbox"});
  allOn.checked = !!ws.secrets_allow_all;
  allOn.addEventListener("change", () => {
    ws.secrets_allow_all = allOn.checked; loadWsSecrets(ws); refreshSave();
  });
  const allLabel = el("label", {class:"check"});
  allLabel.append(allOn, document.createTextNode(T["settings.secrets.ws_allow_all"]));
  const c = card(T["settings.secrets.ws_title"],
    el("div", {class:"hint"},
      T["settings.secrets.ws_hint"]),
    listBox,
    el("div", {class:"row", style:"margin-top:10px"}, allLabel));
  setTimeout(() => loadWsSecrets(ws), 0);
  return c;
}

async function loadWsSecrets(ws) {
  const box = document.getElementById("wssecretslist");
  if (!box) return;
  let j;
  try { j = await fetch("/api/secrets", {headers:{"X-Token":TOKEN}}).then(r=>r.json()); }
  catch (e) { box.textContent=""; box.append(el("div",{class:"hint warn"},T["settings.secrets.load_failed"])); return; }
  box.textContent = "";
  if (j.mode === "locked") {
    box.append(el("div",{class:"hint warn"},T["settings.secrets.ws_locked"]));
    return;
  }
  if (!j.secrets || !j.secrets.length) {
    box.append(el("div",{class:"hint"},T["settings.secrets.ws_none"]));
    return;
  }
  const allowAll = !!ws.secrets_allow_all;
  for (const s of j.secrets) {
    const cb = el("input", {type:"checkbox"});
    cb.checked = allowAll || ws.secrets_allow.includes(s.key);
    cb.disabled = allowAll;
    cb.addEventListener("change", () => {
      const i = ws.secrets_allow.indexOf(s.key);
      if (cb.checked && i < 0) ws.secrets_allow.push(s.key);
      else if (!cb.checked && i >= 0) ws.secrets_allow.splice(i, 1);
      refreshSave();
    });
    const l = el("label", {class:"check",
      style:"display:flex;gap:10px;align-items:center;padding:6px 0;border-bottom:1px solid var(--line)"});
    l.append(cb,
      el("span", {class:"mono", style:"min-width:170px;color:var(--text)"}, s.key),
      el("span", {class:"hint", style:"flex:1"}, s.description || ""));
    box.append(l);
  }
}

// Both export and import operate against the config that's on disk.
// Exporting the in-progress editing state would create a config that only the recipient has
function savedAlready() {
  if (snapshot() === savedSnapshot) return true;
  result(T["settings.ws.save_first"], true);
  return false;
}

const wsShare = (path, body) => fetch(path, {method:"POST",
  headers:{"Content-Type":"application/json", "X-Token":TOKEN}, body})
  .then(r => r.json()).catch(e => ({ok:false, error:e.message || e}));

async function exportWs(i) {
  if (!savedAlready()) return;
  const j = await wsShare("/api/workspace/export", JSON.stringify({index:i}));
  if (j.cancelled) return;
  if (!j.ok) return result(fill(T["settings.ws.export_failed"], {error:j.error || ""}), true);
  result(fill(T["settings.ws.exported"], {path:j.path}));
}

async function importWs() {
  if (!savedAlready()) return;
  const j = await wsShare("/api/workspace/import", null);
  if (j.cancelled) return;
  if (!j.ok) return result(fill(T["settings.ws.import_failed"], {error:j.error || ""}), true);
  // The config has already been rewritten on the server side; reload it here on screen
  await load();
  sel = {ws:wss.length - 1, tab:null, global:false};
  render();
  const moved = (j.moved || []).map(m => m[0] + " → " + m[1]).join(" / ");
  result(fill(T["settings.ws.imported"], {name:j.name, files:j.files})
    + (moved ? "  " + fill(T["settings.ws.imported.moved"], {moved}) : ""));
}

const TEMPLATES = {
  single: [ {name:"Claude", command:"claude"} ],
  review: [ {name:T["settings.template.tab.build"], command:"claude"},
            {name:T["settings.template.tab.review"], id:"reviewer", command:"codex", depth:1, locked:true} ],
  ssh:    [ {name:T["settings.template.tab.server"], command:"ssh user@example.com", profile:"claude", auto_restart:true} ],
  docker: [ {name:T["settings.template.tab.container"], command:"docker exec -it -w /app myapp bash", profile:"claude"} ],
  wsl:    [ {name:"Ubuntu", command:"wsl -d Ubuntu --cd /home/me/proj -- bash", profile:"claude"} ],
};
function addTemplate(kind) {
  const ws = wss[sel.ws];
  const at = (ws.tabs || [])[sel.tab];
  const group = at ? (at.group || 0) : 0;
  ws.tabs = (ws.tabs || []).concat(TEMPLATES[kind].map(x => newTab(Object.assign({group}, x))));
  sel.tab = ws.tabs.length - TEMPLATES[kind].length;
  render();
  msg(T["settings.template.added"]);
}

function tabPane(ws, t) {
  const box = el("div");

  // Basics: name and ID are identity, so place them side by side.
  // If ID is empty, auto-derive one from the name (English → slug / Japanese-only → 5-char hash).
  // The guessed value is shown as a placeholder and finalized once the name field is left
  const idInput = field(t, "id", "", {grow:false, width:280, mono:true});
  const refreshIdPh = () => {
    idInput.placeholder = uniqueId(ws, slugId(t.name), t) || T["settings.tab.id.ph"];
  };
  const nameInput = field(t, "name", T["settings.tab.name.ph"], {grow:false, width:280,
    onInput:() => { renderNav(); refreshIdPh(); }});
  nameInput.addEventListener("blur", () => {
    if (!(t.id || "").trim()) {
      const sug = uniqueId(ws, slugId(t.name), t);
      if (sug) { t.id = sug; idInput.value = sug; refreshSave(); renderNav(); }
    }
  });
  refreshIdPh();
  box.append(card(T["settings.tab.basic"],
    row(T["settings.tab.name"], nameInput),
    row(T["settings.tab.id"], idInput,
        el("span", {class:"hint"}, T["settings.tab.id.hint"]))));

  // What gets launched
  const cmdRow = el("div", {class:"row"});
  const real = launchLine(t);
  const cmdInput = field(t, "command", T["settings.tab.command.ph"],
    {mono:true, onInput:() => { renderNav(); real.schedule(); }});
  cmdInput.setAttribute("list", "cmdlist");
  const detailBox = el("div");
  const rebuild = () => { detailBox.textContent = ""; detailBox.append(kindPanel(t, cmdInput, rebuild)); };
  cmdRow.append(el("label", {}, T["settings.tab.kind"]),
    choose({k:catOf(t.command)}, "k", CAT_LIST, v => {
      setCommand(t, cmdInput, CAT_START[v] || ""); rebuild();
    }));
  rebuild();
  box.append(card(T["settings.tab.launch"], cmdRow, detailBox,
    row(T["settings.tab.command"], cmdInput), real.box));

  // Notify on answer: a beginner-friendly way to get a Slack/Telegram ping when
  // this tab's AI finishes, without writing on_done Lua. Lists the destinations
  // registered under General → Notifications.
  {
    const dests = Object.keys(current.notify || {});
    const opts = [["", T["settings.tab.notify.none"]]].concat(dests.map(n => [n, n]));
    const sel = choose(t, "notify_on_done", opts, v => { if (!v) delete t.notify_on_done; refreshSave(); });
    const hint = dests.length ? T["settings.tab.notify.hint"] : T["settings.tab.notify.none_hint"];
    box.append(card(T["settings.tab.notify.title"],
      row(T["settings.tab.notify.label"], sel, el("span", {class:"hint"}, hint))));
  }

  // Automation: make it visible at a glance what's already configured
  const ev = el("div", {class:"events"});
  for (const [id, label, hint] of eventsFor(t).filter(e => e[0] !== "_shared")) {
    ev.append(el("div", {class:"event"},
      el("div", {class:"name"}, label, el("div", {class:"hint"}, hint)),
      el("span", {class:"state", id:"st-" + id}, "—"),
      el("button", {class:"quiet", onclick:() => openAuto(ws, t, id)}, T["common.edit"])));
  }
  box.append(card(T["settings.tab.automation"], ev));
  loadAutoStates(ws, t);

  // Details: fold away things that are rarely touched
  const det = el("details");
  det.append(el("summary", {}, T["settings.tab.details"]));
  det.append(
    row(T["settings.tab.profile"], field(t, "profile", T["settings.tab.profile.ph"], {grow:false, width:220}),
        el("span", {class:"hint"}, T["settings.tab.profile.hint"])),
    row(T["settings.tab.automation_dir"], ...pathField(t, "automation", T["settings.tab.automation_dir.ph"], "dir",
        T["settings.tab.automation_dir.pick"])),
    row(T["settings.tab.encoding"], choose(t, "encoding",
        [["",T["settings.tab.encoding.utf8"]],["shift_jis","Shift_JIS"],["euc-jp","EUC-JP"]])),
    row(T["settings.tab.scrollback"], field(t, "scrollback", "5000", {type:"number", width:120, grow:false})),
    el("div", {class:"row"}, el("label", {}, T["settings.tab.behavior"]),
       check(t, "locked", T["settings.tab.locked"]),
       check(t, "auto_restart", T["settings.tab.auto_restart"]),
       check(t, "log", T["settings.tab.log"])));
  // Which folder this tab sits in. Same family as the order buttons below — both
  // decide where the tab sits — so they stay together. Only offered when there is
  // more than one folder. Through a holder, because a select speaks strings and a group is a number
  if ((ws.folders || []).length > 1)
    det.append(row(T["settings.tab.folder"],
      choose({at: String(t.group || 0)}, "at",
             ws.folders.map((g, i) => [String(i), folderLabel(g, i)]),
             v => { sel.tab = setTabGroup(ws, sel.tab, Number(v)); render(); refreshSave(); }),
      el("span", {class:"hint"}, T["settings.tab.folder.hint"])));
  det.append(
    el("div", {class:"row"}, el("label", {}, T["settings.tab.order"]),
       el("button", {class:"quiet", onclick:() => moveTab(ws, -1)}, T["settings.tab.move_up"]),
       el("button", {class:"quiet", onclick:() => moveTab(ws, 1)}, T["settings.tab.move_down"]),
       el("button", {class:"quiet", onclick:() => { t.depth = Math.min((t.depth||0)+1, sel.tab); render(); }}, T["settings.tab.indent"]),
       el("button", {class:"quiet", onclick:() => { t.depth = Math.max((t.depth||0)-1, 0); render(); }}, T["settings.tab.outdent"])));
  box.append(el("div", {class:"card"}, det));

  box.append(el("div", {class:"row"},
    el("button", {class:"danger", onclick:() => {
      if (confirm(fill(T["settings.tab.delete_confirm"], {name: t.name || T["settings.tab.unnamed"]}))) {
        ws.tabs.splice(sel.tab, 1); sel.tab = null; render();
      }
    }}, T["settings.tab.delete"])));
  return box;
}

// Moves a tab to another folder, children and all: they are drawn under it and
// work in the same place, so leaving them behind would split one family across
// two folders. The list stays ordered by folder, which is what the headings in
// the nav and the tab bar are drawn from. Returns where the tab ended up
function setTabGroup(ws, i, group) {
  let end = i + 1;
  while (end < ws.tabs.length && ws.tabs[end].depth > ws.tabs[i].depth) end++;
  const moved = ws.tabs.splice(i, end - i);
  moved.forEach(t => t.group = group);
  let at = ws.tabs.length;
  for (let k = 0; k < ws.tabs.length; k++) if ((ws.tabs[k].group || 0) > group) { at = k; break; }
  ws.tabs.splice(at, 0, ...moved);
  return at;
}

function moveTab(ws, d) {
  const i = sel.tab, j = i + d;
  if (j < 0 || j >= ws.tabs.length) return;
  // Swapping across a folder boundary would move a tab to another folder
  // without saying so. The folder is picked in the tab's own settings
  if ((ws.tabs[i].group || 0) !== (ws.tabs[j].group || 0)) return;
  [ws.tabs[i], ws.tabs[j]] = [ws.tabs[j], ws.tabs[i]];
  sel.tab = j; render();
}

/// Type-specific input helpers (SSH / Docker / WSL)
// The AI category's panel. One dropdown lists CLI-type AIs (Claude/Codex/…,
// with a "(not installed)" note if missing) and every registered API provider,
// side by side, plus "＋ Add AI" at the bottom. Picking a provider (API) reveals
// a model-name field whose candidates are auto-loaded (no hardcoded guess).
// Show a CLI's own `--help` in a modal. Asked live from the tool, so it always
// matches the installed version; the output stays in the tool's own language.
async function showCliHelp(head) {
  if (!head) return;
  const pre = el("pre", {class:"mono",
    style:"max-height:60vh;overflow:auto;white-space:pre-wrap;font-size:12px;line-height:1.5;margin:0;padding:12px;background:var(--panel);border-radius:8px"},
    T["settings.tab.ai.flags_loading"]);
  let back;
  const close = el("button", {class:"quiet", onclick:() => back && back.remove()}, T["common.close"]);
  back = openModal(el("h2", {}, head + " --help"), pre,
    el("div", {class:"row", style:"justify-content:flex-end;margin-top:10px"}, close));
  try {
    const r = await fetch("/api/cli-help?cmd=" + encodeURIComponent(head),
      {headers:{"X-Token":TOKEN}}).then(r => r.json());
    pre.textContent = (r && r.ok && (r.help || "").trim())
      ? r.help : ((r && r.error) || T["settings.tab.ai.flags_failed"]);
  } catch (e) { pre.textContent = T["settings.tab.ai.flags_failed"]; }
}

function aiPanel(t, cmdInput, rebuild) {
  const box = el("div");
  const sel = el("select");
  for (const c of AI_CLIS) {
    const ok = c.check ? aiEngines.some(e => e.id === c.check) : true;
    sel.append(el("option", {value:"cli:" + c.cmd}, c.label + (!ok ? T["settings.tab.common.missing"] : "")));
  }
  const provs = Object.keys(current.providers || {});
  for (const n of provs) sel.append(el("option", {value:"prov:" + n}, n));
  sel.append(el("option", {value:"add-ai"}, T["settings.tab.ai.add"]));

  // Reflect the current command in the dropdown.
  const cur = parseModel(t.command);
  if (cur && cur.provider) sel.value = "prov:" + cur.provider;
  else { const h = headOf(t.command); sel.value = AI_CLIS.some(c => c.cmd === h) ? "cli:" + h : ""; }

  const detail = el("div");
  // "Show flags" runs the selected CLI's --help. Hidden for API model tabs
  // (those talk to an endpoint, so there is no local --help to show).
  const helpBtn = el("button", {class:"quiet"}, T["settings.tab.ai.flags"]);
  helpBtn.onclick = () => showCliHelp(headOf(t.command));
  const drawDetail = () => {
    detail.textContent = "";
    const m = parseModel(t.command);
    helpBtn.hidden = !!m || !headOf(t.command);
    // An API model tab holds its exchange in this app, not in a CLI's records:
    // there is no conversation to be handed back at launch
    carry.hidden = !!m || !headOf(t.command);
    if (!m) {
      // A CLI is selected. If it has an "act without asking" flag, surface it
      // as an explicit, explained checkbox — required for autonomous discussion
      // / automation, but it lets the AI edit files and run commands unattended.
      // Toggling only adds/removes that one token, so other args are preserved.
      const flag = cliFlagOf(headOf(t.command));
      if (!flag) return;
      const cb = el("input", {type:"checkbox"});
      cb.checked = (t.command || "").split(/\s+/).includes(flag);
      cb.addEventListener("change", () => {
        let c = (t.command || "").trim();
        const parts = c.split(/\s+/).filter(Boolean);
        if (cb.checked) { if (!parts.includes(flag)) parts.push(flag); }
        else { for (let i = parts.length - 1; i >= 0; i--) if (parts[i] === flag) parts.splice(i, 1); }
        setCommand(t, cmdInput, parts.join(" "));
      });
      detail.append(
        el("label", {class:"row", style:"cursor:pointer;gap:8px"}, cb,
          el("span", {}, T["settings.tab.ai.autoapprove"])),
        el("div", {class:"row"}, el("label", {}, ""),
          el("span", {class:"hint", style:"color:var(--danger)"}, T["settings.tab.ai.autoapprove_risk"])));
      return;
    }
    const modelIn = el("input", {type:"text", class:"mono", style:"width:260px",
      placeholder:T["settings.model.name_ph"]});
    modelIn.value = m.model || "";
    const setModel = name => {
      setCommand(t, cmdInput, "model " + m.provider + (name ? "/" + name : ""));
    };
    modelIn.addEventListener("input", () => setModel(modelIn.value.trim()));
    const cand = modelCandidates(() => current.providers[m.provider] || {},
      id => { modelIn.value = id; setModel(id); });
    detail.append(
      el("div", {class:"row"}, el("label", {}, T["settings.model.name_label"]), modelIn, cand.btn),
      el("div", {class:"row"}, el("label", {}, ""), cand.chips));
    // Auto-load real models and preselect the first if none is set yet.
    cand.load().then(models => {
      if (models && models.length && !(m.model || "").trim()) { modelIn.value = models[0]; setModel(models[0]); }
    });
  };

  sel.addEventListener("change", async () => {
    const v = sel.value;
    if (v === "add-ai") {
      const before = new Set(Object.keys(current.providers || {}));
      await openProvidersPopup();
      const added = Object.keys(current.providers || {}).find(n => !before.has(n));
      if (added) setCommand(t, cmdInput, "model " + added + "/");
      rebuild();                          // redraw: the new provider now appears (and its model field)
      return;
    }
    if (v.startsWith("cli:")) { const h = v.slice(4); const f = cliFlagOf(h); setCommand(t, cmdInput, f ? h + " " + f : h); }
    else if (v.startsWith("prov:")) setCommand(t, cmdInput, "model " + v.slice(5) + "/");
    drawDetail();
  });

  // Whether this tab comes back to the conversation it was having when the app
  // last closed. It sits here, under the CLI's own switches and directly above
  // the command line, because that is where the id it needs is visible: the
  // "what actually runs" line right below shows the conversation argument this
  // is the switch for.
  //
  // Outside `detail`, which is rebuilt per CLI and returns early for the ones
  // with no "act without asking" flag -- this applies to every CLI tab, and a
  // setting that vanished depending on which AI was picked would look like a
  // bug in the settings rather than a choice.
  const carry = el("div");
  {
    // Built the way the "act without asking" row above it is, rather than with
    // checkDefaultOn: that one is shaped for the label-and-field grid, and in a
    // full-width row it puts the words in the narrow left column, where they
    // wrap mid-sentence. Two checkboxes one under the other have to look alike
    const cb = el("input", {type:"checkbox"});
    cb.checked = t.restore_conversation !== false;
    cb.addEventListener("change", () => {
      // Absent means yes, so the common answer leaves nothing in the file
      if (cb.checked) delete t.restore_conversation;
      else t.restore_conversation = false;
      // The line below explains what the conversation argument is for, and the
      // answer just changed. Announced through the command field's own event,
      // the one path every other writer of that line already goes through
      cmdInput.dispatchEvent(new Event("input", {bubbles: true}));
    });
    carry.append(
      el("label", {class:"row", style:"cursor:pointer;gap:8px"}, cb,
        el("span", {}, T["settings.tab.restore_conv"])),
      el("div", {class:"row"}, el("label", {}, ""),
        el("span", {class:"hint"}, T["settings.tab.restore_conv.hint"])));
  }

  box.append(el("div", {class:"row"}, el("label", {}, T["settings.tab.ai.pick"]), sel, helpBtn));
  if (!provs.length)
    box.append(el("div", {class:"row"}, el("label", {}, ""),
      el("span", {class:"hint"}, T["settings.tab.ai.api_hint"])));
  box.append(detail, carry);
  drawDetail();
  return box;
}

// The inline "add an API AI" flow: the Providers editor in a popup, so a
// first-timer can register DeepSeek / a local LLM without leaving this screen.
// The global-settings Providers editor stays too (this reuses the same card).
function openProvidersPopup() {
  return new Promise(resolve => {
    const m = openModal(
      el("h2", {}, T["settings.tab.ai.add_title"]),
      el("div", {class:"hint"}, T["settings.tab.ai.api_hint"]),
      providersCard(),
      el("div", {class:"row", style:"border-top:1px solid var(--line);margin-top:12px;padding-top:12px;justify-content:flex-end"},
        el("button", {class:"primary", onclick: () => { m.remove(); resolve(); }}, T["common.done"])));
    m.addEventListener("click", e => { if (e.target === m) resolve(); });
  });
}

function kindPanel(t, cmdInput, rebuild) {
  if (catOf(t.command) === "ai") return aiPanel(t, cmdInput, rebuild);
  const box = el("div");
  const ssh = parseSsh(t.command), dk = parseDocker(t.command), wsl = parseWsl(t.command);
  const web = parseBrowser(t.command);
  const mdl = parseModel(t.command);
  const sync = (build, o) => () => {
    setCommand(t, cmdInput, build(o));
  };
  const f = (obj, key, label, ph, upd, w, sug) => {
    const i = el("input", {type:"text", placeholder:ph, class:"mono"});
    if (w) i.style.width = w + "px";
    i.value = obj[key] || "";
    i.addEventListener("input", () => { obj[key] = i.value.trim(); upd(); });
    if (sug) suggest(i, sug);
    return [el("label", {}, label), i];
  };
  if (ssh) {
    const upd = sync(buildSsh, ssh);
    box.append(el("div", {class:"row"}, ...f(ssh, "host", T["settings.ssh.host"], "example.com", upd, 240, "ssh"),
      el("label", {class:"beside"}, T["settings.phone.port"]),
      (() => { const i = el("input", {type:"text", class:"mono", style:"width:70px"});
               i.value = ssh.port || ""; i.placeholder = "22";
               i.addEventListener("input", () => { ssh.port = i.value.trim(); upd(); }); return i; })(),
      el("label", {class:"beside"}, T["settings.ssh.user"]),
      (() => { const i = el("input", {type:"text", class:"mono", style:"width:130px"});
               i.value = ssh.user || ""; i.placeholder = "root";
               i.addEventListener("input", () => { ssh.user = i.value.trim(); upd(); }); return i; })()));
    const keyIn = el("input", {type:"text", class:"mono grow", placeholder:T["settings.ssh.key.ph"]});
    keyIn.value = ssh.key || "";
    keyIn.addEventListener("input", () => { ssh.key = keyIn.value.trim(); upd(); });
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.ssh.key"]), keyIn,
      REMOTE ? null : el("button", {class:"quiet", onclick: async () => {
        const p = await pickPath("key", T["settings.ssh.key.pick"], ssh.key);
        if (p !== null) { ssh.key = p; keyIn.value = p; upd(); }
      }}, T["common.browse"])));
    const adv = el("details"); adv.append(el("summary", {}, T["settings.ssh.details"]));
    const fwd = el("input", {type:"text", class:"mono grow",
      placeholder:T["settings.ssh.forward.ph"]});
    fwd.value = (ssh.forwards || []).join(", ");
    fwd.addEventListener("input", () => {
      ssh.forwards = fwd.value.split(",").map(s => s.trim()).filter(Boolean); upd(); });
    adv.append(el("div", {class:"row"}, el("label", {}, T["settings.ssh.forward"]), fwd),
      el("div", {class:"row"}, ...f(ssh, "jump", T["settings.ssh.jump"], "gw.example.com", upd, 200),
        ...f(ssh, "keepalive", T["settings.ssh.keepalive"], "60", upd, 80)),
      el("div", {class:"row"}, el("label", {}, T["settings.ssh.allow"]),
        (() => { const c = el("input", {type:"checkbox"}); c.checked = ssh.agent;
          c.addEventListener("change", () => { ssh.agent = c.checked; upd(); });
          const l = el("label", {class:"check"}); l.append(c, document.createTextNode(T["settings.ssh.agent"]));
          return l; })(),
        (() => { const c = el("input", {type:"checkbox"}); c.checked = ssh.x11;
          c.addEventListener("change", () => { ssh.x11 = c.checked; upd(); });
          const l = el("label", {class:"check"}); l.append(c, document.createTextNode(T["settings.ssh.x11"]));
          return l; })()));
    box.append(adv);
  } else if (web) {
    const upd = sync(buildBrowser, web);
    const u = el("input", {type:"text", class:"mono grow",
      placeholder:"https://example.com/"});
    u.value = web.url || "";
    // Better to flag an unopenable URL while it's being typed than to discover it after opening
    const note = el("span", {class:"hint"});
    const check = () => {
      const bad = web.url && !openableUrl(web.url);
      note.textContent = bad ? T["settings.browser.url.bad"] : T["settings.browser.url.hint"];
      note.style.color = bad ? "var(--danger)" : "";
    };
    u.addEventListener("input", () => { web.url = u.value.trim(); check(); upd(); });
    check();
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.browser.url"]), u));
    box.append(el("div", {class:"row"}, el("label", {}, ""), note));

    // Controls shown on top of the page. Only the ones enabled here take effect
    t.nav = t.nav || {};
    const part = (key, label) => {
      const c = el("input", {type:"checkbox"});
      c.checked = !!t.nav[key];
      c.addEventListener("change", () => { t.nav[key] = c.checked; });
      const l = el("label", {class:"check"});
      l.append(c, document.createTextNode(label));
      return l;
    };
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.browser.nav"]),
      part("back", T["tui.nav.back"]),
      part("forward", T["tui.nav.forward"]),
      part("reload", T["tui.nav.reload"]),
      part("reload_hard", T["tui.nav.reload_hard.short"]),
      part("url", T["tui.nav.url"])));
    box.append(el("div", {class:"row"}, el("label", {}, ""),
      el("span", {class:"hint"}, T["settings.browser.nav.hint"])));

    // Profile (the cookie/login store) vs. private (throwaway).
    // Defaults to "default". Changing the name splits off separate logins (like Chrome's "person").
    // Checking private hides the name field and uses a throwaway area internally
    const profRow = el("div", {class:"row"});
    const profInput = el("input", {type:"text", class:"mono", style:"width:220px", placeholder:"default"});
    profInput.value = t.browser_profile || "";
    profInput.addEventListener("input", () => {
      const v = profInput.value.trim();
      if (v) t.browser_profile = v; else delete t.browser_profile;
    });
    profRow.append(el("label", {}, T["settings.browser.profile"]), profInput,
      el("span", {class:"hint"}, T["settings.browser.profile.hint"]));
    const priv = el("input", {type:"checkbox"});
    priv.checked = !!t.private;
    const privLabel = el("label", {class:"check"});
    privLabel.append(priv, document.createTextNode(T["settings.browser.private"]));
    const applyPriv = () => {
      if (priv.checked) { t.private = true; profRow.style.display = "none"; }
      else { delete t.private; profRow.style.display = ""; }
    };
    priv.addEventListener("change", applyPriv);
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.browser.data"]), privLabel,
      el("span", {class:"hint"}, T["settings.browser.private.hint"])));
    box.append(profRow);
    applyPriv();

    // What this page calls itself. Empty follows the app-wide setting, which
    // is the answer almost every page wants -- this is here for the one that
    // does not
    const uaInput = el("input", {type:"text", class:"mono", style:"flex:1;min-width:0",
      placeholder:T["settings.browser.ua.ph"]});
    uaInput.value = t.user_agent || "";
    uaInput.addEventListener("input", () => {
      const v = uaInput.value.trim();
      if (v) t.user_agent = v; else delete t.user_agent;
    });
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.browser.ua"]), uaInput));
    box.append(el("div", {class:"row"}, el("label", {}, ""),
      el("span", {class:"hint"}, T["settings.browser.ua.hint"])));

    // The band shown at the bottom. A single checkbox isn't enough (needs message text and a button label),
    // so only show the content fields once "show it" is turned on
    const askOn = el("input", {type:"checkbox"});
    askOn.checked = !!t.ask;
    const askLabel = el("label", {class:"check"});
    askLabel.append(askOn, document.createTextNode(T["settings.browser.ask.on"]));
    const askBody = el("div");
    const drawAsk = () => {
      askBody.textContent = "";
      if (!t.ask) return;
      askBody.append(
        el("div", {class:"row"}, el("label", {}, T["settings.browser.ask.text"]),
          field(t.ask, "text", T["settings.browser.ask.text.ph"])),
        el("div", {class:"row"}, el("label", {}, T["settings.browser.ask.label"]),
          field(t.ask, "label", T["tui.ask.label"], {grow:false, width:200})));
    };
    askOn.addEventListener("change", () => {
      t.ask = askOn.checked ? (t.ask || {text:"", label:""}) : null;
      drawAsk();
    });
    drawAsk();
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.browser.ask"]), askLabel));
    box.append(askBody);
    box.append(el("div", {class:"row"}, el("label", {}, ""),
      el("span", {class:"hint"}, T["settings.browser.ask.hint"])));
  } else if (dk || wsl) {
    const o = dk || wsl, upd = sync(dk ? buildDocker : buildWsl, o);
    box.append(el("div", {class:"row"},
      ...(dk ? f(o, "container", T["settings.container.name"], "myapp", upd, 200)
             : f(o, "distro", T["settings.container.distro"], "Ubuntu", upd, 200, "wsl")),
      ...f(o, "dir", T["settings.container.dir"], "/home/me/proj", upd, 220)));
    box.append(el("div", {class:"row"},
      ...f(o, "shell", T["settings.container.shell"], "bash / claude", upd, 220),
      el("span", {class:"hint"}, T["settings.container.hint"])));
  } else if (isGitPanel(t.command)) {
    // A git panel has nothing to launch: no command to pick, no arguments to
    // get right. What it needs is the folder, and that is the row below
    return el("div", {class:"hint"}, T["settings.tab.kind.git.hint"]);
  } else {
    const s = el("select");
    s.append(el("option", {value:""}, T["settings.tab.common.pick"]));
    for (const c of SHELL_CMDS) {
      const ok = c.check ? aiEngines.some(e => e.id === c.check) : true;
      s.append(el("option", {value:c.cmd}, c.label + (c.check && !ok ? T["settings.tab.common.missing"] : "")));
    }
    s.addEventListener("change", () => {
      if (!s.value) return;
      setCommand(t, cmdInput, s.value); s.value = "";
    });
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.tab.common"]), s));
  }
  return box;
}

// ── Automation editor ───────────────────────────────────
// Session hooks. None of these ever fire for a browser
const TAB_EVENTS = [
  ["on_start",    T["automation.on_start"],           T["automation.on_start.hint"]],
  ["on_done",     T["automation.on_done"],     T["automation.on_done.hint"]],
  ["on_question", T["automation.on_question"],     T["automation.on_question.hint"]],
  ["on_exit",     T["automation.on_exit"],           T["automation.on_exit.hint"]],
  ["on_busy",     T["automation.on_busy"],     T["automation.on_busy.hint"]],
  ["_shared",     T["automation._shared"],       ""],
];
// Browser hooks. A page has no state, so the wording differs
const PAGE_EVENTS = [
  ["on_load",     T["automation.on_load"],     T["automation.on_load.hint"]],
  ["on_press",    T["automation.on_press"],    T["automation.on_press.hint"]],
  ["_shared",     T["automation._shared"],       ""],
];
// Lists only the events that actually fire for that tab.
// Having a place to write code that never runs is worse than not having it at all
const eventsFor = t => kindOf(t.command) === "browser" ? PAGE_EVENTS : TAB_EVENTS;
let autoTarget = null, autoData = {}, autoEvent = "on_done";

function autoDirOf(ws, t) {
  if (t.automation) return t.automation;
  const slug = s => (s || "").replace(/[^A-Za-z0-9_-]/g, "").toLowerCase();
  const wi = wss.indexOf(ws) + 1, ti = (ws.tabs || []).indexOf(t) + 1;
  return "scripts/" + (slug(ws.name) || ("ws" + wi)) + "/" + (slug(t.id) || slug(t.name) || ("tab" + ti));
}

async function fetchAuto(dir) {
  try {
    return await (await fetch("/api/automation?dir=" + encodeURIComponent(dir),
        {headers:{"X-Token":TOKEN}})).json();
  } catch (e) { return {}; }
}
async function loadAutoStates(ws, t) {
  const data = await fetchAuto(autoDirOf(ws, t));
  for (const [id] of eventsFor(t)) {
    const s = document.getElementById("st-" + id);
    if (!s) continue;
    const on = (data[id] || "").trim().length > 0;
    s.textContent = on ? T["automation.set"] : T["automation.unset"];
    s.className = "state" + (on ? " on" : "");
  }
}

async function openAuto(ws, t, event) {
  autoTarget = { ws, t, dir: autoDirOf(ws, t) };
  document.getElementById("autotitle").textContent =
      fill(T["automation.editor.title"], {name: t.name || T["settings.tab.unnamed"]});
  document.getElementById("autopath").textContent = autoTarget.dir;
  const s = document.getElementById("autoevent");
  s.textContent = "";
  const events = eventsFor(t);
  for (const [id, label] of events) s.append(el("option", {value:id}, label));
  autoData = await fetchAuto(autoTarget.dir);
  // Use showEvent here (switchEvent would capture the textarea's still-there previous-tab
  // content as the new tab's data)
  // Defaults to whichever event is listed first for that tab (on_load for a browser)
  showEvent(event || events[0][0]);
  document.getElementById("airow").style.display = aiEngines.length ? "flex" : "none";
  document.getElementById("ainone").style.display = aiEngines.length ? "none" : "flex";
  document.getElementById("aipreview").style.display = "none";
  // Don't leave the previous tab's AI request text or generated result behind either
  document.getElementById("autoask").value = "";
  document.getElementById("aicode").textContent = "";
  aiBusy(false);
  automsg("");
  document.getElementById("autobox").style.display = "flex";
}
// Switches events. Stashes the currently displayed content first so it isn't lost
function switchEvent() {
  autoData[autoEvent] = document.getElementById("autocode").value;
  showEvent(document.getElementById("autoevent").value);
}

// Swaps the displayed content without stashing anything.
// Right after opening, the previous tab's content is still there, so it must not be stashed
function showEvent(id) {
  autoEvent = id;
  document.getElementById("autoevent").value = id;
  document.getElementById("autocode").value = autoData[autoEvent] || "";
  const list = autoTarget ? eventsFor(autoTarget.t) : TAB_EVENTS;
  const e = list.find(x => x[0] === autoEvent);
  document.getElementById("autohint").textContent = e ? e[2] : "";
}
function closeAuto() { document.getElementById("autobox").style.display = "none"; }

async function saveAuto() {
  autoData[autoEvent] = document.getElementById("autocode").value;
  const r = await fetch("/api/automation?dir=" + encodeURIComponent(autoTarget.dir),
      {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
       body: JSON.stringify(autoData)});
  if (!r.ok) return automsg(T["automation.editor.save_failed"], true);
  const created = autoTarget.t.automation !== autoTarget.dir;
  autoTarget.t.automation = autoTarget.dir;
  closeAuto();
  loadAutoStates(autoTarget.ws, autoTarget.t);
  msg(created ? T["automation.editor.saved_new"] : T["automation.editor.saved"]);
}

// The in-progress-generation look. Showing elapsed seconds matters most to whoever is
// waiting, because it's the difference between "it's stuck" and "it's thinking"
let aiTimer = null;
function aiBusy(on) {
  clearInterval(aiTimer);
  const btn = document.getElementById("aibtn");
  btn.disabled = on;
  btn.textContent = T[on ? "automation.editor.generating" : "automation.editor.generate"];
  document.getElementById("autoask").disabled = on;
  document.getElementById("aibusy").style.display = on ? "flex" : "none";
  if (!on) return;
  const started = Date.now();
  const tick = () => {
    document.getElementById("aibusytext").textContent =
      fill(T["automation.editor.thinking"], {sec: Math.round((Date.now() - started) / 1000)});
  };
  tick();
  aiTimer = setInterval(tick, 1000);
}

async function askAi() {
  const want = document.getElementById("autoask").value.trim();
  if (!want) return automsg(T["automation.editor.want"], true);
  const ws = autoTarget.ws;
  automsg("");
  aiBusy(true);
  try {
    const r = await fetch("/api/generate", {method:"POST",
        headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
        body: JSON.stringify({event: autoEvent, prompt: want,
          engine: current.ai_engine || null,
          tabs: (ws.tabs || []).map((x, i) => ({index:i+1, name:x.name || fill(T["settings.tab.default_name"], {n: i+1}), id:x.id || ""})),
          self: (ws.tabs || []).indexOf(autoTarget.t) + 1})});
    const j = await r.json();
    if (!j.ok) return automsg(fill(T["automation.editor.failed"], {error: j.error}), true);
    document.getElementById("aicode").textContent = j.code;
    document.getElementById("aipreview").style.display = "block";
    automsg(T["automation.editor.check"]);
  } catch (e) {
    // Previously, if the request itself failed, it would just end silently
    automsg(fill(T["automation.editor.failed"], {error: e.message || e}), true);
  } finally {
    aiBusy(false);
  }
}
function applyAi() {
  document.getElementById("autocode").value = document.getElementById("aicode").textContent;
  document.getElementById("aipreview").style.display = "none";
  automsg(T["automation.editor.applied"]);
}
function automsg(t, warn) { const m = document.getElementById("automsg");
  m.textContent = t; m.style.color = warn ? "var(--danger)" : "var(--muted)"; }

// ── Load / save ──────────────────────────────────
// A workspace's working folders, and never none of them: the one everything
// lands in is a folder like any other. Mirrors foldered() in config.rs, which
// says the same thing on the way to launching them
function foldersOf(w) {
  const folders = (w.folders || []).map(f => ({name:f.name || "", id:f.id || "",
                                               cwd:f.cwd || "", tabs:f.tabs || []}));
  if (!folders.length) folders.push({name:"", id:"", cwd:"", tabs:[]});
  return folders;
}

// The screen keeps one flat list of tabs, each remembering which folder it is in
function readFolders(ws, w) {
  const fs = foldersOf(w);
  ws.folders = fs.map(f => ({name:f.name, id:f.id, cwd:f.cwd}));
  ws.tabs = [];
  fs.forEach((f, i) => flatten(f.tabs, 0, i, ws.tabs));
}

function flatten(tabs, depth, group, out) {
  for (const t of tabs || []) {
    out.push({ name: t.name || "", id: t.id || "", command: cmdToText(t.command),
               profile: t.profile || "", automation: t.automation || t.lua || "",
               drives: t.drives || "",
               browser_profile: t.browser_profile || "", private: !!t.private,
               user_agent: t.user_agent || "",
               locked: !!t.locked, auto_restart: !!t.auto_restart,
               encoding: t.encoding || "", scrollback: t.scrollback ?? "", log: !!t.log,
               notify_on_done: t.notify_on_done || "",
               nav: t.nav || null, ask: t.ask || null, depth, group });
    flatten(t.children, depth + 1, group, out);
  }
  return out;
}
function nest(flat) {
  const roots = [], stack = [];
  for (const f of flat) {
    const node = { name: f.name, command: f.command };
    if (f.id) node.id = f.id;
    if (f.profile) node.profile = f.profile;
    if (f.automation) node.automation = f.automation;
    if (f.drives) node.drives = f.drives;
    if (f.browser_profile) node.browser_profile = f.browser_profile;
    if (f.private) node.private = true;
    if (f.user_agent) node.user_agent = f.user_agent;
    if (f.locked) node.locked = true;
    if (f.auto_restart) node.auto_restart = true;
    if (f.encoding) node.encoding = f.encoding;
    if (f.scrollback) node.scrollback = Number(f.scrollback);
    if (f.log) node.log = true;
    if (f.notify_on_done) node.notify_on_done = f.notify_on_done;
    // Don't write it if none are enabled. Leaving a block of all-false values just hurts readability
    if (f.nav && Object.values(f.nav).some(Boolean)) {
      node.nav = {};
      for (const k of ["back", "forward", "reload", "url"])
        if (f.nav[k]) node.nav[k] = true;
    }
    // The mere presence of this block means "show it"; don't write it when empty
    if (f.ask) {
      node.ask = {};
      if (f.ask.text) node.ask.text = f.ask.text;
      if (f.ask.label) node.ask.label = f.ask.label;
    }
    const d = Math.min(f.depth, stack.length);
    if (d === 0) roots.push(node);
    else (stack[d - 1].children = stack[d - 1].children || []).push(node);
    stack[d] = node; stack.length = d + 1;
  }
  return roots;
}

async function loadAi() {
  try { aiEngines = (await (await fetch("/api/ai", {headers:{"X-Token":TOKEN}})).json()).engines || []; }
  catch (e) { aiEngines = []; }
  const dl = document.getElementById("cmdlist");
  dl.textContent = "";
  for (const c of COMMON_COMMANDS) dl.append(el("option", {value:c.cmd}, c.label));
}

async function load() {
  clearLoadFailure();
  await loadAi();
  const cfg = await readUserJson(await api("GET"));
  if (cfg.failure) return showLoadFailure(cfg.failure);
  current = cfg.value;
  // Show the built-in starter actions as editable rows when none are configured,
  // matching what the sub-input bar displays out of the box. They're dropped again
  // on save unless the user changes them (see payload), so config stays tidy.
  if (!Array.isArray(current.actions) || !current.actions.length) {
    current.actions = defaultActions();
  }
  loadedLanguage = (current.language || "").trim().toLowerCase();
  const list = (Array.isArray(current.workspaces) && current.workspaces.length)
      ? current.workspaces
      : [{ name:"DEFAULT", tabs: current.tabs || [] }];
  wss = [];
  for (const w of list) {
    const ws = { name:w.name || "", file:w.file || null,
                 automation:w.automation || w.lua || "", tabs:[], folders:[],
                 // Not touched from the screen, but kept so saving doesn't drop it
                 browsers:w.browsers || null,
                 secrets_allow: w.secrets_allow || [],
                 secrets_allow_all: !!w.secrets_allow_all,
                 stops: Array.isArray(w.stops) ? w.stops : [],
                 discuss: w.discuss || null };
    if (ws.file) {
      const got = await readUserJson(await wsApi("GET", ws.file));
      // A workspace file is loaded to be written back. If it can't be read, the
      // tabs would come out empty and saving would erase them, so stop here too.
      if (got.failure) return showLoadFailure(got.failure);
      const f = got.value;
      readFolders(ws, f);
      if (!ws.automation) ws.automation = f.automation || f.lua || "";
      if (!ws.secrets_allow.length) ws.secrets_allow = f.secrets_allow || [];
      if (!ws.secrets_allow_all) ws.secrets_allow_all = !!f.secrets_allow_all;
      if (!ws.stops.length && Array.isArray(f.stops)) ws.stops = f.stops;
      if (!ws.discuss && f.discuss) ws.discuss = f.discuss;
    } else readFolders(ws, w);
    wss.push(ws);
  }
  if (sel.ws >= wss.length) sel = {ws:0, tab:null, global:true};
  render();
  markClean();
  msg(T["common.loaded"]);
}

async function save() {
  // Never write over a file we couldn't read. The form is empty because loading
  // failed, not because the user emptied it.
  if (loadFailure) { result(T["settings.broken.save_blocked"], true); return; }
  // A fixed phone token shorter than a secret is refused here, before it can
  // land in the file (the app would refuse to start the remote on it anyway)
  if (current.remote && current.remote.sticky_token && String(current.remote.fixed_token || "").trim().length < 16) {
    if (sel.global) { sel.section = "remote"; render(); }
    result(fill(T["settings.phone.sticky.short"], {n: 16}), true);
    return;
  }
  // Refuse to write a quick action whose Lua doesn't even parse (compile-checked
  // server-side). Jump to the actions card so the red errors are in view.
  if ((current.actions || []).some(a => a.lua)) {
    if (!(await actionsLintClean())) {
      if (sel.global) { sel.section = "actions"; render(); }
      result(T["settings.actions.lint_blocked"], true);
      return;
    }
  }
  const btn = document.getElementById("savebtn");
  btn.disabled = true;
  btn.classList.remove("dirty");
  btn.textContent = T["common.saving"];
  let ok = false;
  try {
    await doSave();
    ok = true;
  } catch (e) {
    // Previously, if the request itself failed, it would end with nothing shown at all
    result(fill(T["settings.save_failed"], {error: e.message || e}), true);
  } finally {
    btn.disabled = false;
    btn.textContent = T["common.save"];
    refreshSave();
  }
  // Opened via a deep-link shortcut (?ret=1): a successful save is the finish, so
  // bounce back to the board it was launched from. closeSettings covers both the
  // window and the phone, and its unsaved-changes guard is a no-op right after a
  // save (markClean already ran inside doSave).
  if (ok && returnOnSave) { returnOnSave = false; closeSettings(); }
}

// Assembles what gets written. Used by both saving and unsaved-change detection.
// Also called from the every-600ms unsaved check, so this must have no side effects
function payload() {
  const out = Object.assign({}, current);
  ["tab_bar_width","max_chain"].forEach(k => {
    const v = out[k]; if (v === "" || v === null || v === undefined) delete out[k]; else out[k] = Number(v);
  });
  ["automation","secrets","ai_engine","browser_data","language","user_agent"].forEach(k => { if (!out[k]) delete out[k]; });
  // Quick actions: drop rows left without a label, and omit the key entirely if none remain.
  if (out.actions) {
    out.actions = out.actions
      .filter(a => a && (a.label || "").trim())
      // `lua` is off unless it's on, so only the exception is worth writing down.
      .map(a => { const o = Object.assign({}, a); if (!o.lua) delete o.lua; return o; });
    // Drop the block when empty, or when it's still exactly the seeded starter set
    // (so an untouched default config isn't written out with the shown rows).
    const def = defaultActions();
    const sameAsDefault = out.actions.length === def.length && out.actions.every((a, i) =>
      a.label === def[i].label && (a.body || "") === (def[i].body || "") && !a.lua === !def[i].lua);
    if (!out.actions.length || sameAsDefault) delete out.actions;
  }
  // Operate limits/policy: coerce to numbers, then drop the block entirely when
  // it still matches the defaults (keeps config tidy). 0 = "no limit" is kept.
  if (out.operate) {
    const o = Object.assign({}, out.operate);
    ["max_rounds","max_seconds","max_tokens","settle_ms"].forEach(k => {
      o[k] = (o[k] === "" || o[k] === null || o[k] === undefined) ? undefined : Number(o[k]);
    });
    const isDefault = (o.max_rounds ?? 40) === 40 && (o.max_seconds ?? 900) === 900
      && (o.max_tokens ?? 400000) === 400000 && (o.on_limit || "stop") === "stop"
      && (o.settle_ms ?? 1800) === 1800 && (o.confirm || "off") === "off";
    if (isDefault) delete out.operate;
    else out.operate = { max_rounds:(o.max_rounds ?? 40), max_seconds:(o.max_seconds ?? 900),
                         max_tokens:(o.max_tokens ?? 400000), on_limit:(o.on_limit || "stop"),
                         settle_ms:(o.settle_ms ?? 1800), confirm:(o.confirm || "off") };
  }
  if (out.remote && !out.remote.enabled && !out.remote.allow_public) delete out.remote;
  // Don't save a provider with an empty base_url (avoids leaving leftover junk from a still-in-progress add)
  if (out.providers) {
    out.providers = Object.fromEntries(
      Object.entries(out.providers).filter(([, p]) => p && (p.base_url || "").trim()));
    if (!Object.keys(out.providers).length) delete out.providers;
  }
  delete out.lua; delete out.tabs;

  // A group is written with its own tabs nested back under it. Its name and id
  // are worth writing only when someone typed them; the folder always is
  const foldersOut = w => (w.folders && w.folders.length ? w.folders : [{}]).map((g, i) => {
    const o = {};
    for (const k of ["name", "id", "cwd"]) if ((g[k] || "").trim()) o[k] = g[k].trim();
    // The branches this folder guards, when it has an answer of its own. An
    // empty list is an answer too ("nothing here"), so what decides is whether
    // there is a list at all
    if (Array.isArray(g.protect)) o.protect = g.protect;
    // What it would take to make this folder on a machine that does not have
    // it. Written by the app when the folder is made and never shown on this
    // screen -- so it has to be carried through a save, or saving the settings
    // is how the answer gets lost
    if (g.source) o.source = g.source;
    o.tabs = nest(w.tabs.filter(t => (t.group || 0) === i));
    return o;
  });

  // Stop conditions are saved after dropping rows with an empty when
  const cleanStops = w => (w.stops || []).filter(s => s && s.when);
  // A discussion is only saved when there are 2 or more participants
  const cleanDiscuss = w => (w.discuss && (w.discuss.agents || []).length >= 2) ? w.discuss : null;

  // A workspace that was split out into a separate file gets written to that file
  const files = [];
  for (const w of wss) {
    if (!w.file) continue;
    const body = { name:w.name, folders:foldersOut(w) };
    if (w.automation) body.automation = w.automation;
    if (w.secrets_allow && w.secrets_allow.length) body.secrets_allow = w.secrets_allow;
    if (w.secrets_allow_all) body.secrets_allow_all = true;
    const st = cleanStops(w); if (st.length) body.stops = st;
    const dc = cleanDiscuss(w); if (dc) body.discuss = dc;
    files.push({ file:w.file, body });
  }
  out.workspaces = wss.map(w => {
    const o = { name:w.name };
    if (w.file) o.file = w.file;
    else { if (w.automation) o.automation = w.automation; o.folders = foldersOut(w); }
    // Don't lose a setting that isn't on screen just because it was saved from the screen
    if (w.browsers) o.browsers = w.browsers;
    // Allow-list of secrets the rally may use (denied by default)
    if (w.secrets_allow && w.secrets_allow.length) o.secrets_allow = w.secrets_allow;
    if (w.secrets_allow_all) o.secrets_allow_all = true;
    // Stop conditions (judge). Already written into the file for a file-referenced workspace, so don't duplicate it here
    if (!w.file) { const st = cleanStops(w); if (st.length) o.stops = st; }
    // AI vs AI discussion
    if (!w.file) { const dc = cleanDiscuss(w); if (dc) o.discuss = dc; }
    return o;
  });
  return { out, files };
}

async function doSave() {
  // Tabs with an empty id get one derived from the name before writing (a safety net against dropped references).
  // Since this is a side effect, it's done only right before saving (never inside payload's unsaved-check)
  for (const w of wss) ensureIds(w);
  const { out, files } = payload();
  for (const f of files) {
    const rf = await wsApi("POST", f.file, JSON.stringify(f.body, null, 2));
    const jf = await rf.json().catch(() => ({ok:false}));
    if (!jf.ok) { result(fill(T["settings.file_save_failed"], {file: f.file}), true); return; }
  }
  const r = await api("POST", JSON.stringify(out, null, 2));
  const j = await r.json();
  if (!j.ok) { result(fill(T["settings.save_failed"], {error: j.error}), true); return; }
  markClean();
  result(T["common.saved"]);
  // The language is only read at launch, so a change won't take effect until a restart.
  // Since a toast gets hidden behind the board on returning to it and goes unnoticed, use a reliable alert instead
  if (languageNeedsRestart()) {
    loadedLanguage = (current.language || "").trim().toLowerCase(); // Avoids showing the notice twice
    alert(T["settings.language.restart"]);
  }
  // Once saved, this screen's job is done. Leaving it open would mean the only way back
  // to the board is "click another tab", making settings feel like it's overstaying
  goIndex();
}

// Whether the saved language setting disagrees with the language currently running.
// document.documentElement.lang holds the running language as decided at launch
function languageNeedsRestart() {
  const active = (document.documentElement.lang || "").trim().toLowerCase();
  const chosen = (current.language || "").trim().toLowerCase();
  // If it's an explicit choice, a restart is only needed when it differs from what's running
  if (chosen) return chosen !== active;
  // When reverted to "Auto": if the original was an explicit choice, it may now change to the OS language
  return !!loadedLanguage;
}

// This screen is a page embedded in a window, so it can talk directly to the main app.
// When opened in an external browser, window.ipc simply doesn't exist (so nothing happens)
function goIndex() {
  try { window.ipc.postMessage(JSON.stringify({kind:"select", tab:0})); } catch (e) {}
}

// Closes settings. Returns to the operating board (INDEX), folding the settings tab away and removing it from the list on the left too.
// If there are unsaved changes, warns first that they'll be lost
function closeSettings() {
  // Nothing was loaded, so there is nothing to lose — don't ask.
  if (!loadFailure && snapshot() !== savedSnapshot && !confirm(T["settings.back.confirm"])) return;
  // In the window this rides the ipc bridge back to the board. On a phone (served
  // over the remote proxy) there is no bridge, so navigate to "/" — the shell,
  // which re-authenticates from its stored token. The unsaved-changes guard above
  // runs first either way.
  if (window.ipc) { try { window.ipc.postMessage(JSON.stringify({kind:"closesettings"})); } catch (e) { goIndex(); } }
  else { location.href = "/"; }
}

// Opens a help/report page in the real browser. The server whitelists `dest`
// and pre-fills the bug template with this build and the OS version.
function openExt(dest) {
  fetch("/api/open?dest=" + dest, {headers:{"X-Token":TOKEN}}).catch(()=>{});
}

// Lay the header out for this screen width before the first paint, so the page
// doesn't flash the desktop arrangement on the way in.
placeHeadLinks();
measureHeader();

// If the URL has addtab=<workspace-index>, start with one tab already added
// to that workspace after loading (this is where the tab bar's + comes from).
// ws=<workspace-index> only expands that group (the gear passes the workspace
// being viewed, so the settings open onto the one you came from).
// Careful: Number(null) is 0, so a missing parameter must be rejected as text
// first — otherwise every plain open would start editing workspace 0
load().then(() => {
  const q = new URLSearchParams(location.search);
  const idx = k => /^\d+$/.test(q.get(k) || "") ? Number(q.get(k)) : -1;
  // A shortcut may ask to bounce back to the board once saved (?ret=1).
  returnOnSave = q.get("ret") === "1";
  // ?section=<id> deep-links straight to one global card (the ⚙ shortcuts).
  const sec = q.get("section");
  if (sec && globalSections().some(s => s.id === sec)) {
    navGlobalOpen = true;
    sel = {ws:sel.ws, tab:null, global:true, section:sec};
    render();
    const s = document.querySelector(".navitem.sel");
    if (s) s.scrollIntoView({block:"center"});
    return;
  }
  const wi = idx("addtab");
  if (wss[wi]) {
    navGlobalOpen = false;
    // Asked for from a folder: that is where it goes. The form used to add it
    // wherever the default was, which is the first folder -- so a tab asked
    // for from the third one turned up in the first
    const same = c => (c || "").replace(/[\\/]+$/, "").toLowerCase();
    const from = (q.get("folder") || "").trim();
    const gi = from
      ? (wss[wi].folders || []).findIndex(g => same(g.cwd) === same(from))
      : -1;
    sel = {ws:wi, grp:null, tab:addTabTo(wss[wi], gi >= 0 ? gi : undefined), global:false};
    sel.grp = wss[wi].tabs[sel.tab].group || 0;
    render();
    const s = document.querySelector(".navitem.sel");
    if (s) s.scrollIntoView({block:"center"});
    return;
  }
  // "Edit settings" (?gen=1), or an open with no workspace to focus, lands on the
  // General group expanded. The sidebar gear (?ws=N, no gen) lands on the workspace
  // it came from with General collapsed — press its ▸ to open it.
  // ?folder=<path> lands on that folder's own page: the tab list's edit
  // entry knows the folder, not which line of the settings file it is on
  const want = (q.get("folder") || "").trim();
  const cur = idx("ws");
  if (want && wss[cur]) {
    const same = c => (c || "").replace(/[\\/]+$/, "").toLowerCase();
    const gi = (wss[cur].folders || []).findIndex(g => same(g.cwd) === same(want));
    if (gi >= 0) {
      navGlobalOpen = false;
      navShut.delete(cur); navOpen.add(cur);
      sel = {ws:cur, grp:gi, tab:null, global:false};
      render();
      const s = document.querySelector(".navitem.sel");
      if (s) s.scrollIntoView({block:"center"});
      return;
    }
  }
  if (q.get("gen") === "1" || !wss[cur]) {
    navGlobalOpen = true;
    sel = {ws:(wss[cur] ? cur : sel.ws), grp:null, tab:null, global:true, section:"basic"};
  } else {
    navGlobalOpen = false;
    navShut.delete(cur); navOpen.add(cur);
    sel = {ws:cur, grp:null, tab:null, global:false};
  }
  render();
  const s = document.querySelector(".navitem.sel");
  if (s) s.scrollIntoView({block:"center"});
});
</script></body></html>
"##;

/// The result view: a finished run's transcript.md rendered as a chat
/// (WhatsApp-style bubbles for a discussion / code review; the same block
/// renderer doubles for a browser rally's request→action→screen log). Verdicts
/// and moderator notes become centered system cards. Tall bubbles (long prose
/// or a pasted git diff) clamp with a "show all" toggle; the download button
/// always hands over the full, untruncated Markdown.
const RESULT_PAGE: &str = r##"<!doctype html>
<html lang="{{__lang__}}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{result.title}}</title>
<style>
 :root {
   {{THEME}}
   color-scheme: {{SCHEME}};
 }
 * { box-sizing:border-box; }
 body { margin:0; background:var(--bg); color:var(--text); font-size:14px; line-height:1.6;
   font-family:system-ui,"Segoe UI","Yu Gothic UI","Hiragino Sans",sans-serif; }
 code, pre { font-family:ui-monospace,Consolas,"Courier New",monospace; }

 header { position:sticky; top:0; z-index:5; display:flex; align-items:center; gap:12px;
   padding:12px 20px; background:color-mix(in srgb, var(--bg) 92%, transparent); backdrop-filter:blur(8px);
   border-bottom:1px solid var(--line); }
 header .ttl { display:flex; flex-direction:column; min-width:0; }
 header h1 { font-size:15px; font-weight:600; margin:0; letter-spacing:.02em; }
 header .sub { color:var(--muted); font-size:12px; white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
 header .spacer { flex:1; }
 button { font-family:inherit; font-size:13px; border-radius:8px; border:1px solid var(--line);
   background:var(--panel2); color:var(--text); padding:8px 14px; cursor:pointer; }
 button:hover { border-color:var(--accent); }
 button.primary { background:var(--accent); border-color:var(--accent); color:#04121c; font-weight:600; }
 button.ghost { background:none; }

 main { max-width:900px; margin:0 auto; padding:20px 18px 80px; }
 .caption { text-align:center; color:var(--muted); font-size:12px; margin:6px auto 16px; }

 .turn { display:flex; margin:14px 0; gap:10px; align-items:flex-end; }
 .turn.right { flex-direction:row-reverse; }
 .avatar { flex:none; width:34px; height:34px; border-radius:50%; display:flex; align-items:center;
   justify-content:center; font-weight:700; font-size:14px; color:#04121c; }
 .col { max-width:78%; min-width:0; display:flex; flex-direction:column; }
 .turn.right .col { align-items:flex-end; }
 .who { font-size:12px; color:var(--muted); margin:0 4px 3px; }
 .who .badge { opacity:.6; margin-left:6px; }
 .bubble { position:relative; background:var(--panel); border:1px solid var(--line);
   border-radius:14px; padding:10px 14px; overflow:hidden; }
 .turn.left .bubble { border-top-left-radius:4px; }
 .turn.right .bubble { border-top-right-radius:4px; }
 .bubble .body { overflow-x:auto; }
 .bubble .body p { margin:0 0 8px; }
 .bubble .body p:last-child { margin-bottom:0; }
 .bubble .body pre { background:#0c0f13; border:1px solid var(--line); border-radius:8px;
   padding:10px 12px; margin:8px 0; overflow-x:auto; font-size:12.5px; line-height:1.5; }
 .bubble .body pre .add { color:#7ee787; display:block; }
 .bubble .body pre .del { color:#ff9a9a; display:block; }
 .bubble .body pre .hunk { color:#79c0ff; display:block; }
 .bubble .body code.inline { background:#0c0f13; border:1px solid var(--line);
   border-radius:4px; padding:1px 5px; font-size:12.5px; }

 /* Tall bubbles clamp; the fade + button invite a click to see the rest. */
 .bubble.clamped .body { max-height:320px; overflow:hidden; }
 .bubble.clamped::after { content:""; position:absolute; left:0; right:0; bottom:34px; height:60px;
   pointer-events:none; background:linear-gradient(transparent, var(--panel)); }
 .bubble .more { margin-top:8px; font-size:12px; padding:4px 10px; }

 /* The verdict / judge ruling: a full-width report, not a chat bubble. */
 .report { background:var(--panel); border:1px solid var(--line); border-left:3px solid var(--accent);
   border-radius:12px; padding:14px 20px; margin:24px 0 12px; overflow:hidden; }
 .report > h3 { margin:0 0 10px; font-size:12px; letter-spacing:.06em; text-transform:uppercase; color:var(--accent); }
 .report .body { overflow-x:auto; }
 /* Speakers' own Markdown, rendered inside a bubble or report. */
 .body .mh { font-weight:700; margin:12px 0 5px; }
 .body .mh1, .body .mh2 { font-size:15px; color:var(--text); }
 .body .mh3, .body .mh4, .body .mh5, .body .mh6 { font-size:13px; color:var(--muted);
   letter-spacing:.03em; }
 .body table { border-collapse:collapse; margin:8px 0; font-size:12.5px; max-width:100%; }
 .body th, .body td { border:1px solid var(--line); padding:4px 11px; text-align:left; white-space:nowrap; }
 .body th { background:var(--panel2); font-weight:600; }
 .note { text-align:center; color:var(--muted); font-size:12px; margin:12px auto; font-style:italic; }
 .empty { text-align:center; color:var(--muted); margin-top:60px; font-size:14px; }
{{TOAST_CSS}}
</style></head>
<body>
<header>
  <div class="ttl">
    <h1>{{result.title}}</h1>
    <span class="sub" id="sub"></span>
  </div>
  <span class="spacer"></span>
  <button class="ghost" id="toggleall" style="display:none"></button>
  <button id="dlr" style="display:none"></button>
  <button class="primary" id="dl"></button>
</header>
<main id="chat"></main>
{{TOAST_HTML}}
<script>
const TOKEN = "__TOKEN__";
const T = __DICT__;
const RUN = new URLSearchParams(location.search).get("run") || "";
const MAXH = 320;
let allOpen = false;

const el = (tag, attrs = {}, ...kids) => {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") n.className = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else if (v !== null && v !== undefined) n.setAttribute(k, v);
  }
  for (const c of kids) if (c !== null && c !== undefined) n.append(c);
  return n;
};
const fill = (s, args) => Object.entries(args)
  .reduce((acc, [k, v]) => acc.replaceAll("{" + k + "}", v), s || "");
const esc = s => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
{{TOAST_JS}}

// A stable color + left/right lane per speaker, assigned on first appearance.
const PALETTE = ["#00aaff","#ffb020","#7ee787","#ff6b9d","#b388ff","#5ad1cd","#ff8f5a","#9aa0ff"];
const lanes = new Map();
function laneOf(name) {
  if (!lanes.has(name)) {
    const i = lanes.size;
    lanes.set(name, { color: PALETTE[i % PALETTE.length], side: i % 2 === 0 ? "left" : "right" });
  }
  return lanes.get(name);
}

// ── Markdown-lite (only what a transcript actually carries) ──
function looksDiff(lines) {
  return lines.some(l => /^diff --git /.test(l) || /^@@ /.test(l));
}
function codeBlock(lines, forceDiff) {
  const diff = forceDiff || looksDiff(lines);
  const rows = lines.map(l => {
    const e = esc(l);
    if (!diff) return e;
    if (/^\+/.test(l)) return '<span class="add">' + e + '</span>';
    if (/^-/.test(l))  return '<span class="del">' + e + '</span>';
    if (/^@@/.test(l)) return '<span class="hunk">' + e + '</span>';
    return e;
  }).join("\n");
  return "<pre><code>" + rows + "</code></pre>";
}
function inline(s) {
  return esc(s)
    .replace(/`([^`]+)`/g, '<code class="inline">$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
}
function isTableSep(l) { return /^\s*\|?[\s:|-]*-[\s:|-]*\|?\s*$/.test(l) && l.indexOf("-") >= 0 && l.indexOf("|") >= 0; }
function tableBlock(rows) {
  const cells = r => r.trim().replace(/^\|/, "").replace(/\|$/, "").split("|").map(c => c.trim());
  const head = cells(rows[0]);
  let h = "<table><thead><tr>" + head.map(c => "<th>" + inline(c) + "</th>").join("") + "</tr></thead><tbody>";
  for (const r of rows.slice(1)) h += "<tr>" + cells(r).map(c => "<td>" + inline(c) + "</td>").join("") + "</tr>";
  return h + "</tbody></table>";
}
function renderBody(lines) {
  const out = [];
  let i = 0;
  while (i < lines.length) {
    const ln = lines[i];
    // A Markdown heading INSIDE a statement is the speaker's own subheading —
    // real transcript boundaries were split off before we got here, so render
    // it as a subheading rather than treating it as a new bubble.
    const h = /^(#{1,6})\s+(.*)$/.exec(ln);
    if (h) { out.push('<div class="mh mh' + h[1].length + '">' + inline(h[2]) + "</div>"); i++; continue; }
    const fence = /^```(\w*)\s*$/.exec(ln.trim());
    if (fence) {
      const lang = fence[1]; const code = []; i++;
      while (i < lines.length && !/^```\s*$/.test(lines[i].trim())) { code.push(lines[i]); i++; }
      i++;
      out.push(codeBlock(code, lang === "diff")); continue;
    }
    // 4-space indented block (browser-rally actions are recorded this way)
    if (/^ {4}\S/.test(ln)) {
      const code = [];
      while (i < lines.length && (/^ {4}/.test(lines[i]) || (lines[i].trim() === "" && /^ {4}/.test(lines[i + 1] || "")))) {
        code.push(lines[i].replace(/^ {4}/, "")); i++;
      }
      out.push(codeBlock(code, false)); continue;
    }
    // A raw pasted git diff (not fenced) — keep it as a diff block
    if (/^diff --git /.test(ln) || /^@@ /.test(ln)) {
      const code = [];
      while (i < lines.length && lines[i].trim() !== "") { code.push(lines[i]); i++; }
      out.push(codeBlock(code, true)); continue;
    }
    // A Markdown table: a "| … |" row followed by a "|---|" separator.
    if (/^\s*\|.*\|\s*$/.test(ln) && isTableSep(lines[i + 1] || "")) {
      const rows = [lines[i]]; i += 2;
      while (i < lines.length && /^\s*\|.*\|\s*$/.test(lines[i])) { rows.push(lines[i]); i++; }
      out.push(tableBlock(rows)); continue;
    }
    if (ln.trim() === "") { i++; continue; }
    const para = [];
    while (i < lines.length && lines[i].trim() !== ""
        && !/^#{1,6}\s/.test(lines[i]) && !/^```/.test(lines[i].trim()) && !/^ {4}\S/.test(lines[i])
        && !/^diff --git /.test(lines[i]) && !/^@@ /.test(lines[i])
        && !(/^\s*\|.*\|\s*$/.test(lines[i]) && isTableSep(lines[i + 1] || ""))) {
      para.push(lines[i]); i++;
    }
    if (para.length) out.push("<p>" + para.map(inline).join("<br>") + "</p>");
  }
  return out.join("");
}

// The one heading a speaker can't forge: the judge's verdict carries the
// judge's name — "判定（審判 X）". No-judge endings use the aggregate / round-
// limit label. Everything else that looks like a heading is a speaker's own
// Markdown, not a transcript boundary. Returns the boundary line index, or -1.
function verdictBoundary(lines) {
  const vj = (T["transcript.discuss.verdict_judge"] || "").split("{me}")[0].replace(/^#+\s*/, "").trim();
  const va = (T["agent.discuss.verdict_agg"] || "").trim();
  const vl = (T["agent.verdict.label"] || "").trim();
  const lastMatch = pred => { let idx = -1; for (let i = 0; i < lines.length; i++) { const m = /^##\s+(.*)$/.exec(lines[i]); if (m && pred(m[1].trim())) idx = i; } return idx; };
  let i = vj ? lastMatch(t => t.indexOf(vj) === 0) : -1;
  if (i < 0 && va) i = lastMatch(t => t.indexOf(va) === 0);
  if (i < 0 && vl) i = lastMatch(t => t === vl || t.indexOf(vl + ":") === 0 || t.indexOf(vl + " ") === 0);
  return i;
}

// Split into turns on speaker headings only. Matched tightly so a speaker's
// own heading Markdown inside their statement is never taken for a turn: a
// discussion turn is a level-3 heading ending in a round number — Name（3） —
// while a rally logs looser Action 1 / Human request, so for a rally any
// level-3 heading begins a turn.
function parseTurns(lines, kind) {
  const strict = /^###\s+(.*?)\s*[(（]\s*(\d+)\s*[)）]\s*$/;
  const loose = /^###\s+(.*)$/;
  const turns = []; const pre = []; let cur = null;
  for (const ln of lines) {
    let name = null, round = null;
    const s = strict.exec(ln);
    if (s) { name = s[1].trim(); round = s[2]; }
    else if (kind === "rally") { const l = loose.exec(ln); if (l) { name = l[1].trim(); } }
    if (name !== null) { if (cur) turns.push(cur); cur = { name, round, body: [] }; }
    else if (cur) cur.body.push(ln);
    else pre.push(ln);
  }
  if (cur) turns.push(cur);
  return { pre, turns };
}

function turnEl(name, round, bodyHtml) {
  const lane = laneOf(name);
  const av = el("div", { class: "avatar", style: "background:" + lane.color },
    (name.trim()[0] || "?").toUpperCase());
  const who = el("div", { class: "who" }, name);
  if (round) who.append(el("span", { class: "badge" }, "#" + round));
  const body = el("div", { class: "body" });
  body.innerHTML = bodyHtml;
  const bubble = el("div", { class: "bubble" }, body);
  const col = el("div", { class: "col" }, who, bubble);
  return el("div", { class: "turn " + lane.side }, av, col);
}
// The verdict (and the judge's structured ruling) reads as a full-width report,
// not a chat bubble — it argues with headings, scores, and tables.
function reportEl(title, bodyHtml) {
  const card = el("div", { class: "report" });
  if (title) card.append(el("h3", {}, title));
  const body = el("div", { class: "body" });
  body.innerHTML = bodyHtml;
  card.append(body);
  return card;
}
function noteEl(text) {
  return el("div", { class: "note" }, text);
}
// Trailing "(...)" lines on a turn are moderator asides written between turns
// (only moderated discussions emit them). Lift them out so they read as
// centered notes rather than tacked onto the previous speaker's bubble.
function peelNotes(lines) {
  const body = lines.slice(); const notes = [];
  while (body.length) {
    const last = body[body.length - 1].trim();
    if (last === "") { body.pop(); continue; }
    if (/^\(.*\)$/.test(last)) { notes.unshift(last); body.pop(); continue; }
    break;
  }
  return { body, notes };
}

function render(data) {
  const chat = document.getElementById("chat");
  chat.textContent = "";
  const md = (data && data.md) || "";
  const kind = (data && data.kind) || "discuss";
  document.getElementById("sub").textContent =
    kind === "rally" ? T["result.kind.rally"] : T["result.kind.discuss"];
  if (!md.trim()) { chat.append(el("div", { class: "empty" }, T["result.empty"])); return; }
  const lines = md.split(/\r?\n/);
  const vb = verdictBoundary(lines);
  const mainLines = vb >= 0 ? lines.slice(0, vb) : lines;
  const verdict = vb >= 0
    ? { title: (/^##\s+(.*)$/.exec(lines[vb]) || [])[1] || T["result.verdict"], body: lines.slice(vb + 1) }
    : null;
  const { pre, turns } = parseTurns(mainLines, kind);
  // A judge speaks its ruling as a turn, then the orchestrator repeats it as
  // the verdict. Drop the duplicate turn so the ruling shows once, as a report.
  const norm = s => s.replace(/\s+/g, " ").trim();
  if (verdict && turns.length) {
    const lb = norm(turns[turns.length - 1].body.join("\n"));
    const vbody = norm(verdict.body.join("\n"));
    if (lb && vbody && (lb === vbody || vbody.indexOf(lb) === 0 || lb.indexOf(vbody) === 0)) turns.pop();
  }
  const preTxt = pre.filter(l => l.trim() !== "" && !/^#/.test(l));
  if (preTxt.length) chat.append(el("div", { class: "caption" }, preTxt.join(" · ")));
  for (const t of turns) {
    const { body, notes } = peelNotes(t.body);
    chat.append(turnEl(t.name, t.round, renderBody(body)));
    for (const n of notes) chat.append(noteEl(n));
  }
  if (verdict) chat.append(reportEl(verdict.title, renderBody(verdict.body)));
  requestAnimationFrame(clampTall);
}

// Clamp bubbles taller than MAXH and give each a show-all toggle.
function clampTall() {
  let any = false;
  document.querySelectorAll(".bubble").forEach(b => {
    const body = b.querySelector(".body");
    if (body.scrollHeight > MAXH + 40 && !b.classList.contains("clampable")) {
      b.classList.add("clampable", "clamped");
      b.append(el("button", { class: "more ghost", onclick: () => {
        const open = b.classList.toggle("clamped") === false;
        b.querySelector(".more").textContent = open ? T["result.collapse"] : T["result.expand"];
      } }, T["result.expand"]));
      any = true;
    }
  });
  const tg = document.getElementById("toggleall");
  tg.style.display = any ? "" : "none";
  tg.textContent = T["result.expand_all"];
}
function toggleAll() {
  allOpen = !allOpen;
  document.querySelectorAll(".bubble.clampable").forEach(b => {
    b.classList.toggle("clamped", !allOpen);
    const m = b.querySelector(".more");
    if (m) m.textContent = allOpen ? T["result.collapse"] : T["result.expand"];
  });
  document.getElementById("toggleall").textContent =
    allOpen ? T["result.collapse_all"] : T["result.expand_all"];
}

async function saveFrom(url, filename) {
  try {
    const r = await fetch(url, { headers: { "X-Token": TOKEN } });
    if (!r.ok) { toast(T["result.empty"], true); return; }
    const blob = await r.blob();
    const u = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = u; a.download = filename;
    document.body.append(a); a.click(); a.remove();
    setTimeout(() => URL.revokeObjectURL(u), 1000);
    toast(T["result.downloaded"]);
  } catch (e) {
    toast(fill(T["result.download_failed"], { e: e.message || e }), true);
  }
}
function download() {
  saveFrom("/api/rally/download" + (RUN ? "?run=" + encodeURIComponent(RUN) : ""),
           "shikisha-" + (RUN || "result") + ".md");
}
function downloadReplay() {
  saveFrom("/api/rally/replay" + (RUN ? "?run=" + encodeURIComponent(RUN) : ""),
           "shikisha-macro-" + (RUN || "latest") + ".lua");
}

async function load() {
  document.getElementById("dl").textContent = T["result.download"];
  document.getElementById("dl").addEventListener("click", download);
  document.getElementById("dlr").textContent = T["result.download_replay"];
  document.getElementById("dlr").addEventListener("click", downloadReplay);
  document.getElementById("toggleall").addEventListener("click", toggleAll);
  document.getElementById("chat").append(el("div", { class: "empty" }, T["result.loading"]));
  try {
    const r = await fetch("/api/rally/transcript?run=" + encodeURIComponent(RUN), { headers: { "X-Token": TOKEN } });
    const data = await r.json();
    if (data.replay) document.getElementById("dlr").style.display = "";
    render(data);
  } catch (e) {
    document.getElementById("chat").textContent = "";
    document.getElementById("chat").append(el("div", { class: "empty" }, T["result.empty"]));
  }
}
load();
</script></body></html>
"##;

/// The manual display page (a simple renderer that only handles the Markdown subset needed here)
const HELP_PAGE: &str = r##"<!doctype html>
<html lang="{{__lang__}}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>{{help.page.title}}</title>
<style>
 :root { {{THEME}} color-scheme: {{SCHEME}}; }
 body { background:var(--bg); color:var(--text); font-family:"Consolas","Meiryo",monospace;
        margin:0; padding:24px 32px; line-height:1.7; }
 h1,h2,h3 { color:var(--c6); border-bottom:1px solid var(--line); padding-bottom:6px; }
 h1 { font-size:20px; } h2 { font-size:17px; margin-top:32px; } h3 { font-size:15px; }
 code { background:var(--panel); color:var(--warn); padding:1px 5px; border-radius:3px; }
 pre { background:var(--panel); border:1px solid var(--line); padding:12px; overflow:auto; }
 pre code { color:var(--c6); background:none; padding:0; }
 table { border-collapse:collapse; margin:12px 0; }
 th,td { border:1px solid var(--line); padding:5px 10px; text-align:left; }
 th { color:var(--accent); }
 hr { border:0; border-top:1px solid var(--line); margin:28px 0; }
 a { color:var(--accent); }
 /* The row that was asked for. Marked, not animated: it has to still be
    obvious a minute later when the reader looks up from the text */
 .hit td, .hit { background:var(--panel2); box-shadow:inset 3px 0 0 var(--accent); }
</style></head><body><div id="doc"></div>
<script>
const MD = __MD__;
const esc = s => s.replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;");
const inline = s => esc(s)
   .replace(/`([^`]+)`/g, "<code>$1</code>")
   .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>");
function render(md) {
  const out = []; const lines = md.split(/\r?\n/);
  let i = 0;
  while (i < lines.length) {
    const l = lines[i];
    if (l.startsWith("```")) {                      // Code block
      const buf = []; i++;
      while (i < lines.length && !lines[i].startsWith("```")) buf.push(lines[i++]);
      i++; out.push("<pre><code>" + esc(buf.join("\n")) + "</code></pre>"); continue;
    }
    if (/^\|/.test(l)) {                            // Table
      const rows = [];
      while (i < lines.length && /^\|/.test(lines[i])) rows.push(lines[i++]);
      const cells = r => r.split("|").slice(1,-1).map(c => c.trim());
      let html = "<table>";
      rows.forEach((r, n) => {
        if (/^\|[\s:|-]+\|$/.test(r)) return;        // Separator row
        const tag = n === 0 ? "th" : "td";
        html += "<tr>" + cells(r).map(c => `<${tag}>${inline(c)}</${tag}>`).join("") + "</tr>";
      });
      out.push(html + "</table>"); continue;
    }
    const h = l.match(/^(#{1,3})\s+(.*)$/);
    if (h) { const n = h[1].length; out.push(`<h${n}>${inline(h[2])}</h${n}>`); i++; continue; }
    if (/^---+$/.test(l)) { out.push("<hr>"); i++; continue; }
    if (/^[-*]\s+/.test(l)) {                        // Bulleted list
      const buf = [];
      while (i < lines.length && /^[-*]\s+/.test(lines[i]))
        buf.push("<li>" + inline(lines[i++].replace(/^[-*]\s+/, "")) + "</li>");
      out.push("<ul>" + buf.join("") + "</ul>"); continue;
    }
    if (l.trim() === "") { i++; continue; }
    const buf = [];
    while (i < lines.length && lines[i].trim() !== "" && !/^(#{1,3}\s|```|\||[-*]\s|---)/.test(lines[i]))
      buf.push(lines[i++]);
    out.push("<p>" + inline(buf.join(" ")) + "</p>");
  }
  return out.join("\n");
}
document.getElementById("doc").innerHTML = render(MD);
// Arrived from a command's name on the settings screen. Land on the row that
// command is written on, rather than at the top of a long page: the reference
// rows are table rows, and the list of every command is the last of them, so
// the last matching row is the definition rather than a mention in passing
const asked = decodeURIComponent((location.hash || "").replace(/^#cmd-/, ""));
if (/^[a-z0-9_]+$/.test(asked)) {
  const needle = "shikisha." + asked + "(";
  const rows = [...document.querySelectorAll("#doc tr")].filter(r => r.textContent.includes(needle));
  const hit = rows.length ? rows[rows.length - 1]
    : [...document.querySelectorAll("#doc p, #doc li")].find(e => e.textContent.includes(needle));
  if (hit) { hit.classList.add("hit"); hit.scrollIntoView({block:"center"}); }
}
</script></body></html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// Conversational text must never be typed into a terminal — only what
    /// the markers (or a lone fence) carry gets through
    #[test]
    fn suggested_commands_come_only_from_markers() {
        assert_eq!(
            extract_cmd("環境はUbuntuと推定します。\n<<<CMD\nfree -h\n>>>\n以上です").unwrap(),
            "free -h"
        );
        assert_eq!(extract_cmd("```bash\nfree -h\n```").unwrap(), "free -h");
        assert!(extract_cmd("メモリを見るには free -h を使います").is_err(), "地の文は拒否");
        assert!(extract_cmd("<<<CMD\n\n>>>").is_err(), "空の提案は拒否");
    }

    /// A file that isn't there yet is a fresh install; a file that's there but
    /// broken is an emergency. Telling those two apart is the whole job here.
    ///
    /// The broken case used to reach the page as 200 with invalid JSON in the
    /// body: the parse threw, the form stayed empty, and pressing Save would
    /// then write that emptiness over the real configuration. So the refusal is
    /// explicit, and it carries enough to point at the mistake.
    #[test]
    fn a_broken_file_is_refused_with_the_spot_it_broke_at() {
        let dir = std::env::temp_dir().join(format!("shikisha-userjson-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let _ = std::fs::remove_file(&path);

        // Missing: hand over the empty shape, no fuss.
        match read_user_json(&path, "{}") {
            UserJson::Text(t) => assert_eq!(t, "{}", "未作成のファイルは空の形で渡す"),
            UserJson::Refused(..) => panic!("未作成なだけで拒否してはいけない"),
        }

        // Fine: hand it over verbatim, so unknown keys and "//" notes survive.
        let good = "{\n  \"//note\": \"kept\",\n  \"max_chain\": 10\n}";
        std::fs::write(&path, good).unwrap();
        match read_user_json(&path, "{}") {
            UserJson::Text(t) => assert_eq!(t, good, "読めたファイルは原文のまま渡す"),
            UserJson::Refused(..) => panic!("正しい JSON を拒否した"),
        }

        // Broken: refuse, and say where. Naming the line is the point — a bare
        // "failed to load" would leave someone hunting through their own file.
        let bad = "{\n  \"name\": \"実装\",\n  \"cwd\": \"D:\\very\"\n}";
        std::fs::write(&path, bad).unwrap();
        let UserJson::Refused(status, body) = read_user_json(&path, "{}") else {
            panic!("壊れた JSON を通してしまった");
        };
        assert_eq!(status, 409);
        assert_eq!(body["ok"], serde_json::json!(false));
        assert_eq!(body["line"], serde_json::json!(3), "壊れた行を指していない");
        assert!(body["column"].as_u64().unwrap() > 0, "壊れた桁を指していない");
        assert_eq!(
            body["text"],
            serde_json::json!(bad),
            "原文が付いていないと、画面が該当行を見せられない"
        );
        assert!(
            body["path"].as_str().unwrap().ends_with("config.json"),
            "どのファイルの話か分からない"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The connection link may reach the clipboard, never the screen. Printing
    /// it without the token showed an address that opens nothing; printing it
    /// with the token puts the key to the machine where a camera can see it.
    #[test]
    fn the_phone_card_hands_the_link_over_rather_than_printing_it() {
        let from = PAGE.find("function remoteCard()").expect("remoteCard が無い");
        let len = PAGE[from..].find("function aiSelect()").expect("カードの終わりが無い");
        let card = &PAGE[from..from + len];
        assert!(card.contains("/api/remote/url"), "コピー用の取り出し口が無い");
        assert!(card.contains("netBadge"), "どの網に繋がるかのバッジが無い");
        assert!(card.contains("copyText("), "共有のクリップボード経路を通っていない");
        assert_eq!(
            card.matches("j.origin").count(),
            1,
            "origin は「見せるものがあるか」の判定だけ。画面に描いてはいけない"
        );
    }

    /// Opening a card must never count as an edit.
    ///
    /// The quick-actions card used to fill in `lua: false` while drawing, so
    /// merely looking at it lit up "unsaved" and then wrote that default into
    /// config.json on the next save. payload() already states this rule for
    /// itself; the card that feeds it has to live by it too.
    #[test]
    fn drawing_the_actions_card_writes_nothing() {
        let from = PAGE.find("function actionsCard()").expect("actionsCard が無い");
        let body = &PAGE[from..from + 2400];
        for write in ["a.label =", "a.body =", "a.lua ="] {
            for (i, _) in body.match_indices(write) {
                let start = body[..i].rfind('\n').map(|n| n + 1).unwrap_or(0);
                let end = body[i..].find('\n').map(|n| i + n).unwrap_or(body.len());
                let line = body[start..end].trim();
                // An assignment inside a handler is a human editing, and is fine.
                // One sitting in the drawing path is the bug.
                assert!(
                    line.contains("addEventListener"),
                    "描画中に書き込んでいる: {line}"
                );
            }
        }
    }

    #[test]
    fn what_a_model_wraps_its_answer_in_is_not_part_of_the_answer() {
        assert_eq!(strip_fence("one\ntwo"), "one\ntwo", "素のままなら素のまま");
        assert_eq!(
            strip_fence("Here it is:\n```rust\nfn main() {}\n```\nhope that helps"),
            "fn main() {}",
            "囲いの中だけが答え"
        );
        // An opening fence with nothing closing it still gives up its contents
        assert_eq!(strip_fence("```\nline\n"), "line");
    }

    #[test]
    fn manual_is_embedded_and_usable() {
        // The spec handed to the AI must be obtainable no matter where it's launched from (regardless of language)
        for (code, text) in EMBEDDED_MANUALS {
            assert!(text.contains("shikisha.send_to_tab"), "{code} の仕様書が空");
        }
        let m = load_manual(std::path::Path::new("/nonexistent/config.json"));
        assert!(m.contains("shikisha."), "埋め込みにフォールバックする");
    }

    /// The spec must explain every event the screen offers as a choice.
    ///
    /// This spec is passed to the AI as-is. If asked for an event it doesn't cover,
    /// the AI will honestly and correctly reply "that's not in the spec, so I won't do anything."
    /// If a feature is added but the spec isn't updated, that feature stays invisible to the AI
    #[test]
    fn the_manual_covers_every_event_the_screen_offers() {
        for (code, text) in EMBEDDED_MANUALS {
            for event in EVENT_FILES {
                // _shared isn't an event; it's a shared location
                if event == "_shared" {
                    continue;
                }
                assert!(
                    text.contains(event),
                    "{code} の仕様書に {event} の説明が無い (AIはこのイベントを知らないまま書く)"
                );
            }
        }
    }


    /// Top-level const/let bindings must not be duplicated.
    /// A duplicate causes a SyntaxError, so the whole script fails to run and only static HTML is left.
    /// The result is a hard-to-diagnose break: the screen shows up but nothing works
    fn top_level_bindings(page: &str) -> Vec<String> {
        page.lines()
            .filter_map(|l| l.strip_prefix("const ").or_else(|| l.strip_prefix("let ")))
            .filter_map(|rest| rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next())
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect()
    }
    
    fn assert_no_duplicate_bindings(name: &str, page: &str) {
        let names = top_level_bindings(page);
        for (i, n) in names.iter().enumerate() {
            assert!(
                !names[..i].contains(n),
                "{name}: `{n}` がトップレベルで二重宣言されている (JS全体が動かなくなる)"
            );
        }
    }

    /// The screen must not still contain a raw `{{key}}` or `__DICT__`.
    /// A forgotten substitution would only be caught at runtime, so it's stopped here instead
    #[test]
    fn pages_are_fully_rendered() {
        for (name, page) in [("PAGE", PAGE), ("HELP_PAGE", HELP_PAGE), ("RESULT_PAGE", RESULT_PAGE)] {
            // Every page is coloured before it is worded, and each one says so
            // by asking for the block. A page that stopped asking would come up
            // with no colours defined and nothing would say why
            assert!(page.contains("{{THEME}}"), "{name} が配色を受け取っていない");
            let html = crate::i18n::render(&themed(page.to_string()))
                .replace("__TOKEN__", "t")
                .replace("__REMOTE__", "false")
                .replace("__DICT__", "{}")
                .replace("__GRANTS__", "[]")
                .replace("__GITLUA__", "\"\"")
                .replace("__PROTECT__", "[]")
                .replace("__MD__", "\"\"");
            // Checked on the finished page, not the template: the shared toast
            // is poured in on the way, and a page that kept a copy of one of
            // its names would only break once it was actually served
            assert_no_duplicate_bindings(name, &html);
            assert!(!html.contains("{{"), "{name} に未置換の {{{{key}}}} が残っている");
            assert!(!html.contains("__"), "{name} に未置換のプレースホルダが残っている");
            assert!(html.contains("<html lang=\"en\">"), "{name} の lang 属性");
        }
    }

    /// A phone reaching the settings over the proxy must never make a window
    /// appear on the PC. The page is served without the buttons, and the
    /// endpoints behind them answer with a refusal instead of a dialog that
    /// would hold the request open until someone walked over to the PC.
    #[test]
    fn a_phone_gets_no_native_dialogs() {
        let dir = std::env::temp_dir().join(format!("shikitest_{}", crate::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, "{}").unwrap();
        let ui = WebUi::start_with(
            cfg,
            Arc::new(std::sync::Mutex::new(RemoteInfo::default())),
            Arc::new(std::sync::Mutex::new(None)),
        )
        .unwrap();
        let (base, token) = ui.url.split_once("/?token=").unwrap();
        let (base, token) = (base.to_string(), token.to_string());
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(std::time::Duration::from_secs(10)))
            .build()
            .new_agent();

        // The page tells itself apart: same HTML, one flag
        let own = agent
            .get(&format!("{base}/"))
            .header("X-Token", &token)
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        assert!(own.contains("const REMOTE = false;"), "the PC's own window is not remote");
        let phone = agent
            .get(&format!("{base}/"))
            .header("X-Token", &token)
            .header(REMOTE_CLIENT_HEADER, "1")
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        assert!(phone.contains("const REMOTE = true;"), "a proxied page must know it is remote");

        // Every dialog endpoint answers, rather than opening a window and waiting.
        // A timeout here (not a failed assert) is the bug this guards against
        for path in NATIVE_DIALOG_PATHS {
            let mut r = agent
                .post(&format!("{base}{path}"))
                .header("X-Token", &token)
                .header(REMOTE_CLIENT_HEADER, "1")
                .header("Content-Type", "application/json")
                .send(r#"{"kind":"dir"}"#)
                .unwrap();
            let v: serde_json::Value =
                serde_json::from_str(&r.body_mut().read_to_string().unwrap()).unwrap();
            assert_eq!(v["ok"], serde_json::json!(false), "{path} should refuse a phone");
            assert_eq!(
                v["error"].as_str().unwrap_or(""),
                crate::i18n::t("settings.pick.no_remote"),
                "{path} should say why"
            );
        }
        ui.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_compare_rejects_mismatch() {
        assert!(crate::crypto::token_eq("abc123", "abc123"));
        assert!(!crate::crypto::token_eq("abc123", "abc124"));
        assert!(!crate::crypto::token_eq("abc", "abc123"));
    }

    #[test]
    fn token_is_parsed_from_query() {
        assert_eq!(query_token("/?token=deadbeef"), "deadbeef");
        assert_eq!(query_token("/api/config?x=1&token=zz"), "zz");
        assert_eq!(query_token("/"), "");
    }

    #[test]
    fn tab_layout_is_described_for_the_ai() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"self":1,"tabs":[{"index":1,"name":"実装"},{"index":2,"name":"検査"}]}"#,
        )
        .unwrap();
        let s = describe_tabs(&v);
        // Pass the index-to-name mapping to the AI so it can be addressed by name
        assert!(s.contains("1. 実装"), "{s}");
        assert!(s.contains("2. 検査"), "{s}");
        assert!(s.contains("this script runs in"), "{s}");
        // Adds nothing when there's no tab info
        assert_eq!(describe_tabs(&serde_json::json!({})), "");
    }

    #[test]
    fn extracts_lua_from_ai_output() {
        // With marker (the expected shape)
        let s = "了解しました\n<<<LUA\nshikisha.log(\"hi\")\n>>>\n以上です";
        assert_eq!(extract_lua(s).unwrap(), "shikisha.log(\"hi\")");
        // Accepted with just a code fence too, as long as it looks like code
        let s2 = "```lua\nshikisha.send_to_tab(1, tab.output)\n```";
        assert_eq!(
            extract_lua(s2).unwrap(),
            "shikisha.send_to_tab(1, tab.output)"
        );
        // Errors out (and isn't saved) for conversational text alone
        assert!(extract_lua("どのような自動化を作りますか？").is_err());
    }

    #[test]
    fn picked_paths_stay_portable_when_inside_the_config_folder() {
        let cfg = std::path::Path::new("D:/app/config.json");
        // Paths under the config folder become relative (so the whole folder stays portable)
        assert_eq!(
            display_path(std::path::Path::new("D:/app/scripts/reviewer"), cfg),
            "scripts/reviewer"
        );
        // Paths outside it stay absolute, just with normalized separators
        assert_eq!(
            display_path(std::path::Path::new("C:\\Users\\me\\.ssh\\id_ed25519"), cfg),
            "C:/Users/me/.ssh/id_ed25519"
        );
    }

    #[test]
    fn workspace_path_rejects_traversal() {
        let cfg = std::path::Path::new("C:/app/config.json");
        // Happy path
        assert!(safe_workspace_path("/api/workspace?file=workspaces/x.json", cfg).is_some());
        // Path traversal, absolute paths, and non-JSON are rejected
        assert!(safe_workspace_path("/api/workspace?file=../secrets.json", cfg).is_none());
        assert!(safe_workspace_path("/api/workspace?file=workspaces/../../x.json", cfg).is_none());
        assert!(safe_workspace_path("/api/workspace?file=C:/windows/x.json", cfg).is_none());
        assert!(safe_workspace_path("/api/workspace?file=workspaces/x.lua", cfg).is_none());
        // URL-encoded .. is rejected too
        assert!(safe_workspace_path("/api/workspace?file=%2E%2E%2Fsecrets.json", cfg).is_none());
    }

    /// A dialog is dismissed by a press on the backdrop, never by a click on it.
    ///
    /// A click is attributed to the nearest ancestor of where the button went
    /// down and where it came up, and the backdrop covers the whole screen. So
    /// selecting text in a field and releasing past the dialog's edge — a
    /// hurried drag to the end of a line — arrived as a click on the backdrop,
    /// and the form vanished taking everything typed into it. Where the press
    /// landed is the only thing that says what was meant.
    #[test]
    fn a_dialog_closes_on_the_press_that_started_outside_it() {
        let page = PAGE;
        assert!(
            page.contains(r#"back.addEventListener("mousedown", e => { if (e.target === back) back.remove(); });"#),
            "モーダルが押下ではなくクリックで閉じている"
        );
        assert!(
            !page.contains(r#"back.addEventListener("click""#),
            "背景のクリックで閉じる書き方が戻っている"
        );
    }

    #[test]
    fn tokens_are_unique_and_long() {
        let a = random_token().unwrap();
        let b = random_token().unwrap();
        assert_eq!(a.len(), 48);
        assert_ne!(a, b);
    }

    /// Actually starts the server and verifies the auth and save behavior
    #[test]
    fn server_requires_token_and_saves_valid_json() {
        let dir = std::env::temp_dir().join("shikisha-webui-test");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, r#"{"max_chain":10}"#).unwrap();

        let ui = WebUi::start_with(
            cfg.clone(),
            Arc::new(std::sync::Mutex::new(RemoteInfo::default())),
            Arc::new(std::sync::Mutex::new(None)),
        )
        .unwrap();
        let token = ui.url.split("token=").nth(1).unwrap().to_string();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let agent = ureq::Agent::new_with_defaults();

        // No token → 403
        let status = agent
            .get(&format!("{base}/api/config"))
            .call()
            .map(|r| r.status().as_u16())
            .unwrap_or_else(|e| match e {
                ureq::Error::StatusCode(c) => c,
                other => panic!("unexpected: {other}"),
            });
        assert_eq!(status, 403, "トークン無しは拒否される");

        // Correct token → the current config can be read
        let body = agent
            .get(&format!("{base}/api/config?token={token}"))
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        assert!(body.contains("max_chain"));

        // Broken JSON is not saved
        let bad = agent
            .post(&format!("{base}/api/config?token={token}"))
            .send("{ broken")
            .map(|r| r.status().as_u16())
            .unwrap_or_else(|e| match e {
                ureq::Error::StatusCode(c) => c,
                other => panic!("unexpected: {other}"),
            });
        assert_eq!(bad, 400);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            r#"{"max_chain":10}"#,
            "検証に失敗したら元の設定は保たれる"
        );

        // Valid JSON is saved
        agent
            .post(&format!("{base}/api/config?token={token}"))
            .send(r#"{"max_chain":5}"#)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            r#"{"max_chain":5}"#
        );

        ui.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

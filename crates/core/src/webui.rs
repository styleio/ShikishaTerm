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
    /// End what a device is holding, by its row in the book.
    ///
    /// Taking a key away is this page's own business -- the book is a file and
    /// this page can write it. What it cannot do is drop the sockets already
    /// carrying a screen to that device, because those belong to the running
    /// remote. Without this, a revoked phone keeps watching until it next asks
    /// for something, which on a pushed screen may be never.
    ///
    /// Absent in the settings-only mode, where nothing is live to end.
    pub cut: Option<Cut>,
}

/// How a connection is ended: named, and told to go.
///
/// Given by whoever is holding the connections, because the settings screen
/// knows which one a person just revoked and nothing about how to stop it
pub type Cut = Arc<dyn Fn(&str) + Send + Sync>;

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
/// are refused outright when the caller is remote. The page knows: its Browse
/// button walks the PC's folders over /api/walk instead, and export/import are
/// left off.
const NATIVE_DIALOG_PATHS: [&str; 3] = [
    "/api/pick",
    "/api/desk/export",
    "/api/desk/import",
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
/// How long the connection test waits. Long enough for a server on the other
/// side of the world to answer, short enough that the button is not still
/// spinning when somebody has given up on it
const SERVER_TEST_MS: u64 = 25_000;
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

/// Where to read how to install a program: git's and GitHub CLI's own pages,
/// or the page the program's profile names
pub fn install_page(prog: &str) -> Option<String> {
    match prog.trim() {
        p if p.eq_ignore_ascii_case("git") => Some("https://git-scm.com/downloads".to_string()),
        p if p.eq_ignore_ascii_case("gh") => Some("https://cli.github.com/".to_string()),
        p => crate::profile::install_url_for(p),
    }
}

/// Opens a URL in the user's default browser instead of the in-app WebView.
/// Uses ShellExecuteW so the whole URL — query string, '&' and percent-escapes
/// included — is handed to the shell verbatim (explorer.exe mis-parses those
/// and falls back to opening a file window instead).
#[cfg(not(windows))]
pub fn open_external(url: &str) {
    // `xdg-open` is the agreement on Linux desktops; on a server there is
    // nothing to open with, and the failure is quiet on purpose -- nobody is
    // sitting there to be told
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

#[cfg(windows)]
pub fn open_external(url: &str) {
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
fn safe_desk_path(
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
    // Uses the same resolution as the main app's automation loader: the layout root first, then beside the exe.
    // If this drifts, you get "the GUI says it's unconfigured but it actually runs" / "editing in the GUI has no effect"
    Some(crate::config::resolve_data_path(&decoded))
}

/// The manual is embedded in the exe. It can always be referenced regardless of where it's
/// launched from, and won't break even if the docs are forgotten in the distribution.
/// To bundle a translation into the exe, add one line here (it's still read if placed in docs/ instead)
const EMBEDDED_MANUALS: &[(&str, &str)] = &[
    ("en", include_str!("../../../docs/AUTOMATION.md")),
    ("ja", include_str!("../../../docs/AUTOMATION.ja.md")),
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
                if let Ok(s) = std::fs::read_to_string(rel)
                    && !s.trim().is_empty() {
                        return s;
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
            ..Default::default()
        },
        Err(e) => RemoteInfo {
            running: false,
            url: String::new(),
            note: e,
            ..Default::default()
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
                ..Default::default()
            },
            true,
        ),
        None => (effective_remote(shared), false),
    }
}


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
    if kind == "key"
        && let Some(home) = std::env::var_os("USERPROFILE") {
            let ssh = std::path::PathBuf::from(&home).join(".ssh");
            if ssh.is_dir() {
                return Some(ssh);
            }
            return Some(std::path::PathBuf::from(home));
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
    ("gemini", &["-p", ""], "Gemini CLI"),
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
        // The name is there so the AI can tell which tab is which; the id is
        // the only thing that will actually reach it (see hooks::TabKey)
        s.push_str(&format!(
            "{i}. {name}{}",
            crate::i18n::tp("ai.tabs.id", &[("id", id)])
        ));
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
    let prompt = crate::i18n::fill(
        crate::asking::AUTOMATION,
        &[
            ("event", event),
            ("want", want),
            ("layout", layout),
            ("manual", &manual),
        ],
    );

    let text = ask_local_ai(&prompt, engine)?;
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
    let prompt = crate::i18n::fill(crate::asking::UNTANGLE, &[("file", name), ("body", body)]);
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
    let name = which_assistant(engine)?;
    // Asked the confined way first: an empty folder of its own, no tools, no
    // hooks, nothing to reach. Everything asked from this program is a
    // question -- none of it is a job -- and an assistant AI handed a
    // question in a project folder treats it as one. Asked why something had
    // stopped, one of them went and changed code and then said it had fixed
    // it, which is the whole reason this is written the way it is
    match ask_confined(name, prompt, ASK_SYSTEM, None, ASK_TIMEOUT) {
        Ok(said) => Ok(said),
        // An older CLI that does not know one of those options would fail
        // outright, and a question that cannot be asked at all is worse than
        // one asked with fewer walls. It still runs where the confined one
        // does -- an empty folder, nothing of the person's within reach --
        // and the instruction rides in the question instead of in a flag
        Err(e) => {
            crate::append_hook_log(&format!(
                "a confined {name} call failed, asking with the plain options: {e:#}"
            ));
            ask_in_own_folder_plainly(name, prompt)
        }
    }
}

/// What every one-off question is told it is for. Not a prompt about any
/// particular question -- the one thing true of all of them
const ASK_SYSTEM: &str = "You are answering a question, not carrying out a task. Everything you need is in the message. Do not change, create, run or open anything.";

/// How long a one-off question may take before it is abandoned
const ASK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// The last resort: the CLI's plain options, still in a folder of its own.
///
/// Fewer walls than [`ask_confined`] -- this one cannot promise the AI has no
/// tools -- but not none: it is started somewhere empty, so what it can see
/// is what it was given. Reached only when the confined way would not run
fn ask_in_own_folder_plainly(name: &str, prompt: &str) -> Result<String> {
    let (_, args, _) = AI_ENGINES
        .iter()
        .find(|(n, _, _)| *n == name)
        .with_context(|| crate::i18n::tp("webui.err.ai_not_found", &[("name", name)]))?;
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    // No flag for it here, so the instruction goes where it cannot be refused
    let asked = format!("{ASK_SYSTEM}\n\n{prompt}");
    let files: [(&str, &[u8]); 1] = [(CLAUDE_SETTINGS_FILE, br#"{"disableAllHooks": true}"#)];
    let ran = run_in_own_folder(name, args, asked, &files, &[], ASK_TIMEOUT)?;
    if !ran.ok {
        let why: String = ran.err.trim().chars().take(300).collect();
        anyhow::bail!("{}", crate::i18n::tp("ai.err.failed", &[("cmd", &ran.cmd), ("error", &why)]));
    }
    Ok(ran.out)
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
        crate::asking::NO_SURVEY.to_string()
    } else {
        env.to_string()
    };
    let prompt = crate::i18n::fill(
        crate::asking::ONE_COMMAND,
        &[("want", want), ("shell", shell), ("screen", screen), ("env", &env_block)],
    );
    extract_cmd(&ask_local_ai(&prompt, engine)?)
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
    if let Some((_, rest)) = text.split_once("```")
        && let Some((body, _)) = rest.split_once("```") {
            let body = body.trim_start_matches(|c: char| c.is_ascii_alphanumeric());
            let cmd = body.trim();
            if !cmd.is_empty() {
                return Ok(cmd.to_string());
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

/// What the AI that would answer is called, as a person would name it --
/// "Claude Code" -- found the same way as the program itself. None when there
/// is none to run
pub fn local_ai_label(want: Option<&str>) -> Option<&'static str> {
    AI_ENGINES
        .iter()
        .filter(|(name, _, _)| want.is_none_or(|w| w == *name))
        .find(|(name, _, _)| crate::tab::resolve_command(name).is_some())
        .map(|(_, _, label)| *label)
}

/// How to start an installed AI program with `args`: the program itself, or
/// cmd.exe in front of it for a .cmd/.bat, which cannot be started any other way
fn launcher(name: &str, args: Vec<String>) -> Option<(String, Vec<String>)> {
    let path = crate::tab::resolve_command(name)?;
    let p = path.to_string_lossy().to_string();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    Some(if matches!(ext.as_deref(), Some("cmd") | Some("bat")) {
        let mut a = vec!["/c".to_string(), p];
        a.extend(args);
        ("cmd.exe".to_string(), a)
    } else {
        (p, args)
    })
}

/// The assistant AI that answers when one is asked for: the one chosen under
/// Basic > Assistant AI, or with none chosen the first one installed. Its name
/// and the name a person knows it by; nothing when it is not installed.
///
/// The same order and the same test [`pick_local_ai`] uses, so the AI a tool
/// says it will send to is the AI that is started
pub fn assistant_ai(want: Option<&str>) -> Option<(&'static str, &'static str)> {
    AI_ENGINES
        .iter()
        .find(|(name, _, _)| {
            want.is_none_or(|w| w == *name) && crate::tab::resolve_command(name).is_some()
        })
        .map(|(name, _, label)| (*name, *label))
}

/// What the first-start setup offers: the assistant AIs, the installed ones to
/// pick from and the rest with the way to install them, and whether GitHub CLI
/// is here. The same list, in the same order, as Basic > Assistant AI, since
/// that is where the pick is kept.
pub fn setup_state() -> crate::uistate::SetupState {
    setup_state_of(
        |name| crate::tab::resolve_command(name).is_some(),
        |name| install_page(name).is_some(),
    )
}

/// [`setup_state`], asked about a machine described rather than this one
fn setup_state_of(installed: impl Fn(&str) -> bool, has_page: impl Fn(&str) -> bool) -> crate::uistate::SetupState {
    let mut out = crate::uistate::SetupState { gh: installed("gh"), ..Default::default() };
    for (name, _, label) in AI_ENGINES {
        let ai = crate::uistate::SetupAi { id: name.to_string(), name: label.to_string(), install: has_page(name) };
        match installed(name) {
            true => out.installed.push(ai),
            false => out.missing.push(ai),
        }
    }
    out
}

/// The name a person knows an assistant AI by, from its name in the settings
pub fn assistant_label(name: &str) -> Option<&'static str> {
    AI_ENGINES.iter().find(|(n, _, _)| *n == name).map(|(_, _, label)| *label)
}

/// The name a picture is written under for the AIs that are handed a file.
const PICTURE_FILE: &str = "picture.png";

/// The name the shape of the answer is written under, for the AI that reads it
/// from a file.
const SCHEMA_FILE: &str = "answer.schema.json";

/// How long an AI is given to read a picture before it is stopped. The slowest
/// of the three took half a minute on a first run (2026-09-14); a minute and
/// a half beyond that is somebody's network, not a program still thinking
const PICTURE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// What to start an assistant AI with so it reads one picture and answers
/// `prompt` about it in the shape `schema` (a JSON Schema): the arguments
/// after the program, what goes in on standard input, and whether the picture
/// has to be on disk beside it.
///
/// Two of the three hold the answer to the shape themselves -- Claude Code
/// with `--json-schema`, Codex CLI with `--output-schema` -- and Gemini CLI has
/// nothing of the kind, so for it the prompt is all there is. Whoever calls
/// this reads the answer as that shape and asks again when it is not one.
///
/// None of the three is given a way to reach anything but the picture. Text
/// on a screen can say anything, "read this file and write it out" included,
/// and the picture is exactly that text. Measured 2026-09-14, each returning
/// the text of the test picture exactly, in about six seconds:
/// - Claude Code takes the picture inside the message itself (stream-json) and
///   runs with no tools at all
/// - Codex CLI attaches it with `-i`, its commands held to read-only
/// - Gemini CLI reads it with `@file`, in its read-only (plan) mode
///
/// A picture goes to an AI only from the tools, and only from a desk that
/// agreed to it -- that is decided in `snip.rs`, before this is ever reached
fn picture_invocation(name: &str, prompt: &str, png: &[u8], schema: &str) -> Option<(Vec<String>, String, bool)> {
    use base64::Engine as _;
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match name {
        "claude" => {
            let data = base64::engine::general_purpose::STANDARD.encode(png);
            let msg = serde_json::json!({
                "type": "user",
                "message": {"role": "user", "content": [
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}},
                    {"type": "text", "text": prompt},
                ]},
            });
            Some((
                v(&["-p", "--tools", "", "--json-schema", schema, "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]),
                format!("{msg}\n"),
                false,
            ))
        }
        "codex" => Some((
            v(&["exec", "--skip-git-repo-check", "--sandbox", "read-only", "--output-schema", SCHEMA_FILE, "-i", PICTURE_FILE, "-"]),
            prompt.to_string(),
            true,
        )),
        "gemini" => Some((
            v(&["--approval-mode", "plan", "-p", &format!("@{PICTURE_FILE}")]),
            prompt.to_string(),
            true,
        )),
        _ => None,
    }
}

/// Whether an assistant AI is one that is known to read a picture
pub fn reads_pictures(name: &str) -> bool {
    picture_invocation(name, "", &[], "{}").is_some()
}

/// What an AI said, out of what it printed. Claude Code prints a line of JSON
/// per event and the answer is in the last one -- the shaped answer, when it
/// gave one, beside its text; the others print the answer
fn picture_answer(name: &str, out: &str) -> Result<String> {
    if name != "claude" {
        return Ok(out.trim().to_string());
    }
    let done = out
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
        .rfind(|v| v.get("type").and_then(|t| t.as_str()) == Some("result"))
        .context(crate::i18n::t("snip.ai.no_answer"))?;
    let text = done.get("result").and_then(|r| r.as_str()).unwrap_or_default().trim().to_string();
    if done.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false) {
        anyhow::bail!("{text}");
    }
    Ok(match done.get("structured_output") {
        Some(shaped) if !shaped.is_null() => shaped.to_string(),
        _ => text,
    })
}

/// Ask the assistant AI `name` about one picture, and wait for its answer,
/// asked for in the shape `schema` (see [`picture_invocation`]). What comes
/// back is the AI's answer as it gave it, to be read by the caller.
///
/// The AI is started in a folder of its own with nothing else in it -- the
/// picture, when that AI is handed a file, and the shape -- and the folder is
/// gone again once it is done. Stopped after [`PICTURE_TIMEOUT`]
pub fn ask_about_picture(name: &str, prompt: &str, png: &[u8], schema: &str) -> Result<String> {
    let (args, input, on_disk) = picture_invocation(name, prompt, png, schema)
        .with_context(|| crate::i18n::tp("webui.err.ai_not_found", &[("name", name)]))?;
    let mut files: Vec<(&str, &[u8])> = vec![(SCHEMA_FILE, schema.as_bytes())];
    if on_disk {
        files.push((PICTURE_FILE, png));
    }
    let ran = run_in_own_folder(name, args, input, &files, &[], PICTURE_TIMEOUT)?;
    if !ran.ok {
        // Claude Code says what went wrong in its answer, not on stderr
        let said = picture_answer(name, &ran.out).err().map(|e| e.to_string()).unwrap_or_default();
        let why = if ran.err.trim().is_empty() { said } else { ran.err.trim().to_string() };
        anyhow::bail!("{}", crate::i18n::tp("ai.err.failed", &[("cmd", &ran.cmd), ("error", &why)]));
    }
    picture_answer(name, &ran.out)
}

/// What an AI started by [`run_in_own_folder`] left behind
struct Ran {
    /// The program that was started, for saying which one failed
    cmd: String,
    ok: bool,
    out: String,
    err: String,
}

/// Start the assistant AI `name` once, in a folder of its own that holds only
/// `files`, with `input` on its standard input, and stop it after `timeout`.
///
/// A folder of its own for two reasons. Nothing the AI reads can reach past
/// what it was handed -- and a project's instructions to its agents
/// (CLAUDE.md, AGENTS.md, GEMINI.md) are not read in either, which is also
/// much of what a one-line answer would otherwise be charged for. And whatever
/// it writes beside itself is gone with the folder
fn run_in_own_folder(
    name: &str,
    args: Vec<String>,
    input: String,
    files: &[(&str, &[u8])],
    env: &[(&str, String)],
    timeout: std::time::Duration,
) -> Result<Ran> {
    use std::io::Write as _;
    let dir = std::env::temp_dir()
        .join("shikisha-term")
        .join("ask")
        .join(crate::random_hex(8));
    // `{dir}` in an argument or a variable is that folder: a file named from
    // it is read the same whether the program resolves names against where it
    // stands or against its own settings folder
    let at = dir.display().to_string();
    let args = args.into_iter().map(|a| a.replace("{dir}", &at)).collect();
    let (cmd, args) = launcher(name, args)
        .with_context(|| crate::i18n::tp("webui.err.ai_not_found", &[("name", name)]))?;
    std::fs::create_dir_all(&dir)?;
    struct Gone(std::path::PathBuf);
    impl Drop for Gone {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _gone = Gone(dir.clone());
    for (file, body) in files {
        std::fs::write(dir.join(file), body)?;
    }
    let mut spawner = std::process::Command::new(&cmd);
    spawner
        .args(&args)
        .current_dir(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in env {
        spawner.env(k, v.replace("{dir}", &at));
    }
    // The way in a tab's programs report through. An AI the app asks something
    // is not a tab, and its hooks must not speak for one
    spawner.env_remove(crate::api::ENV_PIPE);
    // Inheriting the console here would kill the mouse (same reason as open_browser)
    let mut child = crate::detach_console(&mut spawner)
        .spawn()
        .with_context(|| crate::i18n::tp("ai.err.cannot_run", &[("cmd", &cmd)]))?;
    // Each pipe on a thread of its own: a picture is megabytes going in, and an
    // AI that fills its output while its input is still being written would
    // otherwise wait on this side for good
    let mut stdin = child.stdin.take().context(crate::i18n::t("webui.err.stdin"))?;
    let feed = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    fn drain<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    }
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!(
                "{}",
                crate::i18n::tp("snip.ai.timeout", &[("seconds", &timeout.as_secs().to_string())])
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let _ = feed.join();
    Ok(Ran {
        cmd,
        ok: status.success(),
        out: String::from_utf8_lossy(&out.join().unwrap_or_default()).to_string(),
        err: String::from_utf8_lossy(&err.join().unwrap_or_default()).to_string(),
    })
}

/// What the AI giving a short answer is told it is, in place of each CLI's own
/// instructions for working on code -- thousands of tokens about tools it is
/// not given
const LIGHT_SYSTEM: &str =
    "You give one short reply containing only what is asked for. You have no tools and need none.";

/// The file those instructions are written to, beside the AI
const SYSTEM_FILE: &str = "system.md";

/// The settings file Claude Code is pointed at: its hooks off, so a question
/// the app asks is not reported back to the app as a person's request
const CLAUDE_SETTINGS_FILE: &str = "settings.json";

/// How long a short answer is waited for. Measured 2026-09-17 at one to seven
/// seconds for each of the three; the rest is a slow network or a first start
const LIGHT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// What to start an assistant AI with for a short answer that costs as little
/// as it can: the arguments after the program, and what goes in its
/// environment. The prompt goes in on standard input; the instructions are in
/// [`SYSTEM_FILE`] beside it. Files rather than arguments, because two of the
/// three are started through cmd.exe, which takes quotes apart.
///
/// Everything a CLI loads for working on code is left out: its tools, its
/// connected servers and plugins, its long instructions, its thinking, its
/// hooks. Its conversation is not kept, so these do not turn up in the list of
/// conversations to resume. Measured 2026-09-17 on a request for a name and a
/// summary, against the same CLI started plainly:
/// - Claude Code: about 1,400 tokens instead of 9,500. Its smallest model
/// - Codex CLI: about 4,500 instead of 15,400. Low reasoning, read-only, no web
///   search. Its model is the one the person chose: model names differ by
///   account, and a wrong one is a failure rather than a saving. Features are
///   turned off as settings rather than flags, because a flag it does not know
///   stops it and a setting it does not know is passed over
/// - Gemini CLI: about 2,800, where started plainly it spent turns looking
///   through the folder with its tools. Read-only, its instructions replaced.
///   Its own routing already sends a request this small to a small model
///
/// `small` is the part a person can turn off (`summary_small_model`): the
/// smallest model Claude Code has, and Codex's lowest reasoning. Everything
/// else here stays whatever the answer is, because nothing else about it is a
/// choice with two sides -- a name does not want tools, and nobody wants this
/// in their list of conversations. Gemini is not asked either way: it routes a
/// request this small itself, and naming a model there is a way to be wrong
///
/// `schema` holds the AI to a shape for its answer, as a JSON Schema. Two of
/// the three can be held to one, and each wants it a different way -- Claude
/// Code takes it written out in the argument, Codex CLI takes the name of a
/// file ([`SCHEMA_FILE`], which whoever asks has written beside it), and Gemini
/// CLI has nothing of the kind, so for it the prompt is all there is. The same
/// split as [`picture_invocation`], for the same reason: it is what each of
/// them accepts. Whoever asks reads the answer as that shape and copes when it
/// is not one
fn light_invocation(
    name: &str,
    small: bool,
    schema: Option<&str>,
) -> Option<(Vec<String>, Vec<(&'static str, String)>)> {
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match name {
        "claude" => {
            let mut args = v(&["-p"]);
            if small {
                args.extend(v(&["--model", "haiku"]));
            }
            args.extend(v(&[
                "--tools", "", "--no-session-persistence", "--strict-mcp-config",
                "--disable-slash-commands", "--settings", &format!("{{dir}}/{CLAUDE_SETTINGS_FILE}"),
                "--system-prompt-file", &format!("{{dir}}/{SYSTEM_FILE}"),
            ]));
            if let Some(shape) = schema {
                args.push("--json-schema".into());
                args.push(shape.to_string());
            }
            Some((args, vec![("MAX_THINKING_TOKENS", "0".to_string())]))
        }
        "codex" => {
            let mut args = v(&[
                "exec", "--ephemeral", "--skip-git-repo-check", "--sandbox", "read-only", "--color", "never",
                "-c", "web_search=disabled", "-c", "mcp_servers={}",
            ]);
            if small {
                args.push("-c".into());
                args.push("model_reasoning_effort=low".into());
            }
            args.push("-c".into());
            args.push(format!("model_instructions_file={{dir}}/{SYSTEM_FILE}"));
            if schema.is_some() {
                args.push("--output-schema".into());
                args.push(format!("{{dir}}/{SCHEMA_FILE}"));
            }
            for feature in [
                "apps", "browser_use", "computer_use", "image_generation", "goals", "hooks", "multi_agent", "plugins",
                "shell_tool", "sleep_tool", "tool_suggest", "unified_exec", "view_image", "skill_search", "personality",
                "memories", "in_app_browser",
            ] {
                args.push("-c".into());
                args.push(format!("features.{feature}=false"));
            }
            args.push("-".into());
            Some((args, Vec::new()))
        }
        "gemini" => Some((
            v(&["--extensions", "none", "--allowed-mcp-server-names", "none", "--approval-mode", "plan", "-p", ""]),
            vec![("GEMINI_SYSTEM_MD", format!("{{dir}}/{SYSTEM_FILE}"))],
        )),
        _ => None,
    }
}

/// Ask the assistant AI for a short answer, as cheaply as it can be asked
/// ([`light_invocation`]).
///
/// When the light way fails -- an older CLI that does not know one of its
/// options -- it is asked once more the ordinary way. A name a folder waits a
/// few seconds longer for is better than no name at all
pub fn ask_local_ai_light(prompt: &str, engine: Option<&str>) -> Result<String> {
    let (name, _) = assistant_ai(engine).with_context(|| match engine {
        Some(w) => crate::i18n::tp("webui.err.ai_not_found", &[("name", w)]),
        None => crate::i18n::t("webui.err.ai_missing"),
    })?;
    match ask_light_once(name, prompt) {
        Ok(said) => Ok(said),
        Err(e) => {
            crate::append_hook_log(&format!("a light {name} call failed, asking the ordinary way: {e:#}"));
            // Still confined: `ask_local_ai` runs in a folder of its own
            // whichever way it ends up asking
            ask_local_ai(prompt, Some(name))
        }
    }
}

/// Ask the assistant AI for an answer in a shape, the light way.
///
/// Told what it is for in `system`, rather than the one line a name or a tidy
/// gets: this carries a question somebody typed, and the AI has to know what
/// it is looking at. What comes back is the AI's answer as it gave it -- JSON
/// when it held to the shape, and whatever it said when it did not, which
/// whoever asked has to cope with.
pub fn ask_local_ai_shaped(
    prompt: &str,
    system: &str,
    schema: &str,
    engine: Option<&str>,
    timeout: std::time::Duration,
) -> Result<String> {
    let name = which_assistant(engine)?;
    ask_confined(name, prompt, system, Some(schema), timeout)
}

/// Ask a question that needs no tools at all, and take prose back.
///
/// For the questions where everything the AI needs is already in the prompt
/// and the only thing wanted is an opinion on it. **It is given no tools and
/// its own empty folder**, which is not a nicety: asked the ordinary way, an
/// assistant AI reads the question as a job and sets about doing it. Asked
/// why something had stopped, one of them went and changed code, then said
/// it had fixed it
pub fn ask_local_ai_confined(
    prompt: &str,
    system: &str,
    engine: Option<&str>,
    timeout: std::time::Duration,
) -> Result<String> {
    let name = which_assistant(engine)?;
    ask_confined(name, prompt, system, None, timeout)
}

/// Which assistant AI answers, said the same way wherever it is asked
fn which_assistant(engine: Option<&str>) -> Result<&'static str> {
    let (name, _) = assistant_ai(engine).with_context(|| match engine {
        Some(w) => crate::i18n::tp("webui.err.ai_not_found", &[("name", w)]),
        None => crate::i18n::t("webui.err.ai_missing"),
    })?;
    Ok(name)
}

/// One question, in a folder of its own, with no tools and no hooks.
///
/// The one way this program asks an assistant AI anything that is not a
/// conversation. Written once because the alternative is three copies of the
/// same list of precautions, and the copy that forgets one is the copy that
/// lets an AI loose in whatever folder the app happened to be started from
fn ask_confined(
    name: &str,
    prompt: &str,
    system: &str,
    schema: Option<&str>,
    timeout: std::time::Duration,
) -> Result<String> {
    // Asked of the settings here rather than handed down: every caller wants
    // the same answer, and one that reads it for itself cannot be the one
    // that forgets
    let small = crate::config::load().and_then(|c| c.summary_small_model).unwrap_or(true);
    let (args, env) = light_invocation(name, small, schema)
        .with_context(|| crate::i18n::tp("webui.err.ai_not_found", &[("name", name)]))?;
    let mut files: Vec<(&str, &[u8])> = vec![
        (SYSTEM_FILE, system.as_bytes()),
        (CLAUDE_SETTINGS_FILE, br#"{"disableAllHooks": true}"#),
    ];
    if let Some(shape) = schema {
        files.push((SCHEMA_FILE, shape.as_bytes()));
    }
    let ran = run_in_own_folder(name, args, prompt.to_string(), &files, &env, timeout)?;
    if !ran.ok || ran.out.trim().is_empty() {
        let why: String = ran.err.trim().chars().take(300).collect();
        anyhow::bail!("{}", crate::i18n::tp("ai.err.failed", &[("cmd", &ran.cmd), ("error", &why)]));
    }
    Ok(ran.out)
}

/// The light way alone, with nothing to fall back on
fn ask_light_once(name: &str, prompt: &str) -> Result<String> {
    ask_confined(name, prompt, LIGHT_SYSTEM, None, LIGHT_TIMEOUT)
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

/// The words a new tab's automation name is drawn from, as the page reads
/// them. Drawn from `config::pet_nouns` so that a name minted in the page and
/// one minted here come out of the same bag
fn pet_nouns_json() -> String {
    serde_json::to_string(&crate::config::pet_nouns()).unwrap_or_else(|_| "[]".into())
}

/// What the quick-command editor has to know that `quick.rs` decides: the
/// grid's bounds, how long a name and a body may be, where the icons are,
/// and what a secret named in a body looks like. Handed over rather than
/// written again in the page, so the two cannot hold different limits
fn quick_json() -> String {
    use crate::quick as q;
    serde_json::json!({
        "cols": q::COLS_DEFAULT, "rows": q::ROWS_DEFAULT,
        "colsMax": q::COLS_MAX, "rowsMax": q::ROWS_MAX, "pagesMax": q::PAGES_MAX,
        "labelMax": q::LABEL_MAX, "bodyMax": q::BODY_MAX,
        "icons": q::ICONS_PATH, "secretRef": q::SECRET_REF,
    })
    .to_string()
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
                .replace("__HOTKEYS__", &crate::hotkeys::catalog_json())
                .replace("__QUICK__", &quick_json())
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
                .replace(
                    "__THISPC__",
                    &serde_json::to_string(crate::config::THIS_PC).unwrap_or_default(),
                )
                .replace("__PETNOUNS__", &pet_nouns_json())
                .replace("__DICT__", &crate::i18n::dict_json());
            let resp = secure(Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            ));
            req.respond(resp)?;
        }
        // Opening a settings screen, moving the panel, putting it away. None
        // of it happens here: what the panel asked for waits for the loop that
        // draws the window, which is the only thing that can do any of it
        ("POST", "/api/guide/open") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                return Ok(req.respond(json_resp(serde_json::json!({"error": "too big"})))?);
            };
            let want = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("screen").and_then(|s| s.as_str()).map(str::to_string))
                .unwrap_or_default();
            let body = match crate::guide::want_open(&want) {
                true => serde_json::json!({"ok": true}),
                false => serde_json::json!({"error": crate::i18n::t("guide.err.no_screen")}),
            };
            req.respond(json_resp(body))?;
        }
        ("POST", "/api/guide/move") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                return Ok(req.respond(json_resp(serde_json::json!({"error": "too big"})))?);
            };
            let by = serde_json::from_str::<serde_json::Value>(&body).unwrap_or_default();
            let n = |k: &str| by.get(k).and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            crate::guide::want_move((n("x"), n("y")));
            req.respond(json_resp(serde_json::json!({"ok": true})))?;
        }
        // The panel framed on a phone saying it is there. The window says it
        // for the panel it places; a frame the board stands over the settings
        // has nobody to say it but itself, and the settings screen beside it
        // will not let a box be picked for a ? that is not up
        ("POST", "/api/guide/here") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                return Ok(req.respond(json_resp(serde_json::json!({"error": "too big"})))?);
            };
            let up = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("up").and_then(|u| u.as_bool()))
                .unwrap_or(true);
            crate::guide::phone_here(up);
            req.respond(json_resp(serde_json::json!({"ok": true})))?;
        }
        ("POST", "/api/guide/shut") => {
            let mut req = req;
            let _ = read_body(&mut req, MAX_BODY)?;
            crate::guide::want_shut();
            req.respond(json_resp(serde_json::json!({"ok": true})))?;
        }
        // The ? panel. Served from here because the token, the dictionary
        // and the phone's way in are already here; where it sits on the
        // screen is the app's business, not this server's
        ("GET", "/guide") => {
            let html = crate::i18n::render(&themed(crate::guide::page().to_string()))
                .replace("__TOKEN__", token)
                .replace("__DICT__", &crate::i18n::dict_json());
            let resp = secure(Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            ));
            req.respond(resp)?;
        }
        // Put the question to the assistant AI. It takes tens of seconds, and
        // this server answers one request at a time, so it goes to a thread of
        // its own -- otherwise the settings screen behind the panel stops
        // answering for as long as the AI is thinking
        ("POST", "/api/guide/ask") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                return Ok(req.respond(json_resp(serde_json::json!({"error": "too big"})))?);
            };
            let ask: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let question = ask.get("question").and_then(|q| q.as_str()).unwrap_or("").to_string();
            let so_far: Vec<crate::guide::Said> = ask
                .get("so_far")
                .and_then(|s| serde_json::from_value(s.clone()).ok())
                .unwrap_or_default();
            let picked = crate::guide::picked();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(crate::guide::ask(&question, &so_far, picked.as_ref()));
            });
            let body = match rx.recv() {
                Ok(Ok(a)) => {
                    let at = crate::guide::screen_of(&a.open)
                        .map(|s| crate::i18n::tp("guide.open.at", &[("at", &crate::guide::where_it_is(s))]))
                        .unwrap_or_default();
                    serde_json::json!({"say": a.say, "open": a.open, "fill": a.fill, "at": at})
                }
                Ok(Err(e)) => serde_json::json!({"error": format!("{e:#}")}),
                Err(e) => serde_json::json!({"error": e.to_string()}),
            };
            req.respond(json_resp(body))?;
        }
        // The box somebody picked on the settings screen, and putting one down
        ("GET", "/api/guide/picked") => {
            req.respond(json_resp(
                serde_json::to_value(crate::guide::picked()).unwrap_or_default(),
            ))?;
        }
        ("POST", "/api/guide/picked") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                return Ok(req.respond(json_resp(serde_json::json!({"error": "too big"})))?);
            };
            let want: Option<crate::guide::Picked> = serde_json::from_str::<crate::guide::Picked>(&body)
                .ok()
                .filter(|p| !p.label.trim().is_empty());
            crate::guide::pick(want);
            req.respond(json_resp(serde_json::json!({"ok": true})))?;
        }
        // What the settings screen should write into the picked box. Left
        // here by the panel when a person presses the button under an answer,
        // and taken by the settings screen the next time it asks
        ("POST", "/api/guide/fill") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                return Ok(req.respond(json_resp(serde_json::json!({"error": "too big"})))?);
            };
            let text = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(str::to_string))
                .unwrap_or_default();
            let body = match crate::guide::picked() {
                Some(_) => {
                    crate::guide::leave_to_fill(&text);
                    serde_json::json!({"ok": true})
                }
                None => serde_json::json!({"error": crate::i18n::t("guide.err.no_pick")}),
            };
            req.respond(json_resp(body))?;
        }
        // Whether the panel is up, and anything it left to be written. One
        // question because the settings screen asks both on the same beat,
        // and because "write this" only means anything while it is up
        ("GET", "/api/guide/up") => {
            let up = crate::guide::is_up();
            req.respond(json_resp(serde_json::json!({
                "up": up,
                "fill": up.then(crate::guide::take_to_fill).flatten(),
            })))?;
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
                Some("update-notes") => crate::update::notes_url(),
                // The manual on the site. The ? beside the gear used to be
                // this link and now answers instead, so the link lives inside
                // what answers -- nothing that was reachable has been taken away
                Some("manual") => Some(crate::i18n::t("tui.help.url")),
                // How to install the program a tab needs. The address is the
                // app's own, looked up by program name -- the page names a
                // program, never a place to go
                Some("install") => query_param(req.url(), "prog")
                    .map(|p| percent_decode(&p))
                    .and_then(|p| install_page(&p)),
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
        // Which repository each of several folders is in, in one question. The
        // settings tree groups a desk's folders into its projects before it can
        // draw anything, and a round trip per folder drew the tree ungrouped
        // first and then again. Read off git's own files, never by running git
        ("POST", "/api/families") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let mut out = serde_json::Map::new();
            for path in p.get("paths").and_then(|v| v.as_array()).into_iter().flatten().filter_map(|v| v.as_str()) {
                let at = std::path::Path::new(path.trim());
                if at.as_os_str().is_empty() || out.contains_key(path) {
                    continue;
                }
                let family = crate::repo::family_of(at);
                out.insert(path.to_string(), serde_json::json!({
                    "family": family.as_ref().map(|f| f.display().to_string()),
                    "main": crate::repo::main_checkout(at).map(|m| m.display().to_string()),
                    "cut": crate::repo::is_linked(at),
                    "origin": crate::repo::origin_of(at),
                }));
            }
            req.respond(json_resp(serde_json::json!({ "families": out })))?;
        }
        // Why a folder's automatic name and summary could not be written the
        // last time they were tried, if they could not. Kept by the loop that
        // tried, beside the app (`crate::labels::note_outcome`)
        ("GET", "/api/folder-label") => {
            let at = query_param(req.url(), "path")
                .map(|c| percent_decode(&c))
                .unwrap_or_default();
            let at = crate::config::resolve_folder_cwd(&at);
            let failed = crate::labels::outcomes()
                .into_iter()
                .find(|(k, _)| crate::uistate::same_folder(std::path::Path::new(k), &at))
                .and_then(|(_, v)| v.as_str().map(str::to_string));
            req.respond(json_resp(serde_json::json!({ "failed": failed })))?;
        }
        // Which project a folder belongs to, and whether its folder is one the
        // app made. The settings screen cannot work either out: both mean
        // looking at what git keeps behind the folder
        ("GET", "/api/family") => {
            let at = query_param(req.url(), "path")
                .map(|c| percent_decode(&c))
                .unwrap_or_default();
            let at = std::path::Path::new(at.trim());
            // The desk the page is editing, since a project belongs to a desk
            let desk = query_param(req.url(), "desk").map(|d| percent_decode(&d));
            // The project too, because the screen has to be able to say whose
            // setting it is looking at. A devcontainer belongs to a repository,
            // so a branch folder's page must point at the project rather than
            // offer to write one -- and the project's own page must offer it,
            // which is the half that was missing
            let named = crate::config::load()
                .and_then(|c| c.project_of(desk.as_deref(), at).map(|p| (p.name.clone(), p.at.clone())));
            let resp = match at.as_os_str().is_empty() {
                true => serde_json::json!({
                    "family": null, "cut": false, "branch": null,
                    "project": null, "project_at": null,
                }),
                false => serde_json::json!({
                    "family": crate::repo::family_of(at).map(|f| f.display().to_string()),
                    "cut": crate::repo::is_linked(at),
                    "branch": crate::repo::branch_of(at),
                    "project": named.as_ref().map(|(n, _)| n.clone()),
                    "project_at": named.and_then(|(_, a)| a),
                }),
            };
            req.respond(json_resp(resp))?;
        }
        // The GitHub accounts git on this PC holds. With two of them "this PC's
        // git" cannot sign in until it is told which, so each is offered as a
        // choice of its own. Read fresh: the page asks once when it opens
        ("GET", "/api/pc-accounts") => {
            req.respond(json_resp(serde_json::json!({ "accounts": crate::pr::pc_accounts() })))?;
        }
        // A project's ignore file and what it makes git ignore, for the page
        // that decides how each line's files reach a new worktree. With `add`,
        // `remove` or `untrack` it changes something first; the answer is always
        // the state after, so the page draws what is really there
        ("POST", "/api/project/ignore") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let at = std::path::PathBuf::from(p.get("path").and_then(|v| v.as_str()).unwrap_or_default().trim());
            // Always the checkout the project's worktrees are cut from
            let main = crate::repo::main_checkout(&at);
            let resp = match main {
                None => serde_json::json!({ "ok": false, "error": crate::i18n::t("err.worktree.not_a_repo") }),
                Some(main) => {
                    let did = if let Some(line) = p.get("add").and_then(|v| v.as_str()) {
                        crate::worktree::gitignore_add(&main, line)
                    } else if let Some(r) = p.get("remove") {
                        crate::worktree::gitignore_remove(
                            &main,
                            r.get("n").and_then(|v| v.as_u64()).unwrap_or(0) as usize,
                            r.get("text").and_then(|v| v.as_str()).unwrap_or_default(),
                        )
                    } else if let Some(list) = p.get("untrack").and_then(|v| v.as_array()) {
                        let paths: Vec<String> =
                            list.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
                        crate::worktree::untrack(&main, &paths)
                    } else {
                        Ok(())
                    };
                    let found = crate::worktree::ignored(&main);
                    let mut defaults: Vec<serde_json::Value> = Vec::new();
                    for i in &found {
                        if !defaults.iter().any(|d| d["source"] == i.source.as_str() && d["pattern"] == i.pattern.as_str()) {
                            defaults.push(serde_json::json!({
                                "source": i.source, "pattern": i.pattern,
                                "how": crate::worktree::default_how(&found, &i.source, &i.pattern),
                            }));
                        }
                    }
                    serde_json::json!({
                        "ok": did.is_ok(),
                        "error": did.err().map(|e| format!("{e:#}")),
                        "root": main.display().to_string(),
                        "branch": crate::repo::branch_of(&main),
                        "lines": crate::worktree::gitignore_lines(&main),
                        "ignored": found,
                        "defaults": defaults,
                        "tracked": crate::worktree::tracked_but_ignored(&main),
                    })
                }
            };
            req.respond(json_resp(resp))?;
        }
        // Throw a branch's folder away for good. Refused while anything in it
        // is uncommitted -- said before the settings let go of it, so a no
        // costs nothing. The answer waits for the folder to be gone, on a
        // thread of its own so the rest of the page is not held up. `left`
        // says the folder is still on disk, which the page asks about
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
            if let Err(e) = crate::worktree::ready_to_discard(&at) {
                req.respond(json_resp(serde_json::json!({ "ok": false, "error": format!("{e:#}") })))?;
                return Ok(());
            }
            std::thread::spawn(move || {
                let resp = match crate::worktree::discard_waiting(&at) {
                    Ok(()) => serde_json::json!({ "ok": true }),
                    Err(e) => {
                        crate::append_hook_log(&format!("could not remove {}: {e:#}", at.display()));
                        serde_json::json!({ "ok": false, "left": true, "error": format!("{e:#}") })
                    }
                };
                let _ = req.respond(json_resp(resp));
            });
        }
        // Call a branch's folder's branch something else. Asked twice: once
        // with `go` unset, to put the line that would run in front of the
        // person, and once to run it
        ("POST", "/api/folder/rename") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let at = std::path::PathBuf::from(
                p.get("path").and_then(|v| v.as_str()).unwrap_or_default().trim(),
            );
            let to = p.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let go = p.get("go").and_then(serde_json::Value::as_bool).unwrap_or(false);
            let resp = match crate::worktree::rename_plan(&at, to) {
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
                Ok(plan) => match go {
                    false => serde_json::json!({
                        "ok": true, "line": plan.line(), "sent": plan.sent_as,
                    }),
                    true => match crate::worktree::rename(&plan) {
                        Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
                        Ok(()) => serde_json::json!({
                            "ok": true, "done": true, "from": plan.from, "to": plan.to,
                            "sent": plan.sent_as,
                        }),
                    },
                },
            };
            req.respond(json_resp(resp))?;
        }
        // What a project says its environment needs, and what could be
        // proposed when it says nothing. Both read here rather than guessed by
        // the page: they come from files on disk, and one of them is an offer
        // to write into somebody's repository
        ("GET", "/api/devcontainer") => {
            let at = query_param(req.url(), "path")
                .map(|c| percent_decode(&c))
                .unwrap_or_default();
            let at = std::path::Path::new(at.trim());
            let root = crate::repo::main_checkout(at);
            let has = root.as_deref().and_then(crate::devcontainer::of);
            let offer = has
                .is_none()
                .then(|| root.as_deref().and_then(crate::devcontainer::propose))
                .flatten();
            // What this project has been told to run where it cannot have the
            // file. Read from the settings on disk rather than from the page,
            // which is editing a copy it has not saved yet
            let desk = query_param(req.url(), "desk").map(|d| percent_decode(&d));
            let plain = crate::config::load()
                .as_ref()
                .and_then(|c| c.project_of(desk.as_deref(), at).and_then(|p| p.setup.clone()))
                .unwrap_or_default();
            req.respond(json_resp(
                serde_json::json!({ "has": has, "offer": offer, "plain": plain }),
            ))?;
        }
        // Put the offered file in the project. Worked out again on this side,
        // so what lands is what this side proposed whatever a page said
        ("POST", "/api/devcontainer") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let at = std::path::PathBuf::from(
                p.get("path").and_then(|v| v.as_str()).unwrap_or_default().trim(),
            );
            let resp = match crate::repo::main_checkout(&at)
                .as_deref()
                .and_then(crate::devcontainer::propose)
            {
                None => serde_json::json!({
                    "ok": false, "error": crate::i18n::t("err.devcontainer.nothing")
                }),
                Some(d) => match crate::devcontainer::save(&d) {
                    Ok(()) => serde_json::json!({ "ok": true, "at": d.at }),
                    Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
                },
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
        // The phones that have asked to be buzzed, and the key a browser needs
        // to ask. Behind the token like every other settings route: the list
        // says which devices a person has, which is theirs to know.
        ("GET", "/api/push") => {
            let (key, err) = match crate::push::public_key() {
                Ok(k) => (k, String::new()),
                Err(e) => (String::new(), e),
            };
            let subs: Vec<serde_json::Value> = crate::push::subs()
                .into_iter()
                .map(|s| serde_json::json!({ "endpoint": s.endpoint, "name": s.name }))
                .collect();
            req.respond(json_resp(serde_json::json!({
                "key": key, "error": err, "subs": subs,
            })))?;
        }
        ("POST", "/api/push/subscribe") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let resp = match serde_json::from_str::<crate::push::Sub>(&body) {
                Ok(sub) if !sub.endpoint.is_empty() => {
                    crate::push::remember(sub);
                    serde_json::json!({ "ok": true })
                }
                Ok(_) => serde_json::json!({ "ok": false, "error": "no endpoint" }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            };
            req.respond(json_resp(resp))?;
        }
        ("POST", "/api/push/forget") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let endpoint = v.get("endpoint").and_then(|e| e.as_str()).unwrap_or("");
            let gone = crate::push::forget(endpoint);
            req.respond(json_resp(serde_json::json!({ "ok": gone })))?;
        }
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
                        crate::notify::Destination::Slack { webhook }
                        | crate::notify::Destination::Discord { webhook } => {
                            *webhook = deref(webhook)
                        }
                        crate::notify::Destination::Telegram { token, chat_id } => {
                            *token = deref(token);
                            *chat_id = deref(chat_id);
                        }
                        crate::notify::Destination::Windows {}
                        | crate::notify::Destination::Phone {} => {}
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
        // Reach a server with the settings as they stand on screen, before any
        // of it is saved. What comes back is either "it answered and let us
        // in" or the server's own words about why it did not -- which is the
        // difference between fixing one field now and finding out at launch
        ("POST", "/api/server/test") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            // The credentials are read from the vault here, by the names the
            // tab settles on, and never travel through the page
            let pw = password.lock().unwrap().clone();
            if let Some(c) = crate::config::load() {
                crate::ssh::use_secrets(c.resolve_tokens(pw.as_deref()));
            }
            let text = |k: &str| {
                v.get(k).and_then(|x| x.as_str()).unwrap_or_default().trim().to_string()
            };
            let desk = text("desk");
            let tab = text("tab");
            let under = |what: &str| {
                (!desk.is_empty() && !tab.is_empty()).then(|| format!("ssh/{desk}/{tab}/{what}"))
            };
            // Built by the same function a launch builds it with, from the
            // settings as the page has them. A copy of that function here had
            // already drifted: a bastion's port arrives as the text typed into
            // its box, which the copy did not read, so a bastion on any port
            // but 22 was tested on 22 and launched on the right one
            let spec = crate::view::server_spec(
                &text("host"),
                port_of(v.get("port")).unwrap_or(22),
                &text("user"),
                Some(&server_of_page(&v)),
                &under,
            );
            let resp = if spec.host.is_empty() || spec.user.is_empty() {
                serde_json::json!({"ok": false, "error": crate::i18n::t("err.server.incomplete")})
            } else {
                // Asking for a folder is the smallest thing that proves the
                // whole road: the address, the credential, and that the far
                // end will actually serve files over it
                match crate::ssh::files(
                    &spec,
                    crate::ssh::FileJob::List { path: ".".into() },
                    SERVER_TEST_MS,
                ) {
                    Ok(_) => serde_json::json!({"ok": true}),
                    Err(e) => serde_json::json!({"ok": false, "error": format!("{e:#}")}),
                }
            };
            req.respond(json_resp(resp))?;
        }
        // Which server a tab's command reaches, in the one spelling its name
        // is filed under (`ssh::Spec::machine`). The settings screen asks
        // rather than working it out: the tab row finds a server's name by
        // this spelling, and a second way of writing it in the page would
        // file a name the tab row never finds
        ("POST", "/api/server/machine") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let command = v.get("command").and_then(|c| c.as_str()).unwrap_or_default();
            let argv = crate::config::CommandSpec::Line(command.to_string()).argv();
            let server = v.get("server").map(server_of_page);
            let machine = crate::view::machine_of(&argv, server.as_ref());
            req.respond(json_resp(serde_json::json!({ "machine": machine })))?;
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
            // Whether the tab's "come back to this conversation" switch has
            // anything to act on for this command, so the screen can grey a
            // switch that would decide nothing (and say which of the two
            // reasons it is)
            let carry = match argv.is_empty() {
                true => None,
                false => crate::tab::carry_unused(&argv, &str_of("profile")),
            };
            // The program this tab would start, when it is not on this PC:
            // said while the tab is being made, not found out when it does not
            // appear. Only for a command that starts a program here -- an
            // address, a page or the app's own panels start nothing to look
            // for, except the git panel, which needs git
            let head = argv.first().cloned().unwrap_or_default();
            let starts_nothing = crate::config::is_editor_panel(&argv)
                || crate::config::is_sftp_panel(&argv)
                || crate::config::browser_url_of(&argv).is_some()
                || crate::config::ssh_endpoint(&argv).is_some()
                || head.eq_ignore_ascii_case("model");
            let wanted = match () {
                _ if crate::config::is_git_panel(&argv) => Some("git".to_string()),
                _ if starts_nothing || head.trim().is_empty() => None,
                _ => Some(head.clone()),
            };
            let missing = wanted.filter(|p| crate::tab::resolve_command(p).is_none());
            let install_url = missing.as_deref().and_then(install_page);
            req.respond(json_resp(serde_json::json!({
                "argv": line.argv, "added": line.added, "carry": carry,
                "missing": missing, "install_url": install_url,
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
        // Read/write a desk definition file (external file reference)
        ("GET", "/api/desk") => {
            let Some(p) = safe_desk_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            serve_user_json(req, &p, r#"{"tabs":[]}"#)?;
        }
        ("POST", "/api/desk") => {
            let Some(p) = safe_desk_path(req.url(), config_path) else {
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
                            .map(|(k, m)| {
                                serde_json::json!({
                                    "key": k,
                                    "description": m.desc,
                                    "human": m.human,
                                    "ai": m.ai,
                                    "urls": m.urls,
                                })
                            })
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
            let (key, value) = (s("key").trim(), s("value"));
            let path = secrets_file(config_path);
            let pw = password.lock().unwrap().clone();
            // An empty value means "leave the password alone" -- what a secret
            // is *for* can be changed without going to find it again. The store
            // refuses that for a name it has never seen
            let flag = |k, or| p.get(k).and_then(|v| v.as_bool()).unwrap_or(or);
            let urls: Vec<String> = p
                .get("urls")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|h| h.as_str())
                        .map(|h| h.trim().to_string())
                        .filter(|h| !h.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            // The same question the screen asked while it was being typed. A
            // line that reaches too far is refused here as well, because the
            // screen is not the only way in
            let bad = urls
                .iter()
                .find_map(|u| crate::config::url_fault(u).map(|why| (u.clone(), why)));
            let meta = crate::config::SecretMeta {
                human: flag("human", true),
                ai: flag("ai", false),
                urls,
                desc: s("description").to_string(),
            };
            let resp = match bad {
                Some((u, why)) => serde_json::json!({
                    "ok": false,
                    "error": format!("{}: {}", u, crate::i18n::t(why)),
                }),
                None => match crate::config::upsert_secret(&path, pw.as_deref(), key, &meta, value)
                {
                    Ok(()) => serde_json::json!({ "ok": true }),
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                },
            };
            req.respond(json_resp(resp))?;
        }
        // One secret filed again under another name, value and all, without the
        // value ever passing through the page. What a new desk does when it
        // starts from another desk's connections and destinations: each desk
        // keeps its own keys, so deleting one desk later cannot take the
        // other's with it. Only the names the program itself files keys under
        // can be copied, and only into the same kind
        ("POST", "/api/secrets/copy") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let s = |k| p.get(k).and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let (from, to) = (s("from"), s("to"));
            let kind = |k: &str| ["provider/", "notify/"].into_iter().find(|pre| k.starts_with(pre));
            let path = secrets_file(config_path);
            let pw = password.lock().unwrap().clone();
            let resp = match (kind(&from), kind(&to)) {
                (Some(a), Some(b)) if a == b && from != to => {
                    let meta = crate::config::list_secrets(&path, pw.as_deref())
                        .ok()
                        .and_then(|l| l.into_iter().find(|(k, _)| *k == from).map(|(_, m)| m));
                    match (crate::config::secret_value(&path, pw.as_deref(), &from), meta) {
                        (Some(value), Some(meta)) => {
                            match crate::config::upsert_secret(&path, pw.as_deref(), &to, &meta, &value) {
                                Ok(()) => serde_json::json!({ "ok": true }),
                                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                            }
                        }
                        _ => serde_json::json!({ "ok": false, "missing": true }),
                    }
                }
                _ => serde_json::json!({ "ok": false, "error": "not a copyable key" }),
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
        // Export a single desk, scripts and all, as one file.
        // Addressed by index into the saved config — not the screen's in-progress edits
        ("POST", "/api/desk/export") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let index = p.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let resp = match crate::deskpack::pack(config_path, index) {
                Ok((name, text)) => {
                    let picked = picker().and_then(|p| {
                        p.save(
                            &crate::i18n::t("settings.desk.export.title"),
                            config_path.parent(),
                            &name,
                            (&crate::i18n::t("settings.desk.file_kind"), "json"),
                        )
                    });
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
        // Import an exported file. Adds one desk to the config
        ("POST", "/api/desk/import") => {
            let picked = picker().and_then(|p| {
                p.open(
                    &crate::i18n::t("settings.desk.import.title"),
                    config_path.parent(),
                    (&crate::i18n::t("settings.desk.file_kind"), "json"),
                )
            });
            let resp = match picked {
                Some(path) => match std::fs::read_to_string(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|t| crate::deskpack::unpack(config_path, &t))
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
            // A script written as one file holds every trigger in it. Read as a
            // folder it answered "nothing set" for a tab whose script runs
            if dir.is_file() {
                let body = std::fs::read_to_string(&dir).unwrap_or_default();
                map.insert("file".into(), serde_json::Value::String(body));
            }
            for name in EVENT_FILES.iter().filter(|_| !dir.is_file()) {
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
            // One file, written back whole. Never a folder made in its place:
            // that would fail on the file already there, or, where the file was
            // missing, leave a folder with a script's name that nothing loads
            if let Some(code) = parsed.get("file").and_then(|v| v.as_str()) {
                if dir.is_dir() {
                    req.respond(Response::from_string("not a file").with_status_code(400))?;
                    return Ok(());
                }
                crate::crypto::write_atomic(&dir, code)?;
                req.respond(Response::from_string(r#"{"ok":true}"#))?;
                return Ok(());
            }
            if dir.is_file() {
                req.respond(Response::from_string("not a folder").with_status_code(400))?;
                return Ok(());
            }
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

            let picked = picker().and_then(|p| match kind == "dir" {
                true => p.folder(title, start.as_deref()),
                false => p.open(title, start.as_deref(), ("", "")),
            });
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
        // The same folder list the sidebar walks, for the settings screen.
        //
        // A phone has no file dialog of its own, and /api/pick opens one on
        // the PC -- so this hands back what is in a folder and the page walks
        // it, the way the sidebar's "open another folder" already does. A file
        // is answered as itself, so a walk that lands on one is a choice made.
        // Read-only: nothing here writes, the form saves what was chosen
        ("POST", "/api/walk") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let want_files = p.get("files").and_then(|v| v.as_bool()).unwrap_or(false);
            let asked = p.get("path").and_then(|v| v.as_str()).unwrap_or("").trim();
            // Paths in the settings are written relative to the config folder
            // when they are under it (display_path); read them back the same way
            let at = match asked {
                "" => String::new(),
                a => {
                    let raw = std::path::Path::new(a);
                    let full = if raw.is_absolute() {
                        raw.to_path_buf()
                    } else {
                        config_path.parent().unwrap_or(std::path::Path::new(".")).join(raw)
                    };
                    full.display().to_string()
                }
            };
            let here = std::path::Path::new(&at);
            let resp = if !at.is_empty() && here.is_file() {
                serde_json::json!({
                    "ok": true,
                    "file": true,
                    "chosen": display_path(here, config_path),
                })
            } else {
                let walk = if want_files {
                    crate::uistate::BrowseState::with_files(&at)
                } else {
                    crate::uistate::BrowseState::of(&at)
                };
                serde_json::json!({
                    "ok": true,
                    "at": walk.at,
                    "up": walk.up,
                    "dirs": walk.dirs,
                    "files": walk.files,
                    "error": walk.error,
                    // What choosing this folder would write into the settings
                    "chosen": if walk.at.is_empty() {
                        String::new()
                    } else {
                        display_path(std::path::Path::new(&walk.at), config_path)
                    },
                })
            };
            req.respond(json_resp(resp))?;
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
        // Which GitHub account a git account's token speaks as, and for how
        // much longer. Asked per account of one desk, because each keeps its
        // own token.
        //
        // The state and the date, never the value -- and a token that has run
        // out is said out loud rather than quietly becoming "no token", because
        // a row that stops showing pull request numbers looks exactly like a
        // branch that has none
        ("GET", "/api/github") => {
            let desk = query_param(req.url(), "desk")
                .map(|c| percent_decode(&c))
                .unwrap_or_default();
            let account = query_param(req.url(), "account")
                .map(|c| percent_decode(&c))
                .unwrap_or_default();
            // Asked of the account itself, as every other reader does: a gh
            // account's token comes from GitHub CLI, the others' from the store.
            // One not saved yet is not in the settings file, and has only the
            // store to ask
            let spec = crate::config::load().and_then(|cfg| {
                cfg.resolve_desks().0.into_iter().find(|d| d.id == desk.trim())?
                    .git_accounts.into_iter().find(|a| a.name == account.trim())
            });
            let own = match desk.trim().is_empty() || account.trim().is_empty() {
                true => None,
                false => {
                    let pw = password.lock().unwrap().clone();
                    let look = |k: &str| crate::config::secret_value(&secrets_file(config_path), pw.as_deref(), k);
                    match spec {
                        Some(spec) => spec.token(desk.trim(), &look),
                        None => look(&crate::config::git_token_key(desk.trim(), account.trim())),
                    }
                }
            };
            let said = crate::pr::probe(own);
            req.respond(json_resp(serde_json::json!({
                "signed_in": said.ok,
                "source": said.source,
                "login": said.login,
                "expires_at": said.expires_at,
                "expires_days": said.expires_in_days,
                "status": said.status,
            })))?;
        }
        // The secrets nothing in the settings claims any more. Answered here
        // rather than worked out on the screen, because what owns what is a
        // fact about the settings file and is tested with it
        ("GET", "/api/secrets/orphans") => {
            let path = secrets_file(config_path);
            let pw = password.lock().unwrap().clone();
            let keys: Vec<String> = crate::config::list_secrets(&path, pw.as_deref())
                .map(|l| l.into_iter().map(|(k, _)| k).collect())
                .unwrap_or_default();
            let orphans = match crate::config::load() {
                Some(cfg) => crate::config::orphan_secrets(&cfg, &keys),
                // Without a config nothing can be said to be unclaimed, and
                // guessing here would offer to delete somebody's passwords
                None => Vec::new(),
            };
            req.respond(json_resp(serde_json::json!({ "orphans": orphans })))?;
        }
        // Whether Claude Code is signed in on this machine, so the settings can
        // say why the allowance pill is or is not there
        ("GET", "/api/claude") => {
            req.respond(json_resp(serde_json::json!({
                "signed_in": crate::limits::signed_in(),
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
            #[cfg(windows)]
            let body = {
                let r = crate::conpty::report();
                serde_json::json!({
                    "bundled": r.bundled,
                    "version": r.version,
                    "path": r.path.display().to_string(),
                    "missing": r.missing.map(|m| m.id()),
                })
            };
            // A unix pty is the system's own and needs nothing shipped beside
            // the exe, so there is nothing here to report on or to fix
            #[cfg(not(windows))]
            let body = serde_json::json!({
                "bundled": false,
                "version": "",
                "path": "",
                "missing": serde_json::Value::Null,
            });
            req.respond(json_resp(body))?;
        }
        // Every action the window has, with the key it answers to right now.
        // The names are the app's own, so the settings screen never has its
        // own idea of what this program can do
        // The keys that open the tools from any program: what each is set to,
        // whether it could be registered, and when it last arrived
        ("GET", "/api/hotkeys") => {
            req.respond(json_resp(serde_json::json!({
                "rows": crate::hotkeys::rows(),
                "active": crate::hotkeys::active(),
            })))?;
        }
        // The icon set the quick-command picker chooses from. Asked for once,
        // when the picker is first opened
        ("GET", p) if crate::quick::asset(p).is_some() => {
            let bytes = crate::quick::asset(p).unwrap_or_default();
            req.respond(
                Response::from_data(bytes)
                    .with_header(
                        Header::from_bytes(&b"Content-Type"[..], &b"text/plain; charset=utf-8"[..]).unwrap(),
                    )
                    .with_header(
                        Header::from_bytes(&b"Cache-Control"[..], &b"public, max-age=86400"[..]).unwrap(),
                    ),
            )?;
        }
        // Where every quick command sits, worked out by the same function the
        // board is drawn from: after the grid is resized, and before the page
        // shows what a hand-written file says. The page never places a button
        // by a rule of its own, so the settings and the launcher cannot come to
        // disagree about where one is
        ("POST", "/api/quick/arrange") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                return req
                    .respond(Response::from_string("payload too large").with_status_code(413))
                    .map_err(Into::into);
            };
            match serde_json::from_str::<crate::quick::QuickSpec>(&body) {
                Ok(spec) => {
                    let spec = crate::quick::arrange(&spec);
                    req.respond(json_resp(serde_json::json!({
                        "ok": true,
                        "svgs": crate::quick::drawings(&spec),
                        "spec": spec,
                    })))?
                }
                Err(e) => req.respond(json_resp(serde_json::json!({
                    "ok": false,
                    "error": e.to_string(),
                })))?,
            }
        }
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
            // What has a key that works from any program is set in that list,
            // not twice: that key reaches it in this window as well
            let rows: Vec<serde_json::Value> = crate::keys::ACTIONS
                .iter()
                .filter(|a| !crate::hotkeys::ON_THE_BOARD.contains(&a.name))
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
                // Whether a browser will treat this link as a secure context,
                // which decides whether a phone will keep the page on its home
                // screen. Sent as the answer rather than left to be read off
                // the origin: the origin is only ever a "is there anything to
                // show" flag on that page, and it stays that way.
                "https": origin.starts_with("https://"),
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
        // The devices allowed in. Never the keys -- the book holds hashes, and
        // a screen that could show a key would be a screen worth stealing
        ("GET", "/api/remote/clients") => {
            let rows: Vec<_> = crate::clients::load()
                .clients
                .into_iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "name": c.name,
                        "added": c.added,
                        "seen": c.seen,
                    })
                })
                .collect();
            req.respond(json_resp(serde_json::json!({ "clients": rows })))?;
        }
        // Naming a device. A list of six-character ids is a list nobody can act
        // on; "kitchen iPad" is what makes revoking a decision rather than a guess
        ("POST", "/api/remote/clients/name") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let id = p.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or_default();
            let out = match crate::clients::rename(id, name) {
                Ok(()) => serde_json::json!({ "ok": true }),
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
            };
            req.respond(json_resp(out))?;
        }
        // Taking one device's key away. Both halves: the key, so its next
        // request is refused, and whatever it is holding now, so the screen it
        // already has stops being drawn
        ("POST", "/api/remote/clients/revoke") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let id = p.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let gone = crate::clients::revoke(&id).unwrap_or(false);
            if gone {
                let cut = remote.lock().unwrap().cut.clone();
                if let Some(cut) = cut {
                    cut(&id);
                }
            }
            req.respond(json_resp(serde_json::json!({ "ok": gone })))?;
        }
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
        // The update: where it stands, and the presses that move it. All of
        // them answer with the state as it is afterwards, so the page draws
        // what the program holds and never what it hoped
        ("GET", "/api/update") => req.respond(json_resp(crate::update::snapshot()))?,
        ("POST", "/api/update/check") => {
            crate::update::request_check();
            req.respond(json_resp(crate::update::snapshot()))?;
        }
        ("POST", "/api/update/install") => match crate::update::request_install() {
            Ok(()) => req.respond(json_resp(crate::update::snapshot()))?,
            Err(e) => req.respond(json_resp(serde_json::json!({ "ok": false, "error": e.to_string() })).with_status_code(409))?,
        },
        ("POST", "/api/update/skip") => {
            crate::update::skip();
            req.respond(json_resp(crate::update::snapshot()))?;
        }
        ("POST", "/api/update/discard") => {
            crate::update::discard();
            req.respond(json_resp(crate::update::snapshot()))?;
        }
        ("POST", "/api/update/rollback") => match crate::update::request_rollback() {
            Ok(()) => req.respond(json_resp(crate::update::snapshot()))?,
            Err(e) => req.respond(json_resp(serde_json::json!({ "ok": false, "error": e.to_string() })).with_status_code(409))?,
        },
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

/// A port as the settings page holds it: a number, or the text typed into a
/// box. Anything that is not a port is no port
fn port_of(v: Option<&serde_json::Value>) -> Option<u16> {
    match v? {
        serde_json::Value::Number(n) => n.as_u64().and_then(|n| u16::try_from(n).ok()),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// A server connection's own settings as the page holds them before they are
/// saved -- the shape a tab's `server` block is written in, but with whatever
/// the boxes hold (a port as text, an empty key) rather than what is saved
fn server_of_page(v: &serde_json::Value) -> crate::config::ServerSpec {
    let text = |o: &serde_json::Value, k: &str| {
        Some(o.get(k).and_then(|x| x.as_str()).unwrap_or_default().trim().to_string())
            .filter(|t| !t.is_empty())
    };
    crate::config::ServerSpec {
        key: text(v, "key"),
        jump: v.get("jump").filter(|j| j.is_object()).map(|j| crate::config::JumpSpec {
            host: text(j, "host").unwrap_or_default(),
            port: port_of(j.get("port")),
            user: text(j, "user").unwrap_or_default(),
            key: text(j, "key"),
        }),
        keepalive: v.get("keepalive").and_then(|x| x.as_u64()),
        file_command: text(v, "file_command"),
        remote_dir: text(v, "remote_dir"),
    }
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
pub(crate) fn themed(html: String) -> String {
    let look = crate::config::load().map(|c| c.appearance).unwrap_or_default();
    let scheme = look.scheme();
    crate::quick::render(crate::push::inject(crate::toast::render(html)))
        .replace("{{THEME}}", &scheme.css_vars())
        // The colours a project or a server is offered, from the list the app
        // colours an unchosen one from -- so a picked colour and a worked-out
        // one are always from the same eight
        .replace(
            "{{MARK_COLOURS}}",
            &serde_json::to_string(&crate::uistate::PALETTE).unwrap_or_else(|_| "[]".into()),
        )
        .replace(
            "{{SCHEME}}",
            if crate::theme::is_light(&scheme) { "light" } else { "dark" },
        )
}

/// The settings page as it is written, before a language is laid over it.
///
/// Read by the guide, which works out from this script which screens the
/// settings have and which words each one shows. A reader, never a writer:
/// what is served goes through [`crate::i18n::render`] as it always has.
pub fn page() -> &'static str {
    PAGE
}

/// The screen an older name for one of a desk's settings leads to.
///
/// The board holds names this page has carried before (`git-message` for the
/// git screen), so that a button written once goes on working. One answer, in
/// the page's own table, read by whoever has to follow the same link.
pub fn desk_link(name: &str) -> Option<&'static str> {
    let table = PAGE.split_once("const DESK_LINKS = {")?.1.split_once("};")?.0;
    table
        .split(',')
        .filter_map(|kv| kv.split_once(':'))
        .map(|(k, v)| (k.trim().trim_matches('"'), v.trim().trim_matches('"')))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
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
 [hidden] { display:none !important; }
 /* Space, in steps. Every gap on this page is one of these six numbers, so
    that "near" and "apart" mean the same distance wherever they appear. What
    went wrong without them: a label sat as far from its own field as the
    field sat from the next question, and the form read as one long list of
    unrelated lines */
 :root { --s1:4px; --s2:8px; --s3:12px; --s4:16px; --s5:20px; --s6:24px;
   /* Corners: what you press or type in, what holds them, what labels them */
   --r-ctl:6px; --r-card:10px; --r-chip:4px; }
 body { margin:0; background:var(--bg); color:var(--text); font-size:14px; line-height:1.5;
   font-family:system-ui,"Segoe UI","Yu Gothic UI","Hiragino Sans",sans-serif; }
 :root { --mono:ui-monospace,Consolas,"Courier New",monospace; }
 code, .mono, input.mono { font-family:var(--mono); }

 header { position:sticky; top:0; z-index:5; display:flex; align-items:center; gap:12px;
   padding:12px 20px; background:color-mix(in srgb, var(--bg) 90%, transparent); backdrop-filter:blur(8px);
   border-bottom:1px solid var(--line); }
 header h1 { font-size:15px; font-weight:600; margin:0; letter-spacing:.02em; }
 header .spacer { flex:1; }
 /* The secondary links live in one element so a narrow screen can MOVE them into
    the drawer instead of a second copy being written for the phone. */
 .headlinks { display:flex; align-items:center; gap:var(--s3); }
 /* Drawer handle and current-section label: phone only (see the narrow block). */
 .navtoggle { display:none; font-size:17px; line-height:1; padding:6px 10px; }
 /* Where you are, in two parts: the desk gives way first (it ellipsises),
    the thing actually being edited always stays whole. */
 #crumb { display:none; align-items:baseline; gap:var(--s2); min-width:0;
   font-weight:600; font-size:14px; white-space:nowrap; }
 #crumb .up { flex:0 1 auto; min-width:0; overflow:hidden; text-overflow:ellipsis;
   font-weight:400; color:var(--muted); }
 #crumb .cur { flex:0 0 auto; overflow:hidden; text-overflow:ellipsis; }
 #navscrim { display:none; }
 /* Label on a desktop, icon on a phone — one button, two skins. */
 .atnarrow { display:none; }
 #msg { color:var(--muted); font-size:13px; border-radius:var(--r-ctl); padding:4px 10px; }
 #msg.warn { color:var(--danger); }
 /* A setting that could not be used. Said in the place it was set, not in a
    log nobody opens */
 .warn { color:var(--danger); font-size:13px; margin:0 0 var(--s2); }
 /* Replay the animation every time, so a click still registers even if the message text repeats */
 #msg.flash { animation:msgflash 1.1s ease-out; }
 @keyframes msgflash {
   0%   { background:var(--accent); color:var(--bg); }
   60%  { background:var(--accent); color:var(--bg); }
   100% { background:transparent; color:var(--muted); }
 }
 button.primary:disabled { opacity:.55; cursor:default; }
 /* Small text in the header doesn't catch the eye for a save result, so the
    shared toast (src/toast.rs) says it again at the bottom of the screen */
{{TOAST_CSS}}
 /* Mark the save button while there are unsaved changes. It turns amber and
    pulses a glow ring so an unsaved edit is impossible to miss and you remember
    to press Save at the end (it goes back to the normal blue once saved). */
 #savebtn.dirty { background:color-mix(in srgb, var(--warn) 18%, transparent);
   border-color:var(--warn); color:var(--warn); }
 #savebtn.dirty::before { content:"● "; }
 @keyframes savepulse {
   0%   { box-shadow:0 0 0 0 color-mix(in srgb, var(--warn) 60%, transparent); }
   70%  { box-shadow:0 0 0 8px transparent; }
   100% { box-shadow:0 0 0 0 transparent; }
 }
 /* The one thing to press next, wherever it is */
 .pulse { animation:savepulse 1.1s ease-in-out infinite; }
 @media (prefers-reduced-motion: reduce) { .pulse { animation:none; } }

 .layout { display:flex; align-items:flex-start; }
 /* Wider than it was, because the settings have the window now: four levels of
    indentation and a folder's whole path both need somewhere to go, and the
    column on the right is capped anyway */
 nav { width:min(340px, 32vw); flex:none; border-right:1px solid var(--line);
   min-height:calc(100vh - var(--headh)); padding:12px 10px; position:sticky;
   top:var(--headh); max-height:calc(100vh - var(--headh)); overflow:auto; }
 main { flex:1; min-width:0; padding:24px 28px; max-width:820px; }

 .navitem { display:block; width:100%; height:auto; text-align:left; border:0; background:none;
   color:var(--text); padding:7px 10px; border-radius:var(--r-ctl); cursor:pointer;
   font-size:13.5px; font-weight:400; font-family:inherit; }
 .navitem:hover { background:var(--panel); }
 .navitem.sel { background:var(--panel2); }
 .navitem .sub { display:block; color:var(--muted); font-size:11.5px; margin-top:1px;
   white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
 .navgroup { color:var(--muted); font-size:11px; letter-spacing:.08em; text-transform:uppercase;
   margin:var(--s4) var(--s3) var(--s1); }
 /* The desk everything under here belongs to. One button: it lists the
    desks, and choosing one opens that desk's page */
 .deskbanner { width:100%; display:flex; align-items:center; gap:var(--s2);
   padding:6px 8px 6px 4px; min-height:36px; border-radius:var(--r-ctl);
   background:none; border:0; color:var(--text); font-size:14px; font-weight:600;
   cursor:pointer; text-align:left; font-family:inherit; }
 .deskbanner:hover { background:var(--panel); }
 .deskbanner.sel { background:var(--panel2); }
 .deskbanner .nm { min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
 .wsgap { flex:1 1 auto; }
 .wspick { font-size:12px; color:var(--dim); }
 .deskbanner:hover .wspick { color:var(--text); }
 /* The initial, as a plate. A desk has no colour of its own, so this is
    the one thing on the row that says "a desk" rather than "a name" */
 .wsbadge { flex:none; width:22px; height:22px; border-radius:var(--r-chip);
   background:var(--raise); color:var(--dim); font-size:11px; font-weight:600;
   display:flex; align-items:center; justify-content:center; }
 /* Choosing another desk. Floats, so the list under it does not move */
 .fmenu { position:fixed; z-index:60; min-width:220px; max-width:280px;
   background:var(--panel); border:1px solid var(--line); border-radius:var(--r-card);
   box-shadow:0 8px 24px #0007; padding:4px; display:flex; flex-direction:column; }
 .fmenuitem { display:flex; align-items:center; gap:var(--s2); width:100%;
   text-align:left; background:none; border:0; color:var(--text); font-size:13px;
   padding:7px 8px; border-radius:var(--r-ctl); cursor:pointer; }
 .fmenuitem:hover { background:var(--panel2); }
 .fmenuitem.on { background:var(--panel2); }
 .fmenuitem.add { color:var(--dim); border-top:1px solid var(--line);
   border-radius:0 0 var(--r-ctl) var(--r-ctl); margin-top:4px; padding-top:10px; }
 .navgrouphead { color:var(--muted); font-size:11px; letter-spacing:.08em; text-transform:uppercase;
   margin:var(--s3) 0 var(--s1); display:flex; align-items:center; gap:var(--s2); }
 .navgrouphead.sel { color:var(--text); }
 .navgrouphead .caret { font-size:10px; width:14px; display:inline-block; text-align:center;
   border-radius:var(--r-chip); }
 .navgrouphead .caret:hover { background:var(--panel2); }
 .navitem { position:relative; display:flex; align-items:flex-start; gap:var(--s2); }
 /* The name and what is under it, once the rails and the mark have had theirs */
 .navitem .body { flex:1 1 auto; min-width:0; }
 /* The mark: what kind of thing this row is, in the colour of which one.
    A folder is drawn rather than typed, for the reason the board draws its
    branch mark -- a character that means "folder" is one some font has never
    heard of, and an emoji cannot take the project's colour */
 .mark { flex:none; width:16px; height:20px; display:flex; align-items:center;
   justify-content:center; color:var(--dim); }
 .mark svg { display:block; }
 /* A tab, as the dot the board uses, in the colour of the AI it runs */
 .mark .dot { width:7px; height:7px; border-radius:50%; background:currentColor; }
 .crumbs { display:flex; align-items:center; flex-wrap:wrap; gap:var(--s1) var(--s2); margin:0 0 var(--s3);
   font-size:12.5px; color:var(--dim); min-width:0; }
 .crumbs .crumb { border:0; background:none; padding:2px 4px; margin:0 -4px; min-height:24px; border-radius:var(--r-ctl);
   color:var(--dim); font:inherit; cursor:pointer; max-width:240px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
 .crumbs .crumb:hover { color:var(--text); background:var(--panel); }
 .crumbs .sep { color:var(--muted); }
 .crumbs .here { color:var(--text); font-weight:600; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; max-width:320px; }
 .navnone { margin:var(--s1) var(--s3); color:var(--muted); font-size:12px; line-height:1.5; }
 /* A project names the repository its folders are in: the one row in the tree
    that names a thing, so it carries the weight (styleguide §3) */
 .navproject { color:var(--text); font-size:12.5px; font-weight:600; }
 .navproject .sub { font-size:11px; font-weight:400; }
 .projmark { width:8px; height:8px; border-radius:2px; background:var(--dim); }

 .card { background:var(--panel); border:1px solid var(--line); border-radius:var(--r-card);
   padding:var(--s3) var(--s5) var(--s5); margin-bottom:var(--s4); }
 .card h2 { font-size:13.5px; color:var(--text); font-weight:600; letter-spacing:0;
   margin:var(--s1) 0 var(--s2); text-transform:none; }
 /* A card's opening line explains the card; what follows it is the card's
    contents, and they are not the same thing */
 .card > h2 + .hint { margin-bottom:var(--s4); }
 .card > .hint + .rows, .card > .hint + .field, .card > .hint + label.check { margin-top:0; }
 /* The colours a project can be given. Squares rather than a list of names:
    the thing being chosen is the colour itself */
 .swatches { display:flex; flex-wrap:wrap; gap:var(--s2); align-items:center; padding:4px 0 2px; }
 .swatches i { width:22px; height:22px; border-radius:var(--r-ctl); cursor:pointer; display:block;
   border:1px solid #0004; }
 .swatches i.on { outline:2px solid var(--text); outline-offset:2px; }
 .swatches i.any { background:conic-gradient(red,yellow,lime,aqua,blue,magenta,red); }
 .swatches input[type="color"] { position:absolute; width:0; height:0; opacity:0; padding:0;
   border:0; }
 .row { display:flex; align-items:center; gap:var(--s2) var(--s3);
   padding:var(--s2) 0; flex-wrap:wrap; }
 /* The label names the line, so it takes the line. A checkbox's own label and
    one deliberately placed beside a field (a port after a host) stay put */
 .row > label:not(.check):not(.beside) { flex:0 0 100%; width:auto;
   color:var(--text); font-size:12px; font-weight:500; line-height:1.4; }
 /* ...and what a field means goes under it, not out to one side */
 .row > .hint { flex-basis:100%; margin-top:-2px; }
 /* When a thing in a list was last heard from. Kept whole: squeezed beside a
    name on a phone it broke a few letters to a line, five lines tall */
 .listrow .when { color:var(--muted); font-size:12px; flex:1; white-space:nowrap; }
 /* A device that holds this board's key: its name, when, and the way to take
    the key away */
 .devrow .devname { flex:0 1 180px; }
 /* A row of a table: the same question, asked many times over. The name keeps
    its own column so the eye can run down it, and nothing wraps */
 .row.pair { padding:var(--s1) 0; }
 /* A key that works from any program: three held keys and the key */
 .hkpick { display:flex; align-items:center; gap:var(--s1); flex:none; }
 .hkpick .tog, .qseg .tog { height:32px; min-width:52px; padding:0 var(--s2); font-size:12.5px; border:1px solid var(--edge);
   border-radius:var(--r-ctl); background:var(--panel2); color:var(--dim); cursor:pointer; }
 /* Held: marked by its rim, not filled -- the page's one filled button is Save */
 .hkpick .tog.on, .qseg .tog.on { border-color:var(--brand); color:var(--text); box-shadow:inset 0 0 0 1px var(--brand); font-weight:600; }
 /* ── Quick commands: the grid being edited ──
    The button's face is shared with the launcher (quick.rs, poured in just
    below); this is the frame it sits in here. Places are drawn even when empty, because an
    empty place is somewhere to put something */
{{QUICK_CSS}}
 .qseg { display:flex; gap:var(--s1); flex-wrap:wrap; }
 .qsize { max-width:420px; }
 .qsize > .field { flex:1 1 140px; margin-top:var(--s4); }
 .qsizehint { margin-top:var(--s2); }
 .qcrumbs { display:flex; align-items:center; flex-wrap:wrap; gap:var(--s1); margin-top:var(--s5); }
 .qcrumbs button:last-child { color:var(--text); font-weight:600; }
 .qcrumbs .qsep { color:var(--faint); }
 .qegrid { display:grid; gap:var(--s2); margin-top:var(--s2); user-select:none; -webkit-user-select:none; }
 .qslot { width:var(--qs, 88px); height:var(--qs, 88px); overflow:hidden; position:relative; border-radius:var(--r-card);
   border:1px solid var(--edge); background:var(--panel2); cursor:pointer; outline:none;
   -webkit-touch-callout:none; }
 .qslot:hover { border-color:var(--edge-hi); }
 /* "vacant", not "empty": .empty is this page's empty-list message */
 .qslot.vacant { background:transparent; border:1px dashed var(--line); }
 .qslot.vacant:hover { border-color:var(--edge); }
 .qslot.back { background:transparent; border-color:var(--line); }
 /* Picked, or where the keyboard is: the brand rim and its ring (5.1) */
 .qslot.sel, .qslot:focus-visible { border-color:var(--brand);
   box-shadow:0 0 0 3px color-mix(in srgb, var(--brand) 22%, transparent); }
 /* Where a carried button would land */
 .qslot.over, .qpager button.over { border-color:var(--brand); border-style:solid;
   background:color-mix(in srgb, var(--brand) 12%, transparent); }
 .qslot.carried { opacity:.35; }
 .qegrid.tiny .qface .ql, .qegrid.tiny .qface .qk { display:none; }
 .qegrid.tiny .qface { padding:var(--s1); }
 .qegrid.tiny .qface svg { width:60%; height:60%; }
 /* A button with no picture has only its name to be told apart by, so it keeps it, smaller */
 .qegrid.tiny .qface.bare .ql { display:-webkit-box; font-size:10px; -webkit-line-clamp:2; }
 .qghost { position:fixed; z-index:70; width:72px; height:72px; margin:-36px 0 0 -36px;
   pointer-events:none; border-radius:var(--r-card); border:1px solid var(--brand);
   background:var(--panel); box-shadow:0 8px 24px #0007; }
 .qpager { display:flex; align-items:center; justify-content:center; flex-wrap:wrap; gap:var(--s1);
   margin-top:var(--s3); }
 .qpager .qpage { min-width:32px; padding:0 var(--s2); font-variant-numeric:tabular-nums; color:var(--dim); }
 .qpager .qpage.on { color:var(--text); border-color:var(--brand); box-shadow:inset 0 0 0 1px var(--brand); }
 .qpanelhint { margin-top:var(--s4); text-align:center; }
 .qpanel { margin-top:var(--s5); padding-top:var(--s4); border-top:1px solid var(--line); }
 .qpanelhead { font-size:12px; font-weight:500; color:var(--text); }
 .qmake { margin:var(--s2) 0; }
 .qpanelrow { display:flex; align-items:flex-start; gap:var(--s5); flex-wrap:wrap; }
 .qpreview { flex:none; width:88px; height:88px; border-radius:var(--r-card); border:1px solid var(--edge);
   background:var(--panel2); }
 .qfields { flex:1 1 280px; min-width:0; }
 .qfields > .field:first-child { margin-top:0; }
 .qfields textarea.qbody { min-height:96px; }
 /* A few sentences rather than a document: the height of the quick
    command's body above */
 textarea.short { min-height:96px; }
 /* A reason under a row stands on a line of its own */
 .row > .site-warn { flex-basis:100%; }
 .qcount { align-self:center; color:var(--dim); font-size:13px; font-variant-numeric:tabular-nums; }
 .qdelrow { margin-top:var(--s5); }
 .fmenuitem.bad { color:var(--stop); }
 /* The picture picker: a search, then the pictures */
 .qpicker { width:min(640px, 100%); }
 .qpicker .mbody > input { width:100%; }
 .qicons { display:grid; grid-template-columns:repeat(auto-fill, minmax(44px, 1fr)); gap:var(--s1);
   max-height:min(52vh, 440px); overflow:auto; margin-top:var(--s3); align-content:start; }
 .qicons .qicon { height:44px; padding:0; display:flex; align-items:center; justify-content:center;
   background:var(--bg); color:var(--text); }
 .qicons .qicon svg { width:22px; height:22px; fill:none; stroke:currentColor; stroke-width:2;
   stroke-linecap:round; stroke-linejoin:round; }
 .qicons .qicon.on { border-color:var(--brand); box-shadow:inset 0 0 0 1px var(--brand); }
 .qicons .qiconhead { grid-column:1 / -1; font-size:11.5px; color:var(--dim); margin-top:var(--s2); }
 .qicons .qiconhead:first-child { margin-top:0; }
 .qicons .qmore { grid-column:1 / -1; }
 @media (max-width:760px) { .qpanelrow { gap:var(--s3); } .qpreview { width:64px; height:64px; } }
 .hkpick select.hkkey { height:32px; min-width:92px; margin-left:var(--s1); }
 .hkstate.warnline { color:var(--warn); }
 @media (max-width:700px) { .row.pair.hkrow { flex-wrap:wrap; } .row.pair.hkrow > label { flex-basis:100% !important; } }
 .row.pair > label:not(.check):not(.beside) { flex:0 0 210px; align-self:center;
   color:var(--dim); font-weight:400; }
 .row.pair > .hint { flex:0 0 auto; margin-top:0; }
 /* Automation permissions. Two narrow columns on the right, everything else
    on the left, so the eye runs down a column instead of hunting across a row */
 .grantcols { display:flex; align-items:flex-end; gap:0; justify-content:flex-end;
   position:sticky; top:0; background:var(--panel); padding:6px 0 4px; z-index:1; }
 .grantcols span { width:104px; text-align:center; color:var(--muted); font-size:11.5px; }
 .grantcols .grow { width:auto; flex:1; }
 .granthead { display:flex; align-items:center; gap:var(--s2); padding:10px 0 4px;
   border-top:1px solid var(--line); margin-top:4px; }
 .granthead b { font-size:12.5px; font-weight:600; }
 /* Who counts as what, and the one case where the answer surprises people */
 .grantwho { font-size:12.5px; line-height:1.65; margin:var(--s1) 0 var(--s3); }
 .grantwho div + div { margin-top:4px; }
 .grantwarn { font-size:12.5px; line-height:1.65; margin:0 0 var(--s3); padding:8px 12px;
   border-left:3px solid var(--danger); background:var(--panel2); border-radius:0 6px 6px 0; }
 .granthead .foldable { cursor:pointer; display:flex; align-items:center; gap:var(--s2); }
 .granthead .caret { font-size:11px; width:14px; text-align:center; color:var(--muted); }
 .grantrow { display:flex; align-items:center; gap:var(--s2); padding:4px 0; }
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
 .row > label.beside { color:var(--muted); font-size:12px; flex:none; }
 /* A second (or third) label inside one row — "port", "user" next to a host.
    It names the field that follows it, so it sits tight against it rather than
    claiming the row's label column. */
 .row > label.beside { width:auto; }
 .hint { color:var(--faint); font-size:11.5px; }
 /* A hint that is good news rather than an instruction */
 .hint.ok { color:var(--accent); }
 /* The line a tab will really be launched with. It wraps rather than scrolls:
    an argument pushed off the right edge is exactly the argument nobody would
    have seen otherwise */
 .realcmd { margin:var(--s2) 0 var(--s1) 0; }
 .realcmd code { display:block; margin:var(--s1) 0; padding:7px 9px; border-radius:var(--r-chip);
   background:var(--raise); border:1px solid var(--line);
   white-space:pre-wrap; word-break:break-all; font-size:12px; }
 .realcmd .added { color:var(--brand); font-weight:600; }
 /* One row per entry in an editable list (quick actions, providers, notify
    targets, secrets): its name, its fields, then its buttons, divided by a
    hairline. It wraps, so a narrow screen stacks the parts instead of pushing
    them off the edge — which is also why this is a class and not four copies
    of the same inline style. */
 .listrow { display:flex; align-items:center; flex-wrap:wrap; gap:var(--s3);
   padding:7px 0; border-bottom:1px solid var(--line); }
 /* For entries whose fields are taller than their buttons (a quick action's
    body box), so the buttons sit at the top rather than floating mid-height. */
 .listrow.tall { align-items:flex-start; gap:var(--s2); padding:8px 0; }
 /* A list of things, boxed. The border round the whole makes it one object
    instead of a stack of loose lines */
 .rows { border:1px solid var(--line); border-radius:var(--r-ctl); overflow:hidden; }
 .rows > * { border-bottom:1px solid var(--line); }
 .rows > *:last-child { border-bottom:0; }
 /* The lines of a list that is not pressed as a whole -- an ignore file's lines,
    each with its own controls -- keep off the box's edge the same distance */
 #project-bring .rows > *, #project-extra .rows > * { padding-left:var(--s3); padding-right:var(--s3); }
 /* An ignore file, in the groups its own comments make. A comment is the name
    of the lines under it, so it is set as a name above their box rather than
    as one more line inside it: read as a line, every comment was a gap in the
    list. More comment lines are the description under that name */
 .igbox { display:flex; flex-direction:column; gap:var(--s4); }
 .iggroups { display:flex; flex-direction:column; gap:var(--s5); }
 .iggroup { display:flex; flex-direction:column; gap:var(--s2); }
 .ighead { display:flex; flex-direction:column; gap:var(--s1); }
 .igname { font-size:12px; font-weight:500; color:var(--text); }
 /* One line of the file, and what hangs off it, as one item. The parts are
    columns, so the choices stand in one line down the box whatever each line
    matches */
 #project-bring .rows > .igitem { padding-top:var(--s2); padding-bottom:var(--s2); }
 .igrow { display:grid; grid-template-columns:minmax(0,1fr) minmax(0,1fr) 132px 32px;
   align-items:center; gap:var(--s3); }
 .igpat { display:flex; flex-direction:column; min-width:0; }
 .igpat > .mono { color:var(--text); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
 .igmatch { justify-self:end; min-width:0; max-width:100%; }
 .igmatch > button { max-width:100%; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
 .igrow > select { width:100%; }
 .igneg > .hint { grid-column:2 / 4; }
 .ignote { display:flex; align-items:center; flex-wrap:wrap; gap:var(--s2); margin-top:var(--s1); }
 .igbox .caution { color:var(--warn); }
 .igpaths { white-space:pre-wrap; margin-top:var(--s1); }
 /* One secret. Reads across on a window, and stacks into a card on a phone. */
 .secretrow { cursor:pointer; padding:10px var(--s3); gap:var(--s3); }
 .secretrow:hover { background:var(--panel2); }
 .secretrow .go { color:var(--faint); font-size:14px; line-height:1; }
 .secretrow:hover .go { color:var(--text); }
 .secretname { flex:0 0 132px; color:var(--text); overflow:hidden;
   text-overflow:ellipsis; white-space:nowrap; }
 /* A named server: the square of its colour in front of its name, on one line */
 .secretname.markname { display:flex; align-items:center; gap:var(--s2); }
 .secretname.markname > span:last-child { min-width:0; overflow:hidden; text-overflow:ellipsis; }
 /* The facts after the name are columns, so a list of them can be read down
    rather than across: what it is, where it goes, and that a value is held */
 .secretdesc { flex:1 1 140px; overflow:hidden; text-overflow:ellipsis;
   white-space:nowrap; }
 .secretsite { flex:0 0 auto; text-align:right; font-variant-numeric:tabular-nums; }
 /* Somewhere the connection is not protected -- the one thing on this row
    worth catching from across the room */
 .secretsite.plain { color:var(--danger); }
 .secretdots { min-width:44px; text-align:right; }
 .chip { font-size:11px; line-height:18px; height:18px; padding:0 7px;
   color:var(--dim); background:var(--panel2); border:1px solid var(--edge);
   border-radius:var(--r-chip); white-space:nowrap; }
 /* Nothing may use it yet: somebody has to say who before it does anything */
 /* What is always added after a prompt, read-only */
 .promptshape { white-space:pre-wrap; font-size:12px; color:var(--dim); background:var(--sunk, var(--panel2));
   border:1px solid var(--line); border-radius:var(--r-ctl); padding:var(--s2) var(--s3); margin:var(--s2) 0 var(--s4); }
 /* The words a prompt can use, pressed to put one in */
 .promptvars { gap:var(--s2); margin-top:var(--s2); flex-wrap:wrap; align-items:center; }
 /* The label stays at the start of the row, not a line of its own */
 .promptvars > .hint { flex-basis:auto; margin-top:0; }
 .promptvars .chip { cursor:pointer; min-height:0; }
 .promptvars .chip:hover { color:var(--text); border-color:var(--edge-hi); }
 .chip.none { color:var(--warn); border-color:color-mix(in srgb, var(--warn) 45%, transparent);
   background:color-mix(in srgb, var(--warn) 12%, transparent); }
 /* One thing to fill in: its name above it, what it does under it. */
 .field { display:flex; flex-direction:column; gap:var(--s2); margin-top:var(--s5); }
 .field > label { font-size:12px; font-weight:500; color:var(--text); }
 /* The line under a control is the smallest thing on the page, so the label
    above it reads as the name of the pair rather than more of the same */
 .field > .hint { font-size:11px; margin-top:-1px; }
 .field > input, .field > select { width:100%; }
 /* A control and, under it, what is wrong with what was typed */
 .field > .fieldctl { display:flex; flex-direction:column; gap:var(--s2); }
 .field > .fieldctl > input, .field > .fieldctl > select { width:100%; }
 .field > .fieldctl > input.narrow { width:140px; }
 /* Two things that are one question: an address and its port, a box and the
    button that files it. They wrap rather than shrink, so neither becomes
    unusable at a phone's width */
 .row2 { display:flex; gap:var(--s3); align-items:flex-start; flex-wrap:wrap; }
 .row2 > .field { flex:1 1 200px; margin-top:0; }
 .row2 > .field:has(> .fieldctl > input.narrow) { flex:0 0 auto; }
 .row2 > input { flex:1 1 200px; min-width:0; }
 .row2 > button { flex:0 0 auto; }
 /* One of two ways of doing the same thing. Side by side, because they are a
    choice and not a list of two settings */
 .segrow { display:flex; gap:var(--s5); align-items:center; flex-wrap:wrap; }
 .segrow > label.check { margin:0; }
 /* Fields that exist because a box above them is ticked. Set in from the tick,
    with its line carried down, so they read as part of that one question */
 .under-check { padding-left:var(--s5); margin-left:7px;
   border-left:1px solid var(--line); }
 .under-check[hidden] { display:none; }
 /* The three a connection will not be made without */
 .must { color:var(--warn); font-style:normal; font-weight:600; }
 /* A heading inside a folded section: several unrelated awkward cases live
    there and each has to say which one it is */
 .subhead { font-size:11.5px; font-weight:600; letter-spacing:.02em; color:var(--dim);
   margin-top:var(--s5); padding-bottom:var(--s2); border-bottom:1px solid var(--line); }
 .subhead:first-of-type { margin-top:var(--s4); }
 /* What a tick above it means, in the smallest type on the page */
 .note { font-size:11.5px; color:var(--faint); line-height:1.5; margin-top:var(--s2);
   padding-left:22px; }
 /* Proving the connection works, under a line of its own: it is the last thing
    done on this card and it belongs to the whole of it, not to the field above */
 .connfoot { display:flex; align-items:center; gap:var(--s3); flex-wrap:wrap;
   margin-top:var(--s5); padding-top:var(--s3); border-top:1px solid var(--line); }
 /* A thumb needs more than a glyph. */
 .hit { min-width:34px; min-height:34px; }
 /* The warning about a plain connection, and the tick that takes it on.
    A class rather than an inline style, so `hidden` still hides it */
 /* Two answers to one question, side by side. Not a `.row`, whose first
    label is the 150px name column every settings line starts with */
 .whorow { display:flex; align-items:center; gap:var(--s6); flex-wrap:wrap; padding:2px 0 4px; }
 .riskrow { display:flex; flex-direction:column; gap:var(--s2); }
 /* What this field is allowed to hold, said before anything is typed in it */
 label.check.allow { font-size:12px; color:var(--dim); margin-bottom:var(--s3); }
 /* ...and the row that needs it says so, under itself */
 .site-warn { display:flex; gap:var(--s2); margin:var(--s1) 0 var(--s2); padding:var(--s2) var(--s3);
   border-radius:var(--r-ctl); font-size:11.5px; line-height:1.5; color:var(--warn);
   border:1px solid color-mix(in srgb, var(--warn) 35%, transparent);
   background:color-mix(in srgb, var(--warn) 9%, transparent); }
 /* One fact's state, as a dot. Only the state colours go in it */
 .dot { width:8px; height:8px; border-radius:50%; background:var(--dim); flex:none; }
 .dot.on { background:var(--live); }
 .dot.off { background:var(--warn); }
 .site-row { display:flex; gap:var(--s2); }
 .site-row input.bad { border-color:var(--warn); }
 /* Where the answer points. Long enough to find, short enough not to nag */
 @keyframes lookhere {
   0%   { box-shadow:0 0 0 0 color-mix(in srgb, var(--warn) 55%, transparent); }
   100% { box-shadow:0 0 0 6px transparent; }
 }
 .lookhere { animation:lookhere .9s ease-out 2; border-radius:var(--r-ctl); }
 /* Why the save is held. Its own line above the buttons, and it stays */
 .why { flex:1 1 100%; order:-1; font-size:11.5px; color:var(--warn);
   line-height:1.5; padding-bottom:var(--s1); }
 .mfoot { flex-wrap:wrap; }
 .mfoot button { flex:none; white-space:nowrap; }
 /* Off is grey, not a pale version of the live colour: a washed-out blue still
    reads as blue. Held is the same grey, and still takes the press so it can
    say why -- a button that greys out and then ignores you is a dead end */
 button.primary:disabled, button.primary.held {
   background:var(--panel2); border-color:var(--line); color:var(--faint);
   cursor:not-allowed; filter:none; }
 button.primary:disabled:hover, button.primary.held:hover {
   background:var(--panel2); border-color:var(--line); filter:none; }
 .lblopt { color:var(--faint); font-weight:400; }
 .grow { flex:1; min-width:180px; }
 .stoprow { display:flex; align-items:center; gap:var(--s2); flex-wrap:wrap; padding:8px;
   margin:var(--s2) 0; border:1px solid var(--line); border-radius:var(--r-ctl); }
 .stoprow input { width:auto; }
 .stoprow input[type=number] { width:80px; }
 .stoprow .arrow { color:var(--muted); }
 #wsstopslist select { width:auto; }

 /* Which network the phone's connection link leads to. The tone names are its
    own (not the page-wide .warn, which is a paragraph of danger text) so that
    a badge stays a badge whatever else those words come to mean. */
 .netbadge { display:inline-flex; align-items:center; gap:var(--s2); font-size:12px; font-weight:600;
   line-height:1.5; white-space:nowrap; border-radius:999px; padding:2px 10px; border:1px solid; }
 .netbadge.ok   { color:var(--live);   border-color:var(--live);
   background:color-mix(in srgb, var(--live) 14%, transparent); }
 .netbadge.care { color:var(--warn);   border-color:var(--warn);
   background:color-mix(in srgb, var(--warn) 14%, transparent); }
 .netbadge.risk { color:var(--danger); border-color:var(--danger);
   background:color-mix(in srgb, var(--danger) 14%, transparent); }
 .netbadge.mute { color:var(--muted);  border-color:var(--line); }

 /* A field is sunk into the surface it sits on: the page's own ground under
    a card, which is what tells the eye where a thing can be typed. Reading it
    off the card colour instead left the form with no edges at all */
 input[type=text], input[type=number], input[type=password], select {
   height:36px; padding:0 var(--s3); }
 .picked, .picked:focus { border-color: var(--pick) !important;
   box-shadow: 0 0 0 3px color-mix(in srgb, var(--pick) 22%, transparent) !important; }
 input[type=text], input[type=number], input[type=password], select, textarea {
   background:var(--bg); color:var(--text); border:1px solid var(--edge);
   border-radius:var(--r-ctl); font-size:13px; font-family:inherit; outline:none; }
 textarea { padding:var(--s2) var(--s3); }
 input:hover:not(:disabled), select:hover, textarea:hover { border-color:var(--edge-hi); }
 /* Where the keyboard is going. A ring rather than a heavier border, so the
    box does not change size as it is stepped through */
 input:focus, select:focus, textarea:focus { border-color:var(--accent);
   box-shadow:0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent); }
 input:disabled, select:disabled { color:var(--dim); background:var(--panel2);
   cursor:not-allowed; }
 input[type=text]::placeholder, textarea::placeholder {
   color:color-mix(in srgb, var(--muted) 72%, var(--bg)); }
 input[type=checkbox] { width:16px; height:16px; accent-color:var(--accent); margin:0; }
 label.check { display:flex; align-items:center; gap:var(--s2); width:auto; color:var(--text);
   font-size:14px; cursor:pointer; }
 textarea { width:100%; min-height:220px; line-height:1.55; resize:vertical; }

 /* One height keeps a row of buttons tidy. A minimum rather than a fixed one,
    because a few buttons carry two lines (a nav item, a wizard's choice) and a
    fixed height crushes them */
 button { font-family:inherit; font-size:12.5px; font-weight:500; min-height:32px;
   border-radius:var(--r-ctl); cursor:pointer; padding:0 var(--s3);
   border:1px solid var(--edge); background:var(--panel2); color:var(--text); }
 button:hover { background:var(--raise); border-color:var(--edge-hi); }
 /* The one that finishes the job. Filled, because there is one of it */
 button.primary { background:var(--accent); border-color:var(--accent);
   color:var(--bg); font-weight:600; }
 button.primary:hover { filter:brightness(1.08); }
 button.primary:disabled { opacity:.45; cursor:not-allowed; filter:none; }
 button.quiet { background:none; border-color:transparent; color:var(--muted); }
 button.quiet:hover { color:var(--text); background:var(--panel2); border-color:transparent; }
 /* A glyph on its own is square, and big enough for a thumb */
 button.icon { width:32px; padding:0; display:inline-flex; align-items:center;
   justify-content:center; }
 a.quiet { font-size:13px; border-radius:var(--r-ctl); padding:6px 8px; color:var(--muted); text-decoration:none; align-self:center; white-space:nowrap; }
 a.quiet:hover { color:var(--text); background:var(--panel2); }
 button.danger { color:var(--danger); background:none; border-color:transparent; }
 button.danger:hover { background:color-mix(in srgb, var(--danger) 12%, transparent); }

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

 .modal { position:fixed; inset:0; background:rgba(0,0,0,.6); display:flex;
   align-items:flex-start; justify-content:center; overflow:auto;
   padding:56px var(--s4); z-index:20; }
 .modal-inner { background:var(--panel); border:1px solid var(--line);
   border-radius:var(--r-card); width:min(560px,100%); box-shadow:0 8px 24px #0007;
   padding:var(--s5) var(--s6) var(--s6); }
 /* A dialog built as header / body / footer keeps its title and its way out
    in the same place whatever is between them */
 .modal-inner.framed { padding:0; }
 dialog.confirm-box { color:var(--text); margin:56px auto;
   width:min(560px, calc(100% - 2 * var(--s4))); max-height:calc(100% - 2 * var(--s7)); overflow:auto; }
 dialog.confirm-box::backdrop { background:rgba(0,0,0,.6); }
 .confirm-box .mbody { white-space:pre-wrap; overflow-wrap:anywhere; }
 .confirm-box > .mfoot { justify-content:flex-end; }
 .framed > .mhead { display:flex; align-items:center; gap:var(--s3);
   padding:var(--s4) var(--s5); border-bottom:1px solid var(--line); }
 .framed > .mhead h2 { flex:1; margin:0; font-size:13.5px; font-weight:600; }
 .framed > .mbody { padding:var(--s5); }
 .framed > .mfoot { display:flex; align-items:center; gap:var(--s2);
   padding:var(--s3) var(--s5); border-top:1px solid var(--line); }
 .framed > .mfoot .grow { flex:1; }
 .modal-inner h2 { text-transform:none; font-size:15px; color:var(--text); margin:0 0 var(--s1); }
 /* A dialog that keeps something below its frame -- the red way out, which is
    outside every other pane's last card for the same reason */
 .modal-stack { display:flex; flex-direction:column; gap:var(--s3); max-height:92vh; }
 .modal-stack > .modal-inner { min-height:0; }
 /* Its title and the one button that finishes it, on the same line */
 .modalhead { display:flex; align-items:center; gap:var(--s3); padding-bottom:var(--s4);
   border-bottom:1px solid var(--line); }
 .modalhead h2 { flex:1; margin:0; }
 /* The folder list walked on the page (walkPath) */
 .walkat { font-size:12px; color:var(--muted); margin:var(--s2) 0; overflow-wrap:anywhere; }
 .walkerr { color:var(--danger); font-size:12px; white-space:pre-wrap; }
 .walklist { display:flex; flex-direction:column; gap:var(--s1); max-height:52vh; overflow:auto; }
 .walkrow { display:flex; gap:var(--s2); align-items:center; padding:9px 10px; border-radius:var(--r-ctl); cursor:pointer; }
 .walkrow:hover { background:var(--panel2); }
 .walkmark { width:1.4em; text-align:center; flex:none; }
 .walknm { overflow-wrap:anywhere; }
 /* The exact character a parser stopped at, inside an excerpt. */
 pre .at { background:var(--danger); color:#fff; border-radius:var(--r-chip); padding:0 1px; }
 pre { background:var(--panel2); border:1px solid var(--line); border-radius:var(--r-ctl); padding:12px;
   overflow:auto; max-height:240px; font-size:12.5px; }
 a { color:var(--accent); }
 /* AI generation can take tens of seconds. Line up a spinner, a growing bar, and a
    progressing number so it's obvious at a glance that it hasn't stalled */
 #aibusy { flex-direction:column; gap:9px; margin-top:10px; padding:12px 14px;
   background:var(--panel2); border:1px solid var(--accent); border-radius:var(--r-ctl); }
 #aibusy .head { display:flex; align-items:center; gap:var(--s3); }
 #aibusytext { color:var(--accent); font-weight:600; }
 .spin { width:16px; height:16px; flex:none; border-radius:50%;
   border:2px solid var(--line); border-top-color:var(--accent);
   animation:spin .8s linear infinite; }
 @keyframes spin { to { transform:rotate(360deg); } }
 .bar { height:4px; border-radius:3px; background:var(--line); overflow:hidden; }
 .bar > i { display:block; height:100%; width:35%; border-radius:3px;
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
   header { padding:10px 12px; gap:var(--s3); flex-wrap:nowrap; }
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
   .headlinks { flex-direction:column; align-items:stretch; gap:var(--s1);
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
   .row { flex-wrap:wrap; gap:var(--s2) var(--s3); }
   /* A fetch under way: the bar is the number, the line under it the words */
   .ubar { height:6px; border-radius:3px; background:var(--line); overflow:hidden; margin:var(--s1) 0 var(--s1); }
   .ubar i { display:block; height:100%; background:var(--live); transition:width .3s; }
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
   /* No ceiling on the height: the sheet around it is what scrolls (.modal),
      and a box cut off at 92vh drew its last fields outside its own background */
   .modal-inner { width:96vw; padding:16px 14px; }
   /* A row of facts becomes a small card: the name on its own line, the rest
      under it, and the way in still a whole-row press. */
   .secretrow { align-items:flex-start; padding:10px 0; row-gap:var(--s1); }
   /* An ignore line: its name and its choice on the first line, what it
      matches under them */
   .igrow { grid-template-columns:minmax(0,1fr) 132px 32px; row-gap:var(--s1); }
   .igmatch { grid-column:1 / -1; grid-row:2; justify-self:start; }
   /* Its words start under the name; the button's padding hangs out to the left */
   .igmatch > button { margin-left:calc(-1 * var(--s3)); }
   .igneg > .hint { grid-column:1 / -1; grid-row:2; }
   .igneg > :last-child { grid-column:3; grid-row:1; }
   .secretname { flex-basis:100%; font-size:13px; }
   .secretdesc { flex:1 1 auto; }
   /* The last line: where it may go, and the way in at the end of it */
   .secretsite { flex:1 1 auto; text-align:left; min-width:0; }
   .secretdots { display:none; }
   .secretedit { margin-left:auto; }
   /* A device on a phone: the name has the line to itself, and when it was
      last here sits under it beside the way to take its key away */
   .devrow { row-gap:var(--s1); }
   .devrow .devname { flex:1 1 100%; }
 }
 /* Never let the page itself scroll sideways, whatever a stray wide child does. */
 @media (max-width: 760px) { body { overflow-x:hidden; } }
 /* The board's +: only what the new tab runs, in a dialog of its own (style
    guide 5.2). In the window this page IS the dialog -- it is placed in a
    rectangle over the board -- so the page wears the dialog's parts and the
    rest of the settings stay out of sight until "More settings" lets them in */
 #floatbox { display:none; }
 body.float { background:var(--panel); overflow:hidden; }
 body.float > header, body.float > .layout, body.float > #navscrim { display:none; }
 body.float #floatbox { display:flex; flex-direction:column; height:100vh;
   border:1px solid var(--line); box-sizing:border-box; }
 /* Framed over a board (?embed=1) the edge belongs to the frame, which draws it
    and cuts this page to its corners -- a second edge inside would sit just
    within the first and read as a box inside a box */
 body.embed.float #floatbox { border:0; }
 #floatbox .fhead { display:flex; align-items:center; gap:var(--s3); padding:16px 20px;
   border-bottom:1px solid var(--line); }
 #floatbox .fhead h1 { font-size:13.5px; font-weight:600; margin:0; }
 #floatbox .fhead .hint { flex:1; min-width:0; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
 #floatbox .fbody { flex:1; min-height:0; overflow:auto; padding:20px; }
 /* The card is the dialog's body here, not a card on a page */
 #floatbox .fbody > .card { border:0; background:none; padding:0; margin:0; }
 #floatbox .fbody > .card > h2 { display:none; }
 #floatbox .ffoot { display:flex; align-items:center; gap:var(--s3); padding:12px 20px;
   border-top:1px solid var(--line); }
 #floatbox .ffoot .spacer { flex:1; }
</style></head><body>

<div id="floatbox" role="dialog" aria-modal="true" aria-labelledby="floattitle">
  <div class="fhead">
    <h1 id="floattitle">{{tui.tab.add}}</h1>
    <span class="hint" id="floatwhere"></span>
    <button class="quiet" onclick="floatCancel()" title="{{common.close}}" aria-label="{{common.close}}">✕</button>
  </div>
  <div class="fbody" id="floatbody"></div>
  <div class="ffoot">
    <button class="quiet" onclick="floatMore()">{{settings.float.more}}</button>
    <div class="spacer"></div>
    <button class="quiet" onclick="floatCancel()">{{common.cancel}}</button>
    <button class="primary" id="floatadd" onclick="floatAdd()">{{common.add}}</button>
  </div>
</div>

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
// True when this page is not a screen of its own but a dialog: a frame the
// board placed over itself (?embed=1), which is how a browser puts a page
// over the board the way the window places one. The way out is a word to the
// board rather than a walk to "/" -- that would load the board into the frame
const EMBED = new URLSearchParams(location.search).get("embed") === "1";
// True when this page is standing over the board as a sheet (?sheet=1): one
// thing's settings, asked for from the board and put away back to it. It is a
// dialog one size up, so Escape is its way out (style guide 5.2) -- and this
// page is what hears that key, wherever it stands: in the window the board
// behind has no keyboard, and in a frame the key never leaves the frame
const SHEET = new URLSearchParams(location.search).get("sheet") === "1";
const toBoard = act => { try { window.parent.postMessage({cfg:act}, location.origin); } catch (e) {} };
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
// The name this desk's GitHub token is filed under. Poured in from the one
// place that decides it, so the card that offers to set it and the program that
// reaches for it cannot drift apart
const THIS_PC = __THISPC__;
// The short words a new tab's automation name is drawn from. Poured in from the
// app's own word list, the one branch names come from, so there is one list
const PET_NOUNS = __PETNOUNS__;
// A list of branch names as it is typed and as it is stored. Space or comma
// between them, because both are what people reach for
const protectList = text => (text || "").split(/[\s,]+/).filter(Boolean);
const protectText = list => (list || []).join(" ");
// The list a folder inherits when it has said nothing of its own: its
// desk's, or the built-in names for a desk that has said nothing either. The
// same order the program settles at launch, so the greyed-out box on a folder's
// page shows the names that will actually guard it
const protectOf = desk => {
  const g = (desk && desk.git) || {};
  return Array.isArray(g.protect) ? g.protect : PROTECT_DEFAULT;
};
// {name} substitution (same rule as tp on the Rust side)
const fill = (s, args) => Object.entries(args)
  .reduce((acc, [k, v]) => acc.replaceAll("{" + k + "}", v), s || "");
const api = (m, b) => fetch("/api/config", {
   method: m, headers: {"X-Token": TOKEN, "Content-Type":"application/json"}, body: b });
const deskApi = (m, file, b) => fetch("/api/desk?file=" + encodeURIComponent(file), {
   method: m, headers: {"X-Token": TOKEN, "Content-Type":"application/json"}, body: b });

// A plain object: what a desk's maps are written as. A list or null in their
// place is somebody else's shape, read as nothing
const isObj = v => !!v && typeof v === "object" && !Array.isArray(v);
// The colours a project or a server is offered (uistate::PALETTE)
const MARK_COLOURS = {{MARK_COLOURS}};
let current = {};        // Contents of config.json (holds the base settings)
let desks = [];            // Desks and tabs
let sel = {desk:0, tab:null, global:true, section:"basic"};
// Whether a desk's own settings are what is open. They are what a desk
// with neither a folder nor a tab picked shows
const inDeskPlace = () => !sel.global && sel.tab == null && (sel.grp ?? null) === null
  && (sel.proj ?? null) === null;
// Leave a settings place for the tree of a desk: its first folder, the thing
// the tree is for. A desk with no folder has nothing else to show but its
// settings, and stays there
function toTree(di) {
  sel = {desk:di, grp:null, tab:null, global:false, dsection:"basic"};
}
// The entry for the page on screen, brought into view inside the list -- which
// scrolls on its own -- and the page itself shown from its top. Scrolling the
// entry into view the browser's way moved the whole window with it, so a page
// opened from the board started halfway down, below the heading that says
// which page it is
function showSelected(block) {
  const nav = document.getElementById("nav");
  const cur = nav && nav.querySelector(".navitem.sel");
  if (cur) {
    const r = cur.getBoundingClientRect(), n = nav.getBoundingClientRect();
    if (block === "center") nav.scrollTop += (r.top - n.top) - (n.height - r.height) / 2;
    else if (r.top < n.top) nav.scrollTop += r.top - n.top;
    else if (r.bottom > n.bottom) nav.scrollTop += r.bottom - n.bottom;
  }
  window.scrollTo(0, 0);
}
// Bring one card on the page into view and mark it for a moment. The card
// may only be drawn once an answer it waits on arrives, so it is looked for
// for a while rather than once
function lookAtCard(id, tries) {
  const at = document.getElementById(id);
  if (!at) {
    if (tries > 0) setTimeout(() => lookAtCard(id, tries - 1), 100);
    return;
  }
  at.scrollIntoView({block:"center"});
  at.classList.remove("lookhere"); void at.offsetWidth; at.classList.add("lookhere");
}
// One of a desk's settings on screen, by id
function goDeskSection(id, block) {
  sel = {desk:sel.desk, grp:null, tab:null, global:false, dsection:id};
  render();
  showSelected(block);
}
// Put one global card on screen, by id, with its entry in the list in view.
function goSection(id, block) {
  sel = {desk:sel.desk, tab:null, global:true, section:id};
  render();
  showSelected(block);
}
// When opened via a deep-link shortcut (?ret=1), returning to the board after a
// successful save is the natural finish, so the caller doesn't have to close it.
let returnOnSave = false;
// The tab being added from the board's +, while the page is only that dialog
// (?float=1). Null on the settings page proper
let floating = null;
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
{{PUSH_JS}}
{{QUICK_JS}}
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
// One thing to fill in: its name above, the control, and under it the line
// that says what it does (style guide 5.1). Every screen builds these the same
// way, so a form on one page cannot come out a different shape from the next
function sfield(label, control, hint) {
  return el("div", {class:"field"},
    el("label", {}, label),
    el("div", {class:"fieldctl"}, control),
    hint ? el("div", {class:"hint"}, hint) : null);
}

// A credential this tab needs, filed under the desk and the tab.
//
// Nothing about it is written into the settings -- not even a reference. The
// name follows from where it is, so it is never typed twice and a tab that is
// copied does not quietly carry somebody's password with it. What the field
// shows is whether one is stored, never what it is
function secretField(t, which, label, hint, describe) {
  const nameOf = () => {
    const desk = desks[sel.desk];
    const w = (desk && (desk.id || "").trim()), tid = (t.id || "").trim();
    return w && tid ? "ssh/" + w + "/" + tid + "/" + which : "";
  };
  return credentialField(nameOf, label, hint, describe, T["settings.secrets.desk_needs_id"]);
}

// A credential kept under a name worked out from something else on the page,
// never written into the settings. `nameOf` answers "" while there is nothing
// yet to work the name out from, and `needs` is what the person is told then
function credentialField(nameOf, label, hint, describe, needs, saved) {
  const input = el("input", {type:"password", placeholder:hint});
  const note = el("span", {class:"hint"});
  const refresh = async () => {
    const k = nameOf();
    if (!k) { note.textContent = needs; return; }
    const j = await fetchSecrets();
    const has = j && (j.secrets || []).some(x => x.key === k);
    note.textContent = has ? T["settings.ssh.password.set"] : "";
  };
  setTimeout(refresh, 0);
  const go = el("button", {onclick: async () => {
    const k = nameOf();
    if (!k) { toast(needs, true); return; }
    if (!input.value) { toast(T["settings.secrets.value_required"], true); return; }
    const r = await saveSecret({key: k, value: input.value, description: describe(),
                                human: true, ai: false, urls: []});
    if (r.ok) { input.value = ""; toast(saved || T["settings.ssh.password.saved"]); refresh(); }
    else toast(r.error || T["settings.secrets.save_failed"], true);
  }}, T["common.save"]);
  const box = el("div", {class:"field"},
    el("label", {}, label),
    el("div", {class:"fieldctl"}, el("div", {class:"row2"}, input, go)),
    el("div", {class:"hint"}, note));
  // Asked again when what the name is worked out from changes
  box.refresh = refresh;
  return box;
}
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
  // The program this tab would start is not on this PC: said here, while the
  // tab is being made, rather than found out when it does not start
  const missing = el("div", {class:"site-warn"});
  missing.hidden = true;
  const box = el("div", {class:"realcmd"},
    el("div", {class:"hint"}, T["settings.tab.command.real"]), line, note, missing);
  let seq = 0, timer = null;
  // The same answer says whether the conversation switch above decides
  // anything, and the AI panel is redrawn on its own -- so the last answer is
  // kept, and a panel that arrives afterwards is told it straight away rather
  // than waiting for the next keystroke
  let carry, watch = null;
  const tell = v => { carry = v; if (watch) watch(v); };
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
    missing.textContent = "";
    missing.hidden = !(r && r.missing);
    if (r && r.missing) {
      const prog = r.missing;
      // Native append writes an absent link as the word "null"
      missing.append(...[el("span", {}, "⚠"),
        el("span", {}, fill(T[prog === "git" ? "settings.tab.missing.git" : "settings.tab.missing"], {cmd: prog}) + " "),
        r.install_url
          ? el("a", {href: REMOTE ? r.install_url : "#", target: REMOTE ? "_blank" : null, rel: "noopener",
              onclick: REMOTE ? null : e => { e.preventDefault();
                fetch("/api/open?dest=install&prog=" + encodeURIComponent(prog), {headers:{"X-Token":TOKEN}}); }},
              T["settings.tab.missing.install"])
          : null].filter(Boolean));
    }
    if (!r || !r.argv || !r.argv.length) { box.hidden = !(r && r.missing); tell(undefined); return; }
    box.hidden = false;
    tell(r.carry || null);
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
  return { box, schedule, onCarry: fn => { watch = fn; fn(carry); } };
}
function row(label, ...kids) { return el("div", {class:"row"}, el("label", {}, label), ...kids); }
function card(title, ...kids) { return el("div", {class:"card"}, el("h2", {}, title), ...kids); }

// ── Tab id handling ────────────────────────────────────
// Collects existing tab ids within a desk (falls back to the display name if there's no id).
// Used as candidates for reference fields (discussion participants/judge/moderator, stop-condition target tab)
function tabIds(desk) {
  return [...new Set((desk.tabs || [])
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
// base, or the first of base-2, base-3... that nobody in `used` has taken.
// The app walks the same way when it reads a file nobody typed into
// (config.rs unique_id), so a name offered here is the name it settles on
function freeId(base, used) {
  if (!base) return "";
  if (!used.has(base)) return base;
  for (let n = 2; ; n++) { const c = base + "-" + n; if (!used.has(c)) return c; }
}
// Turns base into an id that doesn't collide within desk (-2, -3, ... if already used). self excludes itself
function uniqueId(desk, base, self) {
  return freeId(base, new Set((desk.tabs || [])
    .filter(t => t !== self).map(t => (t.id || "").trim()).filter(Boolean)));
}
// The same, for a desk's own name: automation and the secret store file it
// under this, so it has to be unlike every other desk's, not just unlike
// its neighbours in a folder
function uniqueWsId(base, self) {
  return freeId(base, new Set(desks
    .filter(w => w !== self).map(w => (w.id || "").trim()).filter(Boolean)));
}
// What a tab would be called by automation if nobody says otherwise: its
// display name, or -- for a tab that has none, and is therefore shown by its
// command -- the command. config.rs settles unnamed tabs the same way, and the
// two have to agree: a desk opened in the settings screen must not come
// out under a different name than the one the app filed its secrets beside
function inferredTabId(t) {
  // The program, not the whole command line: two browser tabs become "browser"
  // and "browser-2" rather than two mouthfuls of URL. config.rs reads argv()[0]
  // here, which is the same token for every command written as one line
  const prog = cmdToText(t.command).trim().split(/\s+/)[0] || "";
  return slugId(t.name) || slugId(prog) || "tab";
}
// Fills in an id for every tab that has none, a safety net at save time: a tab
// with no id at all cannot be pointed at -- not by automation, and not by the
// gear that opens its settings. Editable afterwards like any other.
function ensureIds(desk) {
  const tabs = desk.tabs || [];
  const used = new Set(tabs.map(t => (t.id || "").trim()).filter(Boolean));
  for (const t of tabs) {
    if ((t.id || "").trim()) continue;
    const id = freeId(inferredTabId(t), used);
    t.id = id; used.add(id);
  }
}
// Every desk gets a name of its own before writing, for the same reason
// every tab does: what refers to it must not change when the label does
function ensureWsIds() {
  for (const w of desks) {
    if ((w.id || "").trim()) continue;
    w.id = uniqueWsId(slugId(w.name) || "desk", w);
  }
}
// The automation names that two things claim at once. Saving stops on these:
// filling one in silently would send work somewhere nobody asked for
function collidingIds() {
  const out = [];
  const seenWs = new Set();
  for (const w of desks) {
    const id = (w.id || "").trim();
    if (id && seenWs.has(id)) out.push(id); else seenWs.add(id);
    const seen = new Set();
    for (const t of (w.tabs || [])) {
      const ti = (t.id || "").trim();
      if (ti && seen.has(ti)) out.push(ti); else seen.add(ti);
    }
  }
  return [...new Set(out)].sort();
}
// A dropdown for picking a tab id (candidates = existing tab ids). emptyLabel is the label for the empty option.
// Pass exclude(t)=>bool when tabs that are aimed at something should be excluded
function idSelect(desk, val, emptyLabel, onChange, exclude) {
  const s = el("select");
  s.append(el("option", {value:""}, emptyLabel));
  const ids = [...new Set((desk.tabs || [])
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
// The folder list, walked on the page. The same list the sidebar's "open
// another folder" shows, asked of this PC over /api/walk -- so a phone, which
// has no file dialog of its own, chooses from what is actually on the machine
// instead of typing a path from memory. Resolves to what was chosen, in the
// form the settings write it, or null
function walkPath(kind, title, start) {
  return new Promise(resolve => {
    const where = el("div", {class:"walkat mono"});
    const err = el("div", {class:"walkerr"});
    const list = el("div", {class:"walklist"});
    const use = el("button", {class:"primary"}, T["settings.pick.here"]);
    const cancel = el("button", {class:"quiet"}, T["common.cancel"]);
    const back = openModal(el("h2", {}, title || T["settings.pick.title"]), where, err, list,
      el("div", {class:"row", style:"justify-content:flex-end;margin-top:var(--s3)"}, cancel, use));
    let chosen = "";
    const done = p => { back.remove(); resolve(p); };
    // The backdrop closes it too (openModal); that is a cancel
    back.addEventListener("mousedown", e => { if (e.target === back) resolve(null); });
    cancel.onclick = () => done(null);
    use.onclick = () => done(chosen);
    // Folders only when a folder is wanted; files as well, and a file is the
    // answer, when a file is
    use.hidden = kind !== "dir";
    const leaf = p => p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p;
    const line = (mark, name, full, go) => el("div", {class:"walkrow", title:full || "", onclick:go},
      el("span", {class:"walkmark"}, mark), el("span", {class:"walknm"}, name));
    async function go(path) {
      let j = null;
      try {
        const r = await fetch("/api/walk", {method:"POST",
            headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
            body: JSON.stringify({path: path || "", files: kind !== "dir"})});
        j = await r.json();
      } catch (e) { j = null; }
      if (!j || !j.ok) { err.textContent = T["settings.pick.failed"]; return; }
      if (j.file) { done(j.chosen); return; }
      chosen = j.chosen || "";
      where.textContent = j.at || T["tui.browse.top"];
      err.textContent = j.error || "";
      list.textContent = "";
      if (j.up != null) list.append(line("←", "..", "", () => go(j.up)));
      for (const d of j.dirs || []) list.append(line("\u{1F4C1}", leaf(d), d, () => go(d)));
      for (const f of j.files || []) list.append(line("\u{1F4C4}", leaf(f), f, () => go(f)));
      use.disabled = !j.at;
    }
    go(start || "");
  });
}
function pathField(obj, key, ph, kind, title) {
  const i = field(obj, key, ph, {mono:true});
  // On this PC the operating system's own dialog, which knows about quick
  // access, search and network places. On a phone that dialog would open at
  // a screen nobody is looking at, so the page walks the PC's folders itself
  const pick = REMOTE ? walkPath : pickPath;
  const b = el("button", {class:"quiet", onclick: async () => {
    const p = await pick(kind, title, obj[key]);
    if (p !== null) { obj[key] = p; i.value = p; refreshSave(); }
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
const isEditorPanel = c => cmdToText(c).trim().toLowerCase() === "editor";
// The file panel, addressed the way a terminal on another machine is. Told
// apart from that one only by the scheme, so the two read as what they are:
// the same connection, asked for different things. A half-written address is
// still a panel -- the tab has to hold its place and say what it needs
function parseSftpUrl(cmd) {
  const text = (cmdToText(cmd) || "").trim();
  if (!/^sftp:\/\//i.test(text)) return null;
  const m = /^sftp:\/\/([^@\s]*)@?([^\s:/]*)(?::(\d+))?\/?$/i.exec(text);
  return m ? {user: m[1] || "", host: m[2] || "", port: m[3] || ""} : {user:"", host:"", port:""};
}
const buildSftpUrl = o =>
  "sftp://" + (o.user || "") + "@" + (o.host || "") + (o.port ? ":" + o.port : "");
// A terminal on another machine that this program connects to itself, written
// the way the world writes it. Checked before the plain `ssh` command, which is
// a different thing: that one runs ssh.exe and this one does not
function parseRemote(cmd) {
  const m = /^ssh:\/\/([^@\s]+)@([^\s:/]+)(?::(\d+))?\/?$/i.exec((cmdToText(cmd) || "").trim());
  return m ? {user: m[1], host: m[2], port: m[3] || ""} : null;
}
const buildRemote = o =>
  "ssh://" + (o.user || "") + "@" + (o.host || "") + (o.port ? ":" + o.port : "");
const kindOf = c => isGitPanel(c) ? "git" : isEditorPanel(c) ? "editor"
  : parseSftpUrl(c) ? "sftp"
  : parseBrowser(c) ? "browser" : parseModel(c) ? "model"
  : parseRemote(c) ? "remote" : parseSsh(c) ? "ssh" : parseDocker(c) ? "docker" : parseWsl(c) ? "wsl" : "cmd";
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
// The AI a tab starts as when nobody has chosen yet: the one chosen under
// Basic > Assistant AI while it is installed, else the first CLI that is
// installed here, else Claude Code. Aider is skipped -- it has no detection,
// so picking it would only mean "we never looked". With Yolo mode on, the
// CLI's "act without asking" flag is already in the command, where its
// checkbox below shows it ticked and one press takes it out again
const defaultAiCommand = () => {
  const installed = c => c.check && aiEngines.some(e => e.id === c.check);
  const head = (AI_CLIS.find(c => installed(c) && c.cmd === current.ai_engine)
    || AI_CLIS.find(installed) || AI_CLIS[0]).cmd;
  const flag = current.yolo ? cliFlagOf(head) : "";
  return flag ? head + " " + flag : head;
};
// What picking a kind puts in the command field. The AI entry is a function
// because its answer depends on which CLI this machine has.
const CAT_START = {ai:defaultAiCommand, cmd:"", remote:"ssh://user@example.com:22", ssh:"ssh ",
  docker:"docker exec -it ", wsl:"wsl ", browser:"browser https://", git:"git",
  editor:"editor", sftp:"sftp://user@example.com:22"};
const catStart = v => { const s = CAT_START[v]; return (typeof s === "function" ? s() : s) || ""; };
const CAT_LIST = [
  ["ai",      T["settings.tab.cat.ai"]],
  ["cmd",     T["settings.tab.cat.cmd"]],
  ["remote",  T["settings.tab.cat.remote"]],
  ["ssh",     "SSH (ssh.exe)"],
  ["docker",  "Docker"],
  ["wsl",     "WSL"],
  ["browser", T["settings.tab.kind.browser"]],
  ["git",     T["settings.tab.kind.git"]],
  ["editor",  T["settings.tab.kind.editor"]],
  ["sftp",    T["settings.template.sftp"]],
];

// What a tab is called when nobody has named it: the thing it runs, in the
// words the Kind and AI pickers use. For a kind with a choice inside it -- which
// AI, which shell -- the choice, because two tabs both called "AI" say nothing
// about which is which
function kindName(command) {
  const c = cmdToText(command).trim();
  const cat = catOf(c);
  const h = headOf(c);
  if (cat === "ai") {
    const m = parseModel(c);
    if (m) return m.provider || T["settings.tab.cat.ai"];
    const cli = AI_CLIS.find(x => x.cmd === h);
    return cli ? cli.label : (h ? h.charAt(0).toUpperCase() + h.slice(1) : T["settings.tab.cat.ai"]);
  }
  if (cat === "cmd") {
    if (!h) return T["settings.tab.name.cmd"];
    if (h === "powershell" || h === "pwsh") return "PowerShell";
    if (h === "cmd") return T["settings.tab.kind.cmdprompt"];
    return h;
  }
  return ({remote:"SSH", ssh:"SSH", docker:"Docker", wsl:"WSL", sftp:"SFTP", git:"git",
    browser:T["settings.tab.name.browser"], editor:T["settings.tab.kind.editor"]})[cat] || "";
}

// The display names a tab was given by what it runs, remembered so a change of
// kind carries the name along with it -- until somebody types one of their own
const autoNames = new WeakMap();
// Called when a tab's command has just changed from `before`. A name that is
// empty, or is still the one the old command gave, follows the new command; a
// name somebody chose stays
function followKind(t, before) {
  const name = (t.name || "").trim();
  if (name && name !== autoNames.get(t) && name !== kindName(before)) return false;
  const now = kindName(t.command);
  if (!now || now === name) return false;
  t.name = now;
  autoNames.set(t, now);
  return true;
}

// What automation calls a new tab when nobody has said: a short word drawn at
// random, not taken in this desk. Not the command -- a tab that started as
// Claude and was turned into SSH went on being "claude-2" -- and not a counter,
// which says nothing about which tab is which
function petId(desk, self) {
  const used = new Set((desk.tabs || []).filter(t => t !== self)
    .map(t => (t.id || "").trim()).filter(Boolean));
  for (let i = 0; i < 40 && PET_NOUNS.length; i++) {
    const n = PET_NOUNS[Math.floor(Math.random() * PET_NOUNS.length)];
    if (!used.has(n)) return n;
  }
  return freeId(PET_NOUNS[0] || "tab", used);
}

// ── Sidebar ───────────────────────────────────────
// Whether this is the folder the app itself is in. Written "." rather than
// as a path, so that settings carried to another PC land beside the app there
const besideTheApp = g => (g.cwd || "").trim().replace(/[\\/]+$/, "") === ".";
// The folder as a line of its own: its path, or the words for the two cases a
// path cannot say
function folderWhere(g) {
  if (besideTheApp(g)) return T["settings.group.folder.beside"];
  return (g.cwd || "").trim() || T["settings.group.folder.ph"];
}
// What a folder is called in a list: what someone typed, else the folder itself
function folderLabel(g, i) {
  const name = (g.name || "").trim();
  if (name) return name;
  const cwd = (g.cwd || "").trim();
  if (!cwd || besideTheApp(g)) return folderWhere(g);
  return cwd.split(/[\\/]/).filter(Boolean).pop() || cwd;
}

// Which colour a tab wears: its AI's own, or nothing in particular
const AI_COLOURS = {claude:"#d97757", codex:"#19c37d", gemini:"#4285f4",
  deepseek:"#5b7cff", qwen:"#a06bff", aider:"#e5644d", kimi:"#12b3a8"};
const aiColour = c => AI_COLOURS[headOf(c)] || null;

// A folder, drawn. Takes the colour of the row it sits in, so one mark says
// both "this is a working folder" and "this is that project"
function folderMark(colour) {
  const s = el("span", {class:"mark"});
  s.innerHTML = '<svg viewBox="0 0 14 14" width="13" height="13" fill="none" ' +
    'stroke="currentColor" stroke-width="1.3" stroke-linejoin="round">' +
    '<path d="M1.6 11.2V3.4a.8.8 0 0 1 .8-.8h2.7l1.2 1.5h4.1a.8.8 0 0 1 .8.8v6.3' +
    'a.8.8 0 0 1-.8.8H2.4a.8.8 0 0 1-.8-.8Z"/></svg>';
  if (colour) s.style.color = colour;
  return s;
}
// Which desk the sidebar is showing. A menu rather than a list, because
// the list under it belongs to one of them at a time
function pickDesk(anchor) {
  const menu = el("div", {class:"fmenu"});
  desks.forEach((w, i) => {
    menu.append(el("button", {class:"fmenuitem" + (i === sel.desk ? " on" : ""),
      onclick:() => { shut(); toTree(i); render(); }},
      el("span", {class:"wsbadge"}, (w.name || "?").trim().slice(0, 1).toUpperCase()),
      el("span", {class:"nm"}, w.name || T["settings.tab.unnamed"])));
  });
  menu.append(el("button", {class:"fmenuitem add",
    onclick:() => { shut(); addWs(); }}, T["settings.desk.add"]));
  // Under the row it opened from, and never off the left of the screen
  const at = anchor.getBoundingClientRect();
  menu.style.top = Math.round(at.bottom + 4) + "px";
  menu.style.left = Math.max(8, Math.round(at.left)) + "px";
  const away = e => { if (!menu.contains(e.target)) shut(); };
  function shut() {
    menu.remove();
    document.removeEventListener("mousedown", away, true);
  }
  document.body.append(menu);
  setTimeout(() => document.addEventListener("mousedown", away, true), 0);
}

function renderNav() {
  const nav = document.getElementById("nav");
  nav.textContent = "";
  // One column, read top to bottom, the larger world first: the program's own
  // settings, then the desk in view, then its projects. Every entry opens one
  // page; nothing here is entered and left, and nothing here adds anything --
  // projects, worktrees and tabs are added on the board, where they are used.
  // A worktree and a tab are not listed at all: they come and go on the board
  // every day, and their pages are reached from there (right-click, Settings)
  // or from the project's own page
  const item = (s, on, go) => nav.append(el("button", {class:"navitem" + (on ? " sel" : ""), onclick:go},
    el("div", {class:"body"}, el("span", {}, s.label), s.sub ? el("span", {class:"sub"}, s.sub) : null)));
  nav.append(el("div", {class:"navgroup"}, T["settings.global"]));
  for (const s of globalSections()) item(s, sel.global && sel.section === s.id, () => goSection(s.id));

  const desk = desks[sel.desk] || desks[0];
  if (!desk) return;
  // The desk: its name is the heading of everything under it, and pressing it
  // lists the desks -- choosing one switches this column to that desk
  nav.append(el("div", {class:"navgroup"}, T["settings.nav.desk"]));
  nav.append(el("button", {class:"deskbanner", title:T["settings.desk.switch"], onclick:e => pickDesk(e.currentTarget)},
    el("span", {class:"wsbadge"}, (desk.name || "?").trim().slice(0, 1).toUpperCase()),
    el("span", {class:"nm"}, desk.name || T["settings.tab.unnamed"]),
    el("span", {class:"wsgap"}),
    el("span", {class:"wspick"}, "▾")));
  for (const s of deskSections(desk)) item(s, inDeskPlace() && sel.dsection === s.id, () => goDeskSection(s.id));

  // Its projects: a repository's checkout and worktrees are one entry, and a
  // folder in no repository is a project of its own. The entry stays lit while
  // one of its folders or tabs is the page on screen, so where that page
  // belongs is never a question
  nav.append(el("div", {class:"navgroup"}, T["settings.nav.projects"]));
  const {projects, loose} = deskProjects(desk);
  const within = gi => !sel.global && (sel.grp ?? null) === gi;
  for (const p of projects) {
    const on = !sel.global && (sel.proj === p.key && (sel.grp ?? null) === null || p.folders.some(within));
    nav.append(el("button", {class:"navitem navproject" + (on ? " sel" : ""),
        onclick:() => { sel = {desk:sel.desk, proj:p.key, grp:null, tab:null, global:false}; render(); }},
      projectMark(p.family ? (current.folder_colors || {})[p.family] : null),
      el("div", {class:"body"}, el("span", {}, p.name), el("span", {class:"sub"}, p.at || T["settings.project.no_at"]))));
  }
  for (const gi of loose) {
    const g = desk.folders[gi];
    nav.append(el("button", {class:"navitem navproject" + (within(gi) ? " sel" : ""),
        onclick:() => { sel = {desk:sel.desk, grp:gi, tab:null, global:false}; render(); }},
      folderMark(null),
      el("div", {class:"body"}, el("span", {}, folderLabel(g, gi)), el("span", {class:"sub"}, folderWhere(g)))));
  }
  if (!projects.length && !loose.length) nav.append(el("div", {class:"navnone"}, T["settings.nav.projects.none"]));
}

// A project, drawn: a filled square in its colour, the mark the board gives a repository
function projectMark(colour) {
  const m = el("span", {class:"projmark"});
  if (colour) m.style.background = colour;
  return el("span", {class:"mark"}, m);
}

// Which repository each folder is in, as the app last said: path -> {family,
// main, cut}. Asked in one go (see /api/families) and kept for as long as the
// page is open; a path not in here yet is asked for, and the tree drawn again
let FAMILIES = {};

// The GitHub accounts git on this PC holds (see /api/pc-accounts). Asked once
// when the page opens; the menus that offer the PC's git are drawn again when
// the answer comes. Settled either way, so a landing can wait for that redraw
// rather than mark a card the redraw then replaces
let PC_ACCOUNTS = [];
const PC_ACCOUNTS_READ = fetch("/api/pc-accounts", {headers:{"X-Token":TOKEN}})
  .then(r => r.json())
  .then(j => {
    PC_ACCOUNTS = (j && j.accounts) || [];
    // Not under somebody typing: a page redrawn then takes the field away
    const typing = document.activeElement && ["INPUT", "TEXTAREA"].includes(document.activeElement.tagName);
    // and not before the settings themselves have arrived to be drawn
    if (PC_ACCOUNTS.length > 1 && !typing && desks.length) render();
  })
  .catch(() => {});
const familiesAsked = new Set();
let familiesWant = new Set(), familiesTimer = 0;
function askFamilies(paths) {
  for (const p of paths.map(x => (x || "").trim())) {
    if (p && !familiesAsked.has(p)) familiesWant.add(p);
  }
  if (!familiesWant.size) return;
  // Gathered for a moment, so a path being typed into a field is asked about
  // once it has settled rather than once per key
  clearTimeout(familiesTimer);
  familiesTimer = setTimeout(() => {
    const want = [...familiesWant];
    familiesWant = new Set();
    want.forEach(p => familiesAsked.add(p));
    settingsApi("/api/families", {paths: want}).then(j => {
      Object.assign(FAMILIES, (j && j.families) || {});
      // Only the tree is redrawn while somebody is typing on a page; a page
      // redrawn under the cursor would take the field away mid-word. A project's
      // own page is drawn from these answers, so that one is redrawn whole
      const typing = document.activeElement && ["INPUT", "TEXTAREA"].includes(document.activeElement.tagName);
      if ((sel.proj ?? null) !== null && !typing) render(); else renderNav();
    }).catch(() => {});
  }, 250);
}
const familyAt = path => (FAMILIES[(path || "").trim()] || {}).family || null;

// A desk's folders sorted into its projects.
//
// A folder belongs to the project it names, and failing that to the project
// whose own checkout is in the same repository. A repository with folders here
// and no project written down yet still shows as one -- named after its own
// checkout -- and is written into the settings the first time anything about
// it is changed. A folder in no repository stands on its own.
// Returns {projects: [{key, name, at, entry, family, folders:[gi]}], loose:[gi]}
function deskProjects(desk) {
  const folders = desk.folders || [];
  const entries = desk.projects || [];
  askFamilies(folders.map(g => g.cwd).concat(entries.map(p => p.at)));
  const projects = entries.map(p => ({key:"p:" + p.name, name:p.name, at:p.at || "",
    entry:p, family:familyAt(p.at), folders:[]}));
  const loose = [];
  folders.forEach((g, gi) => {
    const said = (g.project || "").trim();
    let home = said ? projects.find(p => p.name === said) : null;
    const fam = familyAt(g.cwd);
    if (!home && fam) home = projects.find(p => p.family === fam);
    if (!home && fam) {
      const main = (FAMILIES[(g.cwd || "").trim()] || {}).main || g.cwd;
      home = {key:"f:" + fam, name:leafName(main), at:main, entry:null, family:fam, folders:[]};
      projects.push(home);
    }
    if (home) home.folders.push(gi); else loose.push(gi);
  });
  return {projects, loose};
}
const leafName = p => ((p || "").replace(/[\\/]+$/, "").split(/[\\/]/).pop()) || "project";
function uniqueProjectName(desk, base) {
  const taken = new Set((desk.projects || []).map(p => p.name));
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) if (!taken.has(base + " " + n)) return base + " " + n;
}
// The written entry for a project in the tree, writing it now if it was only
// worked out from git -- and naming it on its folders, so the tie survives a
// folder being renamed or moved
function ensureProject(desk, p) {
  desk.projects = desk.projects || [];
  if (!p.entry) {
    p.entry = {name: uniqueProjectName(desk, p.name)};
    if ((p.at || "").trim()) p.entry.at = p.at.trim();
    desk.projects.push(p.entry);
  }
  for (const gi of p.folders) {
    const g = desk.folders[gi];
    if (g && !(g.project || "").trim()) g.project = p.entry.name;
  }
  return p.entry;
}

const newTab = (o = {}) => Object.assign(
  {name:"", id:"", command:"", profile:"", automation:"", locked:false, auto_restart:false,
   browser_profile:"", private:false,
   encoding:"", scrollback:"", log:false, depth:0, group:0}, o);

// A tab nobody has filled in yet: no name, no id, and a command still at what
// "Add tab" left there. The empty case is kept because tabs written before
// tabs started arriving as AI have no command at all.
// A tab added on this page is filled in the moment it is made -- a name from
// what it runs, a word for automation -- so "nobody has filled it in" means
// "still exactly what it was given", remembered here
const freshTabs = new WeakMap();
const blankTab = t => {
  const was = freshTabs.get(t);
  if (was) return was.name === (t.name || "") && was.id === (t.id || "")
    && was.command === cmdToText(t.command).trim();
  return !(t.name || "").trim() && !(t.id || "").trim()
    && ["", defaultAiCommand()].includes(cmdToText(t.command).trim());
};

// Adds one tab. But if there's already an in-progress empty tab, just selects that instead.
// Returns the index of the added (or found) tab
function addTabTo(desk, group) {
  desk.tabs = desk.tabs || [];
  // Which folder it will run in. Named by the caller, else the one being
  // looked at, else the first: a tab has to be somewhere, and "somewhere"
  // was the part nobody could answer when the button was at the bottom of
  // the whole list
  if (group === undefined || group === null) {
    const at = desk.tabs[sel.tab];
    group = at ? (at.group || 0) : (sel.grp || 0);
  }
  let i = desk.tabs.findIndex(t => (t.group || 0) === group && blankTab(t));
  if (i < 0) {
    // Beside the others in the same folder, so the list stays in folder order
    let j = desk.tabs.length;
    while (j > 0 && (desk.tabs[j - 1].group || 0) > group) j--;
    // A new tab starts as an AI. This is a terminal for running AIs, and the
    // AI panel is where the switches that decide how one runs live -- a tab
    // that began as a plain shell hid them behind a dropdown nobody knew to
    // open. A shell is one pick away in the Kind row above.
    const command = defaultAiCommand();
    const t = newTab({group, command});
    t.name = kindName(command);
    autoNames.set(t, t.name);
    t.id = petId(desk, t);
    freshTabs.set(t, {name: t.name, id: t.id, command});
    desk.tabs.splice(j, 0, t);
    i = j;
  }
  return i;
}

// ── New-desk wizard ───────────────────────
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

// Child pages automatically answer native JavaScript dialogs for browser
// automation. Settings decisions must remain on the page until a person acts.
// A modal dialog also makes an editor underneath inert and traps keyboard focus.
//
// Every question asked here is about throwing something away -- a secret, a
// device's key, a desk, unsaved changes -- so the button that does it wears
// the colour for breaking things, not the brand's (STYLEGUIDE §5, buttons),
// and the key that is pressed without looking lands on Cancel: an Enter meant
// for the field underneath must not be the one that deletes
let confirming = false;
function confirmAction(message, action) {
  if (confirming) return Promise.resolve(false);
  confirming = true;
  const previous = document.activeElement;
  return new Promise(resolve => {
    const finish = answer => {
      dialog.close(); dialog.remove(); confirming = false;
      if (previous && previous.isConnected) previous.focus();
      resolve(answer);
    };
    const accept = el("button", {class:"danger", onclick:() => finish(true)}, action);
    const cancel = el("button", {class:"quiet", onclick:() => finish(false)}, T["common.cancel"]);
    const dialog = el("dialog", {class:"modal-inner framed confirm-box",
      "aria-labelledby":"confirm-title", "aria-describedby":"confirm-message"},
      el("div", {class:"mhead"},
        el("h2", {id:"confirm-title"}, T["settings.confirm.title"]),
        el("button", {class:"quiet icon", title:T["common.close"],
          "aria-label":T["common.close"], onclick:() => finish(false)}, "✕")),
      el("div", {class:"mbody", id:"confirm-message"}, message),
      el("div", {class:"mfoot"}, cancel, accept));
    dialog.addEventListener("cancel", e => { e.preventDefault(); finish(false); });
    dialog.addEventListener("keydown", e => {
      if (e.key !== "Tab") return;
      e.preventDefault();
      const buttons = [...dialog.querySelectorAll("button")];
      const at = buttons.indexOf(document.activeElement);
      buttons[(at + (e.shiftKey ? -1 : 1) + buttons.length) % buttons.length].focus();
    });
    dialog.addEventListener("mousedown", e => {
      const r = dialog.getBoundingClientRect();
      if (e.target === dialog && (e.clientX < r.left || e.clientX > r.right ||
          e.clientY < r.top || e.clientY > r.bottom)) finish(false);
    });
    document.body.append(dialog);
    dialog.showModal(); cancel.focus();
  });
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
// The desk a new one starts from, chosen on the first screen of adding one
// (or null to start with none of it). Read once, by landOnWs
let startFrom = null;

// What a desk has that is its own alone -- where it notifies, which AI accounts
// it uses, what its automation may do and reach, what git does -- copied for a
// new desk. The keys behind a connection or a destination are filed again under
// the new desk, so each keeps its own and deleting one cannot take the other's
// `only`, when given, is the names of the connections to bring and nothing else
async function copyDeskOwn(from, to, only) {
  const clone = v => JSON.parse(JSON.stringify(v || {}));
  if (only) {
    for (const n of only) if ((from.providers || {})[n]) to.providers[n] = clone(from.providers[n]);
  } else {
    for (const k of ["notify", "providers", "capabilities", "automation_permissions", "git"]) to[k] = clone(from[k]);
    if ((from.primary_notify || "").trim()) to.primary_notify = from.primary_notify;
  }
  const id = (to.id || "").trim(), was = (from.id || "").trim();
  if (!id || !was) return;
  const refile = async (ref, set) => {
    if (!(ref || "").startsWith("@")) return;
    const old = ref.slice(1);
    const kind = ["provider/", "notify/"].find(p => old.startsWith(p + was + "/"));
    if (!kind) return;
    const fresh = kind + id + old.slice((kind + was).length);
    const r = await settingsApi("/api/secrets/copy", {from: old, to: fresh}).catch(() => null);
    if (r && r.ok) set("@" + fresh);
  };
  for (const [n, p] of Object.entries(to.providers)) {
    if (!only || only.includes(n)) await refile(p.api_key, v => p.api_key = v);
  }
  if (only) return;
  for (const d of Object.values(to.notify)) {
    await refile(d.webhook, v => d.webhook = v);
    await refile(d.token, v => d.token = v);
  }
}

async function landOnWs(desk) {
  // Made by a wizard, a template or from nothing -- all of them arrive here, so
  // this is the one place that has to make sure a desk has its folder
  if (!(desk.folders || []).length) desk.folders = [{name:"", id:"", cwd:""}];
  (desk.tabs || []).forEach(t => { if (t.group === undefined) t.group = 0; });
  if (!(desk.id || "").trim()) desk.id = uniqueWsId(slugId(desk.name) || "desk", desk);
  for (const k of ["notify", "providers", "capabilities", "automation_permissions", "git"]) {
    if (!isObj(desk[k])) desk[k] = {};
  }
  const from = startFrom;
  startFrom = null;
  if (from) await copyDeskOwn(from, desk);
  else {
    // A wizard offers the desk in view's connections to choose AIs from. One
    // somebody chose there comes along -- chosen by name, on purpose, for this
    // desk -- and nothing else of that desk does
    const here = desks[sel.desk];
    const used = (desk.tabs || []).map(t => parseModel(t.command)).filter(Boolean).map(m => m.provider)
      .filter(p => here && (here.providers || {})[p] && !desk.providers[p]);
    if (used.length) await copyDeskOwn(here, desk, [...new Set(used)]);
  }
  desks.push(desk); sel = {desk:desks.length - 1, tab:null, global:false}; render(); refreshSave();
}

// "+ Add desk" → first, have the user pick a purpose
function addWs() {
  const m = openModal();
  // Whether the new desk starts with this one's destinations, connections,
  // permissions and git settings. Asked first, because the wizards below offer
  // this desk's connections to choose AIs from
  const here = desks[sel.desk];
  const hasOwn = here && ["notify", "providers", "capabilities", "automation_permissions", "git"]
    .some(k => isObj(here[k]) && Object.keys(here[k]).length);
  const copyBox = el("input", {type:"checkbox"});
  copyBox.checked = !!hasOwn;
  const copyLabel = el("label", {class:"check"});
  copyLabel.append(copyBox, document.createTextNode(
    fill(T["wizard.pick.copy"], {name: (here && here.name) || ""})));
  // A desk read in from a file brings its own, so it starts from nothing here
  const pick = fn => { startFrom = (hasOwn && copyBox.checked && fn !== importWs) ? here : null; m.remove(); fn(); };
  const opt = (emoji, title, desc, fn) => el("button",
    {class:"quiet", style:"display:flex;gap:var(--s3);align-items:flex-start;text-align:left;" +
      "width:100%;padding:14px;border:1px solid var(--line);border-radius:10px;margin:var(--s2) 0",
     onclick:() => pick(fn)},
    el("span", {style:"font-size:22px;line-height:1"}, emoji),
    el("span", {}, el("div", {style:"color:var(--text);font-weight:600;margin-bottom:var(--s1)"}, title),
      el("div", {class:"hint"}, desc)));
  m.firstChild.append(
    el("h2", {}, T["wizard.pick.title"]),
    el("div", {class:"hint"}, T["wizard.pick.hint"]),
    // Nothing to copy from means nothing said about it. Left as a null, the
    // browser writes the word "null" into the dialog
    ...(hasOwn ? [el("div", {style:"margin-top:var(--s3)"}, copyLabel,
      el("div", {class:"hint"}, T["wizard.pick.copy.hint"]))] : []),
    // A desk starts empty or comes in from a file. The three AI templates that
    // stood here -- a discussion, a browser the AI drives, a code review --
    // each promised what the desk they made did not do: the browser one
    // handed its AI nothing at startup, the review's "everybody said LGTM"
    // finish reached no code, and the discussion's own default asked for more
    // hand-offs than the automatic chain allows. Filling a form before the
    // work begins is the wrong shape for this anyway; git holds them
    el("div", {style:"margin-top:var(--s2)"},
      opt("🖥", T["wizard.pick.blank.title"], T["wizard.pick.blank.desc"], createBlankWs),
      // A file dialog is the only way in, and a phone has none to open
      REMOTE ? null
             : opt("📂", T["wizard.pick.import.title"], T["wizard.pick.import.desc"], importWs)),
    el("div", {class:"row", style:"margin-top:var(--s2)"},
      el("button", {class:"quiet", onclick:() => { startFrom = null; m.remove(); }}, T["common.cancel"])));
}
function createBlankWs() {
  landOnWs({name: T["settings.desk"], automation:"", tabs:[]});
}

// ── Narrow screens: the sidebar as a drawer ─────────────────────
// One phone-width layout, driven from here so the CSS and the DOM never disagree
// about where things are. The same nav, the same links, the same header — moved,
// never duplicated, so a card added later needs no phone-specific counterpart.
// The browser's own menu -- Save as, Print, View source -- belongs to a web
// page, and this is the program's settings: a right-click here offers nothing.
// A field keeps its menu, which is where cut, copy and paste are
document.addEventListener("contextmenu", e => {
  if (!(e.target.closest && e.target.closest("input, textarea, [contenteditable]"))) e.preventDefault();
});
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
  const desk = desks[sel.desk];
  if (!desk) return ["", ""];
  const name = desk.name || T["settings.tab.unnamed"];
  const g = (desk.folders || [])[sel.grp];
  if (sel.tab === null) {
    if (!g && (sel.proj ?? null) !== null) {
      const p = deskProjects(desk).projects.find(x => x.key === sel.proj);
      return [name, p ? p.name : ""];
    }
    if (!g) {
      const s = deskSections(desk).find(x => x.id === sel.dsection);
      return [name, s ? s.label : T["settings.dsec.enter"]];
    }
    return [name, folderLabel(g, sel.grp)];
  }
  const t = (desk.tabs || [])[sel.tab];
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
    el("div", {style:"color:var(--danger);margin:var(--s2) 0"}, f.error),
    brokenExcerpt(f),
    el("div", {class:"hint"}, T["settings.broken.body"]),
    el("div", {class:"row", style:"margin-top:var(--s3)"},
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
    // A section may be more than one card: both belong to it, and both
    // are its own siblings rather than one wrapped inside the other
    return d.append(...[].concat(sec.build()));
  }
  const desk = desks[sel.desk];
  if (!desk) return;
  if (sel.tab === null) {
    if ((sel.grp ?? null) === null && (sel.proj ?? null) !== null) {
      const p = deskProjects(desk).projects.find(x => x.key === sel.proj);
      if (!p) { sel.proj = null; toTree(sel.desk); return renderDetail(); }
      return d.append(projectPane(desk, p));
    }
    if (sel.grp === null || sel.grp === undefined) {
      const secs = deskSections(desk);
      const sec = secs.find(s => s.id === sel.dsection) || secs[0];
      sel.dsection = sec.id;
      return d.append(...[].concat(sec.build(desk)));
    }
    const g = (desk.folders || [])[sel.grp];
    if (!g) { sel.grp = null; return renderDetail(); }
    return d.append(folderPane(desk, g, sel.grp));
  }
  const t = desk.tabs[sel.tab];
  if (!t) { sel.tab = null; return renderDetail(); }
  d.append(tabPane(desk, t));
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
    row(T["settings.restore_ws"], checkDefaultOn(current, "restore_desk", T["settings.restore_ws.label"]),
        el("span", {class:"hint"}, T["settings.restore_ws.hint"])),
    row(T["settings.confirm_worktree_delete"], checkDefaultOn(current, "confirm_worktree_delete", T["settings.confirm_worktree_delete.label"]),
        el("span", {class:"hint"}, T["settings.confirm_worktree_delete.hint"])),
    row(T["settings.resident"], checkDefaultOn(current, "resident", T["settings.resident.label"]),
        el("span", {class:"hint"}, T["settings.resident.hint"])),
    // Directly under the ✕, because the ✕ is what it changes the meaning of:
    // one asks what closing the window costs, the other asks whether the
    // window is the program at all
    row(T["settings.split"], check(current, "split", T["settings.split.label"]),
        el("span", {class:"hint"}, T["settings.split.hint"])),
    row(T["settings.tui_clipboard"], checkDefaultOn(current, "tui_clipboard", T["settings.tui_clipboard.label"]),
        el("span", {class:"hint"}, T["settings.tui_clipboard.hint"])),
    row(T["settings.conpty"], conptyState(),
        el("span", {class:"hint"}, T["settings.conpty.hint"])),
    // What a folder opens with: a project added, or an empty folder pressed
    row(T["settings.default_shell"],
        choose(current, "default_shell", [
          ["", T["settings.default_shell.powershell"]],
          ["cmd", T["settings.default_shell.cmd"]],
          ["gitbash", T["settings.default_shell.gitbash"]],
        ]),
        el("span", {class:"hint"}, T["settings.default_shell.hint"])),
    row(T["settings.ai_engine"], aiSelect(),
        el("span", {class:"hint", id:"aihint"}, "")),
    row(T["settings.yolo"], check(current, "yolo", T["settings.yolo.label"]),
        el("span", {class:"hint warn"}, T["settings.tab.ai.autoapprove_risk"])),
    // Naming a folder and its branch is not the work the assistant AI above
    // was chosen for, so it is chosen again here -- and asked in the cheapest
    // way its CLI allows, which is the row under it
    row(T["settings.summary_ai"],
        choose(current, "summary_ai",
          [["", fill(T["settings.summary_ai.assistant"], {name: aiLabelOf(current.ai_engine)})]]
            .concat(aiEngines.map(e => [e.id, e.label]))),
        el("span", {class:"hint"}, T["settings.summary_ai.hint"])),
    row(T["settings.summary_small"],
        checkDefaultOn(current, "summary_small_model", T["settings.summary_small.label"]),
        el("span", {class:"hint"}, T["settings.summary_small.hint"])),
    row(T["settings.browser_data"],
        choose(current, "browser_data", [
          ["", T["settings.browser_data.local"] || "This PC only (recommended)"],
          ["portable", T["settings.browser_data.portable"] || "Share across PCs (Drive sync)"],
        ]),
        el("span", {class:"hint"}, T["settings.browser_data.hint"] || "")),
    row(T["settings.browser_draw"],
        choose(current, "browser_draw", [
          ["", T["settings.browser_draw.here"] || "On this machine (recommended)"],
          ["there", T["settings.browser_draw.there"] || "On the device that is connected"],
        ]),
        el("span", {class:"hint"}, T["settings.browser_draw.hint"] || "")),
    row(T["settings.user_agent"],
        field(current, "user_agent", T["settings.user_agent.ph"], {grow:true}),
        el("span", {class:"hint"}, T["settings.user_agent.hint"] || "")),
    row(T["settings.font"],
        (() => {
          current.appearance = current.appearance || {};
          const a = current.appearance;
          const wrap = el("div", {style:"display:flex;gap:var(--s2);min-width:0;flex:1"});
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

// The colour scheme, chosen by name from the ones this machine already has.
//
// The list is fetched rather than built in because most of it is not ours: it
// is whatever schemes the platform's terminal is carrying plus whatever the
// person dropped in their config folder. Each one shows its own colours, since
// nobody remembers what "Nord" looks like from the word.
function themePicker() {
  current.appearance = current.appearance || {};
  const a = current.appearance;
  const wrap = el("div", {style:"display:flex;gap:var(--s3);align-items:center;min-width:0;flex:1;flex-wrap:wrap"});
  const picker = el("select", {style:"min-width:190px"});
  const strip = el("div", {style:"display:flex;gap:var(--s1)"});
  wrap.append(picker, strip);
  // A scheme written out in the settings by hand is not in any list, and
  // picking from the list is how someone would replace it -- so it is offered
  // as the current choice rather than silently dropped
  const inline = a.theme && typeof a.theme === "object";
  let known = [], mine = null;
  const paint = () => {
    const found = picker.value ? known.find(t => t.name === picker.value) : mine;
    strip.textContent = "";
    for (const c of (found ? found.colors : [])) {
      strip.append(el("span", {style:"width:14px;height:14px;border-radius:var(--r-chip);" +
        "border:1px solid var(--line);background:" + c}));
    }
  };
  picker.addEventListener("change", () => {
    // Picking a name replaces whatever was there. Landing back on "written
    // here" leaves the colours the person wrote exactly as they wrote them
    if (picker.value) a.theme = picker.value;
    else if (!inline) delete a.theme;
    paint();
  });
  (async () => {
    let j;
    try { j = await (await fetch("/api/themes", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    known = j.list || [];
    mine = j.current || null;
    if (inline) picker.append(el("option", {value:""}, T["settings.theme.custom"]));
    for (const t of known) picker.append(el("option", {value:t.name}, t.name));
    // An unset theme is the app's own, not whatever happens to sort first
    picker.value = inline ? "" : (a.theme || j.default || "");
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
// The keys that work from any program on this PC: the tools, and the quick
// commands and the ideas.
//
// Held keys are chosen from Ctrl, Alt and Shift and the key from a list, rather
// than by pressing the combination: a key already registered would open the
// tool instead of being recorded, and a phone has no such keys to press. What
// each row says is what the program found when it registered the key, which is
// the part a person cannot see for themselves
const HOTKEY_KEYS = [..."ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"].concat(Array.from({length: 12}, (_, i) => "F" + (i + 1)));
// Every action in the order the app lists them, and the keys they have out of
// the box (hotkeys.rs)
const HOTKEYS = __HOTKEYS__;
function hotkeysCard() {
  // Read without writing: drawing the card is not an edit, and a section put
  // into the settings just by looking would mark them unsaved
  const hk = isObj(current.hotkeys) ? Object.assign({}, current.hotkeys) : {};
  let status = null;
  const list = el("div", {});
  const box = card(T["settings.hotkeys.title"],
    el("div", {class:"hint", style:"margin-bottom:var(--s3)"}, T["settings.hotkeys.intro"]),
    list);
  const parse = text => {
    const c = {ctrl:false, alt:false, shift:false, key:""};
    for (const part of String(text || "").split("+").map(p => p.trim())) {
      const low = part.toLowerCase();
      if (low === "ctrl" || low === "control") c.ctrl = true;
      else if (low === "alt") c.alt = true;
      else if (low === "shift") c.shift = true;
      else if (HOTKEY_KEYS.includes(part.toUpperCase())) c.key = part.toUpperCase();
    }
    return c;
  };
  const shown = c => [c.ctrl && "Ctrl", c.alt && "Alt", c.shift && "Shift", c.key].filter(Boolean).join("+");
  const dflt = a => HOTKEYS.defaults[a] || "";
  // What the settings hold for an action, as it would be read (one unwritten
  // has its key out of the box)
  const written = a => a in hk ? hk[a] : dflt(a);
  // What each row shows, kept between redraws: a held key pressed before the
  // key is chosen is half a combination, and has to still be there when the
  // other half arrives
  const drafts = {};
  const draw = () => {
    list.textContent = "";
    for (const action of HOTKEYS.actions) {
      const c = drafts[action] || (drafts[action] = parse(written(action)));
      const toggles = ["ctrl", "alt", "shift"].map(m => {
        const b = el("button", {class:"tog" + (c[m] ? " on" : ""), "aria-pressed": String(c[m])},
          {ctrl:"Ctrl", alt:"Alt", shift:"Shift"}[m]);
        b.onclick = () => { c[m] = !c[m]; keep(action, c); };
        return b;
      });
      const pick = el("select", {class:"hkkey"});
      pick.append(el("option", {value:""}, T["settings.hotkeys.none"]));
      for (const k of HOTKEY_KEYS) pick.append(el("option", {value:k}, k));
      pick.value = c.key;
      pick.onchange = () => { c.key = pick.value; keep(action, c); };
      const say = el("span", {class:"hint hkstate"});
      tell(say, action, c);
      list.append(el("div", {class:"row pair hkrow"},
        el("label", {}, T["settings.hotkeys.action." + action]),
        el("div", {class:"hkpick"}, ...toggles, pick),
        say));
    }
  };
  // Written back only where it differs from what an unwritten one means, so a
  // settings file nobody touched here stays without the section
  // Only a whole combination is written. Half of one is no key until it is
  // finished -- the row says what is missing
  const keep = (action, c) => {
    const text = c.key && (c.ctrl || c.alt) ? shown(c) : "";
    if (text === dflt(action)) delete hk[action];
    else hk[action] = text;
    if (Object.keys(hk).length) current.hotkeys = Object.assign({}, hk);
    else delete current.hotkeys;
    refreshSave();
    draw();
  };
  const clock = secs => new Date(secs * 1000).toLocaleString([], {month:"numeric", day:"numeric", hour:"2-digit", minute:"2-digit"});
  function tell(say, action, c) {
    const text = c.key ? shown(c) : "";
    const warn = msg => { say.className = "hint hkstate warnline"; say.textContent = msg; };
    say.className = "hint hkstate";
    if (c.key && !c.ctrl && !c.alt) return warn(T["settings.hotkeys.need_mod"]);
    const row = status && (status.rows || []).find(r => r.action === action);
    if (!status || !status.active) { say.textContent = text ? "" : T["settings.hotkeys.off"]; return; }
    if (!row || row.key !== text) { say.textContent = text ? T["settings.hotkeys.unsaved"] : T["settings.hotkeys.off"]; return; }
    if (row.state === "off") say.textContent = T["settings.hotkeys.off"];
    else if (row.state === "taken") warn(T["settings.hotkeys.taken"]);
    else if (row.state === "twice") warn(T["settings.hotkeys.twice"]);
    else if (row.state === "unreadable") warn(T["settings.hotkeys.unreadable"]);
    else say.textContent = row.last ? fill(T["settings.hotkeys.last"], {when: clock(row.last)}) : T["settings.hotkeys.never"];
  }
  // What the program found. Asked again while the card is open: a save
  // registers the keys a moment after, and a key pressed elsewhere should show
  const load = async () => {
    if (!box.isConnected && status) return;
    try { status = await (await fetch("/api/hotkeys", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { status = null; }
    draw();
    setTimeout(load, 3000);
  };
  draw();
  load();
  return box;
}

// A combination held down, written the way the keys list writes it
// ("Ctrl+Shift+M", "Alt+F4", "F5"), or null for a press that is typing
function pressedCombo(e) {
  if (e.isComposing || ["Control", "Shift", "Alt", "Meta"].includes(e.key)) return null;
  const letter = /^Key([A-Z])$/.exec(e.code || "");
  const digit = /^Digit([0-9])$/.exec(e.code || "");
  const names = {ArrowUp:"Up", ArrowDown:"Down", ArrowLeft:"Left", ArrowRight:"Right", PageUp:"PageUp",
    PageDown:"PageDown", Home:"Home", End:"End", Insert:"Insert", Delete:"Delete", Enter:"Enter",
    Escape:"Esc", Tab:"Tab", Backspace:"Backspace", " ":"Space"};
  const fkey = /^F([1-9]|1[0-2])$/.test(e.key) ? e.key : "";
  const held = e.ctrlKey || e.altKey;
  // Nothing held and not a function key: typing, or walking between boxes
  if (!held && !fkey) return null;
  const key = letter ? letter[1] : digit ? digit[1] : fkey || names[e.key] || (e.key.length === 1 ? e.key : "");
  if (!key) return null;
  return (e.ctrlKey ? "Ctrl+" : "") + (e.altKey ? "Alt+" : "") + (e.shiftKey ? "Shift+" : "") + key;
}
function keysCard() {
  // Read without writing, like the card above: an empty section put in just by
  // opening the page marked every visit unsaved
  const k = isObj(current.keys) ? current.keys : {};
  const attach = () => {
    if (k.prefix === "") delete k.prefix;
    if (Object.keys(k).length) current.keys = k; else delete current.keys;
  };
  const list = el("div", {}, el("div", {class:"hint"}, "…"));
  const problems = el("div", {});
  const box = card(T["settings.keys.in_title"],
    el("div", {class:"hint", style:"margin-bottom:var(--s3)"}, T["settings.keys.intro"]),
    problems,
    row(T["settings.keys.prefix"], field(k, "prefix", "ctrl+b", {width:150, grow:false, onInput:attach}),
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
        attach();
      });
      // Pressed rather than spelled: a combination held down while the box
      // has the caret is written into it the way the list shows keys. A bare
      // character is still typed, since on its own it means "after the prefix"
      inp.addEventListener("keydown", e => {
        const combo = pressedCombo(e);
        if (!combo) return;
        e.preventDefault();
        inp.value = combo;
        inp.dispatchEvent(new Event("input"));
      });
      list.append(el("div", {class:"row pair"},
        el("label", {}, r.desc),
        inp,
        el("span", {class:"hint"}, r.now ? T["settings.keys.now"] + " " + r.now
                                         : T["settings.keys.off"])));
    }
  }
  return box;
}
// ── Quick commands ────────────────────────────────────────────
// Buttons on pages of a grid, each handing a tab a command or a prompt; a
// button can be a folder with a grid of its own. Edited here as the grid it
// is: the places are the places the launcher shows. Where a button may sit is
// never decided in this script -- after anything that could move one, the app
// is asked (/api/quick/arrange, the same function the launcher is drawn from)
const QUICK = __QUICK__;
// Where the editor is standing, kept across redraws of the card: the folders
// walked into by id, the page, and the place picked on it
const quickAt = {path: [], page: 0, pick: null};
// The drawings of the icons in use, by name, as the app hands them back. The
// whole set is fetched only when the picker is opened
const quickSvgs = {};
let quickSet = null;
// The drawings offered first, before anything is typed: the things a command
// or a prompt is usually about. Only the ones the set really has are shown
const QUICK_COMMON_ICONS = ["terminal", "square-terminal", "folder", "folder-open", "folder-git-2",
  "git-branch", "git-commit-horizontal", "git-pull-request", "git-merge", "bot", "sparkles",
  "brain", "wand-sparkles", "message-square", "send", "play", "square", "rotate-ccw", "refresh-cw",
  "rocket", "package", "box", "container", "database", "server", "cloud", "cloud-upload", "globe",
  "code", "braces", "file-code", "file-text", "notebook-pen", "clipboard", "clipboard-check",
  "list-checks", "check", "bug", "test-tube", "flask-conical", "wrench", "hammer", "settings", "key",
  "lock", "shield", "search", "eye", "trash-2", "download", "upload", "zap", "flame", "timer",
  "calendar", "mail", "bell", "star", "heart", "flag", "house", "power", "monitor", "smartphone"];

// The settings as they stand, read without writing: drawing the card is not
// an edit, and a key put into the settings by looking would mark them unsaved
function quickSpec() { return isObj(current.quick_commands) ? current.quick_commands : {items: []}; }
// ...and the same, ready to be written into
function quickOwn() {
  if (!isObj(current.quick_commands)) current.quick_commands = {items: []};
  if (!Array.isArray(current.quick_commands.items)) current.quick_commands.items = [];
  return current.quick_commands;
}
function quickDims() {
  const q = quickSpec();
  return {cols: q.cols || QUICK.cols, rows: q.rows || QUICK.rows};
}
// The grid being shown: the top, or the folder walked into. A folder that is
// no longer there takes the walk back to the last one that still is
function quickHolder(q) {
  let at = q;
  const kept = [];
  for (const id of quickAt.path) {
    const f = (at.items || []).find(i => i.id === id && i.kind === "folder");
    if (!f) break;
    kept.push(id);
    at = f;
  }
  if (kept.length !== quickAt.path.length) {
    quickAt.path = kept; quickAt.page = 0; quickAt.pick = null;
  }
  return at;
}
async function quickArrange(q) {
  try {
    const r = await fetch("/api/quick/arrange", {method:"POST",
      headers:{"X-Token":TOKEN, "Content-Type":"application/json"},
      body: JSON.stringify(q)}).then(x => x.json());
    if (r && r.ok) { Object.assign(quickSvgs, r.svgs || {}); return r.spec; }
  } catch (e) {}
  return null;
}
// The shape quick commands are written in (see payload). Null when there is
// nothing worth writing at all
function quickCanon(q, inFolder) {
  if (!isObj(q)) return null;
  const items = (Array.isArray(q.items) ? q.items : []).filter(isObj).map(i => {
    const folder = i.kind === "folder";
    const o = {id: i.id || "", page: i.page || 0, row: i.row || 0, col: i.col || 0};
    if (i.icon) o.icon = i.icon;
    o.label = i.label || "";
    if (folder || i.kind === "ai") o.kind = i.kind;
    if (!folder && i.body) o.body = i.body;
    if (!folder && i.enter === false) o.enter = false;
    if (i.kind === "ai" && i.ai) o.ai = i.ai;
    if (folder) {
      const inner = quickCanon(i, true) || {};
      if (inner.pages) o.pages = inner.pages;
      o.items = inner.items || [];
    }
    return o;
  }).filter(o => o.kind === "folder" || o.label.trim() || o.body || o.icon);
  const out = {};
  if (!inFolder) {
    if (q.cols && q.cols !== QUICK.cols) out.cols = q.cols;
    if (q.rows && q.rows !== QUICK.rows) out.rows = q.rows;
  }
  // Pages only when there are more than the buttons already take: the app
  // counts those itself, so writing them down is saying nothing
  const used = items.reduce((m, i) => Math.max(m, i.page + 1), 1);
  if (q.pages && q.pages > used) out.pages = q.pages;
  out.items = items;
  if (!inFolder && !items.length && !out.cols && !out.rows && !out.pages) return null;
  return out;
}
// Put the app's answer in, or keep what is there when it could not be asked
async function quickSettle() {
  const a = await quickArrange(quickOwn());
  if (a) current.quick_commands = a;
  refreshSave();
}
function quickNewId() {
  const b = new Uint8Array(6);
  crypto.getRandomValues(b);
  return "q" + [...b].map(x => x.toString(16).padStart(2, "0")).join("");
}
const quickSvg = name => quickSvgs[name] || "";
const quickWayOut = (row, col) => quickAt.path.length > 0 && row === 0 && col === 0;
// Every button inside a folder, folders inside it included
function quickCount(f) {
  return (f.items || []).reduce((n, i) => n + 1 + (i.kind === "folder" ? quickCount(i) : 0), 0);
}
// A secret named in a body, as the app reads one (quick.rs SECRET_REF)
const quickSecretRe = () => new RegExp(QUICK.secretRef, "g");
function quickSecretNames(body) {
  const out = [];
  for (const m of String(body || "").matchAll(quickSecretRe())) if (!out.includes(m[1])) out.push(m[1]);
  return out;
}
// Written the way the app reads it. Put together here rather than typed out,
// because a pair of braces in this page's source is where its words go
const quickSecretRef = name => "{" + "{secrets." + name + "}" + "}";

function quickCard() {
  const body = el("div", {class:"qedit"}, el("div", {class:"hint"}, "…"));
  const box = card(T["settings.quick.title"],
    el("div", {class:"hint"}, T["settings.quick.intro"]), body);
  const draw = () => drawQuick(body, draw);
  (async () => {
    // A file written by hand is shown as the app would place it, ids and all.
    // Saving writes that; merely opening changes nothing
    if (isObj(current.quick_commands)) {
      const a = await quickArrange(current.quick_commands);
      if (a) current.quick_commands = a;
    }
    draw();
  })();
  return box;
}

function drawQuick(body, draw) {
  const q = quickSpec();
  const {cols, rows} = quickDims();
  const here = quickHolder(q);
  const pages = Math.max(1, here.pages || 1);
  if (quickAt.page >= pages) quickAt.page = pages - 1;
  const items = here.items || [];
  const at = (row, col) => items.find(i => i.page === quickAt.page && i.row === row && i.col === col);
  body.textContent = "";

  // The grid's size. Changing it asks the app where the buttons go now
  const size = (label, value, max, key) => {
    const s = el("select");
    for (let n = 1; n <= max; n++) s.append(el("option", {value:String(n)}, String(n)));
    s.value = String(value);
    s.addEventListener("change", async () => {
      quickOwn()[key] = Number(s.value);
      quickAt.pick = null;
      await quickSettle();
      draw();
    });
    return el("div", {class:"field"}, el("label", {}, label), el("div", {class:"fieldctl"}, s));
  };
  body.append(el("div", {class:"row2 qsize"},
      size(T["settings.quick.cols"], cols, QUICK.colsMax, "cols"),
      size(T["settings.quick.rows"], rows, QUICK.rowsMax, "rows")),
    el("div", {class:"hint qsizehint"}, T["settings.quick.size.hint"]));

  // Where in the folders this is. At the top the card's own title says it
  const crumbs = el("div", {class:"qcrumbs", hidden: quickAt.path.length ? null : ""});
  const goUp = n => { quickAt.path = quickAt.path.slice(0, n); quickAt.page = 0; quickAt.pick = null; draw(); };
  crumbs.append(el("button", {class:"quiet", onclick:() => goUp(0)}, T["settings.quick.top"]));
  let walk = q;
  quickAt.path.forEach((id, n) => {
    walk = (walk.items || []).find(i => i.id === id) || {};
    crumbs.append(el("span", {class:"qsep"}, "›"),
      el("button", {class:"quiet", onclick:() => goUp(n + 1)}, walk.label || T["settings.quick.folder.unnamed"]));
  });
  body.append(crumbs);

  // The grid
  // Each place is a square as wide as the card allows, up to 88px. Worked out
  // from the width rather than left to the stylesheet's aspect ratio, which a
  // grid stretches out of shape once the places get narrow
  const grid = el("div", {class:"qegrid",
    style:"grid-template-columns:repeat(" + cols + ", var(--qs, 88px))"});
  const pickSlot = (row, col) => { quickAt.pick = {row, col}; draw(); };
  const enter = f => { quickAt.path.push(f.id); quickAt.page = 0; quickAt.pick = null; draw(); };
  for (let row = 0; row < rows; row++) {
    for (let col = 0; col < cols; col++) {
      const item = at(row, col);
      const way = quickWayOut(row, col);
      const picked = !!quickAt.pick && quickAt.pick.row === row && quickAt.pick.col === col;
      const slot = el("div", {class:"qslot" + (way ? " back" : item ? " item" : " vacant") + (picked ? " sel" : ""),
        tabindex:"0", role:"button", "data-row":String(row), "data-col":String(col),
        title: way ? T["settings.quick.back"] : item ? (item.label || "") : T["settings.quick.empty.title"]});
      if (way) slot.append(quickFace({kind:"back"}));
      else if (item) slot.append(quickFace(item, quickSvg));
      slot.addEventListener("click", () => {
        if (quickDragged) return;
        if (way) { goUp(quickAt.path.length - 1); return; }
        pickSlot(row, col);
      });
      // A folder is walked into with a double press, as a folder is anywhere
      // else; a single press picks it, to rename it or change its picture
      slot.addEventListener("dblclick", () => { if (item && item.kind === "folder") enter(item); });
      slot.addEventListener("keydown", e => {
        if (e.key === "Enter" && item && item.kind === "folder") { e.preventDefault(); enter(item); }
        else if (e.key === "Enter" || e.key === " ") { e.preventDefault(); slot.click(); }
        else if ((e.key === "Delete" || e.key === "Backspace") && item) { e.preventDefault(); quickDelete(item, draw); }
      });
      slot.addEventListener("contextmenu", e => {
        e.preventDefault();
        if (way) return;
        const choices = item
          ? [item.kind === "folder" ? [T["settings.quick.menu.open"], () => enter(item)] : null,
             [T["common.delete"], () => quickDelete(item, draw), true]]
          : [[T["settings.quick.menu.command"], () => quickCreate("command", row, col, draw)],
             [T["settings.quick.menu.folder"], () => quickCreate("folder", row, col, draw)]];
        quickMenu(e.clientX, e.clientY, choices.filter(Boolean));
      });
      if (item) quickDraggable(slot, item, draw);
      grid.append(slot);
    }
  }
  // On a phone's width the names do not fit under the pictures; the picture
  // stays, and the name is in the panel below
  const fit = () => {
    const room = body.clientWidth;
    if (!room) return;
    const size = Math.max(28, Math.min(88, Math.floor((room - 8 * (cols - 1)) / cols)));
    grid.style.setProperty("--qs", size + "px");
    grid.classList.toggle("tiny", size < 60);
  };
  if (window.ResizeObserver) new ResizeObserver(fit).observe(body);
  requestAnimationFrame(fit);
  // Holding a button down on a touch screen is how it is picked up; while one
  // is being carried, the page does not scroll under the finger
  grid.addEventListener("touchmove", e => { if (quickCarrying) e.preventDefault(); }, {passive:false});
  body.append(grid);

  // The pages
  const pager = el("div", {class:"qpager"});
  for (let p = 0; p < pages; p++) {
    pager.append(el("button", {class:"qpage" + (p === quickAt.page ? " on" : ""), "data-page":String(p),
      title: fill(T["settings.quick.page"], {n: p + 1}),
      onclick:() => { quickAt.page = p; quickAt.pick = null; draw(); }}, String(p + 1)));
  }
  if (pages < QUICK.pagesMax) {
    pager.append(el("button", {class:"quiet", title:T["settings.quick.page.add"],
      onclick:() => {
        const h = quickHolder(quickOwn());
        h.pages = pages + 1;
        quickAt.page = pages; quickAt.pick = null;
        refreshSave(); draw();
      }}, "+"));
  }
  const lastEmpty = pages > 1 && !items.some(i => i.page === pages - 1);
  if (lastEmpty) {
    pager.append(el("button", {class:"quiet", title:T["settings.quick.page.remove"],
      onclick:() => {
        const h = quickHolder(quickOwn());
        h.pages = pages - 1;
        if (quickAt.page >= pages - 1) { quickAt.page = pages - 2; quickAt.pick = null; }
        refreshSave(); draw();
      }}, "−"));
  }
  body.append(pager);

  // What is picked
  if (quickAt.pick && !quickWayOut(quickAt.pick.row, quickAt.pick.col)) {
    body.append(quickPanel(at(quickAt.pick.row, quickAt.pick.col), quickAt.pick, draw, enter));
  } else {
    body.append(el("div", {class:"hint qpanelhint"}, T["settings.quick.pick.hint"]));
  }
}

// A button being carried to another place. A mouse picks it up on the first
// few pixels of movement; a finger has to hold still for a moment first, so a
// swipe past the grid still scrolls the page
let quickCarrying = false, quickDragged = false;
function quickDraggable(slot, item, draw) {
  slot.addEventListener("pointerdown", e => {
    if (e.button !== 0) return;
    const x0 = e.clientX, y0 = e.clientY;
    const touch = e.pointerType !== "mouse";
    let ghost = null, over = null, timer = null, last = e;
    const begin = () => {
      quickCarrying = true;
      ghost = el("div", {class:"qghost"});
      ghost.append(quickFace(item, quickSvg));
      document.body.append(ghost);
      slot.classList.add("carried");
      place(last);
    };
    const place = ev => {
      ghost.style.left = ev.clientX + "px";
      ghost.style.top = ev.clientY + "px";
      const under = document.elementFromPoint(ev.clientX, ev.clientY);
      const target = under && under.closest(".qslot, .qpage");
      if (target !== over) {
        if (over) over.classList.remove("over");
        over = target === slot ? null : target;
        if (over) over.classList.add("over");
      }
    };
    const move = ev => {
      last = ev;
      if (!ghost) {
        const far = Math.hypot(ev.clientX - x0, ev.clientY - y0);
        if (touch) { if (far > 8) finish(); return; }
        if (far < 5) return;
        begin();
      }
      ev.preventDefault();
      place(ev);
    };
    const finish = () => {
      clearTimeout(timer);
      removeEventListener("pointermove", move);
      removeEventListener("pointerup", up);
      removeEventListener("pointercancel", finish);
      quickCarrying = false;
      if (!ghost) return;
      ghost.remove();
      slot.classList.remove("carried");
      if (over) over.classList.remove("over");
      // The click that follows a drop is not a press
      quickDragged = true;
      setTimeout(() => { quickDragged = false; }, 0);
    };
    const up = () => {
      const target = over;
      const carried = !!ghost;
      finish();
      if (carried && target) quickDrop(item, target, draw);
    };
    if (touch) timer = setTimeout(() => { if (!ghost) begin(); }, 350);
    addEventListener("pointermove", move, {passive:false});
    addEventListener("pointerup", up);
    addEventListener("pointercancel", finish);
  });
}

// Where a carried button ends up: a free place, a place another button holds
// (the two change places), a folder (into it), the way out (into the grid
// the folder is in), or a page number (onto that page)
async function quickDrop(item, target, draw) {
  const q = quickOwn();
  const here = quickHolder(q);
  const list = here.items || (here.items = []);
  const idx = list.findIndex(i => i.id === item.id);
  if (idx < 0) return;
  const moved = list[idx];
  if (target.classList.contains("qpage")) {
    const p = Number(target.dataset.page);
    if (p === quickAt.page) return;
    // The app moves it off a place that is taken
    Object.assign(moved, {page: p, row: 0, col: 0});
    quickAt.page = p;
    quickAt.pick = null;
  } else if (target.classList.contains("back")) {
    const folderId = quickAt.path[quickAt.path.length - 1];
    let parent = q;
    for (const id of quickAt.path.slice(0, -1)) parent = parent.items.find(i => i.id === id);
    const folder = parent.items.find(i => i.id === folderId);
    list.splice(idx, 1);
    Object.assign(moved, {page: folder ? folder.page : 0, row: 0, col: 0});
    parent.items.push(moved);
    quickAt.pick = null;
  } else {
    const row = Number(target.dataset.row), col = Number(target.dataset.col);
    const other = list.find(i => i.page === quickAt.page && i.row === row && i.col === col);
    if (other && other.id === moved.id) return;
    if (other && other.kind === "folder") {
      list.splice(idx, 1);
      if (!Array.isArray(other.items)) other.items = [];
      // The first place of a folder is its way out, so the app finds this one
      // the first free place instead
      Object.assign(moved, {page: 0, row: 0, col: 0});
      other.items.push(moved);
      quickAt.pick = null;
    } else {
      if (other) Object.assign(other, {page: moved.page, row: moved.row, col: moved.col});
      Object.assign(moved, {page: quickAt.page, row, col});
      quickAt.pick = {row, col};
    }
  }
  await quickSettle();
  draw();
}

function quickCreate(kind, row, col, draw) {
  const here = quickHolder(quickOwn());
  if (!Array.isArray(here.items)) here.items = [];
  const folder = kind === "folder";
  here.items.push(Object.assign({id: quickNewId(), page: quickAt.page, row, col, icon: "",
    label: folder ? T["settings.quick.folder.new"] : "", kind: folder ? "folder" : "terminal",
    body: "", enter: true}, folder ? {pages: 1, items: []} : {}));
  quickAt.pick = {row, col};
  refreshSave();
  draw();
  const name = document.querySelector(".qpanel input.qlabel");
  if (name) { name.focus(); name.select(); }
}

async function quickDelete(item, draw) {
  const n = item.kind === "folder" ? quickCount(item) : 0;
  if (n && !await confirmAction(fill(T["settings.quick.delete.folder"], {name: item.label || "", n}),
                                T["common.delete"])) return;
  const here = quickHolder(quickOwn());
  here.items = (here.items || []).filter(i => i.id !== item.id);
  quickAt.pick = null;
  refreshSave();
  draw();
}

// A small menu at the pointer. The one kind of floating list this page has
function quickMenu(x, y, rows) {
  document.querySelectorAll(".fmenu.qmenu").forEach(m => m.remove());
  const menu = el("div", {class:"fmenu qmenu", role:"menu"});
  const close = () => { menu.remove(); removeEventListener("mousedown", outside, true); removeEventListener("keydown", esc, true); };
  const outside = e => { if (!menu.contains(e.target)) close(); };
  const esc = e => { if (e.key === "Escape") { e.stopPropagation(); close(); } };
  for (const [label, act, bad] of rows) {
    menu.append(el("button", {class:"fmenuitem" + (bad ? " bad" : ""), role:"menuitem",
      onclick:() => { close(); act(); }}, label));
  }
  document.body.append(menu);
  const r = menu.getBoundingClientRect();
  menu.style.left = Math.max(8, Math.min(x, innerWidth - r.width - 8)) + "px";
  menu.style.top = Math.max(8, Math.min(y, innerHeight - r.height - 8)) + "px";
  setTimeout(() => { addEventListener("mousedown", outside, true); addEventListener("keydown", esc, true); }, 0);
  const first = menu.querySelector("button");
  if (first) first.focus();
}

// The picked place: what to make there, or the button that is there
function quickPanel(item, spot, draw, enter) {
  const panel = el("div", {class:"qpanel"});
  if (!item) {
    panel.append(el("div", {class:"qpanelhead"}, T["settings.quick.empty.head"]),
      el("div", {class:"row2 qmake"},
        el("button", {onclick:() => quickCreate("command", spot.row, spot.col, draw)}, T["settings.quick.make.command"]),
        el("button", {onclick:() => quickCreate("folder", spot.row, spot.col, draw)}, T["settings.quick.make.folder"])),
      el("div", {class:"hint"}, T["settings.quick.empty.hint"]));
    return panel;
  }
  const folder = item.kind === "folder";
  const preview = el("div", {class:"qpreview"});
  const repaint = () => {
    preview.textContent = "";
    preview.append(quickFace(item, quickSvg));
    const slot = document.querySelector(".qegrid .qslot.sel");
    if (slot) { slot.textContent = ""; slot.append(quickFace(item, quickSvg)); slot.title = item.label || ""; }
  };
  repaint();

  // The picture
  const iconBtn = el("button", {onclick:() => quickIconPicker(item.icon, name => {
    item.icon = name; refreshSave(); repaint();
    clearIcon.hidden = !item.icon;
  })}, T["settings.quick.icon.pick"]);
  const clearIcon = el("button", {class:"quiet", onclick:() => {
    item.icon = ""; refreshSave(); repaint(); clearIcon.hidden = true;
  }}, T["settings.quick.icon.none"]);
  clearIcon.hidden = !item.icon;
  const fields = el("div", {class:"qfields"},
    el("div", {class:"field"}, el("label", {}, T["settings.quick.icon"]),
      el("div", {class:"fieldctl"}, el("div", {class:"row2"}, iconBtn, clearIcon))));

  // The name
  const label = el("input", {type:"text", class:"qlabel", maxlength:String(QUICK.labelMax),
    placeholder: folder ? T["settings.quick.folder.new"] : T["settings.quick.label.ph"]});
  label.value = item.label || "";
  label.addEventListener("input", () => { item.label = label.value; refreshSave(); repaint(); });
  fields.append(el("div", {class:"field"}, el("label", {}, T["settings.quick.label"]),
    el("div", {class:"fieldctl"}, label)));

  if (folder) {
    const n = quickCount(item);
    fields.append(el("div", {class:"field"}, el("label", {}, T["settings.quick.folder.inside"]),
      el("div", {class:"fieldctl"}, el("div", {class:"row2"},
        el("span", {class:"qcount"}, fill(T["settings.quick.folder.count"], {n})),
        el("button", {onclick:() => enter(item)}, T["settings.quick.menu.open"]))),
      el("div", {class:"hint"}, T["settings.quick.folder.hint"])));
  } else {
    // Who it is for
    const kinds = el("div", {class:"qseg", role:"radiogroup"});
    // Which AI a prompt is for. The same list the tab form offers
    const aiPick = el("select", {});
    aiPick.append(el("option", {value:""}, T["settings.quick.ai.any"]));
    for (const a of AI_CLIS) aiPick.append(el("option", {value:a.cmd}, a.label));
    aiPick.value = item.ai || "";
    aiPick.addEventListener("change", () => { item.ai = aiPick.value; if (!item.ai) delete item.ai; refreshSave(); });
    const aiField = el("div", {class:"field"}, el("label", {}, T["settings.quick.ai"]),
      el("div", {class:"fieldctl"}, aiPick), el("div", {class:"hint"}, T["settings.quick.ai.hint"]));
    const bodyLabel = el("label", {});
    const bodyIn = el("textarea", {rows:"4", class:"qbody", maxlength:String(QUICK.bodyMax), spellcheck:"false"});
    const bodyHint = el("div", {class:"hint"});
    const secretNote = el("div", {class:"site-warn"});
    const drawKind = () => {
      kinds.textContent = "";
      for (const k of ["terminal", "ai"]) {
        const on = (item.kind || "terminal") === k;
        kinds.append(el("button", {class:"tog" + (on ? " on" : ""), role:"radio", "aria-checked":String(on),
          onclick:() => { item.kind = k; if (k !== "ai") delete item.ai; refreshSave(); drawKind(); repaint(); }},
          T["settings.quick.kind." + k]));
      }
      const ai = item.kind === "ai";
      aiField.hidden = !ai;
      bodyLabel.textContent = T[ai ? "settings.quick.body.prompt" : "settings.quick.body.command"];
      bodyIn.placeholder = T[ai ? "settings.quick.body.prompt.ph" : "settings.quick.body.command.ph"];
      bodyIn.classList.toggle("mono", !ai);
      bodyHint.textContent = T[ai ? "settings.quick.body.prompt.hint" : "settings.quick.body.command.hint"];
      drawSecrets();
    };
    // What a secret named in the body means, said while it is being written
    const drawSecrets = () => {
      const names = quickSecretNames(item.body);
      secretNote.hidden = !names.length;
      if (!names.length) return;
      secretNote.textContent = fill(T[item.kind === "ai" ? "settings.quick.secret.ai" : "settings.quick.secret.terminal"],
        {names: names.map(name => fill(T["settings.quick.secret.name"], {name})).join("")});
    };
    bodyIn.value = item.body || "";
    bodyIn.addEventListener("input", () => { item.body = bodyIn.value; refreshSave(); drawSecrets(); });
    const secretBtn = el("button", {class:"quiet", onclick: e => quickSecretMenu(e, bodyIn, () => {
      item.body = bodyIn.value; refreshSave(); drawSecrets();
    })}, T["settings.quick.secret.insert"]);
    const enterBox = el("input", {type:"checkbox"});
    enterBox.checked = item.enter !== false;
    enterBox.addEventListener("change", () => { item.enter = enterBox.checked; refreshSave(); });
    fields.append(
      el("div", {class:"field"}, el("label", {}, T["settings.quick.kind"]), el("div", {class:"fieldctl"}, kinds)),
      aiField,
      el("div", {class:"field"}, bodyLabel,
        el("div", {class:"fieldctl"}, bodyIn, secretNote, el("div", {class:"row2"}, secretBtn)),
        bodyHint),
      el("div", {class:"field"}, el("label", {class:"check"}, enterBox, T["settings.quick.enter"]),
        el("div", {class:"hint"}, T["settings.quick.enter.hint"])));
    drawKind();
  }
  panel.append(el("div", {class:"qpanelrow"}, preview, fields),
    el("div", {class:"qdelrow"}, el("button", {class:"danger", onclick:() => quickDelete(item, draw)},
      T[folder ? "settings.quick.delete.folder.button" : "settings.quick.delete.button"])));
  return panel;
}

// The secrets a body can name. They belong to desks, and a quick command
// belongs to none, so each name is listed with the desks that have it; the
// one used is the one of the desk on screen when the button is pressed
async function quickSecretMenu(e, input, changed) {
  const r = e.currentTarget.getBoundingClientRect();
  const j = await fetchSecrets();
  const byName = new Map();
  for (const s of ((j && j.secrets) || [])) {
    const key = String(s.key || "");
    if (key.includes("/")) continue;
    const dot = key.indexOf(".");
    if (dot < 1) continue;
    const deskId = key.slice(0, dot), name = key.slice(dot + 1);
    const desk = desks.find(d => (d.id || "") === deskId);
    if (!byName.has(name)) byName.set(name, []);
    byName.get(name).push(desk ? (desk.name || deskId) : deskId);
  }
  const rows = [...byName.entries()].sort((a, b) => a[0].localeCompare(b[0])).map(([name, where]) =>
    [name + "  ·  " + where.join(", "), () => {
      const ref = quickSecretRef(name);
      const s = input.selectionStart ?? input.value.length, t = input.selectionEnd ?? s;
      input.value = input.value.slice(0, s) + ref + input.value.slice(t);
      input.focus();
      input.setSelectionRange(s + ref.length, s + ref.length);
      changed();
    }]);
  if (!rows.length) { toast(T["settings.quick.secret.none"], true); return; }
  quickMenu(r.left, r.bottom + 4, rows);
}

// Choosing a picture: a search over the whole set, with the common ones first
async function quickIconPicker(now, done) {
  const search = el("input", {type:"text", placeholder:T["settings.quick.icon.search"], autocomplete:"off", spellcheck:"false"});
  const list = el("div", {class:"qicons"}, el("div", {class:"hint"}, "…"));
  const close = () => back.remove();
  const inner = [
    el("div", {class:"mhead"}, el("h2", {}, T["settings.quick.icon.title"]),
      el("button", {class:"quiet icon", title:T["common.close"], "aria-label":T["common.close"], onclick:close}, "✕")),
    el("div", {class:"mbody"}, search, list),
    el("div", {class:"mfoot"}, el("span", {class:"grow"}),
      el("button", {class:"quiet", onclick:close}, T["common.cancel"]))];
  const back = openModal(...inner);
  back.querySelector(".modal-inner").classList.add("framed", "qpicker");
  back.addEventListener("keydown", e => { if (e.key === "Escape") { e.preventDefault(); close(); } });
  if (!quickSet) {
    try {
      const txt = await fetch(QUICK.icons, {headers:{"X-Token":TOKEN}}).then(r => r.text());
      quickSet = txt.split("\n").filter(l => l && l[0] !== "#").map(l => {
        const [name, words, svg] = l.replace(/\r$/, "").split("\t");
        return {name, words: (words || "").toLowerCase(), svg: svg || ""};
      });
    } catch (e) { quickSet = null; }
  }
  if (!quickSet) { list.textContent = ""; list.append(el("div", {class:"hint"}, T["settings.quick.icon.failed"])); return; }
  const byName = new Map(quickSet.map(i => [i.name, i]));
  let shown = 0, matches = [];
  const button = i => {
    const b = el("button", {class:"qicon" + (i.name === now ? " on" : ""), title:i.name,
      onclick:() => { quickSvgs[i.name] = i.svg; close(); done(i.name); }});
    const s = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    s.setAttribute("viewBox", "0 0 24 24");
    s.innerHTML = i.svg;
    b.append(s);
    return b;
  };
  const more = () => {
    const next = matches.slice(shown, shown + 240);
    shown += next.length;
    list.querySelector(".qmore")?.remove();
    for (const i of next) list.append(i.head ? el("div", {class:"qiconhead"}, i.head) : button(i));
    if (shown < matches.length) {
      list.append(el("button", {class:"quiet qmore", onclick:more}, T["settings.quick.icon.more"]));
    }
  };
  const refill = () => {
    const word = search.value.trim().toLowerCase();
    list.textContent = "";
    shown = 0;
    if (word) {
      matches = quickSet.filter(i => i.name.includes(word) || i.words.includes(word));
      if (!matches.length) { list.append(el("div", {class:"hint"}, T["settings.quick.icon.nothing"])); return; }
    } else {
      const common = QUICK_COMMON_ICONS.map(n => byName.get(n)).filter(Boolean);
      matches = [{head: T["settings.quick.icon.common"]}, ...common,
                 {head: T["settings.quick.icon.all"]}, ...quickSet];
    }
    more();
  };
  search.addEventListener("input", refill);
  refill();
  search.focus();
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
    el("div", {class:"hint", style:"margin-bottom:var(--s3)"}, T["settings.logins.intro"]),
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
  const grid = el("div", {style:"display:flex;flex-wrap:wrap;gap:var(--s3)"}, el("div", {class:"hint"}, "…"));
  const box = card(T["settings.sec.snapshots"],
    el("div", {class:"hint", style:"margin-bottom:var(--s3)"}, T["settings.snapshots.intro"]),
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
        style:"width:220px;height:140px;object-fit:cover;object-position:top;border:1px solid var(--line);border-radius:var(--r-ctl);background:var(--bg)"});
      const row = el("div", {style:"display:flex;align-items:center;gap:var(--s2);margin-top:var(--s1)"});
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
  // A secret goes when the thing that used it goes, so this should never have
  // anything to say. It says something when a settings file was edited by hand
  // or an older version left something behind -- and nothing at all otherwise
  const tidy = el("div", {class:"row"});
  tidy.hidden = true;
  const lookForOrphans = async () => {
    const j = await fetch("/api/secrets/orphans", {headers:{"X-Token":TOKEN}})
      .then(r => r.json()).catch(() => null);
    const list = (j && j.orphans) || [];
    tidy.textContent = "";
    tidy.hidden = !list.length;
    if (!list.length) return;
    tidy.append(
      el("span", {class:"hint warn"}, fill(T["settings.secrets.orphans"], {n: list.length})),
      el("button", {onclick: async () => {
        const lines = list.join(String.fromCharCode(10));
        if (!await confirmAction(fill(T["settings.secrets.orphans_confirm"], {list: lines}), T["settings.secrets.orphans_clean"])) return;
        await dropSecrets(list);
        toast(fill(T["settings.secrets.orphans_done"], {n: list.length}));
        lookForOrphans();
      }}, T["settings.secrets.orphans_clean"]));
  };
  setTimeout(lookForOrphans, 0);
  return card(T["settings.section.files"],
    row(T["settings.automation_global"], ...pathField(current, "automation", "scripts/common", "dir",
        T["settings.tab.automation_dir.pick"]),
        el("span", {class:"hint"}, T["settings.automation_global.hint"])),
    row("secrets", ...pathField(current, "secrets", "secrets.json", "file",
        T["settings.secrets"]),
        el("span", {class:"hint"}, T["settings.secrets.hint"])),
    tidy);
}
// Ordered flat list of the global cards. `id` is the stable deep-link handle
// (the sub-input bar's ⚙ opens ?section=actions, for instance).
function globalSections() {
  return [
    {id:"basic",     label:T["settings.sec.basic"],     sub:T["settings.sec.basic.sub"],     build:basicCard},
    {id:"update",    label:T["settings.sec.update"],    sub:T["settings.sec.update.sub"],    build:updateCard},
    {id:"remote",    label:T["settings.sec.remote"],    sub:T["settings.sec.remote.sub"],    build:remoteCard},
    // Two cards: the keys that work from any program, then the keys inside
    {id:"keys",      label:T["settings.sec.keys"],      sub:T["settings.sec.keys.sub"],
     build:() => el("div", {}, hotkeysCard(), keysCard())},
    {id:"quick",     label:T["settings.sec.quick"],     sub:T["settings.sec.quick.sub"],     build:quickCard},
    {id:"actions",   label:T["settings.sec.actions"],   sub:T["settings.sec.actions.sub"],   build:actionsCard},
    {id:"hosts",     label:T["settings.sec.hosts"],     sub:T["settings.sec.hosts.sub"],     build:hostsCard},
    {id:"servers",   label:T["settings.sec.servers"],   sub:T["settings.sec.servers.sub"],   build:marksCard},
    {id:"operate",   label:T["settings.sec.operate"],   sub:T["settings.sec.operate.sub"],   build:operateCard},
    {id:"claudeusage", label:T["settings.sec.claudeusage"], sub:T["settings.sec.claudeusage.sub"], build:claudeUsageCard},
    {id:"api",       label:T["settings.sec.api"],       sub:T["settings.sec.api.sub"],       build:apiCard},
    {id:"resume",    label:T["settings.sec.resume"],    sub:T["settings.sec.resume.sub"],    build:resumeCard},
    {id:"files",     label:T["settings.sec.files"],     sub:T["settings.sec.files.sub"],     build:filesCard},
    {id:"results",   label:T["settings.sec.results"],   sub:T["settings.sec.results.sub"],   build:rallyResultCard},
    // The phones themselves are this machine's: a phone signs itself up once.
    // Which desk's messages reach it is that desk's page's question
    {id:"notify",    label:T["settings.sec.notify"],    sub:T["settings.sec.notify.sub"],    build:phoneNotifyCard},
    // What automation left behind on this machine: kept last, as housekeeping
    {id:"logins",    label:T["settings.sec.logins"],    sub:T["settings.sec.logins.sub"],    build:loginsCard},
    {id:"snapshots", label:T["settings.sec.snapshots"], sub:T["settings.sec.snapshots.sub"], build:snapshotsCard},
  ];
}

// Links that name one of a desk's settings (the git panel's gear asks for
// "git"): the desk in view, at that entry, since there is no copy of the
// program's to land on. Older names for the same places are kept here
const DESK_LINKS = {git:"git", "git-message":"git", "git-issue":"git", "git-pr":"git", "git-merge":"git", "git-ci":"git", protect:"git", gitaccounts:"gitaccounts", providers:"providers", browser:"browser", words:"browser",
                    permissions:"permissions", caps:"caps", tools:"tools"};

// ── Update ─────────────────────────────────────────────────────
// The one place a newer version is fetched, checked and put in place. The
// sidebar's card and a person who came here on their own press the same
// button, so one road carries everyone -- and a broken road is noticed.
// Draws from /api/update; polls once a second only while something is under
// way, and stops the moment the card leaves the screen.
let updateTimer = null;
function updateCard() {
  const box = el("div", {id:"updatebox"}, el("div", {class:"hint"}, "…"));
  const auto = row(T["settings.update.auto"], checkDefaultOn(current, "update_check", T["settings.update.auto.label"]),
    el("span", {class:"hint"}, T["settings.update.auto.hint"]));
  setTimeout(refreshUpdate, 0);
  return card(T["settings.update.title"], box, auto);
}
async function updateApi(path, post) {
  const r = await fetch("/api/update" + path, {method: post ? "POST" : "GET", headers:{"X-Token":TOKEN}});
  return r.json();
}
async function refreshUpdate() {
  const box = document.getElementById("updatebox");
  if (!box) { if (updateTimer) { clearInterval(updateTimer); updateTimer = null; } return; }
  let u = null;
  try { u = await updateApi(""); } catch (e) {
    box.textContent = ""; box.append(el("div", {class:"hint"}, T["settings.update.unreachable"])); return;
  }
  drawUpdate(box, u);
  const busy = ["checking", "downloading", "verifying", "applying"].includes(u.phase);
  if (busy && !updateTimer) updateTimer = setInterval(refreshUpdate, 1000);
  if (!busy && updateTimer) { clearInterval(updateTimer); updateTimer = null; }
}
// Megabytes with one decimal, for a person: "12.3 MB"
function mb(n) { return (n / 1048576).toFixed(1) + " MB"; }
function drawUpdate(box, u) {
  box.textContent = "";
  const act = async (path) => { try { await updateApi(path, true); } catch (e) {} refreshUpdate(); };
  // Line 1: this version, and when the newest was last looked for
  const when = u.checked_at ? fill(T["settings.update.checked"], {when: new Date(u.checked_at * 1000).toLocaleString()})
                            : T["settings.update.never"];
  box.append(el("div", {class:"row"},
    el("span", {}, fill(T["settings.update.current"], {version: u.current})),
    el("span", {class:"hint", style:"flex-basis:auto"}, when),
    // (an attribute, so absent rather than "false": disabled="false" still disables)
    el("button", {class:"quiet", disabled: u.phase === "checking" ? "" : null, onclick: () => act("/check")},
      u.phase === "checking" ? T["settings.update.checking"] : T["settings.update.check"])));
  // Line 2: where things stand, and the one button
  const v = u.version || "";
  const state = el("div", {class:"row", style:"align-items:center"});
  const main = el("div", {class:"row", style:"gap:var(--s2)"});
  const text = (k, args) => el("span", {}, fill(T[k] || k, args || {}));
  const notes = () => u.notes ? el("a", {href: REMOTE ? u.notes : "#", target: REMOTE ? "_blank" : null, rel:"noopener",
      onclick: REMOTE ? null : (e) => { e.preventDefault(); fetch("/api/open?dest=update-notes", {headers:{"X-Token":TOKEN}}); }},
      T["settings.update.notes"]) : null;
  const primary = (label, path) => el("button", {class:"primary", onclick: () => act(path)}, label);
  const quiet = (label, path) => el("button", {class:"quiet", onclick: () => act(path)}, label);
  switch (u.phase) {
    case "idle":
    case "checking":
      break;
    case "up_to_date":
      state.append(text("settings.update.uptodate"));
      break;
    case "available":
      state.append(text(u.packaged ? "settings.update.available.store" : "settings.update.available", {version: v}), notes());
      main.append(primary(u.packaged ? T["settings.update.install.store"] : T["settings.update.install"], "/install"),
                  quiet(T["settings.update.skip"], "/skip"));
      break;
    case "downloading": {
      const pct = u.total ? Math.min(100, Math.round(u.got * 100 / u.total)) : 0;
      const bar = el("div", {class:"ubar"}, el("i", {style:"width:" + pct + "%"}));
      state.append(el("div", {style:"flex:1 1 100%"}, bar,
        el("div", {class:"hint"}, u.total ? fill(T["settings.update.downloading"], {got: mb(u.got), total: mb(u.total)})
                                          : fill(T["settings.update.downloading.some"], {got: mb(u.got)}))));
      break;
    }
    case "verifying":
      state.append(text("settings.update.verifying"));
      break;
    case "staged":
      state.append(text("settings.update.staged", {version: v}), notes());
      main.append(primary(T["settings.update.install.staged"], "/install"),
                  quiet(T["settings.update.skip"], "/skip"),
                  quiet(T["settings.update.discard"], "/discard"));
      break;
    case "applying":
      state.append(text("settings.update.applying"));
      break;
    case "check_failed":
      state.append(el("span", {style:"color:var(--danger)"}, fill(T["settings.update.failed.check"], {message: u.message || ""})));
      break;
    case "failed":
      state.append(el("span", {style:"color:var(--danger)"},
        v ? fill(T["settings.update.failed"], {version: v, message: u.message || ""})
          : fill(T["settings.update.failed.store"], {message: u.message || ""})));
      main.append(primary(u.packaged ? T["settings.update.install.store"] : T["settings.update.install"], "/install"),
                  quiet(T["settings.update.skip"], "/skip"));
      break;
  }
  if (state.childNodes.length) box.append(state);
  if (main.childNodes.length) {
    box.append(main);
    if (REMOTE) box.append(el("div", {class:"hint"}, T["settings.update.phone"]));
    if (!u.packaged) box.append(el("div", {class:"hint"}, T["settings.update.sync_hint"]));
  }
  // Line 3: what the last update did, and the way back
  if (u.migration_failed) {
    box.append(el("div", {class:"hint", style:"color:var(--danger)"},
      fill(T["settings.update.carry_failed"], {version: u.migrated_from || "", message: u.migration_failed, path: u.backup || ""})));
  } else if (u.migrated_from) {
    box.append(el("div", {class:"hint"}, fill(T["settings.update.carried"], {version: u.migrated_from})));
  }
  if (u.backup) box.append(el("div", {class:"hint"}, fill(T["settings.update.backup"], {path: u.backup})));
  if (u.prev && !u.packaged) {
    box.append(el("div", {class:"row", style:"margin-top:var(--s2)"},
      quiet(fill(T["settings.update.rollback"], {version: u.prev}), "/rollback"),
      el("span", {class:"hint"}, T["settings.update.rollback.hint"])));
  }
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
    box.append(el("div", {class:"row", style:"gap:var(--s3)"},
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

// ── Secrets ────────────────────────────────────────────────────────────────
const secretKey = (desk, name) => (desk.id || "") + "." + name;
// The short name of a secret belonging to this desk, or null for one that
// does not
function secretShortName(desk, key) {
  const head = (desk.id || "") + ".";
  return key.startsWith(head) ? key.slice(head.length) : null;
}
// Everything the store holds, asked for once and handed to whoever is drawing.
// The value is never part of it
async function fetchSecrets() {
  try { return await fetch("/api/secrets", {headers:{"X-Token":TOKEN}}).then(r=>r.json()); }
  catch (e) { return null; }
}
// A secret belongs to the thing that uses it, so it goes when that thing
// goes. Named as a "@ref" in the settings, or by the shape the program files
// it under. Nothing else knows these names, and there is no screen of
// leftovers to tidy them away from later
async function dropSecretRef(ref) {
  const r = (ref || "").trim();
  if (r.startsWith("@")) await deleteSecret(r.slice(1));
}
async function dropSecrets(keys) {
  for (const k of keys) await deleteSecret(k);
}
async function saveSecret(body) {
  return await fetch("/api/secrets/set", {method:"POST",
    headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
    body: JSON.stringify(body)}).then(r=>r.json()).catch(() => ({ok:false}));
}
async function deleteSecret(key) {
  return await fetch("/api/secrets/delete", {method:"POST",
    headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
    body: JSON.stringify({key})}).then(r=>r.json()).catch(() => ({ok:false}));
}
// One address, read the same way here as in config.rs. Everything below is the
// screen's half of that agreement: it refuses while somebody types what the
// store would refuse on arrival, so nothing is turned away by surprise
const isPlain = u => /^http:\/\//i.test((u || "").trim());
// A person who types "example.com" means the safe one. Filling the scheme in
// where they can see it beats a rule that says a bare name means https
const withScheme = v => {
  const t = (v || "").trim();
  if (!t || /^[a-z][a-z0-9+.-]*:\/\//i.test(t)) return t;
  return "https://" + t.replace(/^\/+/, "");
};
// The suffixes everybody shares: "*." in front of one of them is not a site,
// it is the whole internet with a shape
const SHARED_SUFFIX = new Set(["com","net","org","jp","io","dev","app","co","ne","or",
  "co.jp","ne.jp","or.jp","co.uk","com.au","com.br","co.kr","com.cn"]);
// Why this line cannot be used, as the name of the sentence to show, or null.
// The same answers, in the same order, as config.rs's url_fault
function urlFault(text) {
  const t = (text || "").trim();
  if (!t) return "err.secret_url.empty";
  if (/\s/.test(t)) return "err.secret_url.unreadable";
  const at = t.indexOf("://");
  if (at < 0) return "err.secret_url.scheme";
  const scheme = t.slice(0, at).toLowerCase();
  if (scheme !== "http" && scheme !== "https") return "err.secret_url.scheme";
  const host = t.slice(at + 3).split(/[/?#]/)[0].toLowerCase();
  if (!host || host.includes("@") || host.includes("[")) return "err.secret_url.unreadable";
  const stars = (host.match(/\*/g) || []).length;
  if (stars) {
    if (stars > 1 || !host.startsWith("*.")) return "err.secret_url.star_place";
    const under = host.slice(2).split(":")[0];
    if (under.split(".").length < 2 || SHARED_SUFFIX.has(under)) return "err.secret_url.star_wide";
  }
  const port = host.split(":")[1];
  if (port !== undefined && !/^\d{1,5}$/.test(port)) return "err.secret_url.unreadable";
  return null;
}

// Secrets (equivalent to GitHub Secrets). Referenced by key; once saved, the value is never shown again.
// Model connections (Providers). An OpenAI-compatible API registered by name,
// so a tab's command can say `model <name>/<model>`.
//
// A boxed list you read down and one dialog to change one of them, the same
// shape as every other list of records on this page. The key is write-only:
// it is kept in the secrets file and never comes back to the screen.
// A desk's model connections. Each desk registers its own: the account behind
// a connection is billed for the work and handed the code, so a connection
// registered in the work desk is simply not on the list in the personal one
function providersCard(desk) {
  desk.providers = desk.providers || {};
  const listBox = el("div", {id:"providerslist"});
  const draw = () => {
    listBox.textContent = "";
    const names = Object.keys(desk.providers);
    if (!names.length) {
      listBox.append(el("div", {class:"hint"}, T["settings.providers.empty"]));
      return;
    }
    const rows = el("div", {class:"rows"});
    for (const name of names) {
      const p = desk.providers[name] || {};
      const held = (p.api_key || "").startsWith("@");
      rows.append(el("div", {class:"listrow secretrow", onclick: () => providerDialog(desk, name, draw)},
        el("span", {class:"mono secretname"}, name),
        el("span", {class:"hint mono secretdesc"}, p.base_url || T["settings.providers.no_url"]),
        el("span", {class:"hint"}, held ? "••••" : T["settings.providers.key_none"]),
        el("span", {class:"hint secretsite"}, waitText(p.timeout_sec)),
        el("span", {class:"hint"}, p.speaks === "choice" ? T["settings.providers.speaks.choice"] : ""),
        el("span", {class:"go"}, "›")));
    }
    listBox.append(rows);
  };
  const c = card(T["settings.providers.title"],
    el("div", {class:"hint"}, T["settings.providers.hint"]),
    listBox,
    el("div", {class:"row"},
      el("button", {onclick: () => providerDialog(desk, null, draw)}, T["settings.providers.add"])),
    el("div", {class:"hint"}, T["settings.providers.use_hint"]));
  c.id = "desk-providers";
  setTimeout(draw, 0);
  return c;
}
// The connections of the desk being edited, for the pickers that offer them
const deskProviders = () => ((desks[sel.desk] || {}).providers) || {};
// The models a connection really has, asked of the connection itself, so a
// name is picked from what exists instead of typed from memory. getProv()
// hands over that connection {base_url, api_key, headers?}; onPick(id) fills
// the field. Returns {btn, chips, load} -- the button goes inline and the
// chips on the line below
function modelCandidates(getProv, onPick) {
  const chips = el("div", {style:"display:flex;gap:var(--s2);flex-wrap:wrap;margin-top:var(--s2)"});
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
      style:"font-size:12px;padding:var(--s1) var(--s2)", onclick:() => onPick(id)}, id));
    return models;
  }
  const btn = el("button", {class:"quiet", type:"button", onclick: load}, T["settings.model.candidates"]);
  return {btn, chips, load};
}
// How long a reply may take, in the words the field uses. Blank is the app's
// own 180 seconds, and 0 is "as long as it takes"
const waitText = v => (v === undefined || v === null) ? fill(T["settings.providers.wait_default"], {n: 180})
  : (Number(v) === 0 ? T["settings.providers.wait_forever"] : fill(T["settings.providers.wait_n"], {n: v}));

// Services whose address is already known, so adding one is a pick rather
// than a search through somebody's documentation. `speaks` sorts them into the
// two kinds the type field names; `keys` is where that service hands out an
// API key; `here` marks a runner on this PC, which takes no key; `models` are
// the names to offer for a service that has no listing to ask (a decision
// endpoint answers questions and nothing else). Addresses checked against each
// service's own documentation, 2026-09
const PROVIDER_PRESETS = [
  {label:"OpenAI", name:"openai", speaks:"chat", url:"https://api.openai.com/v1",
   keys:"https://platform.openai.com/api-keys"},
  {label:"Anthropic (Claude)", name:"anthropic", speaks:"chat", url:"https://api.anthropic.com/v1/",
   keys:"https://platform.claude.com/settings/keys"},
  {label:"Google Gemini", name:"gemini", speaks:"chat", url:"https://generativelanguage.googleapis.com/v1beta/openai/",
   keys:"https://aistudio.google.com/apikey"},
  {label:"DeepSeek", name:"deepseek", speaks:"chat", url:"https://api.deepseek.com",
   keys:"https://platform.deepseek.com/api_keys"},
  {label:"xAI (Grok)", name:"xai", speaks:"chat", url:"https://api.x.ai/v1",
   keys:"https://console.x.ai"},
  {label:"Mistral", name:"mistral", speaks:"chat", url:"https://api.mistral.ai/v1",
   keys:"https://console.mistral.ai"},
  {label:"Moonshot AI (Kimi)", name:"kimi", speaks:"chat", url:"https://api.moonshot.ai/v1",
   keys:"https://platform.kimi.ai/console/api-keys"},
  {label:"MiniMax", name:"minimax", speaks:"chat", url:"https://api.minimax.io/v1"},
  {label:"Groq", name:"groq", speaks:"chat", url:"https://api.groq.com/openai/v1",
   keys:"https://console.groq.com/keys"},
  {label:"Cerebras", name:"cerebras", speaks:"chat", url:"https://api.cerebras.ai/v1",
   keys:"https://cloud.cerebras.ai"},
  {label:"OpenRouter", name:"openrouter", speaks:"chat", url:"https://openrouter.ai/api/v1",
   keys:"https://openrouter.ai/keys"},
  {label:"Together AI", name:"together", speaks:"chat", url:"https://api.together.ai/v1",
   keys:"https://api.together.ai/settings/api-keys"},
  {label:"Hugging Face", name:"huggingface", speaks:"chat", url:"https://router.huggingface.co/v1",
   keys:"https://huggingface.co/settings/tokens"},
  {label:"NVIDIA NIM", name:"nvidia", speaks:"chat", url:"https://integrate.api.nvidia.com/v1",
   keys:"https://build.nvidia.com"},
  {label:"Ollama", name:"ollama", speaks:"chat", url:"http://localhost:11434/v1", here:true},
  {label:"LM Studio", name:"lmstudio", speaks:"chat", url:"http://localhost:1234/v1", here:true},
  {label:"Jev (TypeSafe)", name:"jev", speaks:"choice", url:"https://api.typesafe.ai/v1/systemone",
   keys:"https://console.typesafe.ai", models:["jev-latest"]},
  {label:"Laya (impossibl)", name:"laya", speaks:"choice", url:"https://api.impossibl.com/v1/systemone",
   models:["convaiinnovations/laya", "convaiinnovations/laya-multilingual"]},
];

// Adding a connection to a desk, or changing one. `name` is null for a new one.
// `saved(name)`, when given, is told the name once it is saved: a picker that
// sent the person here selects what they came back with
function providerDialog(desk, name, redraw, saved) {
  const editing = !!name;
  const p = editing ? (desk.providers[name] || {}) : {};
  const nameIn = el("input", {type:"text", class:"mono", placeholder:T["settings.providers.name_ph"]});
  nameIn.value = name || "";
  nameIn.disabled = editing;
  const urlIn = el("input", {type:"text", class:"mono"});
  urlIn.value = p.base_url || "";
  const hasKey = (p.api_key || "").startsWith("@");
  const keyIn = el("input", {type:"password",
    placeholder: hasKey ? T["settings.providers.key_set_ph"] : T["settings.providers.key_ph"]});
  const waitIn = el("input", {type:"number", min:"0", step:"1", class:"mono narrow",
    placeholder:"180",
    value: (p.timeout_sec === undefined || p.timeout_sec === null) ? "" : String(p.timeout_sec)});
  const speaksIn = el("select");
  for (const [v, label] of [["chat", T["settings.providers.speaks.chat"]],
                            ["choice", T["settings.providers.speaks.choice"]]]) {
    const o = el("option", {value:v}, label);
    if ((p.speaks || "chat") === v) o.selected = true;
    speaksIn.append(o);
  }
  // What the address box asks for follows the type: a conversation is found
  // under a base URL, a decision endpoint is the whole address
  const urlHint = el("div", {class:"hint"});
  const sayUrl = () => {
    const choice = speaksIn.value === "choice";
    urlHint.textContent = T[choice ? "settings.providers.url_hint_choice" : "settings.providers.url_hint"];
    urlIn.placeholder = choice ? "https://api.typesafe.ai/v1/systemone" : "https://api.deepseek.com";
  };
  // The model this connection is used with, so choosing the connection
  // somewhere is enough. A conversation service can be asked what it has
  const modelIn = el("input", {type:"text", class:"mono", style:"flex:1;min-width:0"});
  modelIn.value = (p.models || [])[0] || "";
  const cand = modelCandidates(() => ({base_url: urlIn.value.trim(),
    api_key: keyIn.value.trim() || p.api_key || "", headers: p.headers || {}}),
    id => { modelIn.value = id; cand.chips.textContent = ""; });
  const modelHint = el("div", {class:"hint"});
  const sayModel = () => {
    const choice = speaksIn.value === "choice";
    cand.btn.hidden = choice;
    if (choice) cand.chips.textContent = "";
    modelHint.textContent = T[choice ? "settings.providers.model_hint_choice" : "settings.providers.model_hint"];
  };
  speaksIn.addEventListener("change", sayUrl);
  speaksIn.addEventListener("change", sayModel);
  sayUrl();
  sayModel();

  // A known service, picked to fill the fields below. Only when adding: the
  // name of a saved connection is fixed, and its address is changed by hand
  let preset = null;
  let autoName = "";
  const presetHint = el("div", {class:"hint"}, T["settings.providers.preset_hint"]);
  const presetIn = el("select");
  presetIn.append(el("option", {value:""}, T["settings.providers.preset_none"]));
  for (const kind of ["chat", "choice"]) {
    const group = el("optgroup", {label:T["settings.providers.speaks." + kind]});
    PROVIDER_PRESETS.forEach((s, i) => {
      if (s.speaks !== kind) return;
      group.append(el("option", {value:String(i)},
        s.here ? fill(T["settings.providers.preset_here"], {name: s.label}) : s.label));
    });
    presetIn.append(group);
  }
  presetIn.addEventListener("change", () => {
    preset = presetIn.value === "" ? null : PROVIDER_PRESETS[Number(presetIn.value)];
    presetHint.textContent = "";
    if (!preset) { presetHint.textContent = T["settings.providers.preset_hint"]; return; }
    // The name is the service's own, unless the person has typed one; taken
    // already, it gets the first free number after it
    if (!nameIn.value.trim() || nameIn.value.trim() === autoName) {
      let n = preset.name, i = 2;
      while (desk.providers[n]) n = preset.name + "-" + (i++);
      nameIn.value = autoName = n;
    }
    urlIn.value = preset.url;
    speaksIn.value = preset.speaks;
    modelIn.value = (preset.models || [])[0] || "";
    cand.chips.textContent = "";
    sayUrl();
    sayModel();
    if (preset.here) presetHint.textContent = T["settings.providers.preset_here_hint"];
    else if (preset.keys) presetHint.append(T["settings.providers.preset_keys"],
      el("a", {class:"mono", href:preset.keys, target:"_blank"}, preset.keys));
    else presetHint.textContent = T["settings.providers.preset_keys_elsewhere"];
    recheck();
    (preset.here ? waitIn : keyIn).focus();
  });

  const save = el("button", {class:"primary"}, T["common.save"]);
  const why = el("span", {class:"why"});
  why.hidden = true;
  let held = null;

  // What is wrong, said on the field that is wrong, with the save held rather
  // than dead: it still takes the press, and answers it
  let asked = false;   // has the save been pressed, or the box been typed in
  function fieldFault(input, reason) {
    const wrap = input.parentElement;
    const had = wrap.querySelector(".site-warn");
    const show = reason && (asked || input.value.trim() !== "");
    if (had) had.remove();
    input.classList.toggle("bad", !!show);
    if (show) wrap.append(el("div", {class:"site-warn"},
      el("span", {}, "⚠"), el("span", {}, reason)));
  }
  function recheck() {
    const n = nameIn.value.trim();
    let first = null;
    const nameWhy = !n ? T["settings.providers.name_required"]
      : (!/^[a-z0-9_.-]+$/i.test(n) ? T["settings.providers.name_bad"]
      : (!editing && desk.providers[n] ? T["settings.providers.name_dup"] : null));
    fieldFault(nameIn, nameWhy);
    if (nameWhy) first = {at: nameIn, why: nameWhy};

    const u = urlIn.value.trim();
    const urlWhy = !u ? T["settings.providers.url_required"]
      : (urlFault(u) ? T[urlFault(u)] : null);
    fieldFault(urlIn, urlWhy);
    if (urlWhy && !first) first = {at: urlIn, why: urlWhy};

    const w = waitIn.value.trim();
    const waitWhy = w !== "" && !/^\d+$/.test(w) ? T["settings.providers.wait_bad"] : null;
    fieldFault(waitIn, waitWhy);
    if (waitWhy && !first) first = {at: waitIn, why: waitWhy};

    held = first;
    save.classList.toggle("held", !!held);
    if (!held) why.hidden = true;
    else if (!why.hidden) why.textContent = fill(T["settings.secrets.cannot_save"], {why: held.why});
  }
  function sayWhy() {
    asked = true;
    recheck();
    why.textContent = fill(T["settings.secrets.cannot_save"], {why: held.why});
    why.hidden = false;
    held.at.classList.remove("lookhere");
    void held.at.offsetWidth;
    held.at.classList.add("lookhere");
    held.at.focus();
  }
  for (const i of [nameIn, urlIn, waitIn]) i.addEventListener("input", recheck);

  // The hint is words, or a line of its own that changes with the choices
  const field = (label, control, hint) => el("div", {class:"field"},
    el("label", {}, label), el("div", {class:"fieldctl"}, control),
    !hint ? null : (hint instanceof Node ? hint : el("div", {class:"hint"}, hint)));

  const shut = () => back.remove();
  const back = openModal(
    el("div", {class:"mhead"},
      el("h2", {}, editing ? T["settings.providers.edit_title"] : T["settings.providers.add_title"]),
      el("button", {class:"quiet icon", title:T["common.close"], onclick: () => shut()}, "✕")),
    el("div", {class:"mbody"},
      editing ? null : field(T["settings.providers.preset_label"], presetIn, presetHint),
      field(T["settings.providers.name_label"], nameIn,
            editing ? T["settings.providers.name_fixed"] : T["settings.providers.name_hint"]),
      field(T["settings.providers.speaks_label"], speaksIn, T["settings.providers.speaks_hint"]),
      field(T["settings.providers.url_label"], urlIn, urlHint),
      field(T["settings.providers.key_label"], keyIn, T["settings.providers.key_hint"]),
      field(T["settings.providers.model_label"],
        el("div", {class:"row", style:"padding:0;flex-wrap:nowrap"}, modelIn, cand.btn),
        el("div", {}, modelHint, cand.chips)),
      field(T["settings.providers.wait_label"], waitIn, T["settings.providers.wait_hint"])),
    el("div", {class:"mfoot"},
      editing
        ? el("button", {class:"danger", onclick: async () => {
            if (!await confirmAction(fill(T["settings.providers.delete_confirm"], {name}), T["settings.providers.delete"])) return;
            // Its key goes with it. Nothing else names that secret, and there
            // is no screen of leftovers to tidy it away from later
            if ((p.api_key || "").startsWith("@")) await deleteSecret(p.api_key.slice(1));
            delete desk.providers[name];
            refreshSave(); shut(); redraw();
          }}, T["settings.providers.delete"])
        : null,
      why,
      el("span", {class:"grow"}),
      el("button", {class:"quiet", onclick: () => shut()}, T["common.cancel"]),
      save));
  back.firstChild.classList.add("framed");

  back.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); shut(); return; }
    if (e.key !== "Enter" || e.target.tagName !== "INPUT") return;
    e.preventDefault();
    save.click();
  });

  save.addEventListener("click", async () => {
    if (held) { sayWhy(); return; }
    const n = editing ? name : nameIn.value.trim();
    // The key is filed under this desk, so it needs the name the store files
    // this desk under
    if (keyIn.value.trim() && !(desk.id || "").trim()) { toast(T["settings.secrets.desk_needs_id"], true); return; }
    const it = (desk.providers[n] = desk.providers[n] || {});
    it.base_url = urlIn.value.trim();
    const w = waitIn.value.trim();
    if (w === "") delete it.timeout_sec; else it.timeout_sec = Math.max(0, Math.floor(Number(w)));
    // The ordinary kind is left unwritten: the file says what is unusual
    if (speaksIn.value === "choice") it.speaks = "choice"; else delete it.speaks;
    // The model it is used with comes first. The service's other model names
    // come along only while the address is still that service's: an address
    // changed by hand is somebody else's machine
    const m = modelIn.value.trim();
    const known = (preset && preset.models && it.base_url === preset.url) ? preset.models : (it.models || []);
    const models = (m ? [m] : []).concat(known.filter(x => x !== m));
    if (models.length) it.models = models; else delete it.models;
    // The key never sits in config.json: it goes to the secrets file, under
    // this desk, and only the name of it is kept here
    if (keyIn.value.trim()) {
      const sk = "provider/" + desk.id.trim() + "/" + n;
      const r = await saveSecret({key: sk, description: "model provider " + n,
        value: keyIn.value.trim(), human: true, ai: false, urls: []});
      if (!r.ok) { toast(r.error || T["settings.secrets.save_failed"], true); return; }
      it.api_key = "@" + sk;
    }
    refreshSave(); shut(); redraw();
    toast(fill(T["settings.providers.saved"], {name: n}));
    if (saved) saved(n);
  });
  recheck();
  setTimeout(() => (editing ? urlIn : nameIn).focus(), 0);
}

// ── Driving a page from plain words ─────────────────────────────
// Two models do it: one picks each move, one writes what is typed. Chosen
// for the desk (Desk › Browser) and, where a browser tab wants otherwise, on
// the tab; a tab that leaves one unset has the desk's.
const WORDS_KEYS = ["choose_model", "words_model"];
// "connection/model" split at the first slash: a model's own name may hold one
const splitModel = v => {
  const t = (v || "").trim(), at = t.indexOf("/");
  return at < 0 ? {conn: t, model: ""} : {conn: t.slice(0, at), model: t.slice(at + 1).trim()};
};
// What is wrong with a choice, or null. The writer has to be a conversation
// model: a decision model answers questions and cannot write a word
function wordsFault(desk, key, value) {
  const v = (value || "").trim();
  if (!v) return null;
  const {conn, model} = splitModel(v);
  const prov = (desk.providers || {})[conn];
  if (!prov) return fill(T["settings.words.gone"], {name: conn});
  if (!model) return fill(T["settings.words.no_model"], {name: conn});
  if (key === "words_model" && prov.speaks === "choice") return T["settings.words.not_writer"];
  return null;
}
// Every choice on this desk and its browser tabs that cannot be saved, with
// where it is, for save() to refuse and go to
function wordsFaults(di) {
  const desk = desks[di], out = [];
  if (!desk) return out;
  const b = desk.browser || {};
  for (const k of WORDS_KEYS) {
    const why = wordsFault(desk, k, b[k]);
    if (why) out.push({tab: null, why});
  }
  (desk.tabs || []).forEach((t, i) => {
    if (catOf(cmdToText(t.command).trim()) !== "browser") return;
    for (const k of WORDS_KEYS) {
      const why = wordsFault(desk, k, t[k]);
      if (why) out.push({tab: i, why});
    }
  });
  return out;
}
// One of the two models, picked from this desk's connections. `holder[key]`
// is where the answer is written ("connection/model"). `under`, on a tab, is
// what the desk says, followed while nothing is chosen here; `adopt` is told
// a model added from here, so the desk can take it when it has none
function wordsPicker(desk, holder, key, under, adopt) {
  const writer = key === "words_model";
  const wrap = el("div", {style:"display:flex;flex-direction:column;gap:var(--s2);flex:1 1 100%;min-width:0"});
  const draw = () => {
    wrap.textContent = "";
    const provs = desk.providers || {};
    const now = (holder[key] || "").trim();
    const {conn, model} = splitModel(now);
    const pick = el("select", {style:"width:100%;max-width:420px"});
    const followsDesk = under !== undefined;
    pick.append(el("option", {value:""}, followsDesk
      ? fill(T["settings.words.follow"], {name: (under || "").trim() || T["settings.words.unset"]})
      : T["settings.providers.preset_none"]));
    // The deciding one takes either kind, a decision model first; the
    // writing one only a conversation model. Each shows the model it is used with
    const kinds = writer ? ["chat"] : ["choice", "chat"];
    for (const kind of kinds) {
      const names = Object.keys(provs).filter(n => (provs[n].speaks || "chat") === kind).sort();
      if (!names.length) continue;
      const group = el("optgroup", {label: T["settings.providers.speaks." + kind]});
      for (const n of names) {
        const m = (provs[n].models || [])[0] || DEFAULT_MODEL[n] || "";
        group.append(el("option", {value:n}, m ? n + " / " + m : n));
      }
      pick.append(group);
    }
    // What is written, even when it is not on offer -- a decision model put
    // where writing is needed, a connection since removed -- so the screen
    // shows what the settings say, with what is wrong under it
    if (conn && !Array.from(pick.options).some(o => o.value === conn)) {
      pick.append(el("option", {value:conn}, now));
    }
    pick.append(el("option", {value:"@add"}, T["settings.words.add"]));
    pick.value = conn;
    // The model, for a connection with more than one on offer
    const modelIn = el("input", {type:"text", class:"mono", style:"flex:1;min-width:0;max-width:300px"});
    modelIn.value = model;
    const cand = modelCandidates(() => provs[pick.value] || {}, id => { modelIn.value = id; store(); });
    const modelRow = el("div", {class:"row", style:"padding:0;flex-wrap:nowrap"}, modelIn, cand.btn);
    modelRow.hidden = !conn || !provs[conn];
    if (provs[conn] && provs[conn].speaks === "choice") cand.btn.hidden = true;
    const warn = wordsFault(desk, key, now);
    const store = () => {
      const c = pick.value, m = modelIn.value.trim();
      if (c) holder[key] = c + "/" + m; else delete holder[key];
      refreshSave();
      draw();
    };
    pick.addEventListener("change", () => {
      if (pick.value === "@add") {
        pick.value = conn;
        providerDialog(desk, null, () => {}, n => {
          const m = ((desk.providers[n] || {}).models || [])[0] || DEFAULT_MODEL[n] || "";
          holder[key] = n + "/" + m;
          if (adopt) adopt(holder[key]);
          refreshSave();
          draw();
        });
        return;
      }
      const c = pick.value;
      modelIn.value = c ? ((provs[c] || {}).models || [])[0] || DEFAULT_MODEL[c] || "" : "";
      store();
    });
    modelIn.addEventListener("change", store);
    wrap.append(pick, modelRow, cand.chips);
    if (warn) wrap.append(el("div", {class:"site-warn"}, el("span", {}, "⚠"), el("span", {}, warn)));
  };
  draw();
  return wrap;
}
// The two pickers under their heading, the way the desk and a tab both show them
function wordsRows(desk, holder, under, adopt) {
  return [
    el("div", {class:"hint"}, T["settings.words.hint"]),
    row(T["settings.words.choose_model"],
      wordsPicker(desk, holder, "choose_model", under ? (under.choose_model || "") : undefined, adopt && (v => adopt("choose_model", v))),
      el("span", {class:"hint"}, T["settings.words.choose_model.hint"])),
    row(T["settings.words.words_model"],
      wordsPicker(desk, holder, "words_model", under ? (under.words_model || "") : undefined, adopt && (v => adopt("words_model", v))),
      el("span", {class:"hint"}, T["settings.words.words_model.hint"])),
  ];
}
// Desk › Browser: what this desk's browser tabs follow, and whether pages may
// be sent to those models at all
function deskBrowserCard(desk) {
  desk.browser = desk.browser || {};
  const words = card(T["settings.words.title"], ...wordsRows(desk, desk.browser));
  words.id = "desk-words";
  return [words, deskPagesCard(desk)];
}

// Claude's allowance, read with Claude Code's own sign-in on this PC. Its own
// card, because it is not a connection anybody registers -- it is a thing the
// program can read when Claude Code is signed in, and nothing when it is not
function claudeUsageCard() {
  const state = el("div", {class:"hint"}, T["settings.claude_usage.checking"]);
  const dot = el("span", {class:"dot"});
  const line = el("div", {class:"row"}, dot, state);
  fetch("/api/claude", {headers:{"X-Token":TOKEN}}).then(r => r.json()).then(j => {
    dot.classList.add(j.signed_in ? "on" : "off");
    state.textContent = j.signed_in
      ? T["settings.claude_usage.signed_in"]
      : T["settings.claude_usage.signed_out"];
  }).catch(() => { state.textContent = T["settings.claude_usage.unknown"]; });
  return card(T["settings.claude_usage"],
    el("div", {class:"hint"}, T["settings.claude_usage.sub"]),
    checkDefaultOn(current, "claude_usage", T["settings.claude_usage.label"]),
    el("div", {class:"hint"}, T["settings.claude_usage.hint"]),
    line);
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
      const luaLbl = el("label", {class:"hint", style:"display:flex;align-items:center;gap:var(--s1);flex:none"},
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
    listBox, el("div", {class:"row", style:"margin-top:var(--s3)"}, addBtn));
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

// A desk's table of who may run what. `owner` is the desk the rows are
// written into; `edited` hears about every change, for a card that says how
// many rows differ
function permissionsTable(owner, edited) {
  // Read without writing: merely opening this card must not make the settings
  // look edited. The key appears in the file the first time a box disagrees
  // with the standard answer, and leaves again when it agrees once more
  const saved = () => owner.automation_permissions || {};
  const answerOf = (cmd, col) => {
    const rule = saved()[cmd.name] || {};
    return rule[col] === undefined ? cmd[col] : rule[col];
  };
  const changed = cmd => answerOf(cmd, "human") !== cmd.human || answerOf(cmd, "ai") !== cmd.ai;
  const decide = (cmd, col, on) => {
    const all = owner.automation_permissions || {};
    const rule = all[cmd.name] || {};
    if (on === cmd[col]) delete rule[col]; else rule[col] = on;
    if (Object.keys(rule).length) all[cmd.name] = rule; else delete all[cmd.name];
    if (Object.keys(all).length) owner.automation_permissions = all;
    else delete owner.automation_permissions;
    refreshSave();
    if (edited) edited();
  };

  const body = el("div", {});
  const draw = () => {
    body.textContent = "";
    body.append(el("div", {class:"grantcols"},
      el("span", {class:"grow"}, ""),
      el("span", {}, T["settings.permissions.col.human"]),
      el("span", {}, T["settings.permissions.col.ai"])));
    for (const sec of GRANTS) {
      // Every group starts shut: the table sits on a desk's page among the
      // other cards, and sixty rows open by default push everything below
      // them out of reach. The group heading already says, in its two boxes,
      // whether anything inside is switched off
      const open = grantFolded[sec.group] === false;
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
    // Back to the answers the commands' authors chose
    delete owner.automation_permissions;
    refreshSave();
    draw();
    if (edited) edited();
  }}, T["settings.permissions.reset"]);
  return {body, reset};
}

function permissionsCard(desk) {
  const count = el("div", {class:"hint"});
  const recount = () => {
    const n = Object.keys(desk.automation_permissions || {}).length;
    count.textContent = fill(T["settings.desk.grants.changed"], {n});
  };
  const {body, reset} = permissionsTable(desk, recount);
  recount();
  // What counts as an AI is the first thing on the card, spelled out rather
  // than left to be assumed. The mistake this prevents is a person unticking
  // the AI column, walking away, and the AI they started by hand in a terminal
  // tab carrying on -- it holds that tab's key, and that tab is a terminal
  const c = card(T["settings.sec.permissions"],
    el("div", {class:"hint"}, T["settings.permissions.hint"]),
    el("div", {class:"grantwho"},
      el("div", {}, T["settings.permissions.who.ai"]),
      el("div", {}, T["settings.permissions.who.human"])),
    el("div", {class:"grantwarn"}, T["settings.permissions.caution"]),
    count,
    el("div", {class:"row"}, reset,
      el("a", {href:manualHref(""), target:"_blank"}, T["settings.permissions.manual"])),
    body);
  c.id = "desk-permissions";
  return c;
}

// The branches the panel will not commit straight onto.
//
// It began as two names written into the app, which is right up until somebody
// is working alone on their own repository -- there, "make a branch first" is a
// rule with nobody on the other side of it. So the names are a question, asked
// here for every folder and again on the folder itself for the one project that
// wants something else.
// Where a branch can be opened, other than here.
//
// The shape every list on this page has (section 5.5): a row is a summary and
// a way in, never a form. The form is the dialog the row opens, which is where
// section 5.2 says a form with a save button belongs. The first attempt put
// every field in the row and it read as a pile.
//
// Two kinds in one list, because to the person they are one question -- where
// does this run -- even though they differ in the way that shapes everything:
// a machine reached over ssh is already there and is asked where the project
// is on it, while a sandbox is made when it is wanted and is asked what to
// build it from.
//
// No credential is on this page. A password and a key live in the secrets file
// under names worked out from the name here, so that nobody is invited to type
// a credential into a settings form that is also a file on disk.
function hostsCard() {
  const listBox = el("div");
  const draw = () => {
    listBox.textContent = "";
    const hosts = current.hosts = current.hosts || [];
    if (!hosts.length) {
      listBox.append(el("div", {class:"hint"}, T["settings.hosts.none"]));
      return;
    }
    const rows = el("div", {class:"rows"});
    hosts.forEach((h, i) => {
      const made = (h.kind || "").trim().toLowerCase() === "e2b";
      rows.append(el("div", {class:"listrow secretrow", onclick: () => hostDialog(i, draw)},
        el("span", {class:"mono secretname"}, h.name || ""),
        el("span", {class:"hint mono secretdesc"},
          made ? (h.template || "base") : (h.at || T["settings.hosts.at.ph"])),
        el("span", {class:"hint"}, made ? T["settings.hosts.kind.e2b"] : T["settings.hosts.kind.ssh"]),
        el("span", {class:"go"}, "\u203a")));
    });
    listBox.append(rows);
  };
  const c = card(T["settings.sec.hosts"],
    el("div", {class:"hint"}, T["settings.hosts.hint"]),
    listBox,
    el("div", {class:"row"},
      el("button", {onclick: () => hostDialog(null, draw, "ssh")}, T["settings.hosts.add.ssh"]),
      el("button", {onclick: () => hostDialog(null, draw, "e2b")}, T["settings.hosts.add.e2b"])));
  setTimeout(draw, 0);
  return c;
}

// A name nothing else is using. Two with one name would be one machine to
// everything that looks them up, which is by name
function freeHostName(base) {
  const taken = new Set((current.hosts || []).map(h => (h.name || "").trim()));
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) if (!taken.has(base + "-" + n)) return base + "-" + n;
}

// Adding one, or changing one. `at` is null for a new one, and `kind` says
// which of the two it will be
function hostDialog(at, redraw, kind) {
  const editing = at !== null && at !== undefined;
  const h = editing ? (current.hosts[at] || {}) : {};
  const made = editing ? (h.kind || "").trim().toLowerCase() === "e2b" : kind === "e2b";

  const nameIn = el("input", {type:"text", class:"mono"});
  nameIn.value = h.name || freeHostName(made ? "sandbox" : "machine");
  const atIn = el("input", {type:"text", class:"mono", placeholder:T["settings.hosts.at.ph"]});
  atIn.value = h.at || "";
  const projectIn = el("input", {type:"text", class:"mono", placeholder:T["settings.hosts.project.ph"]});
  projectIn.value = h.project || "";
  const branchesIn = el("input", {type:"text", class:"mono", placeholder:T["settings.hosts.branches.ph"]});
  branchesIn.value = h.branches || "";
  const templateIn = el("input", {type:"text", class:"mono", placeholder:T["settings.hosts.template.ph"]});
  templateIn.value = h.template || "base";
  const minutesIn = el("input", {type:"number", min:"1", class:"mono narrow", placeholder:"30"});
  minutesIn.value = h.minutes != null ? String(h.minutes) : "";
  // One service today. A picker with one entry rather than none, because the
  // next one is a row in this list and not a new screen
  const serviceIn = el("select");
  serviceIn.append(el("option", {value:"e2b"}, "E2B"));
  serviceIn.value = "e2b";

  const save = el("button", {class:"primary"}, T["common.save"]);
  const why = el("span", {class:"why"});
  why.hidden = true;
  let held = null;
  let asked = false;

  function fieldFault(input, reason) {
    const wrap = input.parentElement;
    const had = wrap.querySelector(".site-warn");
    const show = reason && (asked || input.value.trim() !== "");
    if (had) had.remove();
    input.classList.toggle("bad", !!show);
    if (show) wrap.append(el("div", {class:"site-warn"},
      el("span", {}, "\u26a0"), el("span", {}, reason)));
  }
  function recheck() {
    const n = nameIn.value.trim();
    let first = null;
    const dup = (current.hosts || []).some((o, i) => i !== at && (o.name || "").trim() === n);
    const nameWhy = !n ? T["settings.hosts.name_required"]
      : (dup ? T["settings.hosts.name_dup"] : null);
    fieldFault(nameIn, nameWhy);
    if (nameWhy) first = {at: nameIn, why: nameWhy};
    if (!made) {
      const a = atIn.value.trim();
      const atWhy = !a ? T["settings.hosts.at_required"]
        : (!/^ssh:\/\/[^@\s]+@[^\s:]+(:\d+)?$/.test(a) ? T["settings.hosts.at_bad"] : null);
      fieldFault(atIn, atWhy);
      if (atWhy && !first) first = {at: atIn, why: atWhy};
      const pr = projectIn.value.trim();
      const prWhy = !pr ? T["settings.hosts.project_required"] : null;
      fieldFault(projectIn, prWhy);
      if (prWhy && !first) first = {at: projectIn, why: prWhy};
    }
    held = first;
    save.classList.toggle("held", !!held);
    if (!held) why.hidden = true;
    else if (!why.hidden) why.textContent = fill(T["settings.secrets.cannot_save"], {why: held.why});
  }
  function sayWhy() {
    asked = true;
    recheck();
    why.textContent = fill(T["settings.secrets.cannot_save"], {why: held.why});
    why.hidden = false;
    held.at.classList.remove("lookhere");
    void held.at.offsetWidth;
    held.at.classList.add("lookhere");
    held.at.focus();
  }
  for (const i of [nameIn, atIn, projectIn, templateIn, minutesIn]) i.addEventListener("input", recheck);

  const field = (label, control, hint) => el("div", {class:"field"},
    el("label", {}, label), el("div", {class:"fieldctl"}, control),
    hint ? el("div", {class:"hint"}, hint) : null);

  // The credential each kind signs in with. Kept in the secrets file under a
  // name worked out from this machine's, as a tab's is, and never in the
  // settings -- but typed here. The page used to say where it was kept and
  // offer nowhere to type it: the secrets screen files everything under a
  // desk, so a machine's password and the sandbox service's key could not be
  // entered from anywhere at all
  const hostName = () => nameIn.value.trim();
  const credential = made
    ? credentialField(() => "e2b_api_key", T["settings.hosts.e2b_key"], T["settings.ssh.password.hint"],
        () => "E2B", "", T["settings.hosts.e2b_key.saved"])
    : credentialField(() => hostName() ? "ssh/host/" + hostName() + "/password" : "",
        T["settings.hosts.password"], T["settings.ssh.password.hint"],
        () => "SSH " + hostName(), T["settings.hosts.name_required"]);
  nameIn.addEventListener("change", () => credential.refresh());
  // A machine reached over SSH is a server like any a tab reaches, and can be
  // given its name here as well. A sandbox is made and thrown away, and has no
  // lasting server for a name to belong to
  const mark = made ? null : markFields({ask: () => {
    const a = atIn.value.trim();
    return a ? {command: a, server: null} : null;
  }}, false);
  if (mark) atIn.addEventListener("input", () => mark.schedule());

  const shut = () => back.remove();
  const back = openModal(
    el("div", {class:"mhead"},
      el("h2", {}, made ? T["settings.hosts.title.e2b"] : T["settings.hosts.title.ssh"]),
      el("button", {class:"quiet icon", title:T["common.close"], onclick: () => shut()}, "\u2715")),
    el("div", {class:"mbody"},
      field(T["settings.hosts.name"], nameIn, T["settings.hosts.name.hint"]),
      ...(made
        ? [field(T["settings.hosts.provider"], serviceIn, ""),
           credential,
           field(T["settings.hosts.template"], templateIn, T["settings.hosts.template.hint"]),
           field(T["settings.hosts.minutes"], minutesIn, T["settings.hosts.minutes.hint"])]
        : [field(T["settings.hosts.at"], atIn, ""),
           mark.box,
           credential,
           field(T["settings.hosts.project"], projectIn, T["settings.hosts.project.hint"]),
           field(T["settings.hosts.branches"], branchesIn, "")])),
    el("div", {class:"mfoot"},
      editing
        ? el("button", {class:"danger", onclick: async () => {
            if (!await confirmAction(fill(T["settings.hosts.drop.sure"], {name: h.name || ""}),
                                     T["settings.hosts.drop"])) return;
            current.hosts.splice(at, 1);
            refreshSave(); shut(); redraw();
          }}, T["settings.hosts.drop"])
        : null,
      why,
      el("span", {class:"grow"}),
      el("button", {class:"quiet", onclick: () => shut()}, T["common.cancel"]),
      save));
  back.firstChild.classList.add("framed");

  back.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); shut(); return; }
    if (e.key !== "Enter" || e.target.tagName !== "INPUT") return;
    e.preventDefault();
    save.click();
  });

  save.addEventListener("click", () => {
    if (held) { sayWhy(); return; }
    const it = editing ? current.hosts[at] : {};
    it.name = nameIn.value.trim();
    if (made) {
      it.kind = "e2b";
      it.template = templateIn.value.trim() || "base";
      const m = parseInt(minutesIn.value, 10);
      if (Number.isFinite(m) && m > 0) it.minutes = m; else delete it.minutes;
      // The leftovers of the other kind go, so a machine is only ever one kind
      delete it.at; delete it.project; delete it.branches;
    } else {
      delete it.kind; delete it.template; delete it.minutes;
      it.at = atIn.value.trim();
      it.project = projectIn.value.trim();
      const b = branchesIn.value.trim();
      if (b) it.branches = b; else delete it.branches;
    }
    if (!editing) (current.hosts = current.hosts || []).push(it);
    if (mark) mark.commit();
    refreshSave(); shut(); redraw();
  });
  setTimeout(recheck, 0);
}

// The branch names a desk guards. A folder page holds the same field for the
// one project that wants something else
function protectField(owner) {
  const box = el("input", {class:"mono grow", placeholder:T["settings.protect.ph"]});
  const g = owner.git || {};
  box.value = protectText(Array.isArray(g.protect) ? g.protect : PROTECT_DEFAULT);
  // The settings are touched when somebody types, never by looking: a card
  // that wrote itself into the config on the way in would light the save
  // button for a change nobody made
  box.addEventListener("input", () => {
    (owner.git = owner.git || {}).protect = protectList(box.value);
    refreshSave();
  });
  return box;
}

// A prompt the AI is given, whole, in a box: nothing is added out of sight.
// Three states, told apart by whether the key is in the settings at all:
//   absent  -> the default prompt, shown in the box and used
//   written -> what is written
//   ""      -> nothing: the AI is given the change alone
// Written back to exactly the default is absent again, so a later, better
// default still reaches it. `legacy` is an instruction an earlier version kept
// apart from a prompt nobody could see; it is shown on the end of the default,
// which is how it was used, until the box is changed
function promptField(g, key, standardKey, id, vars, legacy) {
  const standard = T[standardKey] || "";
  const old = () => legacy && typeof g[legacy] === "string" && g[legacy].trim() !== "" ? g[legacy].trim() : "";
  const shown = () => typeof g[key] === "string" ? g[key] : old() ? standard + "\n\n" + old() : standard;
  const box = el("textarea", {rows:"14", class:"mono", style:"width:100%"});
  box.value = shown();
  const state = el("span", {class:"chip"});
  const back = el("button", {class:"quiet"}, T["settings.git.prompt.default"]);
  const tell = () => {
    const mine = typeof g[key] === "string" || !!old();
    state.textContent = !mine ? T["settings.git.prompt.standard"]
      : g[key] === "" ? T["settings.git.prompt.empty"] : T["settings.git.prompt.edited"];
    back.hidden = !mine;
  };
  back.onclick = () => {
    delete g[key];
    if (legacy) delete g[legacy];
    box.value = standard;
    tell();
    refreshSave();
  };
  box.addEventListener("input", () => {
    if (legacy) delete g[legacy];
    if (box.value === standard) delete g[key]; else g[key] = box.value;
    tell();
    refreshSave();
  });
  tell();
  // The words the app fills in, under the box: pressed, one goes in where the
  // caret is, so nobody has to remember how it is spelled
  const insert = word => {
    const from = box.selectionStart ?? box.value.length;
    const to = box.selectionEnd ?? from;
    box.setRangeText(word, from, to, "end");
    box.focus();
    box.dispatchEvent(new Event("input"));
  };
  const chips = el("div", {class:"row promptvars"},
    el("span", {class:"hint"}, T["settings.git.prompt.vars"]),
    ...vars.map(v => el("button", {type:"button", class:"chip mono", title:T["settings.git.var." + v],
      onclick:() => insert("{" + v + "}")}, "{" + v + "}")));
  return el("div", {id, style:"margin:var(--s2) 0 var(--s4)"},
    el("div", {class:"row", style:"margin-bottom:var(--s1)"}, state, back),
    box, chips);
}

// How the git panel's AI writes: the commit message, and a pull request's title
// and description. Each prompt is written out whole in its box; for more than
// words, the commit message can be built by Lua instead
function gitFields(owner) {
  const g = owner.git = owner.git || {};
  const hint = promptField(g, "message_prompt", "ai.commit.default_prompt", "desk-git-message",
    ["diff", "ai"], "message_hint");

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
    luaBox.append(el("div", {style:"margin-top:var(--s2)"},
      el("button", {class:"quiet", onclick:() => {
        // The built-in one, as a starting point rather than a blank sheet
        lua.value = GIT_MESSAGE_LUA;
        g.message_lua = lua.value;
        refreshSave();
      }}, T["settings.git.lua.default"]),
      el("a", {href:manualHref("ai_ask"), target:"_blank", style:"margin-left:var(--s3)"},
        T["settings.git.lua.manual"])));
  };
  useLua.addEventListener("change", () => {
    if (useLua.checked) { g.message_lua = lua.value || GIT_MESSAGE_LUA; lua.value = g.message_lua; }
    else delete g.message_lua;
    drawLua();
    refreshSave();
  });
  drawLua();

  return [
    el("div", {class:"hint"}, T["settings.git.hint.about"]),
    hint,
    el("label", {class:"row", style:"cursor:pointer;gap:var(--s2)"}, useLua,
      el("span", {}, T["settings.git.lua.label"])),
    luaBox,
    el("h3", {}, T["settings.git.pr.title"]),
    el("div", {class:"hint"}, T["settings.git.pr.about"]),
    promptField(g, "pr_prompt", "ai.pr.default_prompt", "desk-git-pr", ["branch", "base", "commits", "diff", "ai"]),
    el("details", {class:"promptmore"},
      el("summary", {}, T["settings.git.prompt.more"]),
      el("div", {class:"hint"}, T["settings.git.pr.shape"]),
      el("pre", {class:"mono promptshape"}, T["ai.pr.shape"] || "")),
    el("h3", {}, T["settings.git.merge.title"]),
    el("div", {class:"hint"}, T["settings.git.merge.about"]),
    promptField(g, "merge_prompt", "ai.merge.default_prompt", "desk-git-merge", ["folder", "branch", "base", "files", "language"]),
    el("h3", {}, T["settings.git.ci.title"]),
    el("div", {class:"hint"}, T["settings.git.ci.about"]),
    promptField(g, "ci_prompt", "ai.ci.default_prompt", "desk-git-ci", ["pr", "title", "url", "branch", "folder", "checks", "language"]),
    el("h3", {}, T["settings.git.issue.title"]),
    el("div", {class:"hint"}, T["settings.git.issue.about"]),
    promptField(g, "issue_prompt", "ai.issue.default_prompt", "desk-git-issue", ["text", "ai"]),
    // What is always added after it, shown rather than kept out of sight
    // What is always added after it: shown, but folded -- it is there to be
    // looked up, not read every time the page is opened
    el("details", {class:"promptmore"},
      el("summary", {}, T["settings.git.prompt.more"]),
      el("div", {class:"hint"}, T["settings.git.issue.shape"]),
      el("pre", {class:"mono promptshape"}, T["ai.issue.shape"] || "")),
  ];
}

// What git does in this desk: the branches a commit will not land on, and how
// the commit message is written
function gitCard(desk) {
  const c = card(T["settings.desk.git.title"],
    el("div", {class:"hint", id:"desk-protect"}, T["settings.protect.hint"]),
    el("div", {class:"row"}, protectField(desk)),
    el("div", {class:"hint"}, T["settings.protect.wild"]),
    el("h3", {}, T["settings.sec.git"]),
    ...gitFields(desk));
  c.id = "desk-git";
  return c;
}

// Where the program says something when a tab has finished, or when a script
// needs a person.
//
// Three kinds, and each kind gets the shape it deserves: a chat service is a
// record with an address, so it is a boxed list and a dialog; this PC's own
// notifications have nothing to fill in, so they are a tick; and a phone
// arranges itself from the phone, so that is steps and a QR code. Putting all
// three behind one "add" button was what made the old screen a puzzle.
const CHAT_TYPES = ["slack", "discord", "telegram"];
const isChat = d => CHAT_TYPES.includes((d || {}).type);
const chatLabel = t => t === "slack" ? "Slack" : t === "discord" ? "Discord" : "Telegram";
// What a person would recognise the destination by, without showing a secret
const chatWhere = d => d.type === "telegram"
  ? ((d.chat_id || "").trim() || T["settings.notify.no_chat"])
  : ((d.webhook || "").startsWith("@") ? T["settings.notify.hook_set"] : T["settings.notify.no_hook"]);

// Where this desk's messages go. The chat services it registered, this PC,
// and the phones, in one card: each desk has its own, so nothing a person
// sets up for work is on the list in a personal desk
function notifyCard(desk) {
  desk.notify = desk.notify || {};
  const listBox = el("div", {id:"notifylist"});
  // Where a message goes when the automation names nowhere. Every kind of
  // destination can be it, so it is chosen here rather than inside one dialog
  const prim = el("select");
  prim.addEventListener("change", () => {
    if (prim.value) desk.primary_notify = prim.value; else delete desk.primary_notify;
    refreshSave();
    draw();
  });
  const drawPrim = () => {
    const names = Object.keys(desk.notify);
    prim.textContent = "";
    prim.append(el("option", {value:""}, names.length === 1
      ? fill(T["settings.desk.notify.prim_only"], {name: names[0]})
      : T["settings.desk.notify.prim_none"]));
    for (const n of names) prim.append(el("option", {value:n}, n));
    prim.value = desk.notify[desk.primary_notify] ? desk.primary_notify : "";
  };
  const draw = () => {
    listBox.textContent = "";
    // A destination that was deleted must not linger as the primary
    if (desk.primary_notify && !desk.notify[desk.primary_notify]) {
      delete desk.primary_notify;
    }
    drawPrim();
    const names = Object.keys(desk.notify).filter(n => isChat(desk.notify[n]));
    if (!names.length) {
      listBox.append(el("div", {class:"hint"}, T["settings.notify.empty"]));
      return;
    }
    const rows = el("div", {class:"rows"});
    for (const name of names) {
      const d = desk.notify[name];
      const primary = desk.primary_notify === name;
      rows.append(el("div", {class:"listrow secretrow", onclick: () => chatDialog(desk, name, draw)},
        el("span", {class:"hint", style:"flex:0 0 72px"}, chatLabel(d.type)),
        el("span", {class:"secretname", style:"flex:0 0 120px"}, name),
        el("span", {class:"hint mono secretdesc"}, chatWhere(d)),
        primary ? el("span", {class:"chip"}, T["settings.notify.primary"]) : el("span"),
        el("span", {class:"go"}, "›")));
    }
    listBox.append(rows);
  };
  const c = card(T["settings.desk.notify.title"],
    el("div", {class:"hint"}, T["settings.notify.chat_hint"]),
    listBox,
    el("div", {class:"row"},
      el("button", {onclick: () => {
        if (!(desk.id || "").trim()) { toast(T["settings.secrets.desk_needs_id"], true); return; }
        chatDialog(desk, null, draw);
      }}, T["settings.notify.chat_add"])),
    tickDestination(desk, "windows", T["settings.notify.pc.name"], T["settings.notify.pc.label"],
      T["settings.notify.windows.hint"], draw),
    tickDestination(desk, "phone", T["settings.notify.phone.name"], T["settings.desk.notify.phone.label"],
      T["settings.desk.notify.phone.hint"], draw),
    row(T["settings.desk.notify.primary"], prim));
  c.id = "desk-notify-desk";
  setTimeout(draw, 0);
  return c;
}

// A destination with nothing to fill in -- this PC, or the phones -- so it is
// a tick and a test rather than a record
function tickDestination(desk, type, called, label, hint, redraw) {
  const nameOf = () => Object.keys(desk.notify).find(n => (desk.notify[n] || {}).type === type);
  const box = el("input", {type:"checkbox"});
  box.checked = !!nameOf();
  const tick = el("label", {class:"check"});
  tick.append(box, document.createTextNode(label));
  const test = el("button", {class:"quiet", onclick: async () => {
    const r = await settingsApi("/api/notify/test", {type}).catch(() => null);
    toast((r && r.ok) ? T["settings.notify.test_ok"]
                      : ((r && r.error) || T["settings.notify.test_failed"]), !(r && r.ok));
  }}, T["settings.notify.test"]);
  test.hidden = !box.checked;
  box.addEventListener("change", () => {
    const had = nameOf();
    if (box.checked && !had) desk.notify[called] = {type};
    if (!box.checked && had) delete desk.notify[had];
    test.hidden = !box.checked;
    refreshSave();
    redraw();
  });
  return el("div", {},
    el("div", {class:"row"}, tick, test),
    el("div", {class:"hint"}, hint));
}

// One chat destination. Type, name, the one secret it needs, and -- for
// Telegram -- which chat. Test sits beside Save because the question anybody
// has here is "did that arrive", and the answer is worth having before the
// dialog closes.
function chatDialog(desk, name, redraw) {
  const editing = !!name;
  const d = editing ? desk.notify[name] : {type:"slack"};
  const typeSel = el("select");
  for (const t of CHAT_TYPES) typeSel.append(el("option", {value:t}, chatLabel(t)));
  typeSel.value = d.type;
  const nameIn = el("input", {type:"text", placeholder:T["settings.notify.name_ph"]});
  nameIn.value = name || "";
  const hasSecret = ((d.type === "telegram" ? d.token : d.webhook) || "").startsWith("@");
  const secretIn = el("input", {type:"password",
    placeholder: hasSecret ? T["settings.providers.key_set_ph"] : ""});
  const secretLabel = el("label", {});
  const secretHint = el("div", {class:"hint"});
  const chatIn = el("input", {type:"text", class:"mono", placeholder:"123456789"});
  chatIn.value = (d.chat_id || "").startsWith("@") ? "" : (d.chat_id || "");
  const chatField = el("div", {class:"field"},
    el("label", {}, T["settings.notify.chat_label"]), el("div", {class:"fieldctl"}, chatIn),
    el("div", {class:"hint"}, T["settings.notify.chat_hint_line"]));
  const primIn = el("input", {type:"checkbox"});
  primIn.checked = desk.primary_notify === name;
  const primLabel = el("label", {class:"check"});
  primLabel.append(primIn, document.createTextNode(T["settings.notify.primary_label"]));

  const save = el("button", {class:"primary"}, T["common.save"]);
  const why = el("span", {class:"why"});
  why.hidden = true;
  let held = null;
  let asked = false;

  const forTelegram = () => typeSel.value === "telegram";
  const shape = () => {
    chatField.hidden = !forTelegram();
    secretLabel.textContent = forTelegram()
      ? T["settings.notify.token_label"] : T["settings.notify.webhook_label"];
    secretIn.placeholder = hasSecret && typeSel.value === d.type
      ? T["settings.providers.key_set_ph"]
      : (forTelegram() ? T["settings.notify.token_ph"] : T["settings.notify.webhook_ph"]);
    secretHint.textContent = forTelegram()
      ? T["settings.notify.token_hint"] : T["settings.notify.webhook_hint"];
    recheck();
  };
  function fieldFault(input, reason) {
    const wrap = input.parentElement;
    const had = wrap.querySelector(".site-warn");
    const show = reason && (asked || input.value.trim() !== "");
    if (had) had.remove();
    input.classList.toggle("bad", !!show);
    if (show) wrap.append(el("div", {class:"site-warn"},
      el("span", {}, "⚠"), el("span", {}, reason)));
  }
  function recheck() {
    const n = nameIn.value.trim();
    let first = null;
    const nameWhy = !n ? T["settings.notify.name_required"]
      : ((!editing || n !== name) && desk.notify[n] ? T["settings.notify.name_dup"] : null);
    fieldFault(nameIn, nameWhy);
    if (nameWhy) first = {at: nameIn, why: nameWhy};
    // A destination with no address delivers nothing, and the one already
    // saved counts -- this box is only ever for replacing it
    const keeps = hasSecret && typeSel.value === d.type;
    const secWhy = (!secretIn.value.trim() && !keeps) ? T["settings.notify.secret_required"] : null;
    fieldFault(secretIn, secWhy);
    if (secWhy && !first) first = {at: secretIn, why: secWhy};
    const chatWhy = forTelegram() && !chatIn.value.trim() ? T["settings.notify.chat_required"] : null;
    fieldFault(chatIn, chatWhy);
    if (chatWhy && !first) first = {at: chatIn, why: chatWhy};
    held = first;
    save.classList.toggle("held", !!held);
    if (!held) why.hidden = true;
    else if (!why.hidden) why.textContent = fill(T["settings.secrets.cannot_save"], {why: held.why});
  }
  function sayWhy() {
    asked = true;
    recheck();
    why.textContent = fill(T["settings.secrets.cannot_save"], {why: held.why});
    why.hidden = false;
    held.at.classList.remove("lookhere");
    void held.at.offsetWidth;
    held.at.classList.add("lookhere");
    held.at.focus();
  }
  typeSel.addEventListener("change", shape);
  for (const i of [nameIn, secretIn, chatIn]) i.addEventListener("input", recheck);

  // What would be sent, from what is on screen now -- so a key typed a moment
  // ago can be tested before it is saved
  const payload = () => forTelegram()
    ? {type:"telegram", token: secretIn.value.trim() || d.token || "", chat_id: chatIn.value.trim()}
    : {type: typeSel.value, webhook: secretIn.value.trim() || d.webhook || ""};
  const testBtn = el("button", {onclick: async () => {
    const r = await settingsApi("/api/notify/test", payload()).catch(() => null);
    toast((r && r.ok) ? T["settings.notify.test_ok"]
                      : ((r && r.error) || T["settings.notify.test_failed"]), !(r && r.ok));
  }}, T["settings.notify.test"]);

  const field = (label, control, hint) => el("div", {class:"field"},
    el("label", {}, label), el("div", {class:"fieldctl"}, control),
    hint ? el("div", {class:"hint"}, hint) : null);

  const shut = () => back.remove();
  const back = openModal(
    el("div", {class:"mhead"},
      el("h2", {}, editing ? T["settings.notify.edit_title"] : T["settings.notify.add_title"]),
      el("button", {class:"quiet icon", title:T["common.close"], onclick: () => shut()}, "✕")),
    el("div", {class:"mbody"},
      field(T["settings.notify.type_label"], typeSel, T["settings.notify.type_hint"]),
      field(T["settings.notify.name_label"], nameIn, T["settings.notify.name_hint"]),
      el("div", {class:"field"}, secretLabel,
         el("div", {class:"fieldctl"}, secretIn), secretHint),
      chatField,
      el("div", {class:"field"},
         el("label", {}, T["settings.notify.primary_field"]),
         el("div", {class:"fieldctl"}, primLabel),
         el("div", {class:"hint"}, T["settings.notify.primary_hint"]))),
    el("div", {class:"mfoot"},
      editing
        ? el("button", {class:"danger", onclick: async () => {
            if (!await confirmAction(fill(T["settings.notify.delete_confirm"], {name}), T["settings.notify.delete"])) return;
            await dropSecretRef(d.token);
            await dropSecretRef(d.webhook);
            delete desk.notify[name];
            if (desk.primary_notify === name) delete desk.primary_notify;
            refreshSave(); shut(); redraw();
          }}, T["settings.notify.delete"])
        : null,
      why,
      testBtn,
      el("span", {class:"grow"}),
      el("button", {class:"quiet", onclick: () => shut()}, T["common.cancel"]),
      save));
  back.firstChild.classList.add("framed");
  back.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); shut(); return; }
    if (e.key !== "Enter" || e.target.tagName !== "INPUT" || e.target.type === "checkbox") return;
    e.preventDefault();
    save.click();
  });

  save.addEventListener("click", async () => {
    if (held) { sayWhy(); return; }
    const n = nameIn.value.trim();
    // Renaming moves the record; the secret keeps the name it was filed under
    // unless a new one is being typed in, in which case it is filed afresh
    const it = {type: typeSel.value};
    if (editing) {
      if (d.token) it.token = d.token;
      if (d.webhook) it.webhook = d.webhook;
      if (typeSel.value !== d.type) { delete it.token; delete it.webhook; }
    }
    if (secretIn.value.trim()) {
      // Filed under this desk, in a shape no script can ask for
      const sk = "notify/" + desk.id.trim() + "/" + slugId(n) + (forTelegram() ? "-token" : "");
      const r = await saveSecret({key: sk, description: "notify " + n,
        value: secretIn.value.trim(), human: true, ai: false, urls: []});
      if (!r.ok) { toast(r.error || T["settings.secrets.save_failed"], true); return; }
      if (forTelegram()) { it.token = "@" + sk; delete it.webhook; }
      else { it.webhook = "@" + sk; delete it.token; }
    }
    if (forTelegram()) it.chat_id = chatIn.value.trim();
    if (editing && n !== name) delete desk.notify[name];
    desk.notify[n] = it;
    if (primIn.checked) desk.primary_notify = n;
    else if (desk.primary_notify === n || (editing && desk.primary_notify === name)) {
      delete desk.primary_notify;
    }
    refreshSave(); shut(); redraw();
    toast(fill(T["settings.notify.saved_name"], {name: n}));
  });
  shape();
  setTimeout(() => (editing ? secretIn : nameIn).focus(), 0);
}

// The phones that receive notifications. Nothing here can be typed: the phone
// asks its own browser for permission and hands back what it gets, so this is
// the way to get the phone to the right page, and the list of the ones that
// answered. A phone is this machine's, signed up once; whether a desk's
// messages go to it is ticked on that desk's page
function phoneNotifyCard() {
  return card(T["settings.notify.phone.title"],
    el("div", {class:"hint"}, T["settings.notify.phone.sub"]),
    phoneBox());
}
const settingsApi = (path, body) => fetch(path, body === undefined
  ? {headers:{"X-Token":TOKEN}}
  : {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
     body: JSON.stringify(body)}).then(r => r.json());

// Whether the browser drawing this page can register itself. The app's own
// window (the one with the ipc bridge) is the PC, and a subscription made
// there would buzz the PC -- which already has "This PC" for that.
function canRegisterHere() { return !window.ipc; }

// The registered phones, fetched once and shared by every card that needs to
// know whether "notify a phone" would reach one. Dropped whenever a phone is
// added or forgotten (phonesChanged), so the answer never outlives the list.
let phonesKnown = null;
function phones() {
  if (!phonesKnown) {
    phonesKnown = settingsApi("/api/push")
      .catch(() => ({error: T["settings.notify.phone.failed"], subs: []}));
  }
  return phonesKnown;
}
// Every drawing of the list redraws itself on this, so a phone registered
// from the popup shows up in the row behind it too.
function phonesChanged() {
  phonesKnown = null;
  document.dispatchEvent(new Event("phones-changed"));
}

async function subscribeThisDevice() {
  if (await shikishaSubscribe(settingsApi, T, toast)) phonesChanged();
}

// The pairing QR, at whatever size the place has room for. The image is
// loaded directly rather than via fetch, so auth rides as the token in the URL.
function qrImage(px) {
  return el("img", {src:"/api/remote/qr?token=" + encodeURIComponent(TOKEN),
    style:"width:" + px + "px;height:" + px + "px;border-radius:8px;background:#fff;padding:6px"});
}

// One "phone" destination's row.
function phoneBox() {
  const box = el("div", {style:"flex:1 1 0;min-width:220px"});

  // The phones there are, each with the way to forget it.
  const drawList = (list, subs) => {
    list.textContent = "";
    for (const sub of subs) {
      list.append(el("div", {class:"row", style:"gap:var(--s2);align-items:center"},
        el("span", {class:"hint", style:"flex:1 1 0"}, sub.name || sub.endpoint.slice(0, 40)),
        el("button", {class:"quiet", onclick: async () => {
          await settingsApi("/api/push/forget", {endpoint: sub.endpoint});
          phonesChanged();
        }}, T["common.delete"])));
    }
  };

  // In a browser: the button. It is the one thing to press, so until it has
  // been pressed it is drawn as the primary and blinks.
  const drawHere = () => {
    const said = el("div", {class:"hint", style:"margin-top:var(--s2)"}, T["settings.notify.phone.checking"]);
    const add = el("button", {class:"quiet", onclick: () => subscribeThisDevice()},
                   T["settings.notify.phone.add"]);
    const list = el("div", {style:"margin-top:var(--s1)"});
    // The button on a line of its own, the words under it: side by side, the
    // words ran into the buttons to the right on a phone's width.
    box.append(el("div", {}, add), said, list);
    phones().then(j => {
      if (j.error) { said.textContent = j.error; said.classList.add("warn"); return; }
      const subs = j.subs || [];
      add.className = subs.length ? "quiet" : "primary pulse";
      said.textContent = subs.length ? fill(T["settings.notify.phone.count"], {n: subs.length})
                                     : T["settings.notify.phone.none"];
      said.classList.toggle("warn", !subs.length);
      drawList(list, subs);
    });
  };

  // In the app's window: no button, because nothing it could register would
  // be a phone. Instead, the two steps, the QR that makes the first one a
  // scan, and what stands in the way when the link is not one a phone will
  // accept. While no phone is registered the list is asked again every few
  // seconds, so the row changes on its own the moment the phone presses.
  const drawFromPc = async () => {
    const said = el("div", {class:"hint"}, T["settings.notify.phone.checking"]);
    const list = el("div", {style:"margin-top:var(--s1)"});
    const side = el("div", {style:"flex:none"});
    box.append(
      el("div", {style:"font-weight:600;margin-bottom:var(--s1)"}, T["settings.notify.phone.pc.title"]),
      el("div", {class:"row", style:"gap:var(--s4);align-items:flex-start;flex-wrap:wrap"},
        el("div", {style:"flex:1 1 220px"},
          el("ol", {style:"margin:0 0 var(--s2);padding-left:var(--s5)"},
            el("li", {}, T["settings.notify.phone.pc.step1"]),
            el("li", {}, T["settings.notify.phone.pc.step2"])),
          said, list),
        side));
    const [j, r] = await Promise.all([phones(),
      settingsApi("/api/remote").catch(() => ({}))]);
    if (j.error) { said.textContent = j.error; said.classList.add("warn"); }
    else {
      const subs = j.subs || [];
      said.textContent = subs.length ? fill(T["settings.notify.phone.count"], {n: subs.length})
                                     : T["settings.notify.phone.pc.none"];
      said.classList.toggle("warn", !subs.length);
      drawList(list, subs);
      if (!subs.length) setTimeout(() => {
        if (box.isConnected) { phonesKnown = null; draw(); }
      }, 4000);
    }
    if (r.running && r.origin && r.https) side.append(qrImage(140));
    else {
      side.append(el("div", {class:"hint warn", style:"max-width:220px"},
        r.running ? T["settings.notify.phone.pc.needs_https"]
                  : T["settings.notify.phone.pc.needs_remote"], " ",
        el("a", {href:"#", onclick: e => { e.preventDefault(); goSection("remote"); }},
           T["settings.notify.phone.pc.go_remote"])));
    }
  };

  const draw = () => {
    box.textContent = "";
    if (canRegisterHere()) drawHere(); else drawFromPc();
  };
  // Redrawn whenever the list changes -- and let go of once this row is no
  // longer on the page (a popup that closed), so it does not keep drawing
  // into nothing.
  const onChange = () => {
    if (!box.isConnected) { document.removeEventListener("phones-changed", onChange); return; }
    draw();
  };
  document.addEventListener("phones-changed", onChange);
  draw();
  return box;
}

// The inline "add a destination" flow: the Notifications card in a popup, so
// a tab can be pointed at a phone or a channel without leaving this screen.
// The global card stays too (this reuses it).
function openNotifyPopup() {
  return new Promise(resolve => {
    const m = openModal(
      el("h2", {}, T["settings.tab.notify.add_title"]),
      notifyCard(desks[sel.desk]),
      el("div", {class:"row", style:"border-top:1px solid var(--line);margin-top:var(--s3);padding-top:var(--s3);justify-content:flex-end"},
        el("button", {class:"primary", onclick: () => { m.remove(); resolve(); }}, T["common.done"])));
    m.addEventListener("click", e => { if (e.target === m) resolve(); });
  });
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
    el("div", {class:"hint", style:"margin-bottom:var(--s3)"}, T["settings.resume.intro"]),
    // This page is what somebody reads when they ask whether their conversation
    // comes back, so it has to say where the switch for that is. The switch
    // itself stays in one place -- two controls for one setting is how they
    // start disagreeing
    el("div", {class:"hint", style:"margin-bottom:var(--s3)"}, T["settings.resume.where"]),
    list,
    el("div", {class:"hint", style:"margin-top:var(--s3)"}, T["settings.resume.note"]));
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
    const right = el("div", {style:"display:flex;flex-direction:column;gap:var(--s2);min-width:0;flex:1"});
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
      const pre = el("pre", {class:"mono", style:"display:none;white-space:pre-wrap;margin:var(--s1) 0;" +
        "padding:8px;background:var(--panel);border:1px solid var(--line);border-radius:6px;font-size:11px"},
        r.hook.preview);
      const show = el("a", {href:"#"}, T["settings.resume.show"]);
      show.addEventListener("click", (e) => {
        e.preventDefault();
        pre.style.display = pre.style.display === "none" ? "block" : "none";
      });
      right.append(state, el("div", {style:"display:flex;gap:var(--s2);align-items:center"}, btn, show), pre);
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
  const keyfile = el("div", {class:"hint", style:"margin-top:var(--s2)"});

  const box = card(T["settings.section.api"],
    el("div", {class:"hint", style:"margin-bottom:var(--s3)"}, T["settings.api.intro"]),
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
    el("div", {class:"hint", style:"margin-top:var(--s3)"}, T["settings.api.note"]),
    el("div", {style:"margin-top:var(--s2)"},
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
  const qrbox = el("div", {style:"margin:var(--s3) 0"});

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
      const wrap = el("div", {style:"display:flex;flex-direction:column;gap:var(--s2);min-width:0"});
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
      const bad = el("span", {class:"hint", style:"color:var(--danger)"});
      const check = () => {
        const short = on.checked && tok.value.trim().length < 16;
        bad.textContent = short ? fill(T["settings.phone.sticky.short"], {n: 16}) : "";
        tok.style.borderColor = short ? "var(--danger)" : "";
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
  box.append(el("div", {class:"hint", style:"margin-top:var(--s2)"},
    T["settings.phone.note"]));

  // Who holds a key right now. Each device that opened the link is a row of
  // its own, and each row can be given a name and taken away on its own -- the
  // reason the book exists. A phone lost on a train costs that phone.
  const devices = el("div", {style:"margin-top:var(--s3)"});
  box.append(devices);

  refreshRemote();
  refreshDevices();

  /// When a device was last heard from, said the way a person says it.
  function whenSeen(secs) {
    if (!secs) return T["settings.phone.device.never"] || "has not come back yet";
    const ago = Math.max(0, Math.floor(Date.now() / 1000) - secs);
    if (ago < 90) return T["settings.phone.device.now"] || "just now";
    if (ago < 3600) return fill(T["settings.phone.device.minutes"] || "{n} minutes ago", {n: Math.floor(ago / 60)});
    if (ago < 86400) return fill(T["settings.phone.device.hours"] || "{n} hours ago", {n: Math.floor(ago / 3600)});
    return fill(T["settings.phone.device.days"] || "{n} days ago", {n: Math.floor(ago / 86400)});
  }

  async function refreshDevices() {
    let j = {};
    try { j = await (await fetch("/api/remote/clients", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    const rows = (j.clients || []);
    devices.textContent = "";
    // Titled like the rows above it (port, password, sticky token) rather than
    // as a note. It is a thing you act on, not a remark about one
    devices.append(el("div", {class:"row"}, el("label", {}, T["settings.phone.devices"])));
    if (!rows.length) {
      devices.append(el("div", {class:"hint"}, T["settings.phone.devices.none"]));
      return;
    }
    for (const c of rows) {
      const name = el("input", {type:"text", class:"devname",
        placeholder: T["settings.phone.device.unnamed"], value: c.name || ""});
      name.addEventListener("change", async () => {
        await fetch("/api/remote/clients/name", {method:"POST",
          headers:{"X-Token":TOKEN}, body:JSON.stringify({id:c.id, name:name.value})});
        refreshDevices();
      });
      // Says what goes before it goes. A row of six-character ids with a ✕
      // beside each is a screen where the safe move is to press nothing
      const drop = el("button", {class:"danger", onclick: async () => {
        const called = name.value.trim() || (T["settings.phone.device.unnamed"] || "");
        if (!await confirmAction(fill(T["settings.phone.device.confirm"], {name: called}), T["settings.phone.device.revoke"])) return;
        let ok = false;
        try {
          const r = await fetch("/api/remote/clients/revoke", {method:"POST",
            headers:{"X-Token":TOKEN}, body:JSON.stringify({id:c.id})});
          ok = ((await r.json()) || {}).ok === true;
        } catch (e) {}
        // Saying nothing after a press that did nothing is the worst of the
        // three: the row stays, and whoever pressed it believes the key is gone
        if (!ok) msg(T["settings.phone.device.gone"], true);
        refreshDevices();
      }}, T["settings.phone.device.revoke"]);
      devices.append(el("div", {class:"listrow devrow"}, name,
        el("span", {class:"when"},
          fill(T["settings.phone.device.last"], {when: whenSeen(c.seen)})),
        drop));
    }
  }
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
        style:"width:200px;height:200px;border-radius:var(--r-ctl);background:#fff;padding:var(--s2)"});
      qrbox.append(el("div", {class:"hint"}, T["settings.phone.scan"]), img, linkRow(j.kind));
      // Whether this link is one a browser will treat as a secure context,
      // which is what a phone wants before it will keep the page on its home
      // screen. The link itself is never drawn (it carries the token), so
      // without saying it here there is no way to tell which one you have.
      if (j.https) {
        qrbox.append(el("div", {class:"hint ok", style:"margin-top:var(--s2)"},
                        T["settings.phone.https.on"]));
      } else if (j.tailscale) {
        // Just the port. `tailscale serve` reads that as the loopback, which
        // is the only place it can reach: handed this machine's own tailnet
        // address it does not route back to itself, and every request comes
        // back 502. The board listens on the loopback as well for exactly
        // this (see RemoteUi::start_with).
        qrbox.append(
          el("div", {class:"hint", style:"margin-top:var(--s2)"}, T["settings.phone.https.hint"]),
          el("code", {style:"display:block;margin-top:var(--s1);user-select:all;word-break:break-all"},
             "tailscale serve --bg " + (r.port || 8787)));
      }
      // Keeping it as an app needs two things, and the one above is only the
      // first. The second is a key that is still the same key tomorrow: an
      // icon installed while the key rotates opens a board it cannot pair
      // with, because pairing means scanning a code, which opens the browser
      // rather than the installed app. Said one step at a time -- while the
      // link is not HTTPS the line above is the step, and this would be a
      // second thing to fix at once
      if (j.https) {
        qrbox.append(el("div", {class: "hint" + (r.sticky_token ? " ok" : ""),
                                style:"margin-top:var(--s2)"},
          r.sticky_token ? T["settings.phone.install.on"] : T["settings.phone.install.need"]));
      }
      // Where the whole thing is written out: what works on the same Wi-Fi
      // with nothing installed, what reaching it from a cafe costs, and what
      // the line above is for. The address is the link's own text, so it is
      // readable and typable even where a window will not follow it.
      qrbox.append(el("div", {style:"margin-top:var(--s3)"},
        el("span", {class:"hint"}, T["settings.phone.guide"] + " "),
        el("a", {class:"hint", href:T["settings.phone.guide.url"], target:"_blank"},
           T["settings.phone.guide.url"])));
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
    const row = el("div", {style:"display:flex;align-items:center;gap:var(--s2);margin-top:var(--s2);flex-wrap:wrap"},
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

// What the assistant AI is called, for the rows that say "the same as that
// one". Nothing installed and nothing chosen leaves it unnamed rather than
// naming a CLI this machine does not have
function aiLabelOf(id) {
  const found = aiEngines.find(e => e.id === (id || "")) || ((id || "").trim() ? null : aiEngines[0]);
  return found ? found.label : T["settings.ai_engine.none"];
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

// A desk's settings, one card to an entry in the list -- the same shape the
// program's own settings have. `id` is also the deep-link handle
function deskSections(desk) {
  const s = (id, build) => ({id, label:T["settings.dsec." + id], sub:T["settings.dsec." + id + ".sub"], build});
  const list = [
    s("basic", deskBasic),
    s("notify", notifyCard),
    s("providers", providersCard),
    s("browser", deskBrowserCard),
    s("permissions", permissionsCard),
    s("git", gitCard),
    s("gitaccounts", gitAccountsCard),
    s("secrets", deskSecretsCard),
    s("discuss", deskDiscussCard),
    s("stops", deskStopsCard),
    s("tools", deskToolsCard),
    s("labels", deskLabelsCard),
  ];
  // Written by hand in the file, so listed only where there is something written
  if (deskCapsCard(desk)) list.splice(4, 0, s("caps", deskCapsCard));
  // It ends in a file dialog, which a phone has no way to open
  if (!REMOTE) list.push(s("share", deskShareCard));
  return list;
}

// Whether the tools may send the framed part of the screen to the assistant
// AI. The same answer the tool asks for in place the first time, kept on the
// desk: ticked here or there, it is one setting. What is stored is the AI it
// was agreed for, so a change of assistant AI is asked about again
function deskToolsCard(desk) {
  // The AI that would be sent to: the one chosen, or the first installed --
  // the same order the program picks in (webui.rs, assistant_ai)
  const now = aiEngines.find(e => e.id === (current.ai_engine || "")) || (current.ai_engine ? null : aiEngines[0]);
  const labelOf = id => (aiEngines.find(e => e.id === id) || {}).label || id;
  const box = el("input", {type:"checkbox"});
  box.checked = !!desk.send_pictures_to;
  box.disabled = !now && !desk.send_pictures_to;
  const tick = el("label", {class:"check"});
  tick.append(box, document.createTextNode(T["settings.desk.pictures.label"]));
  const said = el("div", {class:"hint"});
  const draw = () => {
    const agreed = desk.send_pictures_to;
    if (!now) said.textContent = agreed
      ? fill(T["settings.desk.pictures.agreed"], {by: labelOf(agreed)}) + " " + T["settings.desk.pictures.none"]
      : T["settings.desk.pictures.none"];
    else if (!agreed) said.textContent = fill(T["settings.desk.pictures.service"], {by: now.label})
      + " " + T["settings.desk.pictures.unticked"];
    else if (agreed !== now.id) said.textContent = fill(T["settings.desk.pictures.changed"], {by: labelOf(agreed), now: now.label});
    else said.textContent = fill(T["settings.desk.pictures.agreed"], {by: now.label})
      + " " + fill(T["settings.desk.pictures.service"], {by: now.label})
      + " " + T["settings.desk.pictures.withdraw"];
  };
  box.addEventListener("change", () => {
    desk.send_pictures_to = box.checked && now ? now.id : "";
    box.checked = !!desk.send_pictures_to;
    refreshSave();
    draw();
  });
  draw();
  return card(T["settings.desk.pictures.title"], el("div", {class:"row"}, tick), said);
}

// Whether a page may be handed to a model so that a goal written in ordinary
// words can be carried out on it. Stored as the models it was agreed for, so
// pointing the setting at a different company asks again rather than assuming
function deskPagesCard(desk) {
  // Every model a page on this desk would be handed to: the desk's, and each
  // browser tab's own where it has one. One agreement covers them all, and
  // the app checks each page against it
  const b = desk.browser || {};
  const names = [];
  for (const k of WORDS_KEYS) names.push(b[k]);
  (desk.tabs || []).forEach(t => {
    if (catOf(t.command) !== "browser") return;
    for (const k of WORDS_KEYS) names.push((t[k] || "").trim() || b[k]);
  });
  const now = names.map(x => (x || "").trim()).filter(x => x)
    .filter((x, i, a) => a.indexOf(x) === i).sort().join(" + ");
  const box = el("input", {type:"checkbox"});
  box.checked = !!desk.send_pages_to;
  box.disabled = !now && !desk.send_pages_to;
  const tick = el("label", {class:"check"});
  tick.append(box, document.createTextNode(T["settings.desk.pages.label"]));
  const said = el("div", {class:"hint"});
  const draw = () => {
    const agreed = (desk.send_pages_to || "").trim();
    if (!now) said.textContent = agreed
      ? fill(T["settings.desk.pages.agreed"], {by: agreed}) + " " + T["settings.desk.pages.none"]
      : T["settings.desk.pages.none"];
    else if (!agreed) said.textContent = fill(T["settings.desk.pages.service"], {by: now})
      + " " + T["settings.desk.pages.unticked"];
    else if (!now.split(" + ").every(n => agreed.split(" + ").includes(n)))
      said.textContent = fill(T["settings.desk.pages.changed"], {by: agreed, now: now});
    else said.textContent = fill(T["settings.desk.pages.agreed"], {by: now})
      + " " + fill(T["settings.desk.pages.service"], {by: now})
      + " " + T["settings.desk.pages.withdraw"];
  };
  box.addEventListener("change", () => {
    desk.send_pages_to = box.checked && now ? now : "";
    box.checked = !!desk.send_pages_to;
    refreshSave();
    draw();
  });
  draw();
  return card(T["settings.desk.pages.title"], el("div", {class:"row"}, tick), said);
}

// Which AI writes the names and summaries of this desk's folders that have
// Auto on. The assistant AI unless something else is chosen: it is asked the
// light way, so it costs little. A model connection runs on that connection's
// own account, which is the reason to choose one
function deskLabelsCard(desk) {
  const assistant = aiEngines.find(e => e.id === (current.ai_engine || "")) || (current.ai_engine ? null : aiEngines[0]);
  const picker = el("select");
  picker.append(el("option", {value:""}, assistant
    ? fill(T["settings.labels.ai.assistant"], {name: assistant.label})
    : T["settings.labels.ai.assistant_none"]));
  for (const e of aiEngines) picker.append(el("option", {value:e.id}, e.label));
  const providers = Object.keys(deskProviders());
  for (const p of providers) picker.append(el("option", {value:"model:" + p}, fill(T["wizard.discuss.model_suffix"], {name: p})));
  const written = (desk.summary_ai || "").trim();
  const asModel = written.match(/^model\s+([^/\s]+)\/(.*)$/);
  picker.value = asModel ? "model:" + asModel[1] : written;
  // A choice no longer on offer -- an AI since uninstalled, a connection since
  // removed -- is still what the settings say, and shown as that
  if (picker.value !== (asModel ? "model:" + asModel[1] : written)) {
    picker.append(el("option", {value: asModel ? "model:" + asModel[1] : written}, written));
    picker.value = asModel ? "model:" + asModel[1] : written;
  }
  const modelIn = el("input", {type:"text", class:"mono", style:"width:220px"});
  modelIn.value = asModel ? asModel[2].trim() : "";
  const provider = () => picker.value.startsWith("model:") ? picker.value.slice(6) : "";
  const cand = modelCandidates(() => deskProviders()[provider()] || {}, id => { modelIn.value = id; store(); });
  const modelRow = row(T["settings.labels.model"], modelIn, cand.btn, cand.chips);
  const store = () => {
    const p = provider();
    // A connection with no model named asks for the one it is known for, as
    // the placeholder says, rather than for a model called nothing
    const v = p ? "model " + p + "/" + (modelIn.value.trim() || DEFAULT_MODEL[p] || "") : picker.value;
    if (v) desk.summary_ai = v; else delete desk.summary_ai;
    refreshSave();
  };
  const sync = () => {
    const p = provider();
    modelRow.hidden = !p;
    if (p) modelIn.placeholder = DEFAULT_MODEL[p] || T["wizard.discuss.model_ph"];
    else cand.chips.textContent = "";
  };
  picker.addEventListener("change", () => { sync(); store(); });
  modelIn.addEventListener("input", store);
  sync();
  return card(T["settings.labels.title"],
    el("div", {class:"hint"}, T["settings.labels.hint"]),
    row(T["settings.labels.ai"], picker, el("span", {class:"hint"}, T["settings.labels.ai.hint"])),
    modelRow,
    row(T["settings.labels.branch"],
        checkDefaultOn(desk, "rename_branch", T["settings.labels.branch.label"]),
        el("span", {class:"hint"}, T["settings.labels.branch.hint"])));
}

function deskShareCard() {
  // Writing it out is about this desk. Reading one in makes a different
  // one, so it is asked for where another desk is asked for
  return card(T["settings.desk.share"],
    el("div", {class:"row"},
      el("button", {onclick:() => exportWs(sel.desk)}, T["settings.desk.export"])),
    el("div", {class:"hint"}, T["settings.desk.share.hint"]));
}

function deskBasic(desk) {
  const box = el("div");
  // Name and id are identity, and sit together. The id is what automation and
  // the secret store use, so it survives renaming what is on screen -- and a
  // desk that arrives here without one gets it now, from its name, rather
  // than waiting for a save
  if (!(desk.id || "").trim()) {
    desk.id = uniqueWsId(slugId(desk.name) || "desk", desk);
    refreshSave();
  }
  const deskIdInput = field(desk, "id", "", {grow:false, width:280, mono:true});
  box.append(card(T["settings.desk"],
    row(T["settings.desk.name"], field(desk, "name", T["settings.desk.name"], {grow:false, width:280,
        onInput:() => renderNav()})),
    row(T["settings.desk.id"], deskIdInput,
        el("span", {class:"hint"}, T["settings.desk.id.hint"])),
    desk.file ? row(T["settings.desk.file"], el("span", {class:"hint mono"}, desk.file)) : null,
    row(T["settings.tab.automation"], ...pathField(desk, "automation", T["settings.desk.automation.hint"], "dir",
        T["settings.tab.automation_dir.pick"]),
        el("span", {class:"hint"}, T["settings.desk.automation.hint"]))));

  if (!(desk.tabs || []).length) {
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

  box.append(el("div", {class:"row"},
    el("button", {class:"danger", onclick: async () => {
      if (!await confirmAction(fill(T["settings.desk.delete_confirm"], {name: desk.name}), T["settings.desk.delete"])) return;
      // Everything filed under this desk's name goes with it: what its
      // automation used, and what its server tabs signed in with. Said out
      // loud first, because a password cannot be got back
      const id = (desk.id || "").trim();
      if (id) {
        const j = await fetchSecrets();
        const mine = ((j && j.secrets) || [])
          .map(s => s.key)
          .filter(k => k.startsWith(id + ".") || ["ssh/", "provider/", "notify/", "git/"].some(p => k.startsWith(p + id + "/")));
        if (mine.length &&
            !await confirmAction(fill(T["settings.desk.delete_secrets"], {n: mine.length}), T["settings.desk.delete"])) return;
        await dropSecrets(mine);
      }
      desks.splice(sel.desk, 1); toTree(0); render();
    }}, T["settings.desk.delete"])));
  return box;
}

// The git accounts this desk signs in with.
//
// A boxed list you read down and one dialog to change one of them, the same
// shape as the model connections above. Nothing here is chosen for anything:
// a git tab picks its own account on its page, and a project picks the one the
// column beside its folders uses. The token is write-only -- it goes to the
// secrets file under this desk and never comes back to the screen.
// The parts of that card, built once and used twice: on the desk's own page,
// and inside the window the account picker opens (gitAccountsWindow). One
// list, so a way of adding an account cannot appear in one place and not the
// other
function gitAccountsParts(desk) {
  desk.git_accounts = desk.git_accounts || [];
  const listBox = el("div");
  const draw = () => {
    listBox.textContent = "";
    if (!desk.git_accounts.length) {
      listBox.append(el("div", {class:"hint"}, T["settings.gitacct.empty"]));
      return;
    }
    const rows = el("div", {class:"rows"});
    for (const a of desk.git_accounts) {
      const state = el("span", {class:"hint secretsite"}, "");
      rows.append(el("div", {class:"listrow secretrow", onclick: () => gitAccountDialog(desk, a.name, draw)},
        el("span", {class:"mono secretname"}, a.name),
        el("span", {class:"hint mono secretdesc"}, gitAccountAbout(a)),
        el("span", {class:"hint"}, signInLabel(a)),
        state,
        el("span", {class:"go"}, "›")));
      gitAccountState(desk, a, state);
    }
    listBox.append(rows);
  };
  setTimeout(draw, 0);
  return [
    el("div", {class:"hint"}, T["settings.gitacct.hint"]),
    listBox,
    el("div", {class:"row"},
      el("button", {onclick: () => gitAccountDialog(desk, null, draw)}, T["settings.gitacct.add"])),
    el("div", {class:"hint"}, T["settings.gitacct.where"]),
    el("div", {class:"hint"}, T["settings.gitacct.terminal"]),
  ];
}

function gitAccountsCard(desk) {
  const c = card(T["settings.gitacct.title"], ...gitAccountsParts(desk));
  c.id = "desk-gitaccounts";
  return c;
}

// The desk's git accounts, in a window over the page that asked for them.
//
// Opened from the account picker, where somebody holding a token is standing
// when they find out there is nowhere on that page to put it. The page
// underneath keeps everything typed into it, and `done` is called however this
// window is closed -- the picker has to read the list again either way
function gitAccountsWindow(desk, done) {
  const shut = () => { back.remove(); done(); };
  const back = openModal(
    el("div", {class:"mhead"},
      el("h2", {}, (desk.name || T["settings.nav.desk"]) + " › " + T["settings.dsec.gitaccounts"]),
      el("button", {class:"quiet icon", title:T["common.close"], onclick: () => shut()}, "✕")),
    el("div", {class:"mbody"}, ...gitAccountsParts(desk)),
    el("div", {class:"mfoot"},
      el("span", {class:"grow"}),
      el("button", {class:"primary", onclick: () => shut()}, T["common.close"])));
  back.firstChild.classList.add("framed");
  // A press on the backdrop takes the window away on its own (openModal), so
  // the picker is told here rather than in shut()
  back.addEventListener("mousedown", e => { if (e.target === back) done(); });
  back.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); shut(); }
  });
}
const GIT_HOST = "github.com";
// The last line of the account picker, which adds an account instead of
// choosing one. An account is named in letters, digits, _ and -, so this can
// never be one -- the same way THIS_PC cannot
const ADD_ACCOUNT = "@add";
const isSshAccount = a => (a.method || "").trim().toLowerCase() === "ssh";
// An account that signs in as whoever GitHub CLI (gh) is signed in as on this PC
const isGhAccount = a => (a.method || "").trim().toLowerCase() === "gh";
const signInLabel = a => isSshAccount(a) ? T["settings.gitacct.by_ssh"]
  : isGhAccount(a) ? T["settings.gitacct.by_gh"] : T["settings.gitacct.by_token"];
const accountHost = a => ((a.host || "").trim().replace(/\/+$/, "").toLowerCase()) || GIT_HOST;
// Who it signs in as, in a few words: the user name and the server
const gitAccountAbout = a => ((a.login || "").trim() ? a.login.trim() + "@" : "") + accountHost(a);
const gitTokenKey = (desk, name) => "git/" + (desk.id || "").trim() + "/" + name;

// Whether the token still works, whose it is and how long it has left, said on
// the account's row. Only a GitHub account can be asked; the state and the
// date come back, never the value
async function gitAccountState(desk, a, out) {
  if (accountHost(a) !== GIT_HOST || !(desk.id || "").trim()) return;
  let j;
  try {
    j = await (await fetch("/api/github?desk=" + encodeURIComponent(desk.id.trim())
      + "&account=" + encodeURIComponent(a.name), {headers:{"X-Token":TOKEN}})).json();
  } catch (e) { return; }
  if (!j.source) {
    out.textContent = isSshAccount(a) ? T["settings.gitacct.no_pr"]
      : isGhAccount(a) ? T["settings.gitacct.no_gh"] : T["settings.gitacct.no_token"];
    out.classList.toggle("warn", !isSshAccount(a));
  } else if (j.signed_in) {
    const who = j.login ? fill(T["settings.gitacct.as"], {login: j.login}) : T["settings.gitacct.ok"];
    const left = (j.expires_days === null || j.expires_days === undefined) ? ""
      : " · " + fill(T["settings.gitacct.days"], {n: j.expires_days});
    out.textContent = who + left;
    out.classList.toggle("warn", j.expires_days !== null && j.expires_days !== undefined && j.expires_days <= 7);
  } else {
    // Short on the row, which has one line; the whole sentence under the pointer
    out.textContent = j.status === 401 ? T["settings.gitacct.expired_short"] : T["settings.gitacct.unreachable_short"];
    out.title = j.status === 401 ? T["settings.gitacct.expired"]
      : fill(T["settings.gitacct.unreachable"], {status: j.status || 0});
    out.classList.add("warn");
  }
}

// Adding a git account to a desk, or changing one. `name` is null for a new one.
function gitAccountDialog(desk, name, redraw) {
  const editing = !!name;
  const a = editing ? (desk.git_accounts.find(x => x.name === name) || {}) : {};
  const input = (value, attrs) => { const i = el("input", Object.assign({type:"text"}, attrs || {})); i.value = value || ""; return i; };
  const nameIn = input(name, {class:"mono", placeholder:T["settings.gitacct.name_ph"]});
  nameIn.disabled = editing;
  const hostIn = input(a.host, {class:"mono", placeholder:GIT_HOST});
  const method = el("select");
  method.append(el("option", {value:"token"}, T["settings.gitacct.by_token"]),
                el("option", {value:"ssh"}, T["settings.gitacct.by_ssh"]),
                el("option", {value:"gh"}, T["settings.gitacct.by_gh"]));
  method.value = isSshAccount(a) ? "ssh" : isGhAccount(a) ? "gh" : "token";
  const loginIn = input(a.login, {class:"mono", placeholder:T["settings.gitacct.login_ph"]});
  const tokenIn = el("input", {type:"password", placeholder:T["settings.gitacct.token_ph"]});
  const keyIn = input(a.key, {class:"mono", placeholder:T["settings.gitacct.key_ph"]});
  const userIn = input(a.user_name, {placeholder:T["settings.gitacct.user_ph"]});
  const mailIn = input(a.user_email, {class:"mono", placeholder:"me@example.com"});
  const ownersIn = input((a.owners || []).join(", "), {class:"mono", placeholder:T["settings.gitacct.owners_ph"]});
  // Whether a token is already stored, so the field can say "only to change it"
  // and a new token is not demanded of an account that has one -- and how the
  // store keeps what is typed, which is said beside the field before it is
  let hasToken = false, storeMode = "";
  fetchSecrets().then(j => {
    storeMode = (j && j.mode) || "";
    hasToken = editing && !!(desk.id || "").trim()
      && ((j && j.secrets) || []).some(s => s.key === gitTokenKey(desk, name));
    tokenIn.placeholder = hasToken ? T["settings.gitacct.token_set_ph"] : T["settings.gitacct.token_ph"];
    recheck();
  });

  const save = el("button", {class:"primary"}, T["common.save"]);
  const why = el("span", {class:"why"});
  why.hidden = true;
  let held = null, asked = false;
  function fieldFault(inputEl, reason) {
    const wrap = inputEl.parentElement;
    const had = wrap.querySelector(".site-warn");
    const show = reason && (asked || inputEl.value.trim() !== "");
    if (had) had.remove();
    inputEl.classList.toggle("bad", !!show);
    if (show) wrap.append(el("div", {class:"site-warn"}, el("span", {}, "⚠"), el("span", {}, reason)));
  }
  const field = (label, control, hint) => el("div", {class:"field"},
    el("label", {}, label), el("div", {class:"fieldctl"}, control),
    hint ? el("div", {class:"hint"}, hint) : null);
  const loginField = field(T["settings.gitacct.login"], loginIn, T["settings.gitacct.login_hint"]);
  const tokenHint = el("div", {class:"hint"});
  // Where the value ends up. A store nobody has given a master password keeps
  // it as it stands, and that is said here, in the colour for danger, at the
  // moment the token is about to be typed
  const tokenPlain = el("div", {class:"warn"});
  tokenPlain.hidden = true;
  const tokenField = el("div", {class:"field"}, el("label", {}, T["settings.gitacct.token"]),
    el("div", {class:"fieldctl"}, tokenIn), tokenHint, tokenPlain);
  const keyField = field(T["settings.gitacct.key"], keyIn, T["settings.gitacct.key_hint"]);
  // Nothing to fill in for a gh account: it signs in with what GitHub CLI has
  const ghNote = el("div", {class:"hint"}, T["settings.gitacct.gh_hint"]);
  function recheck() {
    const ssh = method.value === "ssh";
    const gh = method.value === "gh";
    loginField.hidden = ssh || gh;
    keyField.hidden = !ssh;
    tokenField.hidden = gh;
    ghNote.hidden = !gh;
    tokenHint.textContent = ssh ? T["settings.gitacct.token_hint_ssh"] : T["settings.gitacct.token_hint"];
    // "empty" is a store with nothing in it yet, which is written the same way
    // the first time something is put in it
    const plain = storeMode === "plaintext" || storeMode === "empty";
    tokenPlain.textContent = plain ? T["settings.gitacct.token_plain"] : "";
    tokenPlain.hidden = !plain || gh;
    const faults = [];
    const n = nameIn.value.trim();
    const nameWhy = !n ? T["settings.gitacct.name_required"]
      : (!/^[A-Za-z0-9_-]+$/.test(n) ? T["settings.gitacct.name_bad"]
      : (!editing && desk.git_accounts.some(x => x.name === n) ? T["settings.gitacct.name_dup"] : null));
    fieldFault(nameIn, nameWhy);
    if (nameWhy) faults.push({at: nameIn, why: nameWhy});
    const tokenWhy = !ssh && !gh && !hasToken && !tokenIn.value.trim() ? T["settings.gitacct.token_required"] : null;
    fieldFault(tokenIn, tokenWhy);
    if (tokenWhy) faults.push({at: tokenIn, why: tokenWhy});
    const keyWhy = ssh && !keyIn.value.trim() ? T["settings.gitacct.key_required"] : null;
    fieldFault(keyIn, keyWhy);
    if (keyWhy) faults.push({at: keyIn, why: keyWhy});
    held = faults[0] || null;
    save.classList.toggle("held", !!held);
    if (!held) why.hidden = true;
    else if (!why.hidden) why.textContent = fill(T["settings.secrets.cannot_save"], {why: held.why});
  }
  for (const i of [nameIn, tokenIn, keyIn]) i.addEventListener("input", recheck);
  method.addEventListener("change", recheck);

  const shut = () => back.remove();
  const back = openModal(
    el("div", {class:"mhead"},
      el("h2", {}, editing ? T["settings.gitacct.edit_title"] : T["settings.gitacct.add_title"]),
      el("button", {class:"quiet icon", title:T["common.close"], onclick: () => shut()}, "✕")),
    el("div", {class:"mbody"},
      field(T["settings.gitacct.name"], nameIn, editing ? T["settings.gitacct.name_fixed"] : T["settings.gitacct.name_hint"]),
      field(T["settings.gitacct.host"], hostIn, T["settings.gitacct.host_hint"]),
      field(T["settings.gitacct.method"], method, null),
      ghNote, loginField, keyField, tokenField,
      field(T["settings.gitacct.user"], userIn, T["settings.gitacct.user_hint"]),
      field(T["settings.gitacct.mail"], mailIn, null),
      field(T["settings.gitacct.owners"], ownersIn, T["settings.gitacct.owners_hint"])),
    el("div", {class:"mfoot"},
      editing
        ? el("button", {class:"danger", onclick: async () => {
            if (!await confirmAction(fill(T["settings.gitacct.delete_confirm"], {name}), T["settings.gitacct.delete"])) return;
            // Its token goes with it. Tabs and projects that chose it keep the
            // name, and say on the git column that it is gone
            if ((desk.id || "").trim()) await deleteSecret(gitTokenKey(desk, name));
            desk.git_accounts = desk.git_accounts.filter(x => x.name !== name);
            refreshSave(); shut(); redraw();
          }}, T["settings.gitacct.delete"])
        : null,
      why,
      el("span", {class:"grow"}),
      el("button", {class:"quiet", onclick: () => shut()}, T["common.cancel"]),
      save));
  back.firstChild.classList.add("framed");
  back.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); shut(); return; }
    if (e.key !== "Enter" || e.target.tagName !== "INPUT") return;
    e.preventDefault();
    save.click();
  });
  recheck();

  save.addEventListener("click", async () => {
    if (held) {
      asked = true; recheck();
      why.textContent = fill(T["settings.secrets.cannot_save"], {why: held.why});
      why.hidden = false;
      held.at.classList.remove("lookhere"); void held.at.offsetWidth; held.at.classList.add("lookhere");
      held.at.focus();
      return;
    }
    // The token is filed under this desk, so it needs the name the store files
    // this desk under
    if (!(desk.id || "").trim()) { toast(T["settings.secrets.desk_needs_id"], true); return; }
    const n = editing ? name : nameIn.value.trim();
    if (method.value !== "gh" && tokenIn.value.trim()) {
      const r = await saveSecret({key: gitTokenKey(desk, n), description: "git account " + n,
        value: tokenIn.value.trim(), human: true, ai: false, urls: []});
      if (!r.ok) { toast(r.error || T["settings.secrets.save_failed"], true); return; }
    }
    const it = editing ? a : {name: n};
    const put = (k, v) => { if ((v || "").trim()) it[k] = v.trim(); else delete it[k]; };
    put("host", hostIn.value.trim().toLowerCase() === GIT_HOST ? "" : hostIn.value);
    if (method.value === "ssh") { it.method = "ssh"; put("key", keyIn.value); delete it.login; }
    else if (method.value === "gh") { it.method = "gh"; delete it.key; delete it.login; }
    else { delete it.method; delete it.key; put("login", loginIn.value); }
    put("user_name", userIn.value);
    put("user_email", mailIn.value);
    const owners = ownersIn.value.split(/[\s,]+/).map(s => s.trim()).filter(Boolean);
    if (owners.length) it.owners = owners; else delete it.owners;
    if (!editing) desk.git_accounts.push(it);
    refreshSave(); shut(); redraw();
    toast(fill(T["settings.gitacct.saved"], {name: n}));
  });
}

// The menu a git tab or a project chooses its account from.
//
// Every account of the desk is offered; the ones that say they are for this
// repository's owner come first, marked. Nothing is picked because of that --
// "not chosen" stays until somebody chooses, and the PC's own git is one of
// the choices rather than what happens when nobody does. `origin` is
// `owner/name` on GitHub, when known
function gitAccountSelect(desk, now, origin, pick) {
  const s = el("select");
  const owner = ((origin || "").split("/")[0] || "").toLowerCase();
  const rank = a => {
    const sameHost = !origin || accountHost(a) === GIT_HOST;
    const fits = sameHost && !!owner && (a.owners || []).some(o => o.trim().toLowerCase() === owner);
    return {fits, rank: fits ? 0 : sameHost ? 1 : 2};
  };
  const list = (desk.git_accounts || []).map((a, i) => Object.assign({a, i}, rank(a)))
    .sort((x, y) => x.rank - y.rank || x.i - y.i);
  s.append(el("option", {value:""}, T["settings.gitacct.pick"]));
  for (const {a, fits} of list) {
    s.append(el("option", {value:a.name},
      a.name + " — " + gitAccountAbout(a) + (fits ? "  " + T["settings.gitacct.fits"] : "")));
  }
  s.append(el("option", {value:THIS_PC}, T["settings.gitacct.pc"]));
  // With two GitHub accounts held, git cannot tell which to use: each is a
  // choice. One chosen before and no longer held is still said
  const chosen = (now || "").trim();
  const asPc = chosen.startsWith(THIS_PC + ":") ? chosen.slice(THIS_PC.length + 1) : "";
  const held = PC_ACCOUNTS.length > 1 ? [...PC_ACCOUNTS] : [];
  if (asPc && !held.includes(asPc)) held.push(asPc);
  for (const login of held) {
    s.append(el("option", {value: THIS_PC + ":" + login}, fill(T["settings.gitacct.pc_as"], {login})));
  }
  if (chosen && chosen !== THIS_PC && !asPc && !list.some(x => x.a.name === chosen)) {
    s.append(el("option", {value:chosen}, fill(T["settings.gitacct.gone"], {name: chosen})));
  }
  // Last, under every account there is: the way to add one, from the one place
  // somebody looking for it is already standing
  s.append(el("option", {value:ADD_ACCOUNT}, T["settings.gitacct.add_here"]));
  s.value = chosen;
  s.addEventListener("change", () => {
    if (s.value !== ADD_ACCOUNT) { pick(s.value); return; }
    // Adding is not a choice: the menu goes back to what was chosen, and the
    // account made in the window becomes the choice once it exists
    const had = (desk.git_accounts || []).map(a => a.name);
    s.value = chosen;
    gitAccountsWindow(desk, () => {
      const made = (desk.git_accounts || []).map(a => a.name).filter(n => !had.includes(n));
      const v = made.length ? made[made.length - 1] : chosen;
      // Set here as well, because a picker on a page that does not redraw
      // itself would otherwise still be showing the old answer
      s.value = v;
      pick(v);
    });
  });
  return s;
}

// What automation running here may reach outside the terminal.
//
// This is the one advanced setting with no editor: the gateways, and the folders
// and hosts raw paths are allowed in, are written by hand into this desk in the
// settings file. So what this card does is say what they are -- a gateway
// carries a token already attached, which is why it is worth being sure.
function deskCapsCard(desk) {
  const spec = desk.capabilities || {};
  const some = o => Object.keys(o.files || {}).length || Object.keys(o.http || {}).length
    || (o.allow_dirs || []).length || (o.allow_hosts || []).length;
  // Nothing is the ordinary state, and a card saying so on every page is
  // noise. Said only where there is something to be sure about
  if (!some(spec)) return null;
  const body = [el("div", {class:"hint"}, T["settings.desk.caps.own"])];
  const put = (label, list) => {
    if (list.length) body.push(el("div", {class:"hint mono"}, label + ": " + list.join(", ")));
  };
  put(T["settings.desk.caps.files"], Object.keys(spec.files || {}));
  put(T["settings.desk.caps.http"], Object.keys(spec.http || {}));
  put(T["settings.desk.caps.dirs"], spec.allow_dirs || []);
  put(T["settings.desk.caps.hosts"], spec.allow_hosts || []);
  body.push(el("div", {class:"hint"}, T["settings.desk.caps.where"]));
  const c = card(T["settings.desk.caps.title"], ...body);
  c.id = "desk-caps";
  return c;
}

// Where the work happens. One folder per group, and a tab has none of its own:
// a reviewer pointed somewhere other than the tab it reviews reviews nothing.
// With a single folder this is one field and the word "group" never appears --
// which is the state anyone who has not asked for a second one stays in.
// A folder, and everything about it. One page per folder, reached the same way
// it is reached in the tab list, because "where does this run" is a fact about
// the folder rather than about the desk it happens to sit in.
// Where a page stands, over it: the desk, the project, the folder, the tab --
// each one above the page a press away, the page itself last and not pressed.
// A worktree and a tab are not in the list, so this is how their pages say
// what they belong to, and the way up to any of those
function pageCrumbs(...parts) {
  const box = el("div", {class:"crumbs"});
  const all = parts.filter(Boolean);
  all.forEach((p, i) => {
    if (i) box.append(el("span", {class:"sep"}, "›"));
    box.append(i === all.length - 1
      ? el("span", {class:"here"}, p.label)
      : el("button", {class:"crumb", onclick:p.go}, p.label));
  });
  return box;
}
const deskCrumb = desk => ({label: desk.name || T["settings.tab.unnamed"], go: () => goDeskSection("basic")});
const projectCrumb = p => ({label: p.name, go: () => { sel = {desk:sel.desk, proj:p.key, grp:null, tab:null, global:false}; render(); window.scrollTo(0, 0); }});
const folderCrumb = (g, gi) => ({label: folderLabel(g, gi), go: () => { sel = {desk:sel.desk, grp:gi, tab:null, global:false}; render(); window.scrollTo(0, 0); }});

function folderPane(desk, g, gi) {
  const box = el("div");
  const tabsHere = () => (desk.tabs || []).filter(t => (t.group || 0) === gi);
  // Up to its project, when it is one of a project's folders
  const home = deskProjects(desk).projects.find(x => x.folders.includes(gi));
  box.append(pageCrumbs(deskCrumb(desk), home ? projectCrumb(home) : null, {label: folderLabel(g, gi)}));

  // This folder's own answer about which branches refuse a direct commit.
  // Unticked it follows the app's, which is what the box shows greyed out
  const ownProtect = el("input", {type:"checkbox"});
  ownProtect.checked = Array.isArray(g.protect);
  const protectBox = el("input", {class:"mono grow"});
  const drawProtect = () => {
    protectBox.disabled = !ownProtect.checked;
    protectBox.placeholder = ownProtect.checked
      ? T["settings.protect.ph"]
      : protectText(protectOf(desk));
    protectBox.value = Array.isArray(g.protect) ? protectText(g.protect) : "";
  };
  ownProtect.addEventListener("change", () => {
    if (ownProtect.checked) g.protect = protectOf(desk).slice();
    else delete g.protect;
    drawProtect();
    refreshSave();
  });
  protectBox.addEventListener("input", () => { g.protect = protectList(protectBox.value); refreshSave(); });
  drawProtect();
  const ownLabel = el("label", {class:"check"});
  ownLabel.append(ownProtect, document.createTextNode(T["settings.group.protect.own"]));

  // The name and the summary, and whether they are written for the folder
  // from what its AIs are asked. Writing either by hand takes that off: what a
  // person wrote is not written over
  const autoBox = el("input", {type:"checkbox"});
  autoBox.checked = !!g.auto_label;
  const autoLabel = el("label", {class:"check"});
  autoLabel.append(autoBox, document.createTextNode(T["settings.group.auto.label"]));
  const autoFailed = el("div", {class:"site-warn", hidden:true});
  const handWritten = () => {
    if (!g.auto_label) return;
    delete g.auto_label;
    autoBox.checked = false;
    autoFailed.hidden = true;
    refreshSave();
  };
  autoBox.addEventListener("change", () => {
    if (autoBox.checked) g.auto_label = true; else delete g.auto_label;
    autoFailed.hidden = true;
    refreshSave();
  });
  const summaryBox = el("textarea", {class:"short", rows:"3", placeholder:T["settings.group.summary.ph"]});
  summaryBox.value = g.summary || "";
  summaryBox.addEventListener("input", () => {
    if (summaryBox.value.trim()) g.summary = summaryBox.value; else delete g.summary;
    handWritten();
    refreshSave();
  });
  // Why the last try to write them failed, when it did. Only while it is on:
  // a folder that no longer asks has nothing to fix
  if (g.auto_label && (g.cwd || "").trim()) {
    fetch("/api/folder-label?path=" + encodeURIComponent(g.cwd), {headers:{"X-Token":TOKEN}})
      .then(r => r.json())
      .then(j => {
        if (!j || !j.failed || !g.auto_label) return;
        autoFailed.textContent = fill(T["settings.group.auto.failed"], {why: j.failed});
        autoFailed.hidden = false;
      })
      .catch(() => {});
  }

  box.append(card(T["settings.group.title"],
    row(T["settings.group.name"], field(g, "name", folderLabel(g, gi), {grow:false, width:280,
        onInput:() => { handWritten(); renderNav(); }}),
        el("span", {class:"hint"}, T["settings.group.name.hint"])),
    row(T["settings.group.summary"], summaryBox,
        el("span", {class:"hint"}, T["settings.group.summary.hint"])),
    row(T["settings.group.auto"], autoLabel,
        el("span", {class:"hint"}, T["settings.group.auto.hint"]), autoFailed),
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
    swatchesInto(colours, now, put);
    if (now) colours.append(el("button", {class:"quiet", onclick:() => put("")},
      T["settings.group.color.auto"]));
  };
  paint(null);
  box.append(card(T["settings.group.color"], colours,
    el("div", {class:"hint"}, T["settings.group.color.hint"])));

  const tabRows = el("div", {class:"rows"});
  (desk.tabs || []).forEach((t, ti) => {
    if ((t.group || 0) !== gi) return;
    tabRows.append(el("div", {class:"listrow secretrow", onclick:() => { sel = {desk:sel.desk, grp:gi, tab:ti, global:false}; render(); }},
      el("span", {class:"secretname"}, t.name || T["settings.tab.unnamed"]),
      el("span", {class:"hint mono secretdesc"}, cmdToText(t.command) || T["automation.unset"]),
      el("span", {class:"go"}, "›")));
  });
  box.append(card(T["settings.group.tabs"],
    el("div", {class:"hint"}, T["settings.group.tabs.hint"]),
    tabsHere().length ? tabRows : el("div", {class:"hint"}, T["settings.group.tabs.none"])));

  // Taking it out of the list, and -- for a folder the app made for a branch --
  // getting rid of the folder itself. Two different acts: one can be undone by
  // opening it again, and the other cannot
  // The tabs standing in the folder go with it. They have to: a line left
  // behind would point at the folder that moved up into its place, and it
  // could not be emptied first anyway -- the tabs written here are started
  // again on every launch, so a folder is never quietly tab-less
  const drop = () => {
    const gone = tabsHere();
    desk.tabs = (desk.tabs || []).filter(t => (t.group || 0) !== gi);
    desk.folders.splice(gi, 1);
    (desk.tabs || []).forEach(t => { if ((t.group || 0) > gi) t.group--; });
    sel = {desk:sel.desk, grp:null, tab:null, global:false};
    render(); refreshSave();
    // What each of them signed in with is that tab's own, and once the line
    // is gone nothing can name it again
    const w = (desk.id || "").trim();
    const keys = [];
    for (const t of gone) {
      const tid = (t.id || "").trim();
      if (w && tid) keys.push("ssh/" + w + "/" + tid + "/password",
                              "ssh/" + w + "/" + tid + "/passphrase");
    }
    if (keys.length) dropSecrets(keys);
  };
  const guard = () => {
    if ((desk.folders || []).length <= 1) { toast(T["settings.group.last"], true); return false; }
    return true;
  };
  // Asked when it takes tabs with it, and only then: the question is what the
  // press costs, and a folder with nothing in it costs nothing
  const askedDrop = async () => {
    const n = tabsHere().length;
    if (n && !await confirmAction(fill(T["settings.group.delete_confirm"],
        {name: folderLabel(g, gi), n: String(n)}), T["settings.group.delete"])) return;
    drop();
  };
  const buttons = el("div", {class:"row"},
    el("button", {class:"danger", onclick:() => { if (guard()) askedDrop(); }},
      T["settings.group.delete"]),
    el("span", {class:"hint"}, T["settings.group.delete.hint"]));
  box.append(buttons);

  familyOf(g.cwd, desk).then(where => {
    paint(where && where.family);
    if (!where) return;
    // A devcontainer and a setup belong to the repository, so they are offered
    // in one place: the project's own page. Every folder of the project -- its
    // own checkout and each worktree -- points there rather than carrying a
    // copy, which would be several places to change one fact
    const home = deskProjects(desk).projects.find(p => p.folders.includes(gi));
    if (home) box.insertBefore(elsewhereCard(desk, home), buttons);
    // Throwing a folder away is only for a branch: the project's own is never
    // on the table
    if (!where.cut) return;
    if (where.branch) box.insertBefore(renameCard(g, where.branch), buttons);
    buttons.append(el("button", {class:"danger", onclick: async () => {
      if (!guard()) return;
      if (!await confirmAction(fill(T["settings.group.discard.sure"], {name: folderLabel(g, gi)}), T["settings.group.discard"])) return;
      toast(T["tui.making.stage.removing"]);
      const r = await fetch("/api/folder/discard",
        {method:"POST", headers:{"X-Token":TOKEN}, body:JSON.stringify({path: g.cwd})})
        .then(r => r.json()).catch(() => ({ok:false, error:""}));
      // The folder would not go. Asked whether to take it off the list anyway,
      // since the files stay on disk either way
      if (!r.ok && r.left) {
        if (!await confirmAction(fill(T["worktree.left.say"], {why: r.error || ""}), T["worktree.left.forget"])) return;
        drop();
        return;
      }
      if (!r.ok) { toast(r.error || T["settings.group.discard.failed"], true); return; }
      drop();
      toast(T["settings.group.discard.done"]);
    }}, T["settings.group.discard"]));
    buttons.append(el("span", {class:"hint"}, T["settings.group.discard.hint"]));
  });
  return box;
}

// What this project says its environment needs, and the offer of one when it
// says nothing.
//
// On the project's own page, not on the one that lists machines and not in the
// dialog that makes folders. It writes into this repository, so it belongs
// where this repository's own settings are -- a machine is used by many
// projects, and somebody making a folder came to make a folder.
// Where a branch's page sends somebody looking for the project's settings.
//
// Not silence: a person who came here to set up the environment would search
// the page, find nothing, and conclude the app cannot do it. One line naming
// the folder that can is the difference between "not here" and "not possible"
function elsewhereCard(desk, p) {
  return card(T["settings.group.env"],
    el("div", {class:"hint"}, fill(T["settings.group.env.owned"], {name: p.name})),
    el("div", {class:"row"},
      el("button", {onclick:() => { sel = {desk:sel.desk, proj:p.key, grp:null, tab:null, global:false}; render(); }},
        fill(T["settings.project.open"], {name: p.name}))));
}

function envCard(desk, p) {
  const box = el("div");
  box.hidden = true;
  // Read from the project's own checkout; the folder the page stands for here
  // is that checkout, named the way the rest of this card expects
  const g = {cwd: p.at || ""};
  fetch("/api/devcontainer?path=" + encodeURIComponent(g.cwd) + "&desk=" + encodeURIComponent(desk.id || ""),
        {headers:{"X-Token":TOKEN}})
    .then(r => r.json())
    .then(said => {
      box.hidden = false;
      if (said && said.has) {
        const missing = said.has.unresolved || [];
        box.append(card(T["settings.group.env"],
          el("div", {class:"hint"}, fill(T["settings.group.env.has"], {path: said.has.from})),
          ...(said.has.setup || []).map(line => el("div", {class:"realcmd"}, el("code", {class:"mono"}, line))),
          missing.length
            ? el("div", {class:"hint"}, fill(T["settings.group.env.unresolved"], {names: missing.join(", ")}))
            : null));
        // Not offered where the file already answers: two places saying what to
        // run is two places to look when it runs the wrong thing
        return;
      }
      // Nothing to propose is said plainly rather than left blank: "there is
      // no devcontainer" and "one could be made for you" are different facts
      if (!said || !said.offer) {
        box.append(card(T["settings.group.env"],
          el("div", {class:"hint"}, T["settings.group.env.none"]),
          el("div", {class:"hint"}, T["settings.group.env.nothing"])));
        box.append(plainSetupCard(desk, p, (said && said.plain) || ""));
        return;
      }
      const keep = el("button", {}, T["settings.group.env.keep"]);
      keep.addEventListener("click", async () => {
        const r = await fetch("/api/devcontainer",
          {method:"POST", headers:{"X-Token":TOKEN}, body:JSON.stringify({path: g.cwd})})
          .then(r => r.json()).catch(() => ({ok:false, error:""}));
        if (!r.ok) { toast(r.error || "", true); return; }
        toast(fill(T["msg.devcontainer.kept"], {path: r.at}));
        // Drawn again from what is now on disk, so the card says what is
        // there rather than what was offered a moment ago
        box.textContent = "";
        box.hidden = true;
        box.append(envCard(desk, p));
      });
      box.append(card(T["settings.group.env"],
        el("div", {class:"hint"}, T["settings.group.env.none"]),
        el("div", {class:"hint"}, T["settings.group.env.offer"]),
        el("div", {class:"realcmd"}, el("code", {class:"mono"}, said.offer.json)),
        el("div", {class:"hint"}, fill(T["settings.group.env.why"], {why: (said.offer.why || []).join(", ")})),
        el("div", {class:"row"}, keep)));
      box.append(plainSetupCard(desk, p, said.plain || ""));
    })
    .catch(() => {});
  return box;
}

// The way out for a project that cannot have a devcontainer.
//
// Not an alternative anybody should reach for first -- a devcontainer is read
// by every other tool and this is read by nothing else -- which is why it sits
// under the offer rather than beside it. But the category is real and this
// repository is in it: a Windows executable with Win32 and WebView2 in it,
// which no Linux container builds. Writing a devcontainer for that would be
// putting a lie in the repository.
//
// Several lines, because a setup is usually several commands and a single
// field would have people joining them with && to fit.
function plainSetupCard(desk, p, current_setup) {
  const box = el("textarea", {rows:"3", class:"mono",
    placeholder:T["settings.project.setup.ph"], style:"width:100%"});
  // What is on the page wins over what the settings file said, once edited
  box.value = (p.entry && typeof p.entry.setup === "string") ? p.entry.setup : (current_setup || "");
  box.addEventListener("input", () => {
    const e = ensureProject(desk, p);
    if (box.value.trim()) e.setup = box.value; else delete e.setup;
    sel.proj = "p:" + e.name;
    refreshSave();
  });
  return card(T["settings.project.setup"],
    el("div", {class:"hint"}, T["settings.project.setup.hint"]),
    box);
}

// ── What a new worktree is given ─────────────────────────────────────────
// Git gives a new worktree everything it tracks and nothing it ignores. These
// two cards say what else it gets: for each line of the project's .gitignore,
// what happens to the files that line matches, and files brought from
// anywhere else. The choices are written into the project; the .gitignore
// itself is the repository's file, changed on the spot.

// The answer about each checkout's ignore file, kept while the page is open
const IGNORES = {};
const BRING_HOWS = ["copy", "replace", "link", "skip"];
const bringLabel = how => T["settings.bring." + how];

// Ask about one checkout, optionally changing something first. Draws the page
// again with what came back, which is what is really on disk now
async function askIgnore(root, change) {
  let j;
  try { j = await settingsApi("/api/project/ignore", Object.assign({path: root}, change || {})); }
  catch (e) { j = {ok:false, error: T["settings.bring.unreachable"]}; }
  if (j && j.lines) IGNORES[root] = j;
  return j || {ok:false};
}

// The project's rule for one ignore line, and writing one
function bringRule(p, source, pattern) {
  return ((p.entry || {}).bring || []).find(r => r.pattern === pattern && (r.source || ".gitignore") === source) || null;
}
function setBringRule(desk, p, source, pattern, change) {
  const e = ensureProject(desk, p);
  e.bring = e.bring || [];
  let r = e.bring.find(x => x.pattern === pattern && (x.source || ".gitignore") === source);
  if (!r) {
    r = {pattern, how: "skip"};
    if (source !== ".gitignore") r.source = source;
    e.bring.push(r);
  }
  change(r);
  if (r.how !== "replace") delete r.replace;
  sel.proj = "p:" + e.name;
  refreshSave();
  return r;
}

// The four ways, as a picker. A folder cannot be replaced inside, so that
// choice is offered only where there is a file
function howSelect(now, files, pick) {
  const s = el("select", {class:"howpick"});
  for (const how of BRING_HOWS) {
    if (how === "replace" && !files && now !== "replace") continue;
    s.append(el("option", {value: how}, bringLabel(how)));
  }
  s.value = now;
  s.addEventListener("change", () => pick(s.value));
  return s;
}

// What a copy has written differently: every "find" becomes "with", as plain
// text unless the regular-expression box is ticked
function replaceDialog(rule, done) {
  const list = (rule.replace || []).map(r => Object.assign({}, r));
  const rows = el("div");
  const draw = () => {
    rows.textContent = "";
    if (!list.length) rows.append(el("div", {class:"hint"}, T["settings.bring.replace.empty"]));
    list.forEach((r, i) => {
      const find = el("input", {type:"text", class:"mono grow", placeholder: T["settings.bring.replace.find"]});
      find.value = r.find || "";
      find.addEventListener("input", () => { r.find = find.value; check(); });
      const withIn = el("input", {type:"text", class:"mono grow", placeholder: T["settings.bring.replace.with"]});
      withIn.value = r.with || "";
      withIn.addEventListener("input", () => { r.with = withIn.value; });
      const box = el("input", {type:"checkbox"});
      box.checked = !!r.regex;
      box.addEventListener("change", () => { r.regex = box.checked; check(); });
      const bad = el("div", {class:"site-warn"}, el("span", {}, "⚠"), el("span", {}, T["settings.bring.replace.bad"]));
      bad.hidden = true;
      r._bad = bad;
      rows.append(el("div", {class:"listrow"},
        find, el("span", {class:"hint"}, "→"), withIn,
        el("label", {class:"check"}, box, document.createTextNode(T["settings.bring.replace.regex"])),
        el("button", {class:"quiet icon", title: T["common.delete"], onclick: () => { list.splice(i, 1); draw(); }}, "✕")),
        bad);
    });
    check();
  };
  // A regular expression the browser cannot read is one the app cannot either:
  // said on its row before saving, rather than found out when a folder is made
  const check = () => {
    for (const r of list) {
      let ok = true;
      if (r.regex && r.find) { try { new RegExp(r.find, "m"); } catch (e) { ok = false; } }
      if (r._bad) r._bad.hidden = ok;
    }
  };
  const shut = () => back.remove();
  const back = openModal(
    el("div", {class:"mhead"},
      el("h2", {}, T["settings.bring.replace.title"]),
      el("button", {class:"quiet icon", title: T["common.close"], onclick: shut}, "✕")),
    el("div", {class:"mbody"},
      el("div", {class:"hint"}, T["settings.bring.replace.hint"]),
      rows,
      el("div", {class:"row"},
        el("button", {onclick: () => { list.push({find:"", with:"", regex:false}); draw(); }}, T["settings.bring.replace.add"])),
      el("div", {class:"hint"}, T["settings.bring.replace.regex_hint"])),
    el("div", {class:"mfoot"},
      el("span", {class:"grow"}),
      el("button", {class:"quiet", onclick: shut}, T["common.cancel"]),
      el("button", {class:"primary", onclick: () => {
        const kept = list.filter(r => (r.find || "") !== "").map(r => {
          const o = {find: r.find, with: r.with || ""};
          if (r.regex) o.regex = true;
          return o;
        });
        done(kept); shut();
      }}, T["common.save"])));
  back.firstChild.classList.add("framed");
  back.addEventListener("keydown", e => { if (e.key === "Escape") { e.preventDefault(); shut(); } });
  draw();
}

// The project's .gitignore, line by line, each with how its files come along
function ignoreCard(desk, p) {
  const root = (p.at || "").trim();
  const box = el("div", {class:"igbox"});
  const known = IGNORES[root];
  if (!known) {
    box.append(el("div", {class:"hint"}, T["settings.bring.reading"]));
    askIgnore(root).then(() => { if (sel.proj === p.key || sel.proj === "p:" + p.name) render(); });
  }
  const j = known || {lines: [], ignored: [], defaults: [], tracked: []};
  const redraw = r => { if (r && r.ok === false && r.error) toast(r.error, true); render(); };
  const matchesOf = (source, pattern) => j.ignored.filter(i => i.source === source && i.pattern === pattern);
  const defaultOf = (source, pattern) => (j.defaults.find(d => d.source === source && d.pattern === pattern) || {}).how || "skip";
  const opened = (ignoreCard.open = ignoreCard.open || new Set());

  // How many lines are set to link: what that does is said once for all of
  // them, above the lists, rather than the same sentence under every one
  let linked = 0;

  // One line that decides something, with its picker, what it matches, and
  // (for the project's own file) a way to take it out
  const ruleRow = (source, pattern, n) => {
    const matched = matchesOf(source, pattern);
    const rule = bringRule(p, source, pattern);
    const how = rule && BRING_HOWS.includes(rule.how) ? rule.how : defaultOf(source, pattern);
    const files = matched.length === 0 || matched.some(m => !m.folder);
    const key = source + "\n" + pattern;
    const count = matched.length === 0
      ? el("span", {class:"hint"}, T["settings.bring.none"])
      : el("button", {class:"quiet", onclick: () => { opened.has(key) ? opened.delete(key) : opened.add(key); render(); }},
          matched.length === 1 ? "→ " + matched[0].path : "→ " + fill(T["settings.bring.n"], {n: matched.length}) + " ›");
    const replaceBtn = how === "replace"
      ? el("button", {class:"quiet", onclick: () => replaceDialog(rule || {}, kept => {
          setBringRule(desk, p, source, pattern, r => { r.how = "replace"; r.replace = kept; });
          render();
        })}, fill(T["settings.bring.replace.n"], {n: ((rule || {}).replace || []).length}))
      : null;
    // Every cell is there on every line, empty or not, so the columns hold
    const row = el("div", {class:"igrow"},
      el("div", {class:"igpat"},
        el("span", {class:"mono", title: pattern}, pattern),
        source === ".gitignore" ? null : el("span", {class:"hint"}, fill(T["settings.bring.from_file"], {file: source}))),
      el("div", {class:"igmatch"}, count),
      howSelect(how, files, v => { setBringRule(desk, p, source, pattern, r => { r.how = v; }); render(); }),
      !n ? el("span") : el("button", {class:"quiet icon", title: T["settings.bring.remove"], onclick: async () => {
        if (!await confirmAction(fill(T["settings.bring.remove_confirm"], {line: pattern}), T["settings.bring.remove"])) return;
        const r = await askIgnore(root, {remove: {n, text: pattern}});
        if (r.ok && p.entry && p.entry.bring) {
          p.entry.bring = p.entry.bring.filter(x => !(x.pattern === pattern && (x.source || ".gitignore") === ".gitignore"));
          refreshSave();
        }
        redraw(r);
      }}, "✕"));
    // What hangs off the line stays inside its item: the replacements of a
    // copy that is rewritten, and what a choice will do that is worth a word
    const under = [];
    const swaps = ((rule || {}).replace || []).length;
    if (how === "link") linked++;
    if (replaceBtn) under.push(el("div", {class:"hint ignote" + (swaps ? "" : " caution")},
      replaceBtn, swaps ? null : el("span", {}, T["settings.bring.replace.none"])));
    if (opened.has(key) && matched.length > 1) under.push(el("div", {class:"hint mono igpaths"}, matched.map(m => m.path).join("\n")));
    return el("div", {class:"igitem"}, row, ...under);
  };

  // A line that takes files back out of what is ignored: nothing to choose
  const negateRow = (line, n) => el("div", {class:"igitem"},
    el("div", {class:"igrow igneg"},
      el("div", {class:"igpat"}, el("span", {class:"mono", title: line}, line)),
      el("span", {class:"hint"}, T["settings.bring.negate"]),
      el("button", {class:"quiet icon", title: T["settings.bring.remove"], onclick: async () => {
        if (!await confirmAction(fill(T["settings.bring.remove_confirm"], {line}), T["settings.bring.remove"])) return;
        redraw(await askIgnore(root, {remove: {n, text: line}}));
      }}, "✕")));

  // The file as its author laid it out. A run of comments opens a group and
  // names the lines that follow, up to the next comment. A comment with
  // nothing under it before a blank line (the note at the top of a file)
  // stands alone. A line of nothing but marks is a ruler drawn in the file,
  // and the grouping already draws it
  const groups = [];
  let group = null;
  j.lines.forEach((text, i) => {
    const line = text.trim();
    if (!line) { if (group && !group.lines.length) group = null; return; }
    if (line.startsWith("#")) {
      if (/^#[#=*_~+\-\s]*$/.test(line)) return;
      if (!group || group.lines.length) groups.push(group = {notes: [], lines: []});
      group.notes.push(line);
      return;
    }
    if (!group) groups.push(group = {notes: [], lines: []});
    group.lines.push(line.startsWith("!") ? negateRow(line, i + 1) : ruleRow(".gitignore", line, i + 1));
  });
  // Written as a sentence ("# Build output") a comment is the name, without
  // its mark. Written as a line switched off ("#.idea/") it is quoted as it
  // is, under the name when there is one
  const sentence = c => /^#+\s/.test(c);
  const heading = notes => {
    const name = notes.findIndex(sentence);
    return el("div", {class:"ighead"},
      name < 0 ? null : el("div", {class:"igname"}, notes[name].replace(/^#+\s+/, "")),
      ...notes.filter((_, k) => k !== name).map(c => sentence(c)
        ? el("div", {class:"hint"}, c.replace(/^#+\s+/, ""))
        : el("div", {class:"hint mono"}, c)));
  };
  const lists = el("div", {class:"iggroups"}, ...groups.map(g => el("div", {class:"iggroup"},
    ...[g.notes.length ? heading(g.notes) : null,
        g.lines.length ? el("div", {class:"rows"}, ...g.lines) : null].filter(Boolean))));
  if (known && !j.lines.some(l => l.trim())) lists.append(el("div", {class:"hint"}, T["settings.bring.no_lines"]));

  // Adding a line
  const addIn = el("input", {type:"text", class:"mono grow", placeholder: T["settings.bring.add_ph"]});
  const add = async () => {
    const line = addIn.value.trim();
    if (!line) return;
    const r = await askIgnore(root, {add: line});
    if (r.ok) { addIn.value = ""; toast(fill(T["settings.bring.added"], {line})); }
    redraw(r);
  };
  addIn.addEventListener("keydown", e => { if (e.key === "Enter" && !e.isComposing) { e.preventDefault(); add(); } });

  // Files git still follows although a line now matches them
  const tracked = j.tracked || [];
  const trackedBox = tracked.length ? el("div", {class:"site-warn"},
    el("span", {}, "⚠"),
    el("span", {class:"grow"}, fill(T["settings.bring.tracked"], {n: tracked.length, names: tracked.slice(0, 5).join(", ")})),
    el("button", {onclick: async () => {
      if (!await confirmAction(fill(T["settings.bring.untrack_confirm"], {n: tracked.length}), T["settings.bring.untrack"])) return;
      redraw(await askIgnore(root, {untrack: tracked}));
    }}, T["settings.bring.untrack"])) : null;

  // Lines from somewhere other than the project's own file: chosen here,
  // changed where they are written
  const others = [];
  for (const d of j.defaults.filter(d => d.source !== ".gitignore")) others.push(ruleRow(d.source, d.pattern, 0));

  // Native append writes an absent part as the word "null", so the parts that
  // may be absent are left out first
  box.append(...[
    el("div", {class:"hint"}, fill(T["settings.bring.hint"], {root: j.root || root, branch: j.branch || "-"})),
    linked ? el("div", {class:"hint caution"}, T["settings.bring.link_warn_lines"]) : null,
    lists,
    el("div", {class:"row"}, addIn, el("button", {onclick: add}, T["settings.bring.add"])),
    trackedBox,
    others.length ? el("div", {class:"hint"}, T["settings.bring.others"]) : null,
    others.length ? el("div", {class:"rows"}, ...others) : null,
    el("div", {class:"hint"}, T["settings.bring.defaults"])].filter(Boolean));
  const c = card(T["settings.bring.title"], box);
  c.id = "project-bring";
  return c;
}

// Files brought from anywhere else, each put at a place inside the worktree.
// A file somebody wants made new is a template kept somewhere and copied
function extraFilesCard(desk, p) {
  const e = p.entry || {};
  const extras = (e.bring || []).filter(r => r.pattern === undefined || r.pattern === null);
  const rows = el("div");
  extras.forEach(r => {
    const change = fn => { const en = ensureProject(desk, p); fn(r); if (r.how !== "replace") delete r.replace; sel.proj = "p:" + en.name; refreshSave(); render(); };
    const fromIn = el("input", {type:"text", class:"mono grow", placeholder: T["settings.bring.extra.from_ph"]});
    fromIn.value = r.from || "";
    fromIn.addEventListener("change", () => change(x => { x.from = fromIn.value.trim(); }));
    const toIn = el("input", {type:"text", class:"mono", style:"width:180px", placeholder: T["settings.bring.extra.to_ph"]});
    toIn.value = r.to || "";
    toIn.addEventListener("change", () => change(x => { x.to = toIn.value.trim(); }));
    const how = BRING_HOWS.includes(r.how) ? r.how : "copy";
    rows.append(el("div", {class:"listrow"},
      fromIn, el("span", {class:"hint"}, "→"), toIn,
      howSelect(how, true, v => change(x => { x.how = v; })),
      how === "replace" ? el("button", {class:"quiet", onclick: () => replaceDialog(r, kept => change(x => { x.replace = kept; }))},
        fill(T["settings.bring.replace.n"], {n: (r.replace || []).length})) : null,
      el("button", {class:"quiet icon", title: T["common.delete"], onclick: () => {
        const en = ensureProject(desk, p);
        en.bring = (en.bring || []).filter(x => x !== r);
        sel.proj = "p:" + en.name; refreshSave(); render();
      }}, "✕")));
    if (how === "link") rows.append(el("div", {class:"hint warn"}, T["settings.bring.link_warn"]));
  });
  if (!extras.length) rows.append(el("div", {class:"hint"}, T["settings.bring.extra.empty"]));
  const c = card(T["settings.bring.extra.title"],
    el("div", {class:"hint"}, T["settings.bring.extra.hint"]),
    el("div", {class:"rows"}, rows),
    el("div", {class:"row"}, el("button", {onclick: () => {
      const en = ensureProject(desk, p);
      en.bring = en.bring || [];
      en.bring.push({from: "", to: "", how: "copy"});
      sel.proj = "p:" + en.name; refreshSave(); render();
    }}, T["settings.bring.extra.add"])),
    el("div", {class:"hint"}, T["settings.bring.extra.setup"]));
  c.id = "project-extra";
  return c;
}

// A project's own page: what it is called, where its own checkout is, the
// folders that are part of it, and the settings that belong to the repository
// rather than to any one folder of it. The one place those are offered, so a
// project's worktrees never each carry a copy
function projectPane(desk, p) {
  const box = el("div");
  box.append(pageCrumbs(deskCrumb(desk), {label: p.name}));
  const nameIn = el("input", {type:"text", value:p.name, style:"width:280px"});
  nameIn.addEventListener("change", () => {
    const to = nameIn.value.trim();
    if (!to || to === p.name) { nameIn.value = p.name; return; }
    if ((desk.projects || []).some(x => x.name === to)) {
      toast(T["settings.project.name_dup"], true); nameIn.value = p.name; return;
    }
    const e = ensureProject(desk, p);
    const was = e.name;
    e.name = to;
    // The folders that named it follow the new name
    for (const g of desk.folders || []) if ((g.project || "").trim() === was) g.project = to;
    sel.proj = "p:" + to;
    refreshSave(); render();
  });
  const atIn = el("input", {type:"text", class:"mono grow", value:p.at || "",
    placeholder:T["settings.project.at.ph"]});
  atIn.addEventListener("change", () => {
    const e = ensureProject(desk, p);
    if (atIn.value.trim()) e.at = atIn.value.trim(); else delete e.at;
    askFamilies([e.at]);
    sel.proj = "p:" + e.name;
    refreshSave(); render();
  });
  // What every branch of this project is called before its own name. Typed
  // once here instead of into every dialog, and kept by the name an AI writes
  // later as well
  const prefixIn = el("input", {type:"text", class:"mono", style:"width:200px",
    value:(p.entry || {}).branch_prefix || "", placeholder:T["settings.project.branch_prefix.ph"]});
  prefixIn.addEventListener("input", () => {
    const e = ensureProject(desk, p);
    if (prefixIn.value.trim()) e.branch_prefix = prefixIn.value.trim(); else delete e.branch_prefix;
    refreshSave();
  });
  box.append(card(T["settings.project.title"],
    row(T["settings.project.name"], nameIn),
    row(T["settings.project.at"], atIn,
      el("span", {class:"hint"}, T["settings.project.at.hint"])),
    row(T["settings.project.branch_prefix"], prefixIn,
      el("span", {class:"hint"}, T["settings.project.branch_prefix.hint"])),
    row(T["settings.project.repo"],
      el("span", {class:"hint mono"}, p.family || T["settings.project.repo.none"])),
    p.entry ? null : el("div", {class:"hint"}, T["settings.project.inferred"])));

  // Its folders, as rows that open them
  const rows = el("div", {class:"rows"});
  for (const gi of p.folders) {
    const g = desk.folders[gi];
    const fam = FAMILIES[(g.cwd || "").trim()] || {};
    rows.append(el("div", {class:"listrow secretrow", onclick:() => { sel = {desk:sel.desk, grp:gi, tab:null, global:false}; render(); }},
      el("span", {class:"secretname"}, folderLabel(g, gi)),
      el("span", {class:"hint mono secretdesc"}, folderWhere(g)),
      el("span", {class:"hint"}, fam.cut ? T["settings.project.worktree"] : T["settings.project.checkout"]),
      el("span", {class:"go"}, "›")));
  }
  box.append(card(T["settings.project.folders"],
    p.folders.length ? rows : el("div", {class:"hint"}, T["settings.project.folders.none"])));

  // The git account the column beside its folders signs in with, and reads
  // pull request numbers with. Chosen here once for every folder of it
  if (p.family) {
    const origin = [p.at].concat(p.folders.map(gi => (desk.folders[gi] || {}).cwd))
      .map(x => (FAMILIES[(x || "").trim()] || {}).origin).find(Boolean);
    const acctCard = card(T["settings.project.gitacct"],
      el("div", {class:"hint"}, T["settings.project.gitacct.hint"]),
      row(T["settings.gitacct.use"],
        gitAccountSelect(desk, (p.entry || {}).git_account, origin, v => {
          const e = ensureProject(desk, p);
          if (v) e.git_account = v; else delete e.git_account;
          sel.proj = "p:" + e.name;
          refreshSave(); render();
        })),
      (desk.git_accounts || []).length || PC_ACCOUNTS.length > 1 ? null : el("div", {class:"hint"}, T["settings.gitacct.tab_none"]));
    acctCard.id = "project-gitacct";
    box.append(acctCard);
  }

  // What a new worktree of it is given beyond what git carries, then the
  // environment and setup of the repository, read from its own checkout --
  // in the order they happen when a worktree is made
  if ((p.at || "").trim()) box.append(ignoreCard(desk, p), extraFilesCard(desk, p), envCard(desk, p));

  if (p.entry) {
    box.append(el("div", {class:"row"},
      el("button", {class:"danger", onclick: async () => {
        if (!await confirmAction(fill(T["settings.project.delete_confirm"], {name: p.name}), T["settings.project.delete"])) return;
        desk.projects = (desk.projects || []).filter(x => x !== p.entry);
        // The folders stay; they are only no longer tied to this name
        for (const g of desk.folders || []) if ((g.project || "").trim() === p.name) delete g.project;
        sel = {desk:sel.desk, grp:null, tab:null, global:false};
        toTree(sel.desk);
        refreshSave(); render();
      }}, T["settings.project.delete"]),
      el("span", {class:"hint"}, T["settings.project.delete.hint"])));
  }
  return box;
}

// Calling this folder's branch something else.
//
// Here rather than in the dialog that makes folders, because the name worth
// having is the one nobody could think of on the first day: work gets its name
// once it is under way. The line that will run is under the box and follows
// what is typed, so nothing happens that was not read first
function renameCard(g, branch) {
  const box = el("input", {type:"text", class:"grow", value: branch});
  // The same box a tab's real command line gets: one look for "this is what
  // will run", wherever in these settings it is being said
  const line = el("code", {class:"mono"});
  const note = el("div", {class:"hint"});
  const said = el("div", {class:"realcmd"}, line, note);
  const go = el("button", {}, T["settings.group.rename.do"]);
  let asked = "";
  const look = async () => {
    const want = box.value.trim();
    asked = want;
    // Nothing typed over the name it already has: nothing to show and nothing
    // to press, and an empty box would be a box with nothing in it
    if (!want || want === branch) { said.hidden = true; line.textContent = ""; note.textContent = ""; go.disabled = true; return; }
    const r = await fetch("/api/folder/rename",
      {method:"POST", headers:{"X-Token":TOKEN}, body:JSON.stringify({path: g.cwd, name: want})})
      .then(r => r.json()).catch(() => ({ok:false, error:""}));
    // An answer about a name that has since been typed over says nothing
    // about the one in the box now
    if (asked !== want) return;
    line.textContent = r.ok ? r.line : "";
    note.textContent = r.ok
      ? (r.sent ? fill(T["settings.group.rename.sent"], {name: r.sent}) : "")
      : (r.error || "");
    said.hidden = false;
    go.disabled = !r.ok;
  };
  box.addEventListener("input", look);
  said.hidden = true;
  go.disabled = true;
  go.addEventListener("click", async () => {
    const want = box.value.trim();
    const r = await fetch("/api/folder/rename",
      {method:"POST", headers:{"X-Token":TOKEN}, body:JSON.stringify({path: g.cwd, name: want, go: true})})
      .then(r => r.json()).catch(() => ({ok:false, error:""}));
    if (!r.ok) { toast(r.error || T["settings.group.rename.failed"], true); return; }
    toast(fill(T["msg.branch.renamed"], {from: r.from, to: r.to}));
    branch = r.to;
    box.value = r.to;
    go.disabled = true;
    line.textContent = "";
    note.textContent = r.sent ? fill(T["settings.group.rename.after"], {name: r.sent}) : "";
    said.hidden = !note.textContent;
    renderNav();
  });
  return card(T["settings.group.rename"],
    el("div", {class:"row"}, box, go),
    el("div", {class:"hint"}, T["settings.group.rename.hint"]),
    said);
}

// Which project a folder belongs to, as the app sees it. Answered by the app
// because it means looking at what git shares behind the folder
async function familyOf(cwd, desk) {
  if (!(cwd || "").trim()) return null;
  try {
    return await fetch("/api/family?path=" + encodeURIComponent(cwd)
                       + "&desk=" + encodeURIComponent((desk && desk.id) || ""),
                       {headers:{"X-Token":TOKEN}}).then(r => r.json());
  } catch (e) { return null; }
}

// AI vs AI discussion. Lines up participant tab ids and cycles them round-robin or moderated (moderator picks the next speaker).
// At the round cap, the judge renders a verdict (winner/synthesis). The goal (topic) is typed into an input field
function deskDiscussCard(desk) {
  const body = el("div", {id:"wsdiscussbody"});
  const ensure = () => {
    desk.discuss = desk.discuss || { agents:[], order:"round-robin", max_rounds:6, verdict:"winner" };
    desk.discuss.personas = desk.discuss.personas || {};
    return desk.discuss;
  };
  const on = el("input", {type:"checkbox"});
  on.checked = !!desk.discuss;
  body.style.display = desk.discuss ? "" : "none";
  on.addEventListener("change", () => {
    if (on.checked) { ensure(); body.style.display=""; } else { desk.discuss = null; body.style.display="none"; }
    render(); refreshSave();
  });
  const onLabel = el("label", {class:"check"});
  onLabel.append(on, document.createTextNode(T["settings.discuss.enable"]));

  const d = desk.discuss || {};
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
    const dd = desk.discuss; if (!dd) return;
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
      personaBox.append(el("div", {style:"margin:var(--s2) 0"},
        el("div", {class:"mono", style:"font-size:12px;color:var(--text);margin-bottom:var(--s1)"}, id), ta));
    });
  };

  const agentsIn = txt((d.agents||[]).join(", "), T["settings.discuss.agents_ph"],
    v => { ensure().agents = v.split(",").map(s=>s.trim()).filter(Boolean); drawChips(); drawPersonas(); });
  // Participant-candidate chips: one click appends an existing tab id to the end of the turn order.
  // A tab aimed at another tab (🎯) already has a turn to keep, so it can't also be a discussion participant
  const chipBox = el("div", {class:"hint", style:"display:flex;gap:var(--s2);flex-wrap:wrap;align-items:center;margin-top:var(--s1)"});
  const drawChips = () => {
    chipBox.textContent = "";
    const cur = (desk.discuss && desk.discuss.agents) || [];
    // Only candidate tabs that are a discussable AI (CLI/model API), not already a participant, and not aimed at anything
    const cand = (desk.tabs || [])
      .filter(t => isDiscussable(t) && !(t.drives||"").trim())
      .map(t => (t.id || "").trim())
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
  const judgeIn = idSelect(desk, d.judge, T["wizard.discuss.judge_none"],
    v => { ensure().judge = v; drawPersonas(); }, notDiscuss);
  const modIn = idSelect(desk, d.moderator, T["wizard.discuss.judge_none"],
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
    el("div", {style:"margin-top:var(--s2)"},
      el("div", {style:"font-size:12px;color:var(--text)"}, T["settings.discuss.persona_section_label"]),
      el("div", {class:"hint"}, T["settings.discuss.persona_section_hint"]),
      personaBox));
  drawChips();
  drawPersonas();

  return card(T["settings.discuss.title"],
    el("div", {class:"hint"},
      T["settings.discuss.card_hint"]),
    el("div", {class:"row", style:"margin-top:var(--s2)"}, onLabel),
    body);
}

// Stop conditions (judge). Per-desk. Evaluated top to bottom; the first one satisfied wins.
// Defines "when does this collaborative task end (success/failure)"
function deskStopsCard(desk) {
  desk.stops = desk.stops || [];
  const list = el("div", {id:"wsstopslist"});
  const redraw = () => {
    list.textContent = "";
    if (!desk.stops.length) list.append(el("div", {class:"hint"}, T["settings.stops.empty"]));
    desk.stops.forEach((s, i) => list.append(stopRow(desk, s, i, redraw)));
  };
  const add = el("button", {onclick:() => {
    desk.stops.push({ when:"screen", outcome:"success", code:0 }); redraw(); refreshSave();
  }}, T["settings.stops.add"]);
  const c = card(T["settings.stops.title"],
    el("div", {class:"hint"},
      T["settings.stops.hint"]),
    list,
    el("div", {class:"row", style:"margin-top:var(--s3)"}, add));
  redraw();
  return c;
}

function stopRow(desk, s, i, redraw) {
  const set = (k, v) => { s[k] = v; refreshSave(); };
  const choice = (val, opts, on) => {
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
  const when = choice(s.when || "screen", [
    ["screen",T["settings.stops.when.screen"]],["css",T["settings.stops.when.css"]],["xpath",T["settings.stops.when.xpath"]],
    ["console",T["settings.stops.when.console"]],["rounds",T["settings.stops.when.rounds"]],["time",T["settings.stops.when.time"]],["tokens",T["settings.stops.when.tokens"]],
  ], v => { s.when = v; redraw(); refreshSave(); });

  // Switch which inputs are shown depending on the type
  const dyn = [];
  if (s.when === "screen" || s.when === "css" || s.when === "xpath" || s.when === "console") {
    if (s.when !== "console")
      dyn.push(idSelect(desk, s.tab, T["settings.stops.target_tab"], v => set("tab", v)));
    if (s.when === "css" || s.when === "xpath")
      dyn.push(inp(s.sel, s.when === "xpath" ? "//button[...]" : "#id", "text", v => set("sel", v)));
    else
      dyn.push(inp(s.pattern, T["settings.stops.pattern_ph"], "text", v => set("pattern", v)));
  } else if (s.when === "rounds" || s.when === "tokens") {
    dyn.push(inp(s.max, T["settings.stops.threshold_ph"], "number", v => set("max", v)));
  } else if (s.when === "time") {
    dyn.push(inp(s.sec, T["settings.stops.seconds_ph"], "number", v => set("sec", v)));
  }

  const outcome = choice(s.outcome || "success", [["success",T["settings.stops.outcome.success"]],["fail",T["settings.stops.outcome.fail"]]], v => set("outcome", v));
  const code = inp(s.code || 0, "code", "number", v => set("code", v));
  const reason = inp(s.reason, T["settings.stops.reason_ph"], "text", v => set("reason", v || null));
  const rm = el("button", {class:"quiet", title:T["common.delete"], onclick:() => { desk.stops.splice(i, 1); redraw(); refreshSave(); }}, "×");

  const row = el("div", {class:"stoprow"}, when, ...dyn,
    el("span", {class:"arrow"}, "→"), outcome, code, reason, rm);
  return row;
}

// The secrets a desk has.
//
// A boxed list you read down, and one dialog to change one of them. The row
// says what a person needs in order to pick one out -- its name, who may use
// it, what it is for, how many addresses it may be typed into -- and the whole
// row is the way in, so there is no button to find.
function deskSecretsCard(desk) {
  const listBox = el("div", {id:"wssecretslist"}, el("div", {class:"hint"}, "…"));
  const add = el("button", {onclick: () => {
    if (!(desk.id || "").trim()) { toast(T["settings.secrets.desk_needs_id"], true); return; }
    secretDialog(desk, null);
  }}, T["settings.secrets.add"]);
  const c = card(T["settings.secrets.desk_title"],
    el("div", {class:"hint"}, T["settings.secrets.desk_hint"]),
    listBox,
    el("div", {class:"row"}, add));
  setTimeout(() => loadWsSecrets(desk), 0);
  return c;
}

// Who may use one. Two independent answers, in the same words the automation
// permission table uses, so the same question reads the same way in both
// places. Neither ticked is a real state -- a secret nothing may use yet
const secretHuman = s => s.human !== false;
const secretWhoText = s => {
  const who = [secretHuman(s) ? T["grant.who.human"] : null, s.ai ? T["grant.who.ai"] : null]
    .filter(Boolean);
  return who.length ? who.join(" / ") : T["settings.secrets.who.none"];
};
const secretWhoChip = s => el("span",
  {class: (secretHuman(s) || s.ai) ? "chip" : "chip none"}, secretWhoText(s));
// How many addresses, and whether one of them is unencrypted -- the thing on
// this row worth catching from across the room
function secretWhere(s) {
  const urls = s.urls || [];
  if (!urls.length) return {text: T["settings.secrets.urls_none"], warn: false};
  const plain = urls.filter(isPlain).length;
  return {
    text: fill(T["settings.secrets.urls_count"], {n: urls.length})
      + (plain ? T["settings.secrets.urls_has_http"] : ""),
    warn: plain > 0,
  };
}

async function loadWsSecrets(desk) {
  const box = document.getElementById("wssecretslist");
  if (!box) return;
  const j = await fetchSecrets();
  if (!j) { box.textContent=""; box.append(el("div",{class:"hint warn"},T["settings.secrets.load_failed"])); return; }
  box.textContent = "";
  if (j.mode === "locked") {
    box.append(el("div",{class:"hint warn"},T["settings.secrets.desk_locked"]));
    return;
  }
  // Kept in the open, because nobody has set a master password yet. Said here
  // because here is where a password is about to be written down
  if (j.mode === "plaintext") {
    box.append(el("div", {class:"hint warn"}, T["settings.secrets.mode.plaintext"]));
  }
  const mine = (j.secrets || [])
    .map(s => ({...s, short: secretShortName(desk, s.key)}))
    .filter(s => s.short !== null);
  if (!mine.length) {
    box.append(el("div",{class:"hint"},T["settings.secrets.desk_none"]));
    return;
  }
  const rows = el("div", {class:"rows"});
  for (const s of mine) {
    const where = secretWhere(s);
    rows.append(el("div", {class:"listrow secretrow", onclick: () => secretDialog(desk, s)},
      el("span", {class:"mono secretname"}, s.short),
      secretWhoChip(s),
      el("span", {class:"hint secretdesc"}, s.description || T["settings.secrets.no_desc"]),
      el("span", {class: where.warn ? "hint secretsite plain" : "hint secretsite"},
         (where.warn ? "⚠ " : "") + where.text),
      el("span", {class:"go"}, "›")));
  }
  box.append(rows);
}

// Adding one, or changing one. `have` is null for a new secret.
//
// One field per line, in the order a person answers them: what it is called,
// what it is, who may use it, and where it may go. The value is write-only --
// there is nothing to show, because nothing here can read it back -- so an
// existing secret asks for one only if you want to replace it.
// `called` fills the name in for a secret the app itself looks for by name --
// today that is the GitHub token, which is only useful under the one name the
// program asks for. Typing it correctly is not a thing to leave to a person
function secretDialog(desk, have, called) {
  const editing = !!have;
  const name = el("input", {type:"text", class:"mono", placeholder:T["settings.secrets.key_ph"]});
  name.value = editing ? have.short : (called || "");
  name.disabled = editing;
  const value = el("input", {type:"password",
    placeholder: editing ? T["settings.secrets.value_set_ph"] : T["settings.secrets.value_ph"]});
  const desc = el("input", {type:"text", placeholder:T["settings.secrets.desc_ph"]});
  desc.value = editing ? (have.description || "") : "";

  // Who may use it: the same two boxes, in the same order, as the automation
  // permission table. They are not two halves of one choice -- a key that only
  // an AI's errands ever touch is a thing somebody may want, and so is a
  // secret parked with neither ticked while it is being set up
  const whoBox = (on) => { const i = el("input", {type:"checkbox"}); i.checked = on; return i; };
  const humanIn = whoBox(editing ? secretHuman(have) : true);
  const aiIn = whoBox(editing ? !!have.ai : false);
  const whoNote = el("div", {class:"hint warn"}, T["settings.secrets.who_none_warn"]);
  const whoLabel = (input, text) => {
    const l = el("label", {class:"check"});
    l.append(input, document.createTextNode(text));
    input.addEventListener("change", () => {
      whoNote.hidden = humanIn.checked || aiIn.checked;
    });
    return l;
  };
  const who = el("div", {},
    el("div", {class:"whorow"},
      whoLabel(humanIn, T["grant.who.human"]),
      whoLabel(aiIn, T["grant.who.ai"])),
    whoNote);
  whoNote.hidden = humanIn.checked || aiIn.checked;

  // Where it may be typed. Every address is written out in full, and the one
  // standing decision -- whether an unencrypted address is allowed here at all
  // -- is made before anything is typed rather than sprung afterwards
  const riskBox = el("input", {type:"checkbox"});
  riskBox.checked = editing && (have.urls || []).some(isPlain);
  const riskLabel = el("label", {class:"check allow"});
  riskLabel.append(riskBox, document.createTextNode(T["settings.secrets.plain_ok"]));
  riskBox.addEventListener("change", recheck);
  const urlBox = el("div", {style:"display:flex;flex-direction:column;gap:var(--s2)"});
  const save = el("button", {class:"primary"}, T["common.save"]);
  const why = el("span", {class:"why"});
  why.hidden = true;
  let held = null;

  const urlsNow = () =>
    [...urlBox.querySelectorAll("input")].map(i => withScheme(i.value)).filter(Boolean);
  // What is wrong with the field, said on the row that is wrong. The save is
  // held rather than dead: it still takes the press, and answers it
  function recheck() {
    let first = null;
    for (const i of urlBox.querySelectorAll("input")) {
      // An empty line is not a mistake: a secret with no address is one
      // nothing on the web may use, which is the state every secret starts in
      const wrote = withScheme(i.value);
      const fault = wrote ? urlFault(wrote) : null;
      const reason = fault ? T[fault]
        : (isPlain(wrote) && !riskBox.checked ? T["settings.secrets.plain_warn"] : null);
      const wrap = i.parentElement.parentElement;
      const had = wrap.querySelector(".site-warn");
      i.classList.toggle("bad", !!reason);
      if (had) had.remove();
      if (reason) wrap.append(el("div", {class:"site-warn"},
        el("span", {}, "⚠"), el("span", {}, reason)));
      if (reason && !first) first = {at: i, why: fault ? T[fault] : T["settings.secrets.plain_held"]};
    }
    held = first;
    save.classList.toggle("held", !!held);
    if (!held) why.hidden = true;
    else if (!why.hidden) why.textContent = fill(T["settings.secrets.cannot_save"], {why: held.why});
  }
  // Pressing a held button is a question. Answer it where the answer stays,
  // and take the eye to the thing that has to change
  function sayWhy() {
    why.textContent = fill(T["settings.secrets.cannot_save"], {why: held.why});
    why.hidden = false;
    for (const n of [held.at, riskLabel]) {
      n.classList.remove("lookhere");
      void n.offsetWidth;
      n.classList.add("lookhere");
    }
    held.at.scrollIntoView({block:"center", behavior:"smooth"});
  }
  const addUrl = (v) => {
    const i = el("input", {type:"text", class:"mono grow", placeholder:"https://example.com/api",
      value: v || ""});
    i.addEventListener("input", recheck);
    // Completed where it can be seen, when the box is left: a bare host means
    // the safe one, and filling it in beats a rule nobody was told
    i.addEventListener("blur", () => { i.value = withScheme(i.value); recheck(); });
    const x = el("button", {class:"quiet icon", title:T["common.delete"], onclick: () => {
      wrap.remove(); recheck();
    }}, "✕");
    const wrap = el("div", {}, el("div", {class:"site-row"}, i, x));
    urlBox.append(wrap);
    return i;
  };
  for (const h of (editing ? (have.urls || []) : [])) addUrl(h);
  if (!urlBox.children.length) addUrl("");

  const field = (label, control, hint) => el("div", {class:"field"},
    el("label", {}, label), control,
    hint ? el("div", {class:"hint"}, hint) : null);

  const shut = () => back.remove();
  const back = openModal(
    el("div", {class:"mhead"},
      el("h2", {}, editing ? T["settings.secrets.edit_title"] : T["settings.secrets.add_title"]),
      el("button", {class:"quiet icon", title:T["common.close"], onclick: () => shut()}, "✕")),
    el("div", {class:"mbody"},
      field(T["settings.secrets.key_label"], name,
            editing ? T["settings.secrets.name_fixed"] : T["settings.secrets.key_hint"]),
      field(T["settings.secrets.value_label"], value,
            editing ? T["settings.secrets.value_keep"] : T["settings.secrets.value_hint"]),
      field(T["settings.secrets.who_label"], who, T["settings.secrets.who_hint"]),
      field(T["settings.secrets.desc_label"], desc, T["settings.secrets.desc_hint"]),
      el("div", {class:"field"},
        el("label", {}, T["settings.secrets.urls_label"],
           el("span", {class:"lblopt"}, T["settings.secrets.urls_many"])),
        riskLabel,
        urlBox,
        el("div", {class:"row"},
          el("button", {class:"quiet", onclick: () => addUrl("").focus()},
             T["settings.secrets.urls_add"])),
        el("div", {class:"hint"}, T["settings.secrets.urls_hint"]))),
    el("div", {class:"mfoot"},
      editing
        ? el("button", {class:"danger", onclick: async () => {
            if (!await confirmAction(fill(T["settings.secrets.delete_confirm"], {key: have.short}), T["common.delete"])) return;
            const r = await deleteSecret(have.key);
            if (r.ok) { toast(fill(T["settings.secrets.deleted"], {key: have.short})); shut(); loadWsSecrets(desk); }
            else toast(r.error || T["settings.secrets.delete_failed"], true);
          }}, T["common.delete"])
        : null,
      why,
      el("span", {class:"grow"}),
      el("button", {class:"quiet", onclick: () => shut()}, T["common.cancel"]),
      save));
  back.firstChild.classList.add("framed");

  // Enter finishes it and Esc leaves it, from anywhere inside. A dialog that
  // is all short fields is one people type through without reaching for the
  // mouse
  back.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); shut(); return; }
    if (e.key !== "Enter" || e.target.tagName !== "INPUT" || e.target.type === "checkbox") return;
    e.preventDefault();
    save.click();
  });

  save.addEventListener("click", async () => {
    if (held) { sayWhy(); return; }
    const short = name.value.trim();
    if (!short) { toast(T["settings.secrets.key_required"], true); return; }
    if (!editing && !value.value) { toast(T["settings.secrets.value_required"], true); return; }
    const r = await saveSecret({key: editing ? have.key : secretKey(desk, short),
      value: value.value, description: desc.value,
      human: humanIn.checked, ai: aiIn.checked, urls: urlsNow()});
    if (r.ok) { toast(fill(T["settings.secrets.saved_key"], {key: short})); shut(); loadWsSecrets(desk); }
    else toast(r.error || T["settings.secrets.save_failed"], true);
  });
  recheck();
  setTimeout(() => (editing ? value : name).focus(), 0);
}
// Both export and import operate against the config that's on disk.
// Exporting the in-progress editing state would create a config that only the recipient has
function savedAlready() {
  if (snapshot() === savedSnapshot) return true;
  result(T["settings.desk.save_first"], true);
  return false;
}

const deskShare = (path, body) => fetch(path, {method:"POST",
  headers:{"Content-Type":"application/json", "X-Token":TOKEN}, body})
  .then(r => r.json()).catch(e => ({ok:false, error:e.message || e}));

async function exportWs(i) {
  if (!savedAlready()) return;
  const j = await deskShare("/api/desk/export", JSON.stringify({index:i}));
  if (j.cancelled) return;
  if (!j.ok) return result(fill(T["settings.desk.export_failed"], {error:j.error || ""}), true);
  result(fill(T["settings.desk.exported"], {path:j.path}));
}

async function importWs() {
  if (!savedAlready()) return;
  const j = await deskShare("/api/desk/import", null);
  if (j.cancelled) return;
  if (!j.ok) return result(fill(T["settings.desk.import_failed"], {error:j.error || ""}), true);
  // The config has already been rewritten on the server side; reload it here on screen
  await load();
  sel = {desk:desks.length - 1, tab:null, global:false};
  render();
  const moved = (j.moved || []).map(m => m[0] + " → " + m[1]).join(" / ");
  result(fill(T["settings.desk.imported"], {name:j.name, files:j.files})
    + (moved ? "  " + fill(T["settings.desk.imported.moved"], {moved}) : ""));
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
  const desk = desks[sel.desk];
  const at = (desk.tabs || [])[sel.tab];
  const group = at ? (at.group || 0) : 0;
  desk.tabs = (desk.tabs || []).concat(TEMPLATES[kind].map(x => newTab(Object.assign({group}, x))));
  sel.tab = desk.tabs.length - TEMPLATES[kind].length;
  render();
  msg(T["settings.template.added"]);
}

// What a tab runs: the kind, the panel for that kind, the command, and the line
// that will really be launched. Its own card because it is asked in two places
// -- on the tab's page, and alone over the board when + adds a tab.
// `renamed` is told when the command changed the tab's name along with it
function launchCard(t, renamed) {
  const cmdRow = el("div", {class:"row"});
  const real = launchLine(t);
  let before = cmdToText(t.command);
  const cmdInput = field(t, "command", T["settings.tab.command.ph"],
    {mono:true, onInput:() => {
      followKind(t, before);
      before = cmdToText(t.command);
      if (renamed) renamed();
      renderNav(); real.schedule();
    }});
  cmdInput.setAttribute("list", "cmdlist");
  const detailBox = el("div");
  const rebuild = () => { detailBox.textContent = ""; detailBox.append(kindPanel(t, cmdInput, rebuild, real)); };
  cmdRow.append(el("label", {}, T["settings.tab.kind"]),
    choose({k:catOf(t.command)}, "k", CAT_LIST, v => {
      setCommand(t, cmdInput, catStart(v)); rebuild();
    }));
  rebuild();
  // A tab that connects is not a tab that starts something, and the heading
  // has to say which one this is
  const conn = catOf(t.command) === "remote" || catOf(t.command) === "sftp";
  return card(conn ? T["settings.server.basics"] : T["settings.tab.launch"],
    cmdRow, detailBox, row(T["settings.tab.command"], cmdInput), real.box);
}

function tabPane(desk, t) {
  const box = el("div");
  const gi = t.group || 0;
  const g = (desk.folders || [])[gi];
  const home = deskProjects(desk).projects.find(x => x.folders.includes(gi));
  box.append(pageCrumbs(deskCrumb(desk), home ? projectCrumb(home) : null, g ? folderCrumb(g, gi) : null,
    {label: t.name || T["settings.tab.unnamed"]}));

  // Basics: name and ID are identity, so place them side by side.
  // If ID is empty, auto-derive one from the name (English → slug / Japanese-only → 5-char hash).
  // The guessed value is shown as a placeholder and finalized once the name field is left
  // A tab that reaches here with no id gets one now -- from its name if it has
  // one, else a unique string -- so the field is never blank and the tab can
  // always be pointed at. The person can change it; it is a normal field.
  if (!(t.id || "").trim()) {
    t.id = uniqueId(desk, inferredTabId(t), t);
    refreshSave(); renderNav();
  }
  const idInput = field(t, "id", "", {grow:false, width:280, mono:true});
  const refreshIdPh = () => {
    idInput.placeholder = uniqueId(desk, inferredTabId(t), t);
  };
  const nameInput = field(t, "name", T["settings.tab.name.ph"], {grow:false, width:280,
    onInput:() => { renderNav(); refreshIdPh(); }});
  // An empty name is shown as the one it will be given
  const refreshNamePh = () => { nameInput.placeholder = kindName(t.command) || T["settings.tab.name.ph"]; };
  nameInput.addEventListener("blur", () => {
    if (!(t.id || "").trim()) {
      const sug = uniqueId(desk, inferredTabId(t), t);
      t.id = sug; idInput.value = sug; refreshSave(); renderNav();
    }
  });
  refreshIdPh();
  refreshNamePh();
  box.append(card(T["settings.tab.basic"],
    row(T["settings.tab.name"], nameInput),
    row(T["settings.tab.id"], idInput,
        el("span", {class:"hint"}, T["settings.tab.id.hint"]))));

  box.append(launchCard(t, () => {
    nameInput.value = t.name || "";
    refreshNamePh();
    refreshIdPh();
  }));

  // Notify on answer: a beginner-friendly way to get a ping when this tab's AI
  // finishes, without writing on_done Lua. Lists the destinations registered
  // on this tab's desk, and ends with the way to add one, so a person who
  // arrives here first does not have to know where that is.
  //
  // Not for a panel: it has no process, so nothing ever starts, answers or
  // exits, and every one of these questions would be about something that
  // cannot happen
  const runs = catOf(t.command) !== "sftp" && catOf(t.command) !== "git";
  if (runs) {
    const nbox = el("div");
    const drawNotify = () => {
      nbox.textContent = "";
      // The desk this pane was drawn for, handed in
      const deskNotify = () => desk.notify || {};
      const dests = Object.keys(deskNotify());
      const opts = [["", T["settings.tab.notify.none"]]]
        .concat(dests.map(n => [n, n]), [["add-dest", T["settings.tab.notify.add"]]]);
      // The reply link and its warning live directly under the destination, so
      // the sentence can name the place the link is going. The risk is not the
      // link, it is who can see it -- and only the person choosing knows that.
      const warn = el("div", {class:"hint warn", style:"margin-top:var(--s2)"});
      // A phone destination with no phone behind it sends nothing. Said here,
      // where the destination is being chosen, with the way to put it right --
      // the failure itself would otherwise only ever show on the board, after.
      const unreached = el("div", {class:"hint warn", style:"margin-top:var(--s2)"});
      const replyBox = el("div");
      const drawReply = () => {
        replyBox.textContent = "";
        warn.textContent = "";
        unreached.textContent = "";
        // Always drawn, even with no destination chosen -- greyed rather than
        // gone. A control that only appears once something else is set is a
        // control nobody finds: you cannot look for what is not there.
        const dest = t.notify_on_done;
        // A link to a page you answer from is for somewhere you are not. A
        // notification on this very PC is already one click from the tab
        // itself, so the link would be a longer way round to the same place --
        // and it never even appears, because a banner shows two lines and the
        // link is on the third. Say so rather than let it be ticked for nothing.
        const kind = (deskNotify()[dest] || {}).type;
        const here = kind === "windows";
        const on = !!dest && !here;
        if (!on) delete t.notify_reply;
        const label = check(t, "notify_reply", T["settings.tab.notify.reply"]);
        const box = label.querySelector("input");
        box.disabled = !on;
        label.style.opacity = on ? "" : ".5";
        if (!on) {
          label.title = here ? T["settings.tab.notify.reply.here"]
                             : T["settings.tab.notify.reply.needs_dest"];
        }
        replyBox.append(label);
        if (here) {
          warn.classList.remove("warn");
          warn.textContent = T["settings.tab.notify.reply.here"];
        } else {
          warn.classList.add("warn");
          if (on && t.notify_reply) {
            warn.textContent = fill(T["settings.tab.notify.reply.warn"], {name: dest});
          }
        }
        if (kind === "phone") {
          phones().then(j => {
            if (t.notify_on_done !== dest || j.error || (j.subs || []).length) return;
            unreached.append(fill(T["settings.tab.notify.phone_none"], {name: dest}), " ",
              el("a", {href:"#", onclick: async e => {
                e.preventDefault(); await openNotifyPopup(); drawNotify();
              }}, T["settings.tab.notify.phone_how"]));
          });
        }
      };
      const before = t.notify_on_done;
      const picker = choose(t, "notify_on_done", opts, async v => {
        if (v === "add-dest") {
          // Not a destination: put back what was chosen, open the editor, and
          // point the tab at whatever it added.
          if (before) t.notify_on_done = before; else delete t.notify_on_done;
          const had = new Set(Object.keys(deskNotify()));
          await openNotifyPopup();
          const added = Object.keys(deskNotify()).find(n => !had.has(n));
          if (added) t.notify_on_done = added;
          drawNotify(); refreshSave();
          return;
        }
        if (!v) { delete t.notify_on_done; delete t.notify_reply; }
        drawReply(); refreshSave();
      });
      replyBox.addEventListener("change", () => { drawReply(); refreshSave(); });
      drawReply();
      const hint = dests.length ? T["settings.tab.notify.hint"] : T["settings.tab.notify.none_hint"];
      nbox.append(card(T["settings.tab.notify.title"],
        row(T["settings.tab.notify.label"], picker, el("span", {class:"hint"}, hint)),
        unreached, replyBox, warn));
    };
    drawNotify();
    box.append(nbox);
  }

  // Automation. One chooser rather than a row per trigger: a tab has eight of
  // them and seven are normally empty, so a list of rows is mostly a list of
  // things that are not happening. What a person wants at a glance is the
  // opposite -- which ones ARE set -- and that is one line under the chooser
  const ev = el("div", {class:"events"});
  {
    const pick = el("select");
    for (const [id, label] of eventsFor(t)) pick.append(el("option", {value:id}, label));
    const hint = el("div", {class:"hint", id:"ev-hint"});
    const drawHint = () => {
      const chosen = eventsFor(t).find(e => e[0] === pick.value);
      hint.textContent = chosen ? chosen[2] : "";
    };
    pick.addEventListener("change", drawHint);
    drawHint();
    ev.append(el("div", {class:"event"},
      el("div", {class:"name"}, pick, hint),
      el("button", {class:"quiet", onclick:() => openAuto(desk, t, pick.value)}, T["common.edit"])));
    ev.append(el("div", {class:"hint", id:"ev-set"}, T["automation.none_set"]));
  }
  if (runs) {
    box.append(card(T["settings.tab.automation"], ev));
    loadAutoStates(desk, t);
  }

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
  if ((desk.folders || []).length > 1)
    det.append(row(T["settings.tab.folder"],
      choose({at: String(t.group || 0)}, "at",
             desk.folders.map((g, i) => [String(i), folderLabel(g, i)]),
             v => { sel.tab = setTabGroup(desk, sel.tab, Number(v)); render(); refreshSave(); }),
      el("span", {class:"hint"}, T["settings.tab.folder.hint"])));
  det.append(
    el("div", {class:"row"}, el("label", {}, T["settings.tab.order"]),
       el("button", {class:"quiet", onclick:() => moveTab(desk, -1)}, T["settings.tab.move_up"]),
       el("button", {class:"quiet", onclick:() => moveTab(desk, 1)}, T["settings.tab.move_down"]),
       el("button", {class:"quiet", onclick:() => { t.depth = Math.min((t.depth||0)+1, sel.tab); render(); }}, T["settings.tab.indent"]),
       el("button", {class:"quiet", onclick:() => { t.depth = Math.max((t.depth||0)-1, 0); render(); }}, T["settings.tab.outdent"])));
  box.append(el("div", {class:"card"}, det));

  box.append(el("div", {class:"row"},
    el("button", {class:"danger", onclick: async () => {
      if (!await confirmAction(fill(T["settings.tab.delete_confirm"], {name: t.name || T["settings.tab.unnamed"]}), T["settings.tab.delete"])) return;
      // The password this tab signs in with is this tab's, and nothing else
      // can name it once the tab is gone
      const w = (desk.id || "").trim(), tid = (t.id || "").trim();
      if (w && tid) await dropSecrets(["ssh/" + w + "/" + tid + "/password",
                                       "ssh/" + w + "/" + tid + "/passphrase"]);
      desk.tabs.splice(sel.tab, 1); sel.tab = null; render();
    }}, T["settings.tab.delete"])));
  return box;
}

// Moves a tab to another folder, children and all: they are drawn under it and
// work in the same place, so leaving them behind would split one family across
// two folders. The list stays ordered by folder, which is what the headings in
// the nav and the tab bar are drawn from. Returns where the tab ended up
function setTabGroup(desk, i, group) {
  let end = i + 1;
  while (end < desk.tabs.length && desk.tabs[end].depth > desk.tabs[i].depth) end++;
  const moved = desk.tabs.splice(i, end - i);
  moved.forEach(t => t.group = group);
  let at = desk.tabs.length;
  for (let k = 0; k < desk.tabs.length; k++) if ((desk.tabs[k].group || 0) > group) { at = k; break; }
  desk.tabs.splice(at, 0, ...moved);
  return at;
}

function moveTab(desk, d) {
  const i = sel.tab, j = i + d;
  if (j < 0 || j >= desk.tabs.length) return;
  // Swapping across a folder boundary would move a tab to another folder
  // without saying so. The folder is picked in the tab's own settings
  if ((desk.tabs[i].group || 0) !== (desk.tabs[j].group || 0)) return;
  [desk.tabs[i], desk.tabs[j]] = [desk.tabs[j], desk.tabs[i]];
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
    style:"max-height:60vh;overflow:auto;white-space:pre-wrap;font-size:12px;line-height:1.5;margin:0;padding:var(--s3);background:var(--panel);border-radius:var(--r-ctl)"},
    T["settings.tab.ai.flags_loading"]);
  let back;
  const close = el("button", {class:"quiet", onclick:() => back && back.remove()}, T["common.close"]);
  back = openModal(el("h2", {}, head + " --help"), pre,
    el("div", {class:"row", style:"justify-content:flex-end;margin-top:var(--s3)"}, close));
  try {
    const r = await fetch("/api/cli-help?cmd=" + encodeURIComponent(head),
      {headers:{"X-Token":TOKEN}}).then(r => r.json());
    pre.textContent = (r && r.ok && (r.help || "").trim())
      ? r.help : ((r && r.error) || T["settings.tab.ai.flags_failed"]);
  } catch (e) { pre.textContent = T["settings.tab.ai.flags_failed"]; }
}

function aiPanel(t, cmdInput, rebuild, real) {
  const box = el("div");
  const picker = el("select");
  for (const c of AI_CLIS) {
    const ok = c.check ? aiEngines.some(e => e.id === c.check) : true;
    picker.append(el("option", {value:"cli:" + c.cmd}, c.label + (!ok ? T["settings.tab.common.missing"] : "")));
  }
  const provs = Object.keys(deskProviders());
  for (const n of provs) picker.append(el("option", {value:"prov:" + n}, n));
  picker.append(el("option", {value:"add-ai"}, T["settings.tab.ai.add"]));

  // Reflect the current command in the dropdown.
  const cur = parseModel(t.command);
  if (cur && cur.provider) picker.value = "prov:" + cur.provider;
  else { const h = headOf(t.command); picker.value = AI_CLIS.some(c => c.cmd === h) ? "cli:" + h : ""; }

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
        el("label", {class:"row", style:"cursor:pointer;gap:var(--s2)"}, cb,
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
    const cand = modelCandidates(() => deskProviders()[m.provider] || {},
      id => { modelIn.value = id; setModel(id); });
    detail.append(
      el("div", {class:"row"}, el("label", {}, T["settings.model.name_label"]), modelIn, cand.btn),
      el("div", {class:"row"}, el("label", {}, ""), cand.chips));
    // Auto-load real models and preselect the first if none is set yet.
    cand.load().then(models => {
      if (models && models.length && !(m.model || "").trim()) { modelIn.value = models[0]; setModel(models[0]); }
    });
  };

  picker.addEventListener("change", async () => {
    const v = picker.value;
    if (v === "add-ai") {
      const before = new Set(Object.keys(deskProviders()));
      await openProvidersPopup();
      const added = Object.keys(deskProviders()).find(n => !before.has(n));
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
    const label = el("label", {class:"row", style:"cursor:pointer;gap:var(--s2)"}, cb,
      el("span", {}, T["settings.tab.restore_conv"]));
    const hint = el("span", {class:"hint"});
    carry.append(label, el("div", {class:"row"}, el("label", {}, ""), hint));
    // A command that names its own conversation is obeyed as written, and a
    // CLI that cannot be told which conversation to open starts a new one
    // either way. In both, this switch changes nothing -- so it says so
    // instead of standing there looking like it decides.
    real.onCarry(why => {
      cb.disabled = !!why;
      label.style.opacity = why ? ".5" : "";
      label.style.cursor = why ? "default" : "pointer";
      hint.textContent = why ? T["settings.tab.restore_conv." + why]
                             : T["settings.tab.restore_conv.hint"];
    });
  }

  box.append(el("div", {class:"row"}, el("label", {}, T["settings.tab.ai.pick"]), picker, helpBtn));
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
      providersCard(desks[sel.desk]),
      el("div", {class:"row", style:"border-top:1px solid var(--line);margin-top:var(--s3);padding-top:var(--s3);justify-content:flex-end"},
        el("button", {class:"primary", onclick: () => { m.remove(); resolve(); }}, T["common.done"])));
    m.addEventListener("click", e => { if (e.target === m) resolve(); });
  });
}

// Everything about one connection this program makes itself: where it is, who
// signs in, with what, and -- folded away -- the awkward cases.
//
// One builder for both kinds of tab that have a connection. A terminal on
// another machine and a panel of that machine's files are the same connection
// asked for two different things, and a second copy of these fields would be
// two screens that slowly stop agreeing about what a server is.
//
// `conn` is the parsed address and `build` puts it back into a command line,
// which is what differs: `ssh://` opens a terminal, `sftp://` opens the lists
function connectionFields(box, t, conn, build, cmdInput) {
  // What this server is called, once which server it is has been worked out.
  // Asked again whenever the address or the way there changes
  const mark = markFields({ask: () => ({command: build(conn), server: t.server || null})}, true);
  const upd = () => { setCommand(t, cmdInput, build(conn)); mark.schedule(); };
  // This program does the connecting, so what it needs is an address, a user,
  // one way of proving who that is, and -- for the file panel -- where on the
  // far end to start looking. Everything below that is for the connections
  // that are not straightforward, and stays folded until somebody has one
  const sv = t.server || (t.server = {});
  // The unsaved mark is worked out by comparing what would be written, on a
  // timer, so nothing here has to remember to announce itself
  const save = () => { refreshSave(); mark.schedule(); };
  // A star on the three it will not connect without. Said once, in the label,
  // rather than as a sentence under every field
  const must = label => el("span", {}, label,
    el("i", {class:"must"}, T["settings.server.required"]));
  box.append(el("div", {class:"row2"},
    sfield(must(T["settings.server.host"]), (() => {
      const i = el("input", {type:"text", class:"mono", placeholder:"example.com"});
      i.value = conn.host || "";
      suggest(i, "ssh");
      i.addEventListener("input", () => { conn.host = i.value.trim(); upd(); });
      return i;
    })()),
    sfield(must(T["settings.phone.port"]), (() => {
      const i = el("input", {type:"text", class:"mono narrow", placeholder:"22"});
      i.value = conn.port || "";
      i.addEventListener("input", () => { conn.port = i.value.trim(); upd(); });
      return i;
    })())));
  box.append(sfield(must(T["settings.server.user"]), (() => {
    const i = el("input", {type:"text", class:"mono", placeholder:"root"});
    i.value = conn.user || "";
    i.addEventListener("input", () => { conn.user = i.value.trim(); upd(); });
    return i;
  })()));
  // Right under where it is: the name is what tells this server from the one
  // like it, and it is decided when the connection is, not found later
  box.append(mark.box);

  // Which credential this connection uses. Not a fallback chain: a key or a
  // password, so that "why did it ask me for a password" has one answer.
  // What is chosen is read from what is filled in -- the key's path is the
  // whole of it -- rather than kept as a second field to disagree with it
  const auth = el("div", {class:"segrow"});
  const keyPart = el("div");
  const pwPart = el("div");
  const drawAuth = () => {
    const byKey = !!(sv.key || "").trim();
    auth.textContent = "";
    for (const [id, label] of [["key", T["settings.server.auth.key"]],
                               ["password", T["settings.server.auth.password"]]]) {
      const on = (id === "key") === byKey;
      const r = el("input", {type:"radio", name:"srvauth"});
      r.checked = on;
      r.addEventListener("change", () => {
        if (id === "password") { sv.key = ""; }
        else if (!(sv.key || "").trim()) { sv.key = "~/.ssh/id_ed25519"; }
        save(); drawAuth();
      });
      auth.append(el("label", {class:"check"}, r, el("span", {}, label)));
    }
    keyPart.hidden = !byKey;
    pwPart.hidden = byKey;
    if (byKey && keyIn) keyIn.value = sv.key || "";
  };
  let keyIn = null;
  box.append(sfield(must(T["settings.server.auth"]), auth));

  keyIn = el("input", {type:"text", class:"mono", placeholder:"~/.ssh/id_ed25519"});
  keyIn.value = sv.key || "";
  keyIn.addEventListener("input", () => { sv.key = keyIn.value.trim(); save(); });
  const keyRow = el("div", {class:"row2"}, keyIn);
  if (!REMOTE) keyRow.append(el("button", {class:"quiet", onclick: async () => {
    const path = await pickPath("key", T["settings.ssh.key.pick"], sv.key);
    if (path !== null) { sv.key = path; keyIn.value = path; save(); }
  }}, T["common.browse"]));
  keyPart.append(sfield(T["settings.server.key.formats"], keyRow));
  keyPart.append(secretField(t, "passphrase", T["settings.server.passphrase.optional"],
    T["settings.server.passphrase.hint"], () => build(conn)));
  pwPart.append(secretField(t, "password", T["settings.ssh.password"],
    T["settings.ssh.password.hint"], () => build(conn)));
  box.append(keyPart, pwPart);
  drawAuth();

  box.append(sfield(T["settings.server.start_dir"], (() => {
    const i = el("input", {type:"text", class:"mono", placeholder:"/var/www/html"});
    i.value = sv.remote_dir || "";
    i.addEventListener("input", () => { sv.remote_dir = i.value.trim(); save(); });
    return i;
  })(), T["settings.server.remote_dir.hint"]));

  const adv = el("details");
  adv.append(el("summary", {}, T["settings.server.advanced"]));
  // Three separate awkward cases, each with its own heading, because they have
  // nothing to do with each other: a server you cannot reach directly, a server
  // whose files belong to somebody else, and a network that cuts a quiet line
  const group = label => el("div", {class:"subhead"}, label);
  const note = text => el("div", {class:"note"}, text);
  adv.append(group(T["settings.server.group.jump"]));
  const jumpPart = el("div", {class:"under-check"});
  const jumpOn = el("input", {type:"checkbox"});
  jumpOn.checked = !!(sv.jump && (sv.jump.host || "").trim());
  jumpOn.addEventListener("change", () => {
    sv.jump = jumpOn.checked ? (sv.jump || {host:"", port:"", user:"", key:""}) : null;
    jumpPart.hidden = !jumpOn.checked;
    save();
  });
  adv.append(el("label", {class:"check"}, jumpOn,
    el("span", {}, T["settings.server.jump.on"])));
  adv.append(note(T["settings.server.jump.note"]));
  const jf = (key, label, ph, narrow) => sfield(label, (() => {
    const i = el("input", {type:"text", class:"mono" + (narrow ? " narrow" : ""), placeholder:ph});
    i.value = (sv.jump && sv.jump[key]) || "";
    i.addEventListener("input", () => {
      sv.jump = sv.jump || {};
      sv.jump[key] = i.value.trim();
      save();
    });
    return i;
  })());
  jumpPart.append(el("div", {class:"row2"},
    jf("host", T["settings.server.jump.host"], "gw.example.com"),
    jf("port", T["settings.phone.port"], "22", true)));
  jumpPart.append(jf("user", T["settings.server.jump.user"], "root"));
  jumpPart.append(jf("key", T["settings.server.jump.key"], "~/.ssh/id_ed25519"));
  jumpPart.hidden = !jumpOn.checked;
  adv.append(jumpPart);

  adv.append(group(T["settings.server.group.shell"]));
  adv.append(sfield(T["settings.server.file_command.label"], (() => {
    const i = el("input", {type:"text", class:"mono", placeholder:"sudo su -c /usr/lib/openssh/sftp-server"});
    i.value = sv.file_command || "";
    i.addEventListener("input", () => { sv.file_command = i.value.trim(); save(); });
    return i;
  })(), T["settings.server.file_command.hint"]));

  adv.append(group(T["settings.server.group.alive"]));
  const alive = el("input", {type:"checkbox"});
  alive.checked = !!sv.keepalive;
  alive.addEventListener("change", () => { sv.keepalive = alive.checked ? 30 : 0; save(); });
  adv.append(el("label", {class:"check"}, alive,
    el("span", {}, T["settings.server.keepalive.on"])));
  adv.append(note(T["settings.server.alive.note"]));
  box.append(adv);

  // Proved before it is needed, so a wrong field is found here rather than
  // at launch. What comes back is the server's own words either way
  const said = el("div", {class:"hint"});
  box.append(el("div", {class:"connfoot"},
    el("button", {onclick: async ev => {
      // Held on to now: an event's target is gone by the time the answer
      // arrives, and reaching for it then is how a button stays grey forever
      const btn = ev.currentTarget;
      const desk = desks[sel.desk];
      said.textContent = T["settings.server.test.doing"];
      said.style.color = "";
      btn.classList.add("held");
      const r = await settingsApi("/api/server/test", {
        host: conn.host || "", port: Number(conn.port || 22),
        user: conn.user || "", key: sv.key || "",
        jump: sv.jump || null, keepalive: sv.keepalive || 0,
        file_command: sv.file_command || "",
        desk: (desk && (desk.id || "").trim()) || "", tab: (t.id || "").trim(),
      });
      btn.classList.remove("held");
      said.textContent = r && r.ok
        ? (T["settings.server.test.ok"] || "")
            .replace("{host}", conn.host || "").replace("{user}", conn.user || "")
        : ((r && r.error) || "");
      said.style.color = r && r.ok ? "var(--live)" : "var(--warn)";
    }}, T["settings.server.test"]), said));
  box.append(el("div", {class:"hint"}, T["settings.ssh.builtin.hint"]));
}

// The colours a mark can be, as squares, and a last square that opens any
// colour at all. Drawn into `box`, which is emptied first; `put` is told the
// colour pressed. The chosen one wears the ring
function swatchesInto(box, now, put) {
  box.textContent = "";
  const on = (now || "").toLowerCase();
  for (const c of MARK_COLOURS) {
    const sw = el("i", {class:(on === c.toLowerCase() ? "on" : ""), title:c, onclick:() => put(c)});
    sw.style.background = c;
    box.append(sw);
  }
  const any = el("input", {type:"color", value:now || "#888888"});
  any.addEventListener("input", () => put(any.value));
  const own = !!on && !MARK_COLOURS.some(c => c.toLowerCase() === on);
  box.append(el("i", {class:"any" + (own ? " on" : ""), title:T["settings.server.mark.color.any"] || "",
    onclick:() => any.click()}), any);
}

// The server's own name for the person ("Production", "Staging"), its colour,
// and whether something that cannot be undone there waits for the name to be
// typed.
//
// It belongs to the server, not to the tab it is typed on: every tab that
// reaches the same machine wears it (config::ServerMark). So which server this
// is has to be known first, and it is asked of the app -- the tab row looks a
// name up by the spelling a launch builds, and a second spelling worked out
// here would file names the row never finds. `from.ask` says what to ask
// about (`{command, server}`, or null while there is nothing to ask);
// `from.machine` is a server already known, spelled the app's way.
//
// `live` writes every change into the settings at once, for a page whose Save
// is the page's. Otherwise nothing is written until `commit`, for a dialog
// that can still be cancelled.
function markFields(from, live) {
  const box = el("div", {class:"markpart"});
  // `unnamed`: the server this was opened on had no name, so any name in the
  // draft was typed here and not read from the settings
  let machine = null, draft = null, touched = false, unnamed = false, seq = 0, timer = null;
  const marks = () => current.server_marks = isObj(current.server_marks) ? current.server_marks : {};
  // Filed under the app's spelling; a hand-edited file may have used another case
  const keyOf = m => Object.keys(marks()).find(k => k.toLowerCase() === (m || "").toLowerCase());
  const read = m => {
    const k = keyOf(m);
    const had = k ? marks()[k] : null;
    return {name: (had && had.name) || "", color: (had && had.color) || "", careful: !!(had && had.careful)};
  };
  const write = () => {
    if (!machine || !draft) return;
    const all = marks();
    const k = keyOf(machine);
    if (k) delete all[k];
    if (draft.name.trim()) {
      const m = {name: draft.name.trim()};
      if (draft.color) m.color = draft.color;
      if (draft.careful) m.careful = true;
      all[machine] = m;
    }
    if (!Object.keys(all).length) delete current.server_marks;
  };
  const changed = () => {
    touched = true;
    if (live) { write(); refreshSave(); }
  };

  const nameIn = el("input", {type:"text", placeholder:T["settings.server.mark.name.ph"]});
  const colours = el("div", {class:"swatches"});
  const careful = el("input", {type:"checkbox"});
  const more = el("div");
  const drawColours = () => swatchesInto(colours, draft.color, c => { draft.color = c; changed(); drawColours(); });
  nameIn.addEventListener("input", () => {
    draft.name = nameIn.value;
    // A name with no colour yet is given one no other server is wearing, so
    // two servers named one after the other do not come out the same colour
    if (draft.name.trim() && !draft.color) {
      const worn = new Set(Object.values(marks()).map(m => (m.color || "").toLowerCase()));
      draft.color = MARK_COLOURS.find(c => !worn.has(c.toLowerCase())) || MARK_COLOURS[0];
    }
    changed();
    drawMore();
  });
  careful.addEventListener("change", () => { draft.careful = careful.checked; changed(); });
  // The colour and the care are about the name, so they wait for one: a
  // colour with no word beside it is not a mark anybody can read
  const drawMore = () => {
    more.hidden = !draft.name.trim();
    if (!more.hidden) drawColours();
  };
  more.append(
    sfield(T["settings.server.mark.color"], colours, T["settings.server.mark.color.hint"]),
    el("div", {class:"field"},
      el("label", {class:"check"}, careful, el("span", {}, T["settings.server.mark.careful"])),
      el("div", {class:"hint"}, T["settings.server.mark.careful.hint"])));

  const draw = () => {
    box.textContent = "";
    if (!machine) {
      box.append(sfield(T["settings.server.mark.name"],
        el("div", {class:"hint"}, T["settings.server.mark.no_address"])));
      return;
    }
    nameIn.value = draft.name;
    careful.checked = draft.careful;
    box.append(
      el("div", {class:"field"},
        el("label", {}, T["settings.server.mark.name"]),
        el("div", {class:"fieldctl"}, nameIn),
        el("div", {class:"hint"}, T["settings.server.mark.name.hint"] + " ",
          el("span", {class:"mono"}, machine))),
      more);
    drawMore();
  };

  // The last server this stood on and what was in the boxes for it. Kept
  // through the moments the address is not one at all -- a host cleared to be
  // typed again -- so what was typed survives the gap
  let last = null;
  const settle = now => {
    if (now === machine && draft) return;
    if (machine && draft) last = {machine, draft, typedHere: touched && unnamed};
    machine = now;
    draft = now ? read(now) : null;
    unnamed = !!draft && !draft.name.trim();
    // The address changed under a name typed a moment ago -- the host was
    // still being finished, say. The name was meant for the server this
    // address is becoming, so it goes with it, unless that server has a name
    // of its own already. A name read from the settings is not carried: it
    // belongs to the server it was read for, and is left there
    if (now && last && last.machine !== now && last.typedHere
        && last.draft.name.trim() && !draft.name.trim()) {
      if (live) {
        const k = keyOf(last.machine);
        if (k) delete marks()[k];
      }
      draft = last.draft;
      last = null;
      if (live) { write(); refreshSave(); }
    }
    draw();
  };
  const refresh = async () => {
    if (from.machine) return settle(from.machine);
    const mine = ++seq;
    const req = from.ask ? from.ask() : null;
    const r = req ? await settingsApi("/api/server/machine", req).catch(() => null) : null;
    if (mine !== seq) return;
    settle((r && r.machine) || null);
  };
  draw();
  refresh();
  return {
    box,
    // Called whenever what the connection reaches may have changed
    schedule: () => { clearTimeout(timer); timer = setTimeout(refresh, 250); },
    // For a dialog: what was typed goes into the settings now
    commit: () => { if (touched) write(); },
    // Why this cannot be kept as it stands, if it cannot, and where to look
    held: () => machine && draft && !draft.name.trim()
      ? {why: T["settings.server.mark.name_required"], at: nameIn} : null,
  };
}

// One server's name, opened from the list of named servers. Its address is
// what it is filed under and is not changed here: a different address is a
// different server, named from the tab that reaches it
function markDialog(machine, redraw) {
  const fields = markFields({machine}, false);
  const shut = () => back.remove();
  const save = el("button", {class:"primary"}, T["common.save"]);
  const why = el("span", {class:"why"});
  why.hidden = true;
  const back = openModal(
    el("div", {class:"mhead"},
      el("h2", {}, T["settings.server.mark.title"]),
      el("button", {class:"quiet icon", title:T["common.close"], onclick: () => shut()}, "\u2715")),
    el("div", {class:"mbody"}, fields.box),
    el("div", {class:"mfoot"},
      el("button", {class:"danger", onclick: () => {
        const all = current.server_marks || {};
        for (const k of Object.keys(all)) if (k.toLowerCase() === machine.toLowerCase()) delete all[k];
        if (!Object.keys(all).length) delete current.server_marks;
        refreshSave(); shut(); redraw();
      }}, T["settings.server.mark.drop"]),
      why,
      el("span", {class:"grow"}),
      el("button", {class:"quiet", onclick: () => shut()}, T["common.cancel"]),
      save));
  back.firstChild.classList.add("framed");
  // Grey while there is no name, and the reason follows what is in the box
  const recheck = () => {
    const h = fields.held();
    save.classList.toggle("held", !!h);
    if (!h) why.hidden = true;
  };
  back.addEventListener("input", recheck);
  save.addEventListener("click", () => {
    const h = fields.held();
    if (h) {
      why.textContent = fill(T["settings.secrets.cannot_save"], {why: h.why});
      why.hidden = false;
      h.at.classList.remove("lookhere");
      void h.at.offsetWidth;
      h.at.classList.add("lookhere");
      h.at.focus();
      return;
    }
    fields.commit();
    refreshSave(); shut(); redraw();
  });
  back.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); shut(); return; }
    if (e.key !== "Enter" || e.target.tagName !== "INPUT" || e.target.type !== "text") return;
    e.preventDefault();
    save.click();
  });
  recheck();
  setTimeout(() => { const i = fields.box.querySelector("input[type=text]"); if (i) i.focus(); }, 0);
}

// Every server somebody has named, to find one again and change it or take
// its name away -- including a server no tab reaches any more, whose name
// would otherwise sit in the settings with nowhere to be seen
function marksCard() {
  const listBox = el("div");
  const draw = () => {
    listBox.textContent = "";
    const all = isObj(current.server_marks) ? current.server_marks : {};
    const named = Object.entries(all).filter(([, m]) => m && (m.name || "").trim())
      .sort((a, b) => a[1].name.localeCompare(b[1].name));
    if (!named.length) {
      listBox.append(el("div", {class:"hint"}, T["settings.server.marks.none"]));
      return;
    }
    const rows = el("div", {class:"rows"});
    for (const [machine, m] of named) {
      rows.append(el("div", {class:"listrow secretrow", onclick: () => markDialog(machine, draw)},
        el("span", {class:"secretname markname"}, projectMark(m.color || null), el("span", {}, m.name)),
        el("span", {class:"hint mono secretdesc"}, machine),
        m.careful ? el("span", {class:"chip"}, T["settings.server.marks.careful"]) : null,
        el("span", {class:"go"}, "\u203a")));
    }
    listBox.append(rows);
  };
  setTimeout(draw, 0);
  const c = card(T["settings.server.marks"],
    el("div", {class:"hint"}, T["settings.server.marks.hint"]),
    listBox);
  return c;
}

// The controls a browser tab can show over its page, in the order they are
// offered, each with its label. Saving writes the same list, so one offered
// here cannot be dropped by the next save
const NAV_PARTS = ["back", "forward", "reload", "reload_hard", "url", "point"];
const NAV_LABEL = {back:"tui.nav.back", forward:"tui.nav.forward", reload:"tui.nav.reload",
  reload_hard:"tui.nav.reload_hard.short", url:"tui.nav.url", point:"tui.nav.point"};

function kindPanel(t, cmdInput, rebuild, real) {
  if (catOf(t.command) === "ai") return aiPanel(t, cmdInput, rebuild, real);
  const box = el("div");
  const ssh = parseSsh(t.command), dk = parseDocker(t.command), wsl = parseWsl(t.command);
  const remote = parseRemote(t.command);
  const sftpPanel = parseSftpUrl(t.command);
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
  if (sftpPanel) {
    // The same connection, asked for its files instead of a terminal. Every
    // field is here, on this tab, because that is where somebody adding a file
    // panel looks for them
    box.append(el("div", {class:"hint"}, T["settings.sftp.hint"]));
    connectionFields(box, t, sftpPanel, buildSftpUrl, cmdInput);
  } else if (remote) {
    connectionFields(box, t, remote, buildRemote, cmdInput);
  } else if (ssh) {
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
    // Plain words: this page's own models, else the desk's. A model added
    // from here is also the desk's when the desk has none, so the next
    // browser tab starts with it
    const here = desks[sel.desk];
    here.browser = here.browser || {};
    const wordsBox = el("div", {id:"tab-words"},
      el("div", {class:"row"}, el("label", {}, T["settings.words.title"])),
      ...wordsRows(here, t, here.browser, (k, v) => {
        if (!(here.browser[k] || "").trim()) here.browser[k] = v;
      }));
    box.append(wordsBox);
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.browser.nav"]),
      ...NAV_PARTS.map(k => part(k, T[NAV_LABEL[k]]))));
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
    // get right. What it needs is the folder, and that is the row below --
    // and the account it signs in with, which is this tab's own choice
    const desk = desks[sel.desk] || {};
    const home = ((desk.folders || [])[t.group || 0] || {}).cwd || "";
    askFamilies([home]);
    box.append(el("div", {class:"hint"}, T["settings.tab.kind.git.hint"]));
    box.append(row(T["settings.gitacct.label"],
      gitAccountSelect(desk, t.git_account, (FAMILIES[home.trim()] || {}).origin, v => {
        if (v) t.git_account = v; else delete t.git_account;
        refreshSave();
      })));
    box.append(el("div", {class:"hint"}, (desk.git_accounts || []).length || PC_ACCOUNTS.length > 1
      ? T["settings.gitacct.tab_hint"] : T["settings.gitacct.tab_none"]));
    return box;
  } else if (isEditorPanel(t.command)) {
    return el("div", {class:"hint"}, T["settings.tab.kind.editor.hint"]);
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
  ["on_start",      T["automation.on_start"],      T["automation.on_start.hint"]],
  ["on_busy",       T["automation.on_busy"],       T["automation.on_busy.hint"]],
  ["on_question",   T["automation.on_question"],   T["automation.on_question.hint"]],
  ["on_done",       T["automation.on_done"],       T["automation.on_done.hint"]],
  ["on_failed",     T["automation.on_failed"],     T["automation.on_failed.hint"]],
  ["on_limit",      T["automation.on_limit"],      T["automation.on_limit.hint"]],
  ["on_background", T["automation.on_background"], T["automation.on_background.hint"]],
  ["on_exit",       T["automation.on_exit"],       T["automation.on_exit.hint"]],
  ["_shared",       T["automation._shared"],       ""],
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
// A script kept as one file, rather than a file per trigger. The same thing to
// the engine; to this screen it is one text holding every trigger
const isOneFile = data => typeof (data && data.file) === "string";
const definedIn = source =>
  [...(source || "").matchAll(/^\s*function\s+([A-Za-z_]\w*)\s*\(/gm)].map(m => m[1]);

function autoDirOf(desk, t) {
  if (t.automation) return t.automation;
  const slug = s => (s || "").replace(/[^A-Za-z0-9_-]/g, "").toLowerCase();
  const wi = desks.indexOf(desk) + 1, ti = (desk.tabs || []).indexOf(t) + 1;
  return "scripts/" + (slug(desk.name) || ("desk" + wi)) + "/" + (slug(t.id) || slug(t.name) || ("tab" + ti));
}

async function fetchAuto(dir) {
  try {
    return await (await fetch("/api/automation?dir=" + encodeURIComponent(dir),
        {headers:{"X-Token":TOKEN}})).json();
  } catch (e) { return {}; }
}
async function loadAutoStates(desk, t) {
  const data = await fetchAuto(autoDirOf(desk, t));
  const inFile = isOneFile(data) ? definedIn(data.file) : null;
  const set = eventsFor(t)
      .filter(([id]) => inFile ? inFile.includes(id) : (data[id] || "").trim().length > 0)
      .map(([, label]) => label);
  const line = document.getElementById("ev-set");
  if (!line) return;
  line.textContent = set.length
      ? fill(T["automation.set_list"], {names: set.join(" / ")})
      : T["automation.none_set"];
  line.className = "hint" + (set.length ? " on" : "");
}

async function openAuto(desk, t, event) {
  autoTarget = { desk, t, dir: autoDirOf(desk, t) };
  document.getElementById("autotitle").textContent =
      fill(T["automation.editor.title"], {name: t.name || T["settings.tab.unnamed"]});
  document.getElementById("autopath").textContent = autoTarget.dir;
  const s = document.getElementById("autoevent");
  s.textContent = "";
  const events = eventsFor(t);
  for (const [id, label] of events) s.append(el("option", {value:id}, label));
  autoData = await fetchAuto(autoTarget.dir);
  autoTarget.file = isOneFile(autoData);
  // One file is edited whole: there is no trigger to pick, and what the AI
  // writes is the body of one trigger, which has nowhere to go in it
  for (const e of [s, s.previousElementSibling]) e.style.display = autoTarget.file ? "none" : "";
  if (autoTarget.file) {
    document.getElementById("autocode").value = autoData.file;
    document.getElementById("autohint").textContent = T["automation.editor.file"];
  } else {
    // Use showEvent here (switchEvent would capture the textarea's still-there previous-tab
    // content as the new tab's data)
    // Defaults to whichever event is listed first for that tab (on_load for a browser)
    showEvent(event || events[0][0]);
  }
  document.getElementById("airow").style.display = aiEngines.length && !autoTarget.file ? "flex" : "none";
  document.getElementById("ainone").style.display = aiEngines.length || autoTarget.file ? "none" : "flex";
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
  if (autoTarget.file) {
    const r = await fetch("/api/automation?dir=" + encodeURIComponent(autoTarget.dir),
        {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
         body: JSON.stringify({file: document.getElementById("autocode").value})});
    if (!r.ok) return automsg(T["automation.editor.save_failed"], true);
    closeAuto();
    loadAutoStates(autoTarget.desk, autoTarget.t);
    msg(T["automation.editor.saved"]);
    return;
  }
  autoData[autoEvent] = document.getElementById("autocode").value;
  const r = await fetch("/api/automation?dir=" + encodeURIComponent(autoTarget.dir),
      {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
       body: JSON.stringify(autoData)});
  if (!r.ok) return automsg(T["automation.editor.save_failed"], true);
  const created = autoTarget.t.automation !== autoTarget.dir;
  autoTarget.t.automation = autoTarget.dir;
  closeAuto();
  loadAutoStates(autoTarget.desk, autoTarget.t);
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
  const desk = autoTarget.desk;
  automsg("");
  aiBusy(true);
  try {
    const r = await fetch("/api/generate", {method:"POST",
        headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
        body: JSON.stringify({event: autoEvent, prompt: want,
          engine: current.ai_engine || null,
          tabs: (desk.tabs || []).map((x, i) => ({index:i+1, name:x.name || fill(T["settings.tab.default_name"], {n: i+1}), id:x.id || ""})),
          self: (desk.tabs || []).indexOf(autoTarget.t) + 1})});
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
// A desk's working folders, and never none of them: the one everything
// lands in is a folder like any other. Mirrors foldered() in config.rs, which
// says the same thing on the way to launching them
function foldersOf(w) {
  // Everything the folder says is kept, not only what this screen draws: the
  // app writes keys of its own onto a folder (the machine it is on, the project
  // it belongs to, where it came from), and a key dropped here is a key the
  // next save erases
  const folders = (w.folders || []).map(f => Object.assign({}, f, {name:f.name || "", id:f.id || "",
                                               cwd:f.cwd || "", tabs:f.tabs || []}));
  if (!folders.length) folders.push({name:"", id:"", cwd:"", tabs:[]});
  // Tabs written the old way, beside the folders instead of inside one. The
  // program reads them again (config.rs does the same folding), and this screen
  // has to as well: showing "no tabs" for a desk that is running two is
  // worse than not showing them at all, because the next save would be made
  // from what is on the screen. They join the folder they would have been put
  // in, and saving writes them there.
  if (Array.isArray(w.tabs) && w.tabs.length) {
    folders[0].tabs = folders[0].tabs.concat(w.tabs);
  }
  return folders;
}

// The screen keeps one flat list of tabs, each remembering which folder it is in
function readFolders(desk, w) {
  const fs = foldersOf(w);
  desk.folders = fs.map(f => { const g = Object.assign({}, f); delete g.tabs; return g; });
  desk.tabs = [];
  fs.forEach((f, i) => flatten(f.tabs, 0, i, desk.tabs));
}

function flatten(tabs, depth, group, out) {
  for (const t of tabs || []) {
    out.push({ name: t.name || "", id: t.id || "", command: cmdToText(t.command),
               profile: t.profile || "", automation: t.automation || t.lua || "",
               drives: t.drives || "",
               browser_profile: t.browser_profile || "", private: !!t.private,
               user_agent: t.user_agent || "",
               choose_model: t.choose_model || "", words_model: t.words_model || "",
               locked: !!t.locked, auto_restart: !!t.auto_restart,
               encoding: t.encoding || "", scrollback: t.scrollback ?? "", log: !!t.log,
               notify_on_done: t.notify_on_done || "", notify_reply: !!t.notify_reply,
               nav: t.nav || null, ask: t.ask || null,
               // Everything about a server connection that will not fit in its
               // address. Carried whole: a field this screen has never heard of
               // still has to survive being saved from it
               server: t.server || null, depth, group });
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
    if ((f.git_account || "").trim()) node.git_account = f.git_account.trim();
    if (f.browser_profile) node.browser_profile = f.browser_profile;
    if (f.private) node.private = true;
    if (f.user_agent) node.user_agent = f.user_agent;
    for (const k of WORDS_KEYS) if ((f[k] || "").trim()) node[k] = f[k].trim();
    if (f.locked) node.locked = true;
    if (f.auto_restart) node.auto_restart = true;
    if (f.encoding) node.encoding = f.encoding;
    if (f.scrollback) node.scrollback = Number(f.scrollback);
    if (f.log) node.log = true;
    if (f.notify_on_done) node.notify_on_done = f.notify_on_done;
    if (f.notify_on_done && f.notify_reply) node.notify_reply = true;
    // Don't write it if none are enabled. Leaving a block of all-false values just hurts readability
    if (f.nav && Object.values(f.nav).some(Boolean)) {
      node.nav = {};
      for (const k of NAV_PARTS)
        if (f.nav[k]) node.nav[k] = true;
    }
    // A server connection's own settings, written only when there is
    // something in them -- an empty block would say "this tab is a server"
    // about tabs that are not
    if (f.server && Object.values(f.server).some(v =>
        v !== null && v !== undefined && v !== "" && v !== 0)) {
      node.server = {};
      for (const k of ["key", "jump", "keepalive", "file_command", "remote_dir"]) {
        const v = f.server[k];
        if (v === null || v === undefined || v === "" || v === 0) continue;
        // A bastion nobody named is not one, and its port is a number even
        // though it was typed into a text box
        if (k === "jump") {
          if (!(v.host || "").trim()) continue;
          const j = {host: (v.host || "").trim(), user: (v.user || "").trim()};
          if (String(v.port || "").trim()) j.port = Number(v.port);
          if ((v.key || "").trim()) j.key = v.key.trim();
          node.server.jump = j;
          continue;
        }
        node.server[k] = k === "keepalive" ? Number(v) : v;
      }
      if (!Object.keys(node.server).length) delete node.server;
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
  const list = (Array.isArray(current.desks) && current.desks.length)
      ? current.desks
      : [{ name:"DEFAULT", folders: current.folders || [], tabs: current.tabs || [] }];
  // Read once, from wherever it was written, and then let go of the old spelling
  // so that saving settles the file into one shape rather than both.
  delete current.tabs;
  desks = [];
  for (const w of list) {
    const desk = { name:w.name || "", id:w.id || "", file:w.file || null,
                 automation:w.automation || w.lua || "", tabs:[], folders:[],
                 // Not touched from the screen, but kept so saving doesn't drop it
                 browsers:w.browsers || null,
                 secrets_allow: w.secrets_allow || [],
                 secrets_allow_all: !!w.secrets_allow_all,
                 // This desk's own, whole. Nothing of the app's stands behind any
                 // of them, so an unwritten one is simply empty
                 notify: isObj(w.notify) ? w.notify : {},
                 primary_notify: w.primary_notify || "",
                 providers: isObj(w.providers) ? w.providers : {},
                 // Written by hand in the file, shown but not edited here, and
                 // carried through a save rather than dropped by one
                 capabilities: isObj(w.capabilities) ? w.capabilities : {},
                 automation_permissions: isObj(w.automation_permissions) ? w.automation_permissions : {},
                 git: isObj(w.git) ? w.git : {},
                 // This desk's own projects (the same repository in another
                 // desk is another project there)
                 projects: Array.isArray(w.projects) ? w.projects : [],
                 // Its git accounts. Read in as well as written out: left out
                 // here, the page showed none and the next save erased them
                 git_accounts: Array.isArray(w.git_accounts) ? w.git_accounts : [],
                 // The assistant AI this desk agreed to send pictures to, by name
                 send_pictures_to: (w.send_pictures_to || "").trim(),
                 send_pages_to: (w.send_pages_to || "").trim(),
                 // What its browser tabs follow. Whole, so a browser setting
                 // this page does not show yet survives being saved from it
                 browser: isObj(w.browser) ? Object.assign({}, w.browser) : {},
                 // What writes its automatic names, and whether they reach the
                 // branch. Read in as well as written out: a setting the page
                 // never saw is a setting the next save erases
                 summary_ai: (w.summary_ai || "").trim(),
                 rename_branch: w.rename_branch !== false,
                 stops: Array.isArray(w.stops) ? w.stops : [],
                 discuss: w.discuss || null };
    if (desk.file) {
      const got = await readUserJson(await deskApi("GET", desk.file));
      // A desk file is loaded to be written back. If it can't be read, the
      // tabs would come out empty and saving would erase them, so stop here too.
      if (got.failure) return showLoadFailure(got.failure);
      const f = got.value;
      readFolders(desk, f);
      if (!desk.automation) desk.automation = f.automation || f.lua || "";
      if (!desk.secrets_allow.length) desk.secrets_allow = f.secrets_allow || [];
      if (!desk.secrets_allow_all) desk.secrets_allow_all = !!f.secrets_allow_all;
      if (!desk.stops.length && Array.isArray(f.stops)) desk.stops = f.stops;
      if (!desk.discuss && f.discuss) desk.discuss = f.discuss;
    } else readFolders(desk, w);
    desks.push(desk);
  }
  // The two models that drive a page from plain words were once the whole
  // app's. They are each desk's now: a desk with none of its own takes them,
  // and they are not written up there again
  const op = current.operate || {};
  for (const k of WORDS_KEYS) {
    const v = (op[k] || "").trim();
    if (v) for (const d of desks) if (!(d.browser[k] || "").trim()) d.browser[k] = v;
    delete op[k];
  }
  if (sel.desk >= desks.length) sel = {desk:0, tab:null, global:true};
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
  // Two things answering to the same automation name is not a preference to
  // save: whichever came first would quietly take everything addressed to
  // either. Say which name, and let the person choose who keeps it
  // A model that cannot do the job it is chosen for -- a decision model set
  // to write, a connection since removed, one with no model named -- is not
  // saved: the page it would drive would stop at the first move. Go to it
  for (let di = 0; di < desks.length; di++) {
    const bad = wordsFaults(di)[0];
    if (!bad) continue;
    if (bad.tab == null) {
      sel = {desk:di, grp:null, tab:null, global:false};
      goDeskSection("browser", "center");
    } else {
      sel = {desk:di, grp:desks[di].tabs[bad.tab].group || 0, tab:bad.tab, global:false};
      render();
      const at = document.getElementById("tab-words");
      if (at) at.scrollIntoView({block:"center"});
    }
    result(fill(T["settings.words.cannot_save"], {why: bad.why}), true);
    return;
  }
  const clash = collidingIds();
  if (clash.length) { result(fill(T["settings.id.duplicate"], {names: clash.join(", ")}), true); return; }
  const btn = document.getElementById("savebtn");
  btn.disabled = true;
  btn.classList.remove("dirty");
  btn.textContent = T["common.saving"];
  let ok = false;
  try {
    // A save that said why it could not be done is not a finished save, and
    // nothing below should act as if it were -- closing would take the reason
    // away with the page
    ok = await doSave();
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
  return ok;
}

// Assembles what gets written. Used by both saving and unsaved-change detection.
// Also called from the every-600ms unsaved check, so this must have no side effects
function payload() {
  const out = Object.assign({}, current);
  ["tab_bar_width","max_chain"].forEach(k => {
    const v = out[k]; if (v === "" || v === null || v === undefined) delete out[k]; else out[k] = Number(v);
  });
  ["automation","secrets","ai_engine","browser_data","browser_draw","language","user_agent"].forEach(k => { if (!out[k]) delete out[k]; });
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
    else {
      out.operate = { max_rounds:(o.max_rounds ?? 40), max_seconds:(o.max_seconds ?? 900),
                      max_tokens:(o.max_tokens ?? 400000), on_limit:(o.on_limit || "stop"),
                      settle_ms:(o.settle_ms ?? 1800), confirm:(o.confirm || "off") };
    }
  }
  // Quick commands, written in one shape whatever state the editor left them
  // in: what is the default is left out (the app reads it back the same), a
  // command never filled in is dropped, and nothing is left when nothing is
  // there -- so opening the editor and closing it is not an edit
  if (out.quick_commands) {
    const q = quickCanon(out.quick_commands, false);
    if (q) out.quick_commands = q; else delete out.quick_commands;
  }
  if (out.remote && !out.remote.enabled && !out.remote.allow_public) delete out.remote;
  // Where these used to be written for the whole app. They are each desk's
  // now, and a copy left up here would read as an answer that still applies
  for (const k of ["notify", "primary_notify", "providers", "capabilities", "automation_permissions", "git", "projects"]) delete out[k];
  delete out.lua; delete out.tabs;

  // A group is written with its own tabs nested back under it. Its name and id
  // are worth writing only when someone typed them; the folder always is
  const foldersOut = w => (w.folders && w.folders.length ? w.folders : [{}]).map((g, i) => {
    // Everything the folder already said, carried through. The app writes keys
    // this screen never shows -- the machine a folder is on, the project it is
    // a piece of, where it came from -- and a save that wrote only the keys it
    // knew was how a folder on a server turned into a folder here
    const o = Object.assign({}, g);
    delete o.tabs;
    for (const k of ["name", "id", "cwd"]) {
      if ((g[k] || "").trim()) o[k] = g[k].trim(); else delete o[k];
    }
    // The branches this folder guards, when it has an answer of its own. An
    // empty list is an answer too ("nothing here"), so what decides is whether
    // there is a list at all
    if (!Array.isArray(g.protect)) delete o.protect;
    for (const k of ["project", "host"]) if (!(g[k] || "").trim()) delete o[k];
    if (!g.source) delete o.source;
    o.tabs = nest(w.tabs.filter(t => (t.group || 0) === i));
    return o;
  });

  // Stop conditions are saved after dropping rows with an empty when
  const cleanStops = w => (w.stops || []).filter(s => s && s.when);
  // A discussion is only saved when there are 2 or more participants
  const cleanDiscuss = w => (w.discuss && (w.discuss.agents || []).length >= 2) ? w.discuss : null;

  // A desk that was split out into a separate file gets written to that file
  const files = [];
  for (const w of desks) {
    if (!w.file) continue;
    const body = { name:w.name, folders:foldersOut(w) };
    if (w.automation) body.automation = w.automation;
    if (w.secrets_allow && w.secrets_allow.length) body.secrets_allow = w.secrets_allow;
    if (w.secrets_allow_all) body.secrets_allow_all = true;
    const st = cleanStops(w); if (st.length) body.stops = st;
    const dc = cleanDiscuss(w); if (dc) body.discuss = dc;
    files.push({ file:w.file, body });
  }
  out.desks = desks.map(w => {
    const o = { name:w.name, id:w.id };
    if (w.file) o.file = w.file;
    else { if (w.automation) o.automation = w.automation; o.folders = foldersOut(w); }
    // Don't lose a setting that isn't on screen just because it was saved from the screen
    if (w.browsers) o.browsers = w.browsers;
    // Allow-list of secrets the rally may use (denied by default)
    // Which secrets a desk was allowed to borrow, from when there was one
    // pool of them to borrow from. Nothing reads it any more except the step
    // that carries an older secrets file forward, which needs it to know whose
    // secret was whose -- so it is kept rather than dropped on the first save
    if (w.secrets_allow && w.secrets_allow.length) o.secrets_allow = w.secrets_allow;
    if (w.secrets_allow_all) o.secrets_allow_all = true;
    // This desk's own notification destinations, model connections, doors,
    // permission table and git settings. Each written only when it holds
    // something, so a desk with none of them stays a short entry
    const some = v => isObj(v) && Object.keys(v).length > 0;
    if (some(w.notify)) o.notify = w.notify;
    if ((w.primary_notify || "").trim() && some(w.notify) && w.notify[w.primary_notify.trim()]) {
      o.primary_notify = w.primary_notify.trim();
    }
    // Don't save a connection with an empty base_url (leftover junk from a
    // still-in-progress add)
    const provs = Object.fromEntries(Object.entries(w.providers || {})
      .filter(([, p]) => p && (p.base_url || "").trim()));
    if (some(provs)) o.providers = provs;
    if (some(w.capabilities)) o.capabilities = w.capabilities;
    if (some(w.automation_permissions)) o.automation_permissions = w.automation_permissions;
    if (some(w.git)) o.git = w.git;
    // Its projects, each with a name; an entry left without one is not one
    const projs = (w.projects || []).filter(p => p && (p.name || "").trim())
      .map(p => {
        const c = Object.assign({}, p);
        for (const k of ["at", "setup", "branch_prefix"]) if (!(c[k] || "").trim()) delete c[k];
        // A file from elsewhere with nowhere to come from or go is not one yet
        if (Array.isArray(c.bring)) {
          c.bring = c.bring.filter(r => r && (r.pattern || ((r.from || "").trim() && (r.to || "").trim())));
          if (!c.bring.length) delete c.bring;
        }
        return c;
      });
    if (projs.length) o.projects = projs;
    // Its git accounts, each with a name. The tokens are in the secrets file
    const accts = (w.git_accounts || []).filter(a => a && (a.name || "").trim());
    if (accts.length) o.git_accounts = accts;
    if ((w.send_pictures_to || "").trim()) o.send_pictures_to = w.send_pictures_to.trim();
    if ((w.send_pages_to || "").trim()) o.send_pages_to = w.send_pages_to.trim();
    // What its browser tabs follow; an unchosen model is not written
    const br = Object.assign({}, w.browser || {});
    for (const k of WORDS_KEYS) if ((br[k] || "").trim()) br[k] = br[k].trim(); else delete br[k];
    if (some(br)) o.browser = br;
    // What writes this desk's automatic names, and whether they reach the
    // branch. Both are the desk's own answer, so a desk that has not given one
    // stays a short entry and follows the app
    if ((w.summary_ai || "").trim()) o.summary_ai = w.summary_ai.trim();
    if (w.rename_branch === false) o.rename_branch = false;
    // Stop conditions (judge). Already written into the file for a file-referenced desk, so don't duplicate it here
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
  for (const w of desks) ensureIds(w);
  ensureWsIds();
  const { out, files } = payload();
  for (const f of files) {
    const rf = await deskApi("POST", f.file, JSON.stringify(f.body, null, 2));
    const jf = await rf.json().catch(() => ({ok:false}));
    if (!jf.ok) { result(fill(T["settings.file_save_failed"], {file: f.file}), true); return false; }
  }
  const r = await api("POST", JSON.stringify(out, null, 2));
  const j = await r.json();
  if (!j.ok) { result(fill(T["settings.save_failed"], {error: j.error}), true); return false; }
  markClean();
  result(T["common.saved"]);
  // The language is only read at launch, so a change won't take effect until a restart.
  // Since a toast gets hidden behind the board on returning to it and goes unnoticed, use a reliable alert instead
  if (languageNeedsRestart()) {
    loadedLanguage = (current.language || "").trim().toLowerCase(); // Avoids showing the notice twice
    alert(T["settings.language.restart"]);
  }
  // Once saved, this screen's job is done. Leaving it open would mean the only way back
  // to the board is "click another tab", making settings feel like it's overstaying.
  // Not from the dialog over the board: that was opened from a tab, and the tab
  // it adds is where the person is going, not the board
  if (!floating) goIndex();
  return true;
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
async function closeSettings() {
  // Nothing was loaded, so there is nothing to lose — don't ask.
  if (!loadFailure && snapshot() !== savedSnapshot &&
      !await confirmAction(T["settings.back.confirm"], T["settings.back.discard"])) return;
  // In the window this rides the ipc bridge back to the board. On a phone (served
  // over the remote proxy) there is no bridge, so navigate to "/" — the shell,
  // which re-authenticates from its stored token. The unsaved-changes guard above
  // runs first either way.
  if (window.ipc) { try { window.ipc.postMessage(JSON.stringify({kind:"closesettings"})); } catch (e) { goIndex(); } }
  // Framed over a board that is still running: the board takes the frame down
  // and is there underneath, with nothing to load again
  else if (EMBED) toBoard("close");
  else { location.href = "/"; }
}

// The board's +, as a dialog: what the new tab runs, and nothing else. The name
// and the automation name were filled in when the tab was made, so answering
// this one question is enough to add it
function enterFloat(wi, i) {
  const desk = desks[wi];
  const t = desk.tabs[i];
  floating = {wi, i, t};
  document.body.classList.add("float");
  const gi = t.group || 0;
  const g = (desk.folders || [])[gi];
  // Where it is going, when there is a where. A group with no folder yet is
  // asked about below instead, and "adding to no folder chosen" is not a line
  // anybody should have to read
  document.getElementById("floatwhere").textContent =
    g && (g.cwd || "").trim() ? fill(T["settings.float.where"], {folder: folderLabel(g, gi)}) : "";
  const body = document.getElementById("floatbody");
  body.textContent = "";
  body.append(launchCard(t));
  // A group with no folder of its own has nowhere to run anything, and a tab
  // added to one waits instead of starting. The question is asked here, while
  // the tab is being made, rather than left to be discovered on a tab that
  // sits there saying nothing happened
  if (g && !(g.cwd || "").trim()) {
    body.append(card(T["settings.group.folder"],
      row(T["settings.group.folder"],
          ...pathField(g, "cwd", T["settings.group.folder.ph"], "dir", T["settings.group.folder.pick"]),
          el("span", {class:"hint"}, T["settings.group.folder.hint"]))));
  }
  const first = body.querySelector("select, input");
  if (first) first.focus();
}
// Add it. The dialog closes once the file is written; if it could not be, the
// reason stays on screen with the dialog still there to fix it in
async function floatAdd() {
  if (!floating) return;
  const btn = document.getElementById("floatadd");
  btn.disabled = true;
  try {
    if (await save()) closeSettings();
  } finally {
    btn.disabled = false;
  }
}
// Not adding after all. The tab only ever existed on this page, so there is
// nothing to ask about losing
function floatCancel() {
  if (!floating) return;
  savedSnapshot = snapshot();
  closeSettings();
}
// The whole of the new tab's page, in the whole of the window: the same tab,
// with what was chosen so far kept
function floatMore() {
  if (!floating) return;
  const {wi, i, t} = floating;
  floating = null;
  document.body.classList.remove("float");
  if (window.ipc) { try { window.ipc.postMessage(JSON.stringify({kind:"settingsfull"})); } catch (e) {} }
  // Framed: the board gives the frame the whole screen. The page it holds is
  // this one, so everything chosen so far is still chosen
  else if (EMBED) toBoard("full");
  sel = {desk:wi, grp:t.group || 0, tab:i, global:false};
  render();
  showSelected("center");
}
// Esc is the dialog's way out (style guide 5.2), and only the dialog's: a
// confirmation opened over it takes its own Esc first
document.addEventListener("keydown", e => {
  if (e.key !== "Escape" || document.querySelector("dialog[open]")) return;
  // The one question the board's + asks: not adding after all
  if (floating) { e.preventDefault(); floatCancel(); return; }
  // A sheet standing over the board: the same way out, and the same guard
  // about work not saved that its Close button goes through
  if (SHEET) { e.preventDefault(); closeSettings(); }
});

// Opens a help/report page in the real browser. The server whitelists `dest`
// and pre-fills the bug template with this build and the OS version.
function openExt(dest) {
  fetch("/api/open?dest=" + dest, {headers:{"X-Token":TOKEN}}).catch(()=>{});
}

// Said once, before the first paint: this page is inside a frame the board
// drew an edge around (see body.embed in the style block)
if (EMBED) document.body.classList.add("embed");
// Lay the header out for this screen width before the first paint, so the page
// doesn't flash the desktop arrangement on the way in.
placeHeadLinks();
measureHeader();

// ── Being written in for ──────────────────────────────────────
// While the guide's ? is open, pressing a box here picks it: the guide is
// told what that box is called and what goes in it, and writes the answer
// back into it. What is IN the box never leaves this page -- the guide is
// answering "what should go here", not reading what is there.
//
// A box that holds a secret cannot be picked at all. Whether it is one is
// read off the box itself (a password field) rather than from a list of
// names, because a list is a thing to keep up to date and a type is not.
let pickedBox = null;
let guideUp = false;

// What a box is: what goes in it, and what it offers when it offers a list
function boxKind(box) {
  if (box.tagName === "SELECT") return {kind:"choice", options:[...box.options].map(o => o.textContent)};
  if (box.type === "checkbox") return {kind:"tick", options:[]};
  if (box.type === "number") return {kind:"number", options:[]};
  return {kind:"text", options:[]};
}
// What the page calls this box: the name written beside its row, the line
// under it, and the card it is on. Read from what is drawn, so it cannot
// disagree with what the person is looking at
function boxNamed(box) {
  const row = box.closest(".row") || box.closest(".card") || document.body;
  const own = box.closest("label.check");
  const label = (own ? own.textContent : (row.querySelector("label") || {}).textContent) || "";
  const hint = (row.querySelector(".hint") || {}).textContent || "";
  const card = box.closest(".card");
  const head = card ? (card.querySelector("h2") || {}).textContent || "" : "";
  return {label: label.trim(), hint: hint.trim(), screen: head.trim()};
}
function canPick(box) {
  return box && box.type !== "password" && !box.disabled && !box.readOnly;
}
function unpick() {
  if (pickedBox) pickedBox.classList.remove("picked");
  pickedBox = null;
}
function guidePost(path, body) {
  return fetch("/api/guide" + path, {method:"POST",
    headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
    body: JSON.stringify(body || {})}).then(r => r.json()).catch(() => ({}));
}
document.addEventListener("click", e => {
  if (!guideUp) return;
  const box = e.target.closest("input, select, textarea");
  if (!canPick(box)) return;
  if (pickedBox === box) { unpick(); guidePost("/picked", {}); return; }
  unpick();
  pickedBox = box;
  box.classList.add("picked");
  guidePost("/picked", Object.assign(boxNamed(box), boxKind(box)));
}, true);

// Whether the ? is up, and what it left to be written. Asked for rather than
// pushed, because this page is the only thing that knows how to write into
// its own boxes -- a value set without the page's own "input" event leaves it
// showing one thing and holding another
async function readGuide() {
  let now = null;
  try { now = await (await fetch("/api/guide/up", {headers:{"X-Token":TOKEN}})).json(); }
  catch (e) { return; }
  const was = guideUp;
  guideUp = !!(now && now.up);
  // Put away from the board: nothing here is picked for it any more. Said out
  // loud as well, because the box was this page's and a ? that was closed
  // rather than put away (a phone's frame taken down, a browser shut) never
  // got to let go of it
  if (was && !guideUp) {
    const had = !!pickedBox;
    unpick();
    if (had) guidePost("/picked", {});
  }
  if (!guideUp || !pickedBox) return;
  const text = now.fill;
  if (text === null || text === undefined) return;
  const box = pickedBox;
  if (box.type === "checkbox") {
    const on = /^(1|true|on|yes|はい)$/i.test(String(text).trim());
    if (box.checked !== on) { box.checked = on; box.dispatchEvent(new Event("change", {bubbles:true})); }
  } else if (box.tagName === "SELECT") {
    const want = [...box.options].find(o => o.textContent.trim() === String(text).trim()
                                          || o.value === String(text).trim());
    if (want) { box.value = want.value; box.dispatchEvent(new Event("change", {bubbles:true})); }
  } else {
    box.value = text;
    box.dispatchEvent(new Event("input", {bubbles:true}));
    box.dispatchEvent(new Event("change", {bubbles:true}));
  }
  box.focus();
}
// Asked at once as well as on the beat: for the first beat after this page
// opened, `guideUp` was false and pressing a box did nothing at all -- and
// the way here is usually a button in the panel, which is to say that the
// first second is exactly when somebody presses one
readGuide();
setInterval(readGuide, 900);

// If the URL has addtab=<desk-index>, start with one tab already added
// to that desk after loading (this is where the tab bar's + comes from).
// desk=<desk-index> only expands that group (the gear passes the desk
// being viewed, so the settings open onto the one you came from).
// Careful: Number(null) is 0, so a missing parameter must be rejected as text
// first — otherwise every plain open would start editing desk 0
load().then(() => {
  const q = new URLSearchParams(location.search);
  const idx = k => /^\d+$/.test(q.get(k) || "") ? Number(q.get(k)) : -1;
  // A shortcut may ask to bounce back to the board once saved (?ret=1).
  returnOnSave = q.get("ret") === "1";
  // ?section=<id> deep-links straight to one global card (the ⚙ shortcuts).
  const sec = q.get("section");
  if (sec && globalSections().some(s => s.id === sec)) {
    goSection(sec, "center");
    return;
  }
  // One of a desk's settings, named as the guide names one: every entry a
  // desk has is reachable this way, where DESK_LINKS holds only the few the
  // board links to by an older name of their own
  if (sec && sec.startsWith("desk:")) {
    const want = sec.slice(5);
    if (deskSections(desks[sel.desk] || {}).some(s => s.id === want)) {
      const at = idx("desk");
      sel = {desk:(desks[at] ? at : sel.desk), grp:null, tab:null, global:false};
      goDeskSection(want, "center");
      return;
    }
  }
  // ?tabkey=<name> lands on a browser tab, by the name its page goes by (its
  // id, else its name). &section=words then shows its models: what the board
  // opens when a page is to be driven in words and has none chosen
  const tabKey = (q.get("tabkey") || "").trim();
  const keyDesk = idx("desk");
  if (tabKey && desks[keyDesk]) {
    const tabs = desks[keyDesk].tabs || [];
    const ti = tabs.findIndex(t => catOf(t.command) === "browser"
      && ((t.id || "").trim() || (t.name || "").trim() || "browser") === tabKey);
    if (ti >= 0) {
      sel = {desk:keyDesk, grp:tabs[ti].group || 0, tab:ti, global:false};
      render();
      const words = sec === "words" && document.getElementById("tab-words");
      if (words) words.scrollIntoView({block:"start"}); else showSelected("center");
      return;
    }
  }
  // One of a desk's settings: that desk, at that entry
  if (sec && DESK_LINKS[sec]) {
    const at = idx("desk");
    sel = {desk:(desks[at] ? at : sel.desk), grp:null, tab:null, global:false};
    goDeskSection(DESK_LINKS[sec], "center");
    // Asked for one field, not the card: that field, marked
    if (sec === "git-message") lookAtCard("desk-git-message", 50);
    if (sec === "git-issue") lookAtCard("desk-git-issue", 50);
    if (sec === "git-pr") lookAtCard("desk-git-pr", 50);
    if (sec === "git-merge") lookAtCard("desk-git-merge", 50);
    if (sec === "git-ci") lookAtCard("desk-git-ci", 50);
    return;
  }
  const wi = idx("addtab");
  if (desks[wi]) {
    // Asked for from a folder: that is where it goes. The form used to add it
    // wherever the default was, which is the first folder -- so a tab asked
    // for from the third one turned up in the first
    const same = c => (c || "").replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
    const from = (q.get("folder") || "").trim();
    const gi = from
      ? (desks[wi].folders || []).findIndex(g => same(g.cwd) === same(from))
      : -1;
    sel = {desk:wi, grp:null, tab:addTabTo(desks[wi], gi >= 0 ? gi : undefined), global:false};
    sel.grp = desks[wi].tabs[sel.tab].group || 0;
    // The board's + asks for only the one question, over the board
    if (q.get("float") === "1") { render(); enterFloat(wi, sel.tab); return; }
    render();
    showSelected("center");
    return;
  }
  // "Edit settings" (?gen=1), or an open with no desk to focus, lands on the
  // General group expanded. The sidebar gear (?desk=N, no gen) lands on the desk
  // it came from with General collapsed — press its ▸ to open it.
  // ?tab=<id or name> lands on that tab's card: the gear pressed while a
  // tab is in view means the settings for that tab. Its folder narrows the
  // search when two folders have a tab of the same name; failing that, any
  // folder's will do, and failing that the page lands where it would have
  const cur = idx("desk");
  // ?tabpos=<n>&folder=<cwd> lands on the n-th terminal tab of that folder:
  // the gear pressed while a tab is in view. A place, not a name, so a tab
  // that was never named lands the same. An empty folder means the one with
  // no path of its own (the app's folder), where a group-less tab lives.
  const tabPos = /^\d+$/.test(q.get("tabpos") || "") ? Number(q.get("tabpos")) : -1;
  // ...and &tabname=<title> is the tab itself, found by the title it goes by:
  // its name, or -- unnamed -- what its command is called, the way the app
  // titles it. Tried first; the ordinal is only for a title two tabs share
  const tabName = (q.get("tabname") || "").trim().toLowerCase();
  if ((tabPos >= 0 || tabName) && desks[cur]) {
    const same = c => (c || "").replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
    const from = (q.get("folder") || "").trim();
    const gi = from
      ? (desks[cur].folders || []).findIndex(g => same(g.cwd) === same(from))
      : (desks[cur].folders || []).findIndex(g => !(g.cwd || "").trim());
    if (gi >= 0) {
      const tabs = desks[cur].tabs || [];
      const here = [];
      // Only the ones the board counts as terminals. A page, a git or a file
      // panel is written in the same list and is not one of them, so counting
      // it sent the board's second tab to whatever stood second here
      const terminal = t => {
        const c = cmdToText(t.command).trim();
        return !["browser", "git", "sftp", "editor"].includes(catOf(c));
      };
      tabs.forEach((t, i) => { if ((t.group || 0) === gi && terminal(t)) here.push(i); });
      const titleOf = t => ((t.name || "").trim()
        || (cmdToText(t.command).trim().split(/\s+/)[0] || "").split(/[\/]/).pop().replace(/\.[^.]*$/, "")).toLowerCase();
      const named = tabName ? here.filter(i => titleOf(tabs[i]) === tabName) : [];
      const ti = named.length === 1 ? named[0] : here[tabPos];
      if (ti != null) {
        sel = {desk:cur, grp:gi, tab:ti, global:false};
        render();
        showSelected("center");
        return;
      }
    }
  }
  // ?folder=<path> lands on that folder's own page: the tab list's edit
  // entry knows the folder, not which line of the settings file it is on
  const want = (q.get("folder") || "").trim();
  // ?section=project&folder=<path> lands on the page of the project that folder
  // is in: where its git account is chosen. Which project a folder is in may
  // only be known once git has said which repository it is, so that answer is
  // waited for -- and only a folder git says is in no repository, or an answer
  // that never comes, settles for the folder's own page
  // ...and ?section=project-gitacct lands on that page's git account card,
  // for a refusal that is put right there
  if ((sec === "project" || sec === "project-gitacct") && want && desks[cur]) {
    // Either slash: the settings write D:/work, a path said by Windows is D:\work
    const same = c => (c || "").replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
    const gi = (desks[cur].folders || []).findIndex(g => same(g.cwd) === same(want));
    if (gi >= 0) {
      const cwd = (desks[cur].folders[gi].cwd || "").trim();
      // Asked once, then waited for: asking again restarts the moment the
      // question is gathered in, and a question asked every few moments is never sent
      deskProjects(desks[cur]);
      const land = tries => {
        if (!(cwd in FAMILIES) && tries > 0) return setTimeout(() => land(tries - 1), 200);
        const home = deskProjects(desks[cur]).projects.find(p => p.folders.includes(gi));
        sel = home ? {desk:cur, proj:home.key, grp:null, tab:null, global:false}
                   : {desk:cur, grp:gi, tab:null, global:false};
        render();
        showSelected("center");
        if (home && sec === "project-gitacct") PC_ACCOUNTS_READ.then(() => lookAtCard("project-gitacct", 50));
      };
      land(100);
      return;
    }
  }
  if (want && desks[cur]) {
    const same = c => (c || "").replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
    const gi = (desks[cur].folders || []).findIndex(g => same(g.cwd) === same(want));
    if (gi >= 0) {
      sel = {desk:cur, grp:gi, tab:null, global:false};
      render();
      showSelected("center");
      return;
    }
  }
  if (q.get("gen") === "1" || !desks[cur]) {
    sel = {desk:(desks[cur] ? cur : sel.desk), grp:null, tab:null, global:true, section:"basic"};
  } else {
    // The desk's tree, not its settings: the gear on the board means "this
    // desk", and which of its settings is wanted is the person's to choose
    toTree(cur);
  }
  render();
  showSelected("center");
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
 button { font-family:inherit; font-size:13px; border-radius:var(--r-ctl); border:1px solid var(--line);
   background:var(--panel2); color:var(--text); padding:8px 14px; cursor:pointer; }
 button:hover { border-color:var(--accent); }
 button.primary { background:var(--accent); border-color:var(--accent); color:#04121c; font-weight:600; }
 button.ghost { background:none; }

 main { max-width:900px; margin:0 auto; padding:20px 18px 80px; }
 .caption { text-align:center; color:var(--muted); font-size:12px; margin:var(--s2) auto var(--s4); }

 .turn { display:flex; margin:var(--s4) 0; gap:var(--s3); align-items:flex-end; }
 .turn.right { flex-direction:row-reverse; }
 .avatar { flex:none; width:34px; height:34px; border-radius:50%; display:flex; align-items:center;
   justify-content:center; font-weight:700; font-size:14px; color:#04121c; }
 .col { max-width:78%; min-width:0; display:flex; flex-direction:column; }
 .turn.right .col { align-items:flex-end; }
 .who { font-size:12px; color:var(--muted); margin:0 var(--s1) var(--s1); }
 .who .badge { opacity:.6; margin-left:6px; }
 .bubble { position:relative; background:var(--panel); border:1px solid var(--line);
   border-radius:var(--r-card); padding:10px 14px; overflow:hidden; }
 .turn.left .bubble { border-top-left-radius:4px; }
 .turn.right .bubble { border-top-right-radius:4px; }
 .bubble .body { overflow-x:auto; }
 .bubble .body p { margin:0 0 var(--s2); }
 .bubble .body p:last-child { margin-bottom:0; }
 .bubble .body pre { background:#0c0f13; border:1px solid var(--line); border-radius:var(--r-ctl);
   padding:10px 12px; margin:var(--s2) 0; overflow-x:auto; font-size:12.5px; line-height:1.5; }
 .bubble .body pre .add { color:#7ee787; display:block; }
 .bubble .body pre .del { color:#ff9a9a; display:block; }
 .bubble .body pre .hunk { color:#79c0ff; display:block; }
 .bubble .body code.inline { background:#0c0f13; border:1px solid var(--line);
   border-radius:var(--r-chip); padding:1px 5px; font-size:12.5px; }

 /* Tall bubbles clamp; the fade + button invite a click to see the rest. */
 .bubble.clamped .body { max-height:320px; overflow:hidden; }
 .bubble.clamped::after { content:""; position:absolute; left:0; right:0; bottom:34px; height:60px;
   pointer-events:none; background:linear-gradient(transparent, var(--panel)); }
 .bubble .more { margin-top:8px; font-size:12px; padding:4px 10px; }

 /* The verdict / judge ruling: a full-width report, not a chat bubble. */
 .report { background:var(--panel); border:1px solid var(--line); border-left:3px solid var(--accent);
   border-radius:var(--r-card); padding:14px 20px; margin:var(--s6) 0 var(--s3); overflow:hidden; }
 .report > h3 { margin:0 0 var(--s3); font-size:12px; letter-spacing:.06em; text-transform:uppercase; color:var(--accent); }
 .report .body { overflow-x:auto; }
 /* Speakers' own Markdown, rendered inside a bubble or report. */
 .body .mh { font-weight:700; margin:var(--s3) 0 var(--s2); }
 .body .mh1, .body .mh2 { font-size:15px; color:var(--text); }
 .body .mh3, .body .mh4, .body .mh5, .body .mh6 { font-size:13px; color:var(--muted);
   letter-spacing:.03em; }
 .body table { border-collapse:collapse; margin:var(--s2) 0; font-size:12.5px; max-width:100%; }
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
// judge's name — "Verdict (judge X)". No-judge endings use the aggregate / round-
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
 code { background:var(--panel); color:var(--warn); padding:1px 5px; border-radius:var(--r-chip); }
 pre { background:var(--panel); border:1px solid var(--line); padding:12px; overflow:auto; }
 pre code { color:var(--c6); background:none; padding:0; }
 table { border-collapse:collapse; margin:var(--s3) 0; }
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

/// The desktop that can open a file dialog, if one is running.
///
/// Set once by whatever owns that desktop. The settings server never builds
/// one: a build with no desktop simply has no picker, and the page that asked
/// is answered "no" instead of waiting on a dialog nobody can see.
static PICKER: std::sync::OnceLock<Box<dyn shikisha_shared::FilePicker>> = std::sync::OnceLock::new();

pub fn use_file_picker(p: Box<dyn shikisha_shared::FilePicker>) {
    let _ = PICKER.set(p);
}

fn picker() -> Option<&'static dyn shikisha_shared::FilePicker> {
    PICKER.get().map(|p| p.as_ref())
}

#[cfg(test)]
mod tests {

    /// The light way of asking stays light when the small model is turned
    /// off: only the model goes.
    ///
    /// The setting is about one thing -- whether a name is written by the
    /// cheapest model the CLI has -- and everything else that makes this cheap
    /// (no tools, nothing kept, no thinking) has no second side to argue for.
    /// A checkbox that quietly turned the rest back on would cost the person
    /// nine thousand tokens for a folder's name
    #[test]
    fn turning_the_small_model_off_turns_off_only_the_model() {
        let args = |name: &str, small: bool| {
            super::light_invocation(name, small, None).expect("this CLI has a light way").0.join(" ")
        };
        assert!(args("claude", true).contains("--model haiku"));
        assert!(!args("claude", false).contains("--model"), "{}", args("claude", false));
        assert!(args("codex", true).contains("model_reasoning_effort=low"));
        assert!(!args("codex", false).contains("model_reasoning_effort"), "{}", args("codex", false));
        for small in [true, false] {
            let claude = args("claude", small);
            assert!(claude.contains("--tools  --no-session-persistence"), "{claude}");
            assert!(claude.contains("--disable-slash-commands"), "{claude}");
            let codex = args("codex", small);
            assert!(codex.contains("--ephemeral") && codex.contains("--sandbox read-only"), "{codex}");
            assert!(codex.contains("web_search=disabled"), "{codex}");
            // Gemini routes a request this size itself; there is nothing to say
            assert!(!args("gemini", small).contains("--model"));
        }
    }

    /// What the two new answers about automatic names are asked on screen, and
    /// written down afterwards.
    ///
    /// A setting the page shows but never saves is worse than no setting: it
    /// answers the question in front of the person and forgets it on the next
    /// press. `summary_ai` was exactly that until this test existed -- the
    /// desk's card wrote it into the page's own copy, and the save built its
    /// desks field by field and never asked for it
    #[test]
    fn the_naming_settings_are_shown_and_kept() {
        for held in [
            // Asked app-wide, under the assistant AI it defaults to
            r#"choose(current, "summary_ai","#,
            r#"checkDefaultOn(current, "summary_small_model", T["settings.summary_small.label"])"#,
            // Asked of a desk, in the card about its automatic names
            r#"checkDefaultOn(desk, "rename_branch", T["settings.labels.branch.label"])"#,
            // And kept: read into the page, and written back out
            r#"summary_ai: (w.summary_ai || "").trim(),"#,
            r#"rename_branch: w.rename_branch !== false,"#,
            r#"if ((w.summary_ai || "").trim()) o.summary_ai = w.summary_ai.trim();"#,
            r#"if (w.rename_branch === false) o.rename_branch = false;"#,
            // The project's prefix: shown, and kept when it says something
            r#"e.branch_prefix = prefixIn.value.trim(); else delete e.branch_prefix;"#,
            r#"for (const k of ["at", "setup", "branch_prefix"])"#,
        ] {
            assert!(super::PAGE.contains(held), "the settings page no longer has: {held}");
        }
    }

    /// Every name a page calls is a name that page has.
    ///
    /// The script is a string as far as the compiler is concerned, so a
    /// function deleted along with the feature that used it takes no callers
    /// with it. The card that calls one draws nothing at all -- the browser
    /// stops mid-draw, the pane stays empty, and nobody is told. That is what
    /// `modelCandidates` did to "Automatic names" and to an AI tab set to a
    /// model connection, from 2026-09-18 until a person pressed it and asked.
    ///
    /// The pages are checked as they are served, shared blocks and all: the
    /// helpers another module splices in are the page's too.
    #[test]
    fn every_page_has_what_it_calls() {
        let served = |page: &str| {
            crate::quick::render(crate::push::inject(crate::toast::render(page.to_string())))
        };
        for (which, page) in [
            ("the settings page", super::PAGE),
            ("the result view", super::RESULT_PAGE),
            ("the manual", super::HELP_PAGE),
            // The board and the guide's panel are pages of this app too, and
            // a name that is not there breaks them the same silent way
            ("the board", crate::shell::PAGE),
            ("the guide's panel", crate::guide::page()),
        ] {
            let code = crate::pagelint::code_only(&crate::pagelint::scripts_of(&served(page)));
            let missing = crate::pagelint::dangling_calls(&code);
            assert!(missing.is_empty(), "{which} calls what it does not have: {missing:?}");
        }
    }

    /// A server's settings are read the way the page holds them, before they
    /// are saved: a port as the text in its box. A bastion on port 2222 was
    /// tested on 22 while the text went unread
    #[test]
    fn a_server_is_read_as_the_page_holds_it() {
        let v = serde_json::json!({
            "key": " ", "keepalive": 30, "file_command": "",
            "jump": {"host": " gw.example.com ", "port": "2222", "user": "me", "key": ""},
        });
        let sv = super::server_of_page(&v);
        let jump = sv.jump.as_ref().expect("the bastion was dropped");
        assert_eq!((jump.host.as_str(), jump.port, jump.key.as_deref()), ("gw.example.com", Some(2222), None));
        assert_eq!((sv.key.as_deref(), sv.keepalive, sv.file_command.as_deref()), (None, Some(30), None));
        let spec = crate::view::server_spec("10.0.0.5", 22, "deploy", Some(&sv), &|_| None);
        assert_eq!(spec.machine(), "gw.example.com:2222>10.0.0.5:22");
        assert_eq!(super::port_of(Some(&serde_json::json!(22))), Some(22));
        assert_eq!(super::port_of(Some(&serde_json::json!("70000"))), None);
        assert_eq!(super::port_of(Some(&serde_json::json!(null))), None);
    }

    /// An AI handed a picture is handed nothing else it could reach with.
    /// The picture is a screen, and a screen can say "read this file and
    /// write it out"
    #[test]
    fn an_ai_reading_a_picture_reaches_nothing_but_the_picture() {
        let png = b"\x89PNG\r\n\x1a\n";
        let schema = r#"{"type":"object"}"#;
        let (args, input, on_disk) = super::picture_invocation("claude", "read", png, schema).unwrap();
        let tools = args.iter().position(|a| a == "--tools").expect("claude is given no tools option");
        assert_eq!(args[tools + 1], "", "claude is given tools");
        assert!(args.windows(2).any(|w| w == ["--json-schema", schema]), "claude is not given the shape of the answer");
        assert!(!on_disk, "claude should not be given the picture as a file");
        let msg: serde_json::Value = serde_json::from_str(input.trim()).unwrap();
        assert_eq!(msg["message"]["content"][0]["type"], "image");
        assert_eq!(msg["message"]["content"][1]["text"], "read");

        let (args, input, on_disk) = super::picture_invocation("codex", "read", png, schema).unwrap();
        assert!(args.windows(2).any(|w| w == ["--sandbox", "read-only"]), "codex is not read-only");
        assert!(args.windows(2).any(|w| w == ["--output-schema", super::SCHEMA_FILE]), "codex is not given the shape of the answer");
        assert!(on_disk && input == "read");

        let (args, _, on_disk) = super::picture_invocation("gemini", "read", png, schema).unwrap();
        assert!(args.windows(2).any(|w| w == ["--approval-mode", "plan"]), "gemini is not read-only");
        assert!(on_disk);

        assert!(super::picture_invocation("aider", "read", png, schema).is_none());
        for (name, _, _) in super::AI_ENGINES {
            assert!(super::reads_pictures(name), "{name} has no decided way to be given a picture");
        }
    }

    /// Each installed assistant AI, started the way the tools start it, reads
    /// the text of a real picture. Costs a call to each; run by hand with
    /// `SHIKISHA_PICTURE=<png> cargo test -- --ignored every_installed_ai`
    #[test]
    #[ignore]
    fn every_installed_ai_reads_a_picture() {
        let Ok(path) = std::env::var("SHIKISHA_PICTURE") else { return };
        let png = std::fs::read(path).unwrap();
        for (name, _, _) in super::AI_ENGINES {
            if crate::tab::resolve_command(name).is_none() {
                continue;
            }
            let t0 = std::time::Instant::now();
            let said = super::ask_about_picture(
                name,
                &crate::snip::prompt_for("text"),
                &png,
                &crate::snip::shape_of("text").unwrap().to_string(),
            );
            eprintln!("{name} {:.1}s -> {said:?}", t0.elapsed().as_secs_f32());
            let said = said.unwrap_or_else(|e| panic!("{name} could not read it: {e:#}"));
            assert!(said.contains("\"text\""), "{name} did not answer in the required shape");
        }
    }

    /// Each installed assistant AI answers a short question the light way,
    /// without falling back to the ordinary one. Costs a call to each; run by
    /// hand with `cargo test -- --ignored every_installed_ai_answers_lightly`
    #[test]
    #[ignore]
    fn every_installed_ai_answers_lightly() {
        for (name, _, _) in super::AI_ENGINES {
            if crate::tab::resolve_command(name).is_none() {
                continue;
            }
            let t0 = std::time::Instant::now();
            let said = super::ask_light_once(name, "Reply with the single word PONG in capital letters.");
            eprintln!("{name} {:.1}s -> {said:?}", t0.elapsed().as_secs_f32());
            let said = said.unwrap_or_else(|e| panic!("{name} could not be asked lightly: {e:#}"));
            assert!(said.contains("PONG"), "{name} did not answer what it was asked");
        }
    }

    /// Claude Code's answer is the last event's, and an error it reports is an
    /// error, not an answer
    #[test]
    fn claude_codes_answer_is_read_out_of_its_events() {
        let out = "{\"type\":\"system\"}\n{\"type\":\"result\",\"is_error\":false,\"result\":\" 貸借対照表 \"}\n";
        assert_eq!(super::picture_answer("claude", out).unwrap(), "貸借対照表");
        let shaped = "{\"type\":\"result\",\"is_error\":false,\"result\":\"x\",\"structured_output\":{\"text\":\"貸借\"}}";
        assert_eq!(super::picture_answer("claude", shaped).unwrap(), "{\"text\":\"貸借\"}", "the shaped answer is not read");
        let bad = "{\"type\":\"result\",\"is_error\":true,\"result\":\"limit\"}";
        assert!(super::picture_answer("claude", bad).is_err());
        assert!(super::picture_answer("claude", "not json").is_err());
        assert_eq!(super::picture_answer("codex", " text \n").unwrap(), "text");
    }
    use super::*;

    /// A repository's own settings are offered in exactly one place: the
    /// project's page.
    ///
    /// The devcontainer and the setup belong to the repository, so the folders
    /// of one project -- its own checkout and each worktree -- must not each
    /// offer them; that is several places to change one fact, and the last one
    /// pressed wins. Every folder page points at the project instead.
    #[test]
    fn the_repositorys_own_settings_are_offered_where_the_repository_is() {
        assert!(
            PAGE.contains("if ((p.at || \"\").trim()) box.append(ignoreCard(desk, p), extraFilesCard(desk, p), envCard(desk, p));"),
            "the cards that belong to the repository are not on the project's page"
        );
        assert!(
            PAGE.contains("if (home) box.insertBefore(elsewhereCard(desk, home), buttons);"),
            "a folder's page does not point at its project"
        );
        assert!(
            !PAGE.contains("box.insertBefore(envCard("),
            "a folder page still offers the repository's settings itself"
        );
        // Projects are a desk's own, and the page asks about them by desk
        assert!(PAGE.contains("projects: Array.isArray(w.projects) ? w.projects : [],"));
        assert!(PAGE.contains("\"&desk=\" + encodeURIComponent(desk.id || \"\")"));
    }

    /// Every trigger this screen offers is a trigger that actually fires.
    ///
    /// The two lists live in different languages -- `HOOK_NAMES` in Rust says
    /// what the engine will call, `TAB_EVENTS` in the page's script says what
    /// a person may write -- and nothing but this joins them. An entry the
    /// page offers and the engine never calls is a box to type code into that
    /// silently never runs, which is worse than not offering it at all; an
    /// ending the engine can reach with no entry here can only be handled by
    /// editing a file by hand.
    /// `sel` is the page's one record of what is chosen, and every pane reads
    /// it. A local of the same name anywhere in a function hides it for the
    /// whole of that block, and a read of `sel.desk` before the local's line
    /// throws -- the pane is left blank with nothing on screen to say why.
    /// Adding a tab did exactly that once: the notification card named its
    /// dropdown `sel`, and every AI tab's pane came up empty.
    #[test]
    fn nothing_on_the_settings_page_hides_the_selection() {
        let shadows: Vec<&str> = ["const sel ", "let sel ", "var sel ", "const sel=", "let sel=", "(sel)", "(sel,", " sel =>", ", sel)"]
            .into_iter()
            .filter(|p| PAGE.matches(p).count() > usize::from(*p == "let sel "))
            .collect();
        assert!(PAGE.contains("let sel = {"), "the selection is no longer where this looks for it");
        assert!(shadows.is_empty(), "something on the page is also called sel: {shadows:?}");
    }

    #[test]
    fn every_trigger_the_screen_offers_is_one_the_engine_fires() {
        let block = PAGE
            .split("const TAB_EVENTS = [")
            .nth(1)
            .and_then(|r| r.split("];").next())
            .expect("TAB_EVENTS is gone from the screen");
        let offered: Vec<&str> = block
            .lines()
            .filter_map(|l| l.trim().strip_prefix("[\""))
            .filter_map(|l| l.split('"').next())
            .collect();
        assert!(offered.len() >= 8, "the list of triggers is too short: {offered:?}");
        for name in &offered {
            if *name == "_shared" {
                continue; // not a trigger: shared helpers the others call
            }
            assert!(
                crate::hooks::HOOK_NAMES.contains(name),
                "{name} is on the screen, but the engine never calls it"
            );
        }
        // And every way a turn can end is reachable from here
        for state in [
            crate::detect::TabState::Done,
            crate::detect::TabState::Failed,
            crate::detect::TabState::Limit,
            crate::detect::TabState::Background,
        ] {
            let hook = crate::hooks::ending_hook(state);
            assert!(
                offered.contains(&hook),
                "for {}, the hook {hook} cannot be chosen on the screen",
                state.label()
            );
        }
    }

    /// A server tab's settings survive being saved from this screen.
    ///
    /// The page reads the settings into a flat list and writes them back from
    /// a fixed set of fields. A field left out of either half is not an error
    /// anywhere: it simply disappears the next time somebody presses Save, and
    /// the connection that worked yesterday cannot find its key. The screen
    /// where a person edits their settings must not be the thing that loses
    /// them, so both halves are checked here by name
    #[test]
    fn a_desks_git_accounts_survive_a_reload_and_a_save() {
        assert!(
            PAGE.contains("git_accounts: Array.isArray(w.git_accounts) ? w.git_accounts : [],"),
            "reading drops the git accounts, so the page shows none and a save erases them"
        );
        assert!(PAGE.contains("if (accts.length) o.git_accounts = accts;"), "writing drops the git accounts");
    }

    /// An account is added from the picker that wanted one.
    ///
    /// Everything about a git account was on the desk's own page, and the
    /// place a person holds a token is the project's page, where the picker
    /// only offers what already exists. The last line of the picker opens the
    /// desk's list over the page, and closing it comes back to the picker with
    /// the account that was made
    #[test]
    fn a_token_can_be_added_from_the_picker_that_wants_one() {
        assert!(
            PAGE.contains(r#"s.append(el("option", {value:ADD_ACCOUNT}, T["settings.gitacct.add_here"]));"#),
            "the picker has no way to add an account"
        );
        assert!(PAGE.contains("function gitAccountsWindow(desk, done)"), "there is no window to add one in");
        // The same list in both places, so a way of adding an account cannot
        // appear on the desk's page and not in the window
        assert_eq!(
            PAGE.matches("...gitAccountsParts(desk)").count(),
            2,
            "the window and the desk's page draw the accounts from different code"
        );
        // Whatever the window is closed by, the picker reads the list again
        assert!(
            PAGE.contains(r#"back.addEventListener("mousedown", e => { if (e.target === back) done(); });"#),
            "closing the window by the backdrop leaves the picker showing the old list"
        );
        // A name a person can type is letters, digits, _ and -, so the line
        // that adds one can never be mistaken for an account
        assert!(PAGE.contains(r#"const ADD_ACCOUNT = "@add";"#));
        assert!(!crate::config::valid_secret_name("@add"));
    }

    /// The token field says what will become of the token.
    ///
    /// It used to say it was kept encrypted, full stop. That is true only
    /// where somebody has set a master password; without one the store is
    /// written as it stands. The screen now says which of the two this is,
    /// and says the unsafe one in the colour for danger
    #[test]
    fn the_token_field_says_when_nothing_is_encrypting_it() {
        assert!(PAGE.contains(r#"T["settings.gitacct.token_plain"]"#), "nothing is said about a store with no password");
        assert!(
            PAGE.contains(r#"const plain = storeMode === "plaintext" || storeMode === "empty";"#),
            "a store yet to be written is not counted as one with no password"
        );
        let said = crate::i18n::t("settings.gitacct.token_hint");
        assert!(
            !said.to_lowercase().contains("encrypt"),
            "the hint promises encryption that only a master password provides: {said}"
        );
    }

    #[test]
    fn the_issue_tabs_settings_button_opens_the_project() {
        assert!(
            PAGE.contains(r#"if ((sec === "project" || sec === "project-gitacct") && want && desks[cur]) {"#),
            "there is no link to a project's page"
        );
        // A refusal about the account lands on the card that chooses it, not
        // at the top of a long page with the card to be found somewhere below
        assert!(PAGE.contains(r#"acctCard.id = "project-gitacct";"#), "the account card has no address");
        assert!(
            PAGE.contains(r#"PC_ACCOUNTS_READ.then(() => lookAtCard("project-gitacct", 50))"#),
            "the link does not bring the card into view"
        );
        // The folder as the settings spell it, not as the disk answers about
        // it: a project on a mapped network drive is written down under the
        // drive letter, and the page it is on cannot be found by the path the
        // drive points at
        assert!(
            crate::shell::page().contains(r#"openSettings("project-gitacct", true, proj.at || proj.dir)"#),
            "the Issue tab sends people to the folder instead of its project's account"
        );
    }

    /// The words between one text and the next quote, wherever the text appears.
    fn named_after(text: &str, pat: &str) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        let mut at = 0;
        while let Some(i) = text[at..].find(pat) {
            let from = at + i + pat.len();
            let Some(len) = text[from..].find('"') else { break };
            out.insert(text[from..from + len].to_string());
            at = from + len;
        }
        out
    }

    /// The body of a function or an object literal, by the line it starts on.
    fn body_of<'a>(text: &'a str, opens: &str, closes: &str) -> &'a str {
        let at = text.find(opens).unwrap_or_else(|| panic!("{opens} is gone"));
        let from = at + opens.len();
        let len = text[from..].find(closes).unwrap_or_else(|| panic!("{opens} does not end"));
        &text[from..from + len]
    }

    /// Every screen the board sends somebody to is a screen this page has.
    ///
    /// A deep link is a word: the board hands `?section=<word>` over and this
    /// page looks it up. Nothing fails when the word is not found -- the page
    /// opens on the general cards, and whoever pressed "edit this" is left
    /// looking for what they pressed. So a card renamed or taken away here,
    /// while the board goes on naming it, breaks a button silently and in the
    /// other file. This is the one thing that says so.
    ///
    /// It reads both pages rather than holding a list of its own: what the
    /// board asks for is every `openSettings("...")`, plus the two fields whose
    /// value is handed to that same function (a row's `settings:` and the git
    /// panel's `edit:`). A new field carrying a section has to be named here.
    #[test]
    fn every_screen_the_board_asks_for_is_one_the_settings_have() {
        let board = crate::shell::page();
        let mut asks = named_after(&board, "openSettings(\"");
        asks.extend(named_after(&board, "settings: \""));
        asks.extend(named_after(&board, "edit:\""));
        assert!(asks.len() > 5, "the board's links are no longer being found: {asks:?}");

        // What this page can be asked for: a card of its own, a desk's card by
        // the name the board uses for it, or a project's page
        let global = named_after(body_of(PAGE, "function globalSections() {", "\n}"), "{id:\"");
        let desk_ids = named_after(body_of(PAGE, "function deskSections(desk) {", "\n}"), "s(\"");
        let links = body_of(PAGE, "const DESK_LINKS = {", "};");
        let desk_links: Vec<(String, String)> = links
            .split(',')
            .filter_map(|kv| kv.split_once(':'))
            .map(|(k, v)| {
                (k.trim().trim_matches('"').to_string(), v.trim().trim_matches('"').to_string())
            })
            .collect();
        assert!(!global.is_empty() && !desk_ids.is_empty() && !desk_links.is_empty(),
            "the settings' own lists are no longer being found");
        // The two names that are neither: a project's page, and the card on it
        let project = PAGE
            .contains(r#"if ((sec === "project" || sec === "project-gitacct") && want && desks[cur]) {"#);
        assert!(project, "the project's own page is no longer reachable by name");

        for ask in &asks {
            let known = global.contains(ask)
                || desk_links.iter().any(|(k, _)| k == ask)
                || ask == "project"
                || ask == "project-gitacct";
            assert!(known, "the board sends people to \"{ask}\", which is no screen these settings have");
        }
        // ...and the desk's own cards, which those names point at
        for (k, v) in &desk_links {
            assert!(desk_ids.contains(v), "\"{k}\" points at the desk card \"{v}\", which is gone");
        }
        // A link that marks one field brings that field into view by its id,
        // so the id has to be given out somewhere as well as asked for here
        for id in named_after(PAGE, "lookAtCard(\"") {
            let given = PAGE.matches(&format!("\"{id}\"")).count();
            assert!(given > 1, "nothing on the page is called \"{id}\", so the link marks nothing");
        }
    }

    #[test]
    fn a_server_tabs_settings_survive_a_save() {
        assert!(PAGE.contains("server: t.server || null"), "reading drops the connection settings");
        assert!(
            PAGE.contains(r#"for (const k of ["key", "jump", "keepalive", "file_command", "remote_dir"])"#),
            "writing drops the connection settings"
        );
        // A tab that is not a server does not get an empty block, and a
        // bastion nobody named is not written down as one
        assert!(PAGE.contains("if (!Object.keys(node.server).length) delete node.server;"),
                "it writes out connection settings with nothing in them");
        assert!(PAGE.contains(r#"if (!(v.host || "").trim()) continue;"#), "it writes out a jump host with no name");
    }

    /// Conversational text must never be typed into a terminal — only what
    /// the markers (or a lone fence) carry gets through
    #[test]
    fn suggested_commands_come_only_from_markers() {
        assert_eq!(
            extract_cmd("環境はUbuntuと推定します。\n<<<CMD\nfree -h\n>>>\n以上です").unwrap(),
            "free -h"
        );
        assert_eq!(extract_cmd("```bash\nfree -h\n```").unwrap(), "free -h");
        assert!(extract_cmd("メモリを見るには free -h を使います").is_err(), "plain prose is refused");
        assert!(extract_cmd("<<<CMD\n\n>>>").is_err(), "an empty suggestion is refused");
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
        let dir = crate::test_temp("userjson");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let _ = std::fs::remove_file(&path);

        // Missing: hand over the empty shape, no fuss.
        match read_user_json(&path, "{}") {
            UserJson::Text(t) => assert_eq!(t, "{}", "a file not made yet is handed over as an empty shape"),
            UserJson::Refused(..) => panic!("it must not refuse merely because the file is not made yet"),
        }

        // Fine: hand it over verbatim, so unknown keys and "//" notes survive.
        let good = "{\n  \"//note\": \"kept\",\n  \"max_chain\": 10\n}";
        std::fs::write(&path, good).unwrap();
        match read_user_json(&path, "{}") {
            UserJson::Text(t) => assert_eq!(t, good, "a file that was read is handed over exactly as written"),
            UserJson::Refused(..) => panic!("valid JSON was refused"),
        }

        // Broken: refuse, and say where. Naming the line is the point — a bare
        // "failed to load" would leave someone hunting through their own file.
        let bad = "{\n  \"name\": \"実装\",\n  \"cwd\": \"D:\\very\"\n}";
        std::fs::write(&path, bad).unwrap();
        let UserJson::Refused(status, body) = read_user_json(&path, "{}") else {
            panic!("broken JSON got through");
        };
        assert_eq!(status, 409);
        assert_eq!(body["ok"], serde_json::json!(false));
        assert_eq!(body["line"], serde_json::json!(3), "it does not point at the broken line");
        assert!(body["column"].as_u64().unwrap() > 0, "it does not point at the broken column");
        assert_eq!(
            body["text"],
            serde_json::json!(bad),
            "without the original text, the screen cannot show the line in question"
        );
        assert!(
            body["path"].as_str().unwrap().ends_with("config.json"),
            "it is not clear which file this is about"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The connection link may reach the clipboard, never the screen. Printing
    /// it without the token showed an address that opens nothing; printing it
    /// with the token puts the key to the machine where a camera can see it.
    #[test]
    fn the_phone_card_hands_the_link_over_rather_than_printing_it() {
        let from = PAGE.find("function remoteCard()").expect("there is no remoteCard");
        let len = PAGE[from..].find("function aiSelect()").expect("the card has no end");
        let card = &PAGE[from..from + len];
        assert!(card.contains("/api/remote/url"), "there is no place to take the URL for copying");
        assert!(card.contains("netBadge"), "there is no badge for which network it connects to");
        assert!(card.contains("copyText("), "it does not go through the shared clipboard path");
        assert_eq!(
            card.matches("j.origin").count(),
            1,
            "origin only decides whether there is anything to show. It must not be drawn on screen"
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
        let from = PAGE.find("function actionsCard()").expect("there is no actionsCard");
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
                    "it writes while drawing: {line}"
                );
            }
        }
    }

    /// Saving the settings keeps what a folder says that this screen never
    /// draws.
    ///
    /// The screen loaded a folder as its name, id and folder, and saved it as
    /// those plus two keys it knew of. The app writes more onto a folder -- the
    /// machine it is on, the project it is a piece of, where it came from -- and
    /// every save erased them: a folder on a server became a folder here, and a
    /// project's setup command stopped reaching its worktrees.
    #[test]
    fn a_save_keeps_what_a_folder_says_that_this_screen_does_not_show() {
        let read = PAGE.find("function readFolders(").expect("there is no readFolders");
        let read = &PAGE[read..read + 400];
        assert!(!read.contains("({name:f.name, id:f.id, cwd:f.cwd})"), "it keeps only the keys decided when reading");
        let of = PAGE.find("function foldersOf(").expect("there is no foldersOf");
        assert!(PAGE[of..of + 600].contains("Object.assign({}, f,"), "reading drops keys");
        let out = PAGE.find("const foldersOut = ").expect("there is no foldersOut");
        let out = &PAGE[out..out + 900];
        assert!(out.contains("Object.assign({}, g)"), "saving writes only the keys it knows");
        assert!(!out.contains("const o = {};"), "saving writes only the keys it knows");
    }

    /// A folder is taken out of the list with the tabs standing in it, rather
    /// than refused until somebody empties it first.
    ///
    /// Emptying it first was not a thing anybody could do: the tabs are
    /// written in these settings, so the next launch starts them again, and
    /// the folder was never tab-less for long enough to be taken out. The
    /// refusal was a wall with no door in it -- one folder, on the list
    /// forever. The lines go with the folder now, and the press asks first
    #[test]
    fn a_folder_goes_with_its_tabs_rather_than_waiting_to_be_emptied() {
        assert!(!PAGE.contains("if (tabsHere().length) { toast("),
            "the folder is still refused while it has tabs");
        assert!(PAGE.contains("desk.tabs = (desk.tabs || []).filter(t => (t.group || 0) !== gi);"),
            "the tabs standing in the folder are left behind");
        // ...and they are left behind pointing at whatever moved up into its
        // place, which is the bug the refusal was standing in front of
        assert!(PAGE.contains("desk.folders.splice(gi, 1);
    (desk.tabs || []).forEach(t => { if ((t.group || 0) > gi) t.group--; });"),
            "the folders below it are not renumbered");
        assert!(PAGE.contains(r#"fill(T["settings.group.delete_confirm"],"#),
            "a folder with tabs in it goes without a word");
        assert!(PAGE.contains(r#"if ((desk.folders || []).length <= 1) { toast(T["settings.group.last"], true); return false; }"#),
            "the last folder of a desk can be taken away");
    }

    #[test]
    fn what_a_model_wraps_its_answer_in_is_not_part_of_the_answer() {
        assert_eq!(strip_fence("one\ntwo"), "one\ntwo", "plain in, plain out");
        assert_eq!(
            strip_fence("Here it is:\n```rust\nfn main() {}\n```\nhope that helps"),
            "fn main() {}",
            "only what is inside the fence is the answer"
        );
        // An opening fence with nothing closing it still gives up its contents
        assert_eq!(strip_fence("```\nline\n"), "line");
    }

    /// The settings screen offers the languages this program has, and no
    /// others. Two lists that drift apart mean a language that can be chosen
    /// and has nothing behind it, or one that exists and cannot be reached
    #[test]
    fn the_settings_offer_exactly_the_languages_there_are() {
        let page = super::PAGE;
        for (code, called, _) in crate::i18n::LANGUAGES {
            assert!(
                page.contains(&format!("[\"{code}\", \"{called}\"]")),
                "the settings do not offer {code} ({called})"
            );
        }
        // ...and offer no others. A code the picker has and this list does
        // not is a language somebody can choose with nothing behind it
        let picker = page
            .split("choose(current, \"language\"")
            .nth(1)
            .and_then(|rest| rest.split("]),").next())
            .expect("the language picker has moved");
        for line in picker.lines() {
            let Some(rest) = line.trim().strip_prefix("[\"") else { continue };
            let Some((code, _)) = rest.split_once('\"') else { continue };
            // The empty code is "follow the machine", not a language
            if code.is_empty() {
                continue;
            }
            assert!(
                crate::i18n::LANGUAGES.iter().any(|(c, _, _)| *c == code),
                "the settings offer {code}, which this program has no words for"
            );
        }
    }

    #[test]
    fn manual_is_embedded_and_usable() {
        // The spec handed to the AI must be obtainable no matter where it's launched from (regardless of language)
        for (code, text) in EMBEDDED_MANUALS {
            assert!(text.contains("shikisha.send_to_tab"), "the spec for {code} is empty");
        }
        let m = load_manual(std::path::Path::new("/nonexistent/config.json"));
        assert!(m.contains("shikisha."), "it falls back to the embedded one");
    }

    /// The spec must explain every event the screen offers as a choice.
    ///
    /// This spec is passed to the AI as-is. If asked for an event it doesn't cover,
    /// the AI will honestly and correctly reply "that's not in the spec, so I won't do anything."
    /// If a feature is added but the spec isn't updated, that feature stays invisible to the AI
    /// Every assistant AI this program starts for a one-off question is
    /// started with its hands tied.
    ///
    /// Not a style rule. Asked an ordinary question in an ordinary folder, an
    /// assistant AI reads it as a job: asked why the program had stopped, one
    /// went and edited code and then reported that it had fixed it. Nothing
    /// this program asks is a job -- every one of these is a question whose
    /// answer is already in the prompt -- so each CLI is given the strongest
    /// "read only, no tools" it has.
    ///
    /// Checked by name, per program, so that adding a fourth AI fails here
    /// rather than quietly shipping one that can act
    #[test]
    fn a_one_off_question_never_hands_the_ai_any_tools() {
        // What each program has to be told, in its own words
        let required: [(&str, &[&str]); 3] = [
            ("claude", &["--tools", "--no-session-persistence", "--strict-mcp-config", "--disable-slash-commands"]),
            ("codex", &["--sandbox", "read-only", "--ephemeral"]),
            ("gemini", &["--approval-mode", "plan", "--extensions", "none"]),
        ];
        for (name, must) in required {
            let (args, _) = super::light_invocation(name, false, None)
                .unwrap_or_else(|| panic!("{name} has no confined way to be asked"));
            for want in must {
                assert!(
                    args.iter().any(|a| a == want),
                    "{name} is asked without {want}: {args:?}"
                );
            }
        }
        // ...and every AI this program knows about is in that list. A new one
        // added to AI_ENGINES and forgotten here would be the one with tools
        for (name, _, _) in super::AI_ENGINES {
            assert!(
                required.iter().any(|(n, _)| *n == name),
                "{name} can be started but has no confined way written for it"
            );
        }
    }

    /// The picture tools take the same care. A screenshot is handed over with
    /// a question about it, which is no more a job than any other question
    #[test]
    fn asking_about_a_picture_hands_over_no_tools_either() {
        let png = [0u8; 4];
        for (name, must) in [
            ("claude", vec!["--tools"]),
            ("codex", vec!["--sandbox", "read-only"]),
            ("gemini", vec!["--approval-mode", "plan"]),
        ] {
            let (args, _, _) = super::picture_invocation(name, "what does this say", &png, "{}")
                .unwrap_or_else(|| panic!("{name} cannot be asked about a picture"));
            for want in must {
                assert!(args.iter().any(|a| a == want), "{name}: {args:?}");
            }
        }
    }

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
                    "the spec for {code} does not explain {event} (the AI writes without knowing this event)"
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
                "{name}: `{n}` is declared twice at the top level (the whole script stops working)"
            );
        }
    }

    /// Every word a new tab's automation name can be drawn as is one automation
    /// accepts as it stands, and there are enough of them that a desk of tabs
    /// does not run out.
    #[test]
    fn a_new_tab_is_named_from_words_automation_accepts() {
        let words = crate::config::pet_nouns();
        assert!(words.len() >= 100, "too few words to draw from: {}", words.len());
        for w in &words {
            assert!((3..=6).contains(&w.len()), "{w} is not a short word");
            assert!(w.bytes().all(|b| b.is_ascii_lowercase()), "{w} is not a name automation takes as it is");
            // The name the settings screen settles on for it is itself
            assert_eq!(crate::config::slug_id(w), *w, "{w} would be tidied into something else");
            assert!(!crate::config::NOT_A_TAB_NAME.contains(w), "{w} is on the list of words a tab is not called");
        }
        let json: Vec<String> = serde_json::from_str(&super::pet_nouns_json()).expect("the list is not JSON");
        assert_eq!(json.len(), words.len());
    }

    /// A tab added from the settings is named by what it runs and called by a
    /// word of its own -- not by its command. Called by its command, a tab that
    /// started as Claude and was turned into SSH went on being "claude-2".
    #[test]
    fn a_new_tab_is_named_by_what_it_runs_and_called_by_a_word() {
        let add = PAGE.split("function addTabTo(desk, group) {").nth(1).expect("addTabTo is gone");
        let add = &add[..add.find("\n}\n").expect("addTabTo does not end")];
        assert!(add.contains("t.name = kindName(command);"), "a new tab is not named by what it runs");
        assert!(add.contains("t.id = petId(desk, t);"), "a new tab is not called by a word of its own");
        assert!(!add.contains("inferredTabId"), "a new tab is called by its command again");
        // The name follows the kind while nobody has typed their own
        assert!(PAGE.contains("followKind(t, before);"), "changing what a tab runs no longer carries its name along");
        // One card for what a tab runs, on its page and in the dialog alike
        assert_eq!(PAGE.matches("append(launchCard(t").count(), 2, "what a tab runs is asked in more or fewer places than two");
    }

    /// The board's + asks only what the new tab runs, in a dialog, and a save
    /// that failed leaves the dialog there with the reason.
    #[test]
    fn the_board_plus_asks_one_question_in_a_dialog() {
        assert!(PAGE.contains(r#"if (q.get("float") === "1") { render(); enterFloat(wi, sel.tab); return; }"#),
            "the board's + opens the whole settings page again");
        assert!(PAGE.contains("<div id=\"floatbox\""), "there is no dialog to show");
        assert!(PAGE.contains("if (await save()) closeSettings();"), "adding closes the dialog whether or not it was saved");
        assert!(PAGE.contains("ok = await doSave();"), "a save that said why it failed still counts as done");
        assert!(PAGE.contains("if (!floating) goIndex();"), "adding a tab from its dialog sends the person to INDEX");
        assert!(PAGE.contains(r#"postMessage(JSON.stringify({kind:"settingsfull"}))"#),
            "More settings does not ask the window for the whole of it");
        // A tab added to a group with no folder would wait instead of starting,
        // so the dialog asks for the folder while the tab is being made
        assert!(PAGE.contains(r#"if (g && !(g.cwd || "").trim()) {"#),
            "the dialog adds a tab to a group with nowhere to work and asks nothing");
    }

    /// The settings screen's script is JavaScript a browser can actually parse.
    ///
    /// It is one script, so one syntax error anywhere in it takes the whole
    /// settings screen down at once: nothing is defined, no card is drawn, and
    /// what is left is an empty window with no way to fix the settings that
    /// opened it. Every other test here reads the page as text, and text
    /// cannot tell a broken expression from a fine one, so this one hands it
    /// to a parser. Node does the parsing, because no crate here parses
    /// JavaScript and node is on both CI runners; where there is none it says
    /// so rather than pretending to have checked
    #[test]
    fn the_settings_script_is_javascript_a_browser_can_parse() {
        let html = crate::i18n::render(&themed(PAGE.to_string()))
            .replace("__TOKEN__", "t")
            .replace("__REMOTE__", "false")
            .replace("__HOTKEYS__", &crate::hotkeys::catalog_json())
            .replace("__QUICK__", &quick_json())
            .replace("__DICT__", "{}")
            .replace("__GRANTS__", "[]")
            .replace("__GITLUA__", "\"\"")
            .replace("__PROTECT__", "[]")
            .replace("__THISPC__", "\"@pc\"")
            .replace("__PETNOUNS__", "[]")
            .replace("__MD__", "\"\"");
        let mut script = String::new();
        let mut rest = html.as_str();
        while let Some(at) = rest.find("<script>") {
            rest = &rest[at + "<script>".len()..];
            let Some(end) = rest.find("</script>") else { break };
            script.push_str(&rest[..end]);
            script.push('\n');
            rest = &rest[end..];
        }
        assert!(script.len() > 10_000, "the script could not be pulled out of the page: {} characters", script.len());
        let file = std::env::temp_dir().join(format!("shikisha-settings-{}.js", std::process::id()));
        std::fs::write(&file, &script).expect("could not write the script out");
        let checked = std::process::Command::new("node").arg("--check").arg(&file).output();
        let _ = std::fs::remove_file(&file);
        match checked {
            Ok(done) => assert!(
                done.status.success(),
                "the settings script dies whole on a syntax error:\n{}",
                String::from_utf8_lossy(&done.stderr)
            ),
            Err(e) => eprintln!("node is missing, so the syntax check did not run ({e}). It runs in CI"),
        }
    }

    /// The same dialog, framed over a board in a browser (?embed=1).
    ///
    /// A browser has no WebView to place this page in, so the board frames it
    /// instead. Framed, the way out is a word to the board: walking to "/"
    /// would load the board inside the frame, and the board is already there
    /// behind it.
    #[test]
    fn the_dialog_framed_over_a_board_talks_to_the_board() {
        assert!(PAGE.contains(r#"const EMBED = new URLSearchParams(location.search).get("embed") === "1";"#),
            "the page cannot tell it is framed");
        assert!(PAGE.contains(r#"const toBoard = act => { try { window.parent.postMessage({cfg:act}, location.origin); } catch (e) {} };"#),
            "there is no way to answer the board, or it answers any origin");
        assert!(PAGE.contains(r#"else if (EMBED) toBoard("close");"#),
            "closing a framed dialog loads the board inside the frame");
        assert!(PAGE.contains(r#"else if (EMBED) toBoard("full");"#),
            "More settings leaves the whole of the settings in a dialog-sized frame");
        // The frame draws the edge; a second one inside reads as a box in a box
        assert!(PAGE.contains(" body.embed.float #floatbox { border:0; }"),
            "the framed page draws an edge inside the frame's own");
        assert!(PAGE.contains(r#"if (EMBED) document.body.classList.add("embed");"#),
            "nothing says the page is framed before it is painted");
        // A sheet is a dialog one size up, so Escape is its way out -- and
        // this page is what hears that key, in the window as in a frame
        assert!(PAGE.contains(r#"const SHEET = new URLSearchParams(location.search).get("sheet") === "1";"#),
            "the page cannot tell it is standing over a board");
        assert!(PAGE.contains(r#"if (SHEET) { e.preventDefault(); closeSettings(); }"#),
            "Escape does not put a sheet away");
        assert!(crate::shell::page().contains(r#"if (sheet) { p.sheet = "1"; openCfgLayer(p, "sheet"); } else walkToSettings(p);"#),
            "a browser's sheet is never told that it is one");
    }

    /// The screen must not still contain a raw `{{key}}` or `__DICT__`.
    /// A forgotten substitution would only be caught at runtime, so it's stopped here instead
    #[test]
    fn pages_are_fully_rendered() {
        for (name, page) in [("PAGE", PAGE), ("HELP_PAGE", HELP_PAGE), ("RESULT_PAGE", RESULT_PAGE)] {
            // Every page is coloured before it is worded, and each one says so
            // by asking for the block. A page that stopped asking would come up
            // with no colours defined and nothing would say why
            assert!(page.contains("{{THEME}}"), "{name} does not receive the colors");
            let html = crate::i18n::render(&themed(page.to_string()))
                .replace("__TOKEN__", "t")
                .replace("__REMOTE__", "false")
                .replace("__HOTKEYS__", &crate::hotkeys::catalog_json())
                .replace("__QUICK__", &quick_json())
                .replace("__DICT__", "{}")
                .replace("__GRANTS__", "[]")
                .replace("__GITLUA__", "\"\"")
                .replace("__PROTECT__", "[]")
                .replace("__THISPC__", "\"@pc\"")
                .replace("__PETNOUNS__", "[]")
                .replace("__MD__", "\"\"");
            // Checked on the finished page, not the template: the shared toast
            // is poured in on the way, and a page that kept a copy of one of
            // its names would only break once it was actually served
            assert_no_duplicate_bindings(name, &html);
            assert!(!html.contains("{{"), "an unfilled {{{{key}}}} is left in {name}");
            assert!(!html.contains("__"), "an unfilled placeholder is left in {name}");
            assert!(html.contains("<html lang=\"en\">"), "the lang attribute of {name}");
        }
    }

    /// A phone reaching the settings over the proxy must never make a window
    /// appear on the PC. The page is served without the buttons, and the
    /// endpoints behind them answer with a refusal instead of a dialog that
    /// would hold the request open until someone walked over to the PC.
    /// The screen that lists devices, exercised the way the page does it.
    ///
    /// The point of the whole book is that a device can be named and taken
    /// away on its own, so that is what this walks: two devices in, one named,
    /// one revoked, and the other still there afterwards.
    #[test]
    fn the_settings_page_can_name_and_revoke_one_device() {
        let _book = crate::clients::tests::OwnBook::new();
        let dir = std::env::temp_dir().join(format!("shikitest_{}", crate::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, "{}").unwrap();

        // What the running remote would hand over: a way to end what a revoked
        // device is holding. Recorded here rather than acted on, so the test
        // can say whether it was called
        let ended: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let heard = Arc::clone(&ended);
        let info = RemoteInfo {
            running: true,
            cut: Some(Arc::new(move |id: &str| heard.lock().unwrap().push(id.to_string()))),
            ..Default::default()
        };
        let ui = WebUi::start_with(
            cfg,
            Arc::new(std::sync::Mutex::new(info)),
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

        let (phone, _) = crate::clients::pair("").unwrap();
        let (laptop, laptop_key) = crate::clients::pair("laptop").unwrap();

        let listed = |agent: &ureq::Agent| -> serde_json::Value {
            let mut r = agent
                .get(&format!("{base}/api/remote/clients"))
                .header("X-Token", &token)
                .call()
                .unwrap();
            serde_json::from_str(&r.body_mut().read_to_string().unwrap()).unwrap()
        };

        // Both are listed, and no key is
        let seen = listed(&agent);
        let text = seen.to_string();
        assert!(text.contains(&phone.id) && text.contains(&laptop.id), "the devices are not listed: {text}");
        assert!(!text.contains(&laptop_key), "the key itself shows on the screen");
        assert!(!text.contains(&laptop.hash), "even the hash used for matching shows");

        // Naming one
        agent
            .post(&format!("{base}/api/remote/clients/name"))
            .header("X-Token", &token)
            .send(serde_json::json!({"id": phone.id, "name": "台所のiPad"}).to_string())
            .unwrap();
        assert!(listed(&agent).to_string().contains("台所のiPad"), "the name is not kept");

        // Taking one away: the row goes, the running remote is told, and the
        // other device is untouched
        agent
            .post(&format!("{base}/api/remote/clients/revoke"))
            .header("X-Token", &token)
            .send(serde_json::json!({"id": phone.id}).to_string())
            .unwrap();
        let after = listed(&agent).to_string();
        assert!(!after.contains(&phone.id), "it is not removed from the list: {after}");
        assert!(after.contains(&laptop.id), "it was removed by mistake: {after}");
        assert_eq!(
            ended.lock().unwrap().as_slice(),
            [phone.id.as_str()],
            "the key was taken away, but the screen that device is watching was not stopped"
        );

        ui.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    /// The phone chooses a folder from what is on the PC, not from memory. The
    /// walk is the sidebar's (uistate::BrowseState), reached from the settings
    /// screen, and it reads settings-style paths back the way they are written.
    #[test]
    fn a_phone_walks_the_pcs_folders_instead() {
        let dir = std::env::temp_dir().join(format!("shikitest_{}", crate::random_hex(8)));
        std::fs::create_dir_all(dir.join("scripts").join("inner")).unwrap();
        std::fs::write(dir.join("scripts").join("secrets.json"), "{}").unwrap();
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
        let walk = |body: &str| -> serde_json::Value {
            let mut r = agent
                .post(&format!("{base}/api/walk"))
                .header("X-Token", &token)
                .header(REMOTE_CLIENT_HEADER, "1")
                .header("Content-Type", "application/json")
                .send(body)
                .unwrap();
            serde_json::from_str(&r.body_mut().read_to_string().unwrap()).unwrap()
        };

        // The top is this computer: the person's own folder and the drives
        let top = walk(r#"{"path":""}"#);
        assert_eq!(top["ok"], serde_json::json!(true), "{top}");
        assert!(top["up"].is_null(), "the top has nowhere to go back to: {top}");
        assert!(!top["dirs"].as_array().unwrap().is_empty(), "the drives are not shown: {top}");
        assert_eq!(top["chosen"], serde_json::json!(""), "the top is not a place that can be chosen");

        // A settings-style relative path is read against the config folder, and
        // choosing the folder writes it back the same way
        let sub = walk(r#"{"path":"scripts"}"#);
        assert_eq!(sub["ok"], serde_json::json!(true), "{sub}");
        assert!(sub["at"].as_str().unwrap().ends_with("scripts"), "{sub}");
        assert_eq!(sub["chosen"], serde_json::json!("scripts"), "{sub}");
        let dirs = sub["dirs"].as_array().unwrap();
        assert_eq!(dirs.len(), 1, "{sub}");
        assert!(dirs[0].as_str().unwrap().ends_with("inner"));
        assert!(sub["files"].as_array().unwrap().is_empty(), "files got mixed into choosing a folder: {sub}");

        // Asked for files, the same folder lists them; walking onto one is the choice
        let with = walk(r#"{"path":"scripts","files":true}"#);
        let files = with["files"].as_array().unwrap();
        assert_eq!(files.len(), 1, "{with}");
        let file = files[0].as_str().unwrap().to_string();
        let picked = walk(&serde_json::json!({"path": file, "files": true}).to_string());
        assert_eq!(picked["file"], serde_json::json!(true), "{picked}");
        assert_eq!(picked["chosen"], serde_json::json!("scripts/secrets.json"), "{picked}");

        // A folder that cannot be read says why rather than showing nothing
        let gone = walk(&serde_json::json!({"path": dir.join("nowhere")}).to_string());
        assert_eq!(gone["ok"], serde_json::json!(true), "{gone}");
        assert!(gone["error"].as_str().map(|e| !e.is_empty()).unwrap_or(false), "{gone}");

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

    /// The setup splits the assistant AIs by whether this PC has them, in the
    /// settings' own order, and says which have a page to install from.
    #[test]
    fn the_first_start_setup_offers_what_is_installed_and_points_at_the_rest() {
        let ids = |v: &[crate::uistate::SetupAi]| v.iter().map(|a| a.id.clone()).collect::<Vec<_>>();
        let st = super::setup_state_of(|n| n == "codex", |n| n != "gemini");
        assert_eq!(ids(&st.installed), ["codex"]);
        assert!(!st.gh, "GitHub CLI is said to be here");
        assert!(super::setup_state_of(|n| n == "gh", |_| true).gh, "GitHub CLI is not found");
        assert_eq!(super::install_page("gh").as_deref(), Some("https://cli.github.com/"), "GitHub CLI's page is not its own");
        assert_eq!(ids(&st.missing), ["claude", "gemini"], "the rest is out of order or missing");
        assert!(st.missing[0].install && !st.missing[1].install, "a page is claimed that is not there");
        assert_eq!(st.installed[0].name, "Codex CLI", "not the name the settings call it");
        // Nothing installed: nothing to pick, all three to install
        let none = super::setup_state_of(|_| false, |_| true);
        assert!(none.installed.is_empty());
        assert_eq!(ids(&none.missing), ["claude", "codex", "gemini"]);
        // Every one of them has a page in the profiles that ship
        for (name, _, _) in super::AI_ENGINES {
            assert!(crate::profile::install_url_for(name).is_some(), "{name} has no install page");
        }
    }

    /// A git account can sign in with GitHub CLI: offered in the dialog, asks
    /// for no token, and is saved as such
    #[test]
    fn a_git_account_can_sign_in_with_github_cli() {
        assert!(PAGE.contains(r#"el("option", {value:"gh"}, T["settings.gitacct.by_gh"])"#), "gh is not offered");
        assert!(PAGE.contains("const tokenWhy = !ssh && !gh && !hasToken"), "a gh account is asked for a token");
        assert!(PAGE.contains(r#"else if (method.value === "gh") { it.method = "gh"; delete it.key; delete it.login; }"#),
            "a gh account is not saved as one");
        assert!(PAGE.contains(r#"if (method.value !== "gh" && tokenIn.value.trim())"#), "a gh account files a token");
    }

    /// A tab starts as the AI chosen in the setup, and with Yolo mode its
    /// flag is already in the command -- where the checkbox shows it
    #[test]
    fn a_new_tab_starts_as_the_chosen_ai_and_carries_yolo_in_its_command() {
        assert!(PAGE.contains("AI_CLIS.find(c => installed(c) && c.cmd === current.ai_engine)"),
            "the Assistant AI does not decide what a new tab runs");
        assert!(PAGE.contains(r#"const flag = current.yolo ? cliFlagOf(head) : "";"#),
            "Yolo mode does not reach a new tab");
        assert!(PAGE.contains(r#"check(current, "yolo", T["settings.yolo.label"])"#),
            "Yolo mode is not under Basic");
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
    fn desk_path_rejects_traversal() {
        let cfg = std::path::Path::new("C:/app/config.json");
        // Happy path
        assert!(safe_desk_path("/api/desk?file=desks/x.json", cfg).is_some());
        // Path traversal, absolute paths, and non-JSON are rejected
        assert!(safe_desk_path("/api/desk?file=../secrets.json", cfg).is_none());
        assert!(safe_desk_path("/api/desk?file=desks/../../x.json", cfg).is_none());
        assert!(
            safe_desk_path(
                &format!("/api/desk?file={}", crate::outside_path("x.json")),
                cfg
            )
            .is_none()
        );
        assert!(safe_desk_path("/api/desk?file=desks/x.lua", cfg).is_none());
        // URL-encoded .. is rejected too
        assert!(safe_desk_path("/api/desk?file=%2E%2E%2Fsecrets.json", cfg).is_none());
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
            "the modal closes on click instead of on press"
        );
        assert!(
            !page.contains(r#"back.addEventListener("click""#),
            "the way of closing on a background click is back"
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
        assert_eq!(status, 403, "without a token it is refused");

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
            "if validation fails, the original settings are kept"
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

//! The runtime: everything SHIKISHA does that a window is not needed for.
//!
//! Tabs and their pseudo consoles, the state detector, the Lua automation, git,
//! ssh, the servers a phone and the settings screen talk to -- none of it draws
//! anything. What a shell provides instead arrives as a trait
//! (`shikisha_shared::BrowserHost`, `Toasts`, `FilePicker`), so this builds and
//! runs with no shell at all.

pub mod agenthook;
pub mod api;
pub mod attach;
pub mod ball;
pub mod bridge;
pub mod browserstate;
pub mod caps;
pub mod config;
/// Windows's own pseudo console, and the copy of it we ship beside the exe.
/// A unix pty needs no such thing, so the module is not built there
#[cfg(windows)]
pub mod conpty;
pub mod crypto;
pub mod detect;
pub mod digest;
pub mod discover;
pub mod exchange;
pub mod folders;
pub mod git;
pub mod grants;
pub mod hooks;
pub mod host;
pub mod i18n;
pub mod instance;
pub mod job;
pub mod keymap;
pub mod keys;
pub mod lastsession;
pub mod layout;
pub mod limits;
pub mod mailbox;
pub mod migrate;
pub mod netaddr;
pub mod notify;
pub mod pr;
pub mod profile;
pub mod push;
pub mod pwa;
pub mod reader;
pub mod remote;
pub mod reply;
pub mod repo;
pub mod session_log;
pub mod runtime;
pub mod send;
pub mod serve;
pub mod sessionfind;
pub mod shell;
pub mod ssh;
pub mod tab;
pub mod tailscale;
pub mod theme;
pub mod toast;
pub mod uistate;
pub mod update;
pub mod usage;
pub mod vault;
pub mod view;
pub mod watch;
pub mod webui;
pub mod winpath;
pub mod workspace;
pub mod worktree;
pub mod ws;
pub mod wspack;

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

/// Keeps a child process from popping up a window.
///
/// Console apps like cmd.exe show a black window if launched quietly.
/// That would flash briefly every time a browser is opened, so it's suppressed from the start.
#[cfg(windows)]
pub fn detach_console(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

/// Nothing to suppress: a process started here has no console of its own to
/// flash, which is the whole of what the Windows version is for.
#[cfg(not(windows))]
pub fn detach_console(cmd: &mut std::process::Command) -> &mut std::process::Command {
    cmd
}

pub fn random_hex(bytes: usize) -> String {
    match random_bytes(bytes) {
        Some(buf) => buf.iter().map(|b| format!("{b:02x}")).collect(),
        None => "shikisha-fallback-token".into(),
    }
}

/// Randomness from the system, or nothing.
///
/// `None` rather than a fallback, because the callers that want raw bytes want
/// them for keys (src/push.rs), and a key made from a stand-in is worse than
/// no key at all: it works, so nobody looks at it again.
pub fn random_bytes(n: usize) -> Option<Vec<u8>> {
    use rand::TryRng as _;
    let mut buf = vec![0u8; n];
    rand::rngs::SysRng.try_fill_bytes(&mut buf).ok()?;
    Some(buf)
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

/// The desktop whose clipboard this is, if a shell is running.
static CLIPBOARD: std::sync::OnceLock<Box<dyn shikisha_shared::Clipboard>> =
    std::sync::OnceLock::new();

pub fn use_clipboard(c: Box<dyn shikisha_shared::Clipboard>) {
    let _ = CLIPBOARD.set(c);
}

pub(crate) fn clipboard() -> Option<&'static dyn shikisha_shared::Clipboard> {
    CLIPBOARD.get().map(|c| c.as_ref())
}

/// The shortest a fixed remote token may be. Below this it is guessable, and a
/// guessable token is the whole board
pub const FIXED_TOKEN_MIN: usize = 16;

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
pub fn resume_plan_of(id: Option<&str>) -> tab::Resume {
    match id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => tab::Resume::Id(tab::Session {
            id: id.to_string(),
            source: tab::SessionSource::Store,
        }),
        None => tab::Resume::Fresh,
    }
}

/// The repository this crate lives in.
///
/// Tests read what the app ships -- the profiles, the manual, the rally
/// template -- and cargo runs them from this package's own directory, which is
/// two below the repository. One place says how to get back up, so a test that
/// needs a shipped file does not have to know where it is being run from.
#[cfg(test)]
pub fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crates/core sits two below the repository")
        .to_path_buf()
}

/// Every Rust source file in the repository, wherever a crate keeps it.
///
/// The checks that read the source itself -- which words a page asks for, which
/// keys a profile names -- used to walk one `src` folder. There are several now,
/// and a file moving between crates must not quietly take itself out of a check.
#[cfg(test)]
pub fn source_files() -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let root = repo_root();
    let mut out = Vec::new();
    walk(&root.join("src"), &mut out);
    walk(&root.join("crates"), &mut out);
    out
}


//! The runtime: everything SHIKISHA does that a window is not needed for.
//!
//! Tabs and their pseudo consoles, the state detector, the Lua automation, git,
//! ssh, the servers a phone and the settings screen talk to -- none of it draws
//! anything. What a shell provides instead arrives as a trait
//! (`shikisha_shared::BrowserHost`, `Toasts`, `FilePicker`), so this builds and
//! runs with no shell at all.

pub mod ace;
pub mod addproject;
pub mod agenthook;
pub mod api;
pub mod asking;
pub mod askpass;
pub mod asks;
pub mod asktab;
pub mod cli;
mod cli_launch;
pub mod attach;
pub mod awake;
pub mod ball;
pub mod bridge;
pub mod browserstate;
pub mod caps;
#[cfg(test)]
mod http_security_tests;
#[cfg(test)]
mod browser_contract_tests;
pub mod charset;
pub mod cdp;
pub mod chrome;
pub mod clients;
pub mod closed;
pub mod config;
pub mod console;
pub mod convo;
/// Windows's own pseudo console, and the copy of it we ship beside the exe.
/// A unix pty needs no such thing, so the module is not built there
#[cfg(windows)]
pub mod conpty;
pub mod crypto;
pub mod detect;
pub mod diff;
pub mod downloads;
pub mod digest;
pub mod discover;
pub mod exchange;
pub mod faraway;
pub mod farlink;
pub mod farmissed;
pub mod pairing;
pub mod farterm;
pub mod fardaemon;
pub mod farterms;
pub mod farops;
pub mod keepipe;
pub mod localkeep;
pub mod termstate;
pub mod files;
pub mod folders;
pub mod git;
pub mod gitops;
pub mod github;
pub mod grants;
pub mod guest;
pub mod guide;
pub mod hooks;
pub mod adr;
pub mod ideas;
pub mod disk;
pub mod mdkit;
pub mod mdnew;
pub mod notes;
pub mod host;
pub mod i18n;
mod http;
pub mod urlcodec;
pub mod inherit;
pub mod inside;
pub mod instance;
pub mod job;
pub mod keeper;
pub mod keymap;
pub mod keys;
pub mod labels;
pub mod lastexit;
pub mod lastsession;
pub mod layout;
pub mod limits;
pub mod localpage;
pub mod loginpath;
pub mod mailbox;
pub mod mcp;
pub mod migrate;
pub mod netaddr;
pub mod notify;
pub mod orch;
pub mod pagejs;
pub mod pagelint;
pub mod pageops;
pub mod pick;
pub mod paste;
pub mod placed;
pub mod pr;
pub mod pressure;
pub mod profile;
pub mod push;
pub mod pwa;
pub mod quick;
pub mod reader;
pub mod readview;
pub mod remote;
#[cfg(windows)]
pub mod reserve;
/// Standing by in the notification area with no window to be seen in. What a
/// runtime split from its window is, between one window and the next
#[cfg(windows)]
pub mod resident;
pub mod revive;
pub mod reply;
pub mod repo;
pub mod secretscan;
pub mod session_log;
pub mod runtime;
pub mod send;
pub mod serve;
pub mod serverai;
pub mod sessionfind;
pub mod sqlite;
pub mod shell;
pub mod skill;
pub mod hotkeys;
pub mod snip;
pub mod source;
pub mod devcontainer;
pub mod e2b;
pub mod elsewhere;
pub mod microvm;
/// The runtime half of a program split in two on one machine: it keeps the
/// work, and the window that draws it is another process entirely
#[cfg(windows)]
pub mod split;
/// The arrangement each split row owns. One is on screen at a time: the one
/// belonging to the row in front
pub mod splits;
pub mod ssh;
pub mod tab;
pub mod tailscale;
pub mod termlink;
pub mod theme;
pub mod toast;
/// The icon in the notification area. Windows' own, and the only place a
/// runtime with no window of its own can be seen or ended from
#[cfg(windows)]
pub mod tray;
pub mod transfer;
pub mod trust;
pub mod tunnel;
pub mod uistate;
pub mod update;
pub mod usage;
pub mod vault;
pub mod view;
pub mod watch;
pub mod webui;
pub mod winpath;
pub mod desk;
pub mod deskpack;
pub mod vaudio;
/// The Rust view of libvpx. Only where there is one to link against.
#[cfg(all(not(windows), feature = "vp8"))]
pub mod vpx;
pub mod vcast;
pub mod vencode;
pub mod vframe;
pub mod webrtc;
pub mod worktree;
pub mod foldercare;
pub mod ws;

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

/// Said on the command line for a copy that somebody else is working beside:
/// the window opens at the bottom of the pile and never puts itself in front.
pub const BEHIND: &str = "--behind";

/// Whether this process was started `--behind`.
///
/// A copy started to be driven through its pipe and its DevTools port is
/// started from a terminal the person at the machine is typing into, and
/// Windows lets the child of the window in front take the front for itself:
/// the typing went into the copy. Started this way, the window never asks for
/// the front and never takes the keyboard on its own. A person who clicks it
/// has it like any other window, and while they do, it behaves like one.
pub fn stays_behind() -> bool {
    static AT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AT.get_or_init(|| std::env::args().skip(1).any(|a| a == BEHIND))
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
    // Authentication must fail closed if the system cannot supply entropy.
    // A fixed substitute would open every server using it to anyone.
    hex_token(random_bytes(bytes))
}

fn hex_token(bytes: Option<Vec<u8>>) -> String {
    bytes.expect("system random source unavailable; refusing to create a token")
        .iter().map(|b| format!("{b:02x}")).collect()
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

/// JSON made safe to stand inside a page's `<script>`: the same value, with
/// every character that could end the script element or start markup written
/// as a JSON escape.
///
/// A page pours values into its script as JSON (`const X = __X__;`), and JSON
/// does not escape `<`: a string holding `</script>` -- a project's name, a
/// word in a language file, a line of the manual -- closed the element and
/// whatever came after it ran as the page's own code, with the page's key to
/// the app. `<`, `>` and `&` are only ever inside strings in JSON, where
/// `<` reads back as the same character; U+2028 and U+2029 are escaped
/// for the engines that once took them for line ends
pub fn script_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for c in json.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out
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

/// Decides the remote UI's token.
/// Uses the one in secrets if present; otherwise saves one to data\remote-token
/// and reuses it (a token that changes every time would force reconnecting
/// phones each time and make it impossible to show the QR from settings).
/// Shortest fixed token accepted (hex chars of a 64-bit secret; anything
/// shorter is guessable from the open internet a Tailscale-less LAN may be)
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

/// The same path, spelled the way this system spells one.
///
/// The tests were written where this app grew up, so they say `D:\work\proj`.
/// That is not an absolute path anywhere else -- it is an ordinary relative
/// name -- so a test handing one over was quietly measuring the folder the
/// test runner happened to be standing in. Give it the Windows spelling and it
/// hands back that same path as written here.
///
/// **Whatever the letter.** It folded away `D:` alone once, because that is
/// the drive this app grew up on -- and a test that said `C:` or `E:`, both of
/// which read perfectly naturally, went on being a relative name on a system
/// with no drives at all. That cost two afternoons in one day: a folder told
/// to move somewhere that was not a path, and a page whose folder could not be
/// matched with the folder it was written in.
#[cfg(test)]
pub fn local_path(win: &str) -> String {
    match cfg!(windows) {
        true => win.to_string(),
        false => {
            let s = win.replace('\\', "/");
            let drive = s.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                && s.as_bytes().get(1) == Some(&b':');
            match drive {
                true => s[2..].to_string(),
                false => s,
            }
        }
    }
}

/// The person's own folder, as this system names it: `USERPROFILE` on
/// Windows, `HOME` everywhere else. Every place that needs it asks here, so
/// a system where one of them is missing is answered the same way throughout
/// (reading only `USERPROFILE` once left Linux with no home at all)
/// Which system this runs on, by the one word the settings page and the
/// board use to tell them apart: "windows", "mac" or "linux"
pub fn system_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else {
        "linux"
    }
}

pub fn home_dir() -> Option<std::path::PathBuf> {
    home_in(|k| std::env::var_os(k))
}

/// The same, given the environment, so a test can ask what it would answer
pub(crate) fn home_in(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<std::path::PathBuf> {
    let keys: &[&str] = if cfg!(windows) { &["USERPROFILE", "HOME"] } else { &["HOME"] };
    keys.iter().filter_map(|k| env(k)).find(|v| !v.is_empty()).map(std::path::PathBuf::from)
}

/// A path a profile wrote with `{home}` in it, with the home folder put in.
/// `{home}` is the only thing a profile may stand in for: a profile that
/// could name any path would be naming a file to overwrite
pub fn with_home(path: &str) -> std::path::PathBuf {
    let home = home_dir().unwrap_or_default();
    std::path::PathBuf::from(path.replace("{home}", &home.to_string_lossy()))
}

/// Whether this system's folder names ignore the case of their letters:
/// Windows, and a Mac, whose disks are made that way unless somebody chose
/// otherwise. On Linux `Work` and `work` are two folders
pub const FOLDERS_IGNORE_CASE: bool = cfg!(any(windows, target_os = "macos"));

/// A path as the folder it names is known here, for comparing: the parts it
/// is made of, `.` left out and a trailing separator with it; on Windows the
/// direction of the slashes folded away, and where names ignore case, the
/// case. Everywhere else a backslash is an ordinary character in a name
pub fn folder_key(p: &std::path::Path) -> String {
    let parts: Vec<String> = p
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .map(|c| {
            let part = c.as_os_str().to_string_lossy();
            match cfg!(windows) {
                true => part.replace('\\', "/"),
                false => part.into_owned(),
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    let key = parts.join("/");
    match FOLDERS_IGNORE_CASE {
        true => key.to_lowercase(),
        false => key,
    }
}

/// Whether two paths name the same folder.
///
/// Spelled out rather than left to `==`, because a system hands the same
/// folder back in whatever spelling it likes: a config says `D:/Simic2`, the
/// CLI writes down `D:\Simic2`, and the disk itself may hold `D:\simic2` --
/// Windows opens all three, and a Mac opens `~/Work` as `~/work`. Compared as
/// written they are three folders, and a tab whose folder was spelled with
/// the wrong case never found its conversation, silently, forever. Where the
/// case does decide (Linux), folding it would hand one folder's things to
/// another
pub fn same_folder(a: &std::path::Path, b: &std::path::Path) -> bool {
    folder_key(a) == folder_key(b)
}

/// Where this system keeps a program's own things for one person on one
/// machine, never synced: `%LOCALAPPDATA%` on Windows, `~/Library/Application
/// Support` on a Mac, `$XDG_DATA_HOME` (or `~/.local/share`) on Linux. Each
/// caller names its own folder inside
pub fn machine_data_dir() -> Option<std::path::PathBuf> {
    data_home(|k| std::env::var_os(k))
}

/// The same, given the environment, so a test can ask what it would answer
pub(crate) fn data_home(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<std::path::PathBuf> {
    let set = |k: &str| env(k).filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    if cfg!(windows) {
        set("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        Some(home_in(&env)?.join("Library").join("Application Support"))
    } else {
        set("XDG_DATA_HOME").or_else(|| Some(home_in(&env)?.join(".local").join("share")))
    }
}

/// The same folder written another way this system accepts, for a test that
/// checks a folder is known however it is spelled: a separator on the end,
/// and where names ignore case, every letter's case turned over. On Linux the
/// case is the name, so it is left as it is
#[cfg(test)]
pub fn respelled(path: &str) -> String {
    let turned: String = match FOLDERS_IGNORE_CASE {
        true => path
            .chars()
            .map(|c| match c.is_uppercase() {
                true => c.to_lowercase().next().unwrap_or(c),
                false => c.to_uppercase().next().unwrap_or(c),
            })
            .collect(),
        false => path.to_string(),
    };
    format!("{turned}{}", std::path::MAIN_SEPARATOR)
}

/// The commit this was built from, as the board's footer shows it.
///
/// Stamped by this crate's build script, so a runtime with no window anywhere
/// near it can still say which source it is.
pub fn build_rev() -> &'static str {
    env!("BUILD_REV")
}

/// A program a test can put in a tab: one that starts, holds the terminal and
/// waits.
///
/// The tests name `cmd.exe` because that is the machine they were written on.
/// None of them is testing a shell -- what they want is a process with a
/// pseudo terminal attached that does not exit on its own -- so everywhere
/// else it is `sh`.
///
/// That this was wrong went unnoticed for a while, because the only Linux it
/// had been run on was WSL: Windows' own PATH is inherited there, `cmd.exe` is
/// found, and twenty-two tests passed on a machine no user has.
#[cfg(test)]
pub fn test_shell() -> String {
    match cfg!(windows) {
        true => "cmd.exe".to_string(),
        false => "sh".to_string(),
    }
}

/// A folder in the temporary area that belongs to this run alone, emptied the
/// first time this run asks for it by that name.
///
/// Two things make this more than `temp_dir().join(name)`.
///
/// A name with this process's id in it is not a new name: Windows hands the
/// same id out again within the day, and what the earlier run left behind is
/// sitting there under it. A worktree test found exactly that -- a folder
/// already standing where a branch was about to be cut, which the app refuses,
/// correctly -- and the failure read as a fault in the app rather than as
/// litter on the machine. Emptied once per name per run, never on the second
/// ask, so a test that names its own folder twice does not lose its work.
///
/// And what an old run left is taken away with it. Runs leave one folder each
/// otherwise, for every family of them; 7,773 had gathered on the machine
/// this was written on. Only folders of the same family, only ones this run
/// did not make, and only ones nothing has touched for two hours -- a suite
/// takes seconds, so nothing live is ever that quiet.
#[cfg(test)]
pub fn test_temp(what: &str) -> std::path::PathBuf {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static EMPTIED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let family = format!("shikisha-{what}-");
    let at = std::env::temp_dir().join(format!("{family}{}", std::process::id()));
    let first = EMPTIED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .map(|mut seen| seen.insert(what.to_string()))
        .unwrap_or(false);
    if !first {
        return at;
    }
    // Links go before the tree does, or whatever removes it walks into a
    // junction and takes the other side's contents with it (see unhook_links)
    crate::worktree::unhook_links(&at);
    let _ = std::fs::remove_dir_all(&at);
    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 60 * 60);
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return at };
    for e in rd.flatten() {
        let old = e.path();
        if old == at || !old.is_dir() {
            continue;
        }
        if !e.file_name().to_string_lossy().starts_with(&family) {
            continue;
        }
        if e.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < stale) {
            crate::worktree::unhook_links(&old);
            let _ = std::fs::remove_dir_all(&old);
        }
    }
    at
}

/// A path that is absolute, and nowhere near anything this app owns.
///
/// Written once because it differs: `C:/windows/x` is an absolute path on
/// Windows and an ordinary relative name everywhere else, so a test that used
/// it to prove a fence holds proved nothing at all on Linux.
#[cfg(test)]
pub fn outside_path(name: &str) -> String {
    match cfg!(windows) {
        true => format!("C:/windows/{name}"),
        false => format!("/etc/{name}"),
    }
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

#[cfg(test)]
mod tests {
    /// The home folder and the folder for a program's own things, as each
    /// system names them, asked of a made-up environment
    #[test]
    fn each_system_names_its_own_home_and_its_own_place_for_programs() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| std::ffi::OsString::from(*v))
        };
        let both = env(&[("USERPROFILE", "/u/win"), ("HOME", "/u/unix"), ("LOCALAPPDATA", "/u/win/local"), ("XDG_DATA_HOME", "/u/xdg")]);
        let home = super::home_in(&both).unwrap();
        let data = super::data_home(&both).unwrap();
        if cfg!(windows) {
            assert_eq!(home, std::path::PathBuf::from("/u/win"));
            assert_eq!(data, std::path::PathBuf::from("/u/win/local"));
        } else if cfg!(target_os = "macos") {
            assert_eq!(home, std::path::PathBuf::from("/u/unix"));
            assert_eq!(data, std::path::PathBuf::from("/u/unix/Library/Application Support"));
        } else {
            assert_eq!(home, std::path::PathBuf::from("/u/unix"));
            assert_eq!(data, std::path::PathBuf::from("/u/xdg"));
            assert_eq!(super::data_home(env(&[("HOME", "/u/unix")])).unwrap(), std::path::PathBuf::from("/u/unix/.local/share"));
        }
        // An empty variable is no answer, and neither is none at all
        assert_eq!(super::home_in(env(&[("HOME", ""), ("USERPROFILE", "")])), None);
        assert_eq!(super::home_in(|_: &str| None), None);
    }

    #[test]
    #[should_panic(expected = "refusing to create a token")]
    fn token_generation_has_no_predictable_fallback() {
        super::hex_token(None);
    }

    /// A value poured into a page's script cannot end the script element or
    /// start markup, and reads back as exactly what it was
    #[test]
    fn json_in_a_script_cannot_close_it() {
        let name = "x</script><script>alert(1)</script>&\u{2028}";
        let raw = serde_json::to_string(&serde_json::json!({ name: 1, "list": [name] })).unwrap();
        let safe = super::script_json(&raw);
        for bad in ["<", ">", "&", "\u{2028}"] {
            assert!(!safe.contains(bad), "{bad:?} is left in {safe}");
        }
        let back: serde_json::Value = serde_json::from_str(&safe).unwrap();
        assert_eq!(back, serde_json::from_str::<serde_json::Value>(&raw).unwrap());
    }

    /// A path a test writes the way this app's own machine writes one is an
    /// absolute path wherever the test runs.
    ///
    /// Not a detail: a relative path where an absolute one was meant is
    /// measured from wherever the test runner happens to be standing, so the
    /// test goes on passing while proving something else entirely -- or fails
    /// on one machine only, which is how both of these were found.
    #[test]
    fn a_path_written_the_way_this_machine_writes_one_is_absolute_anywhere() {
        for p in ["D:/work/one", "C:/work/two", "E:/elsewhere", r"F:\deep\place"] {
            let said = super::local_path(p);
            assert!(
                std::path::Path::new(&said).is_absolute(),
                "{p} became {said}, which is a name measured from wherever this is run"
            );
        }
        // A path with no drive on it is left alone, whatever it says
        assert_eq!(super::local_path("/etc/hosts"), "/etc/hosts");
    }

    /// Says a file or a folder was last touched then. A folder needs a word
    /// of its own on Windows, where opening one at all is asking for a
    /// backup's view of the disk
    #[cfg(test)]
    fn aged(at: &std::path::Path, when: std::time::SystemTime) {
        let mut how = std::fs::File::options();
        how.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            const BACKUP: u32 = 0x0200_0000; // FILE_FLAG_BACKUP_SEMANTICS
            // Saying when something was touched is writing to it, folder or not
            how.write(true).custom_flags(BACKUP);
        }
        #[cfg(not(windows))]
        how.write(!at.is_dir());
        how.open(at)
            .unwrap_or_else(|e| panic!("cannot open {at:?}: {e}"))
            .set_modified(when)
            .unwrap_or_else(|e| panic!("cannot age {at:?}: {e}"));
    }

    /// A run's own folder is empty when the run starts, keeps what the run
    /// puts in it, and takes an old run's folder away with it.
    ///
    /// The first of the three is what a worktree test was failing on: a
    /// process id comes round again, and the folder the last run left under it
    /// was still standing where this run was about to cut a branch.
    #[test]
    fn a_runs_own_folder_starts_empty_and_takes_the_last_run_with_it() {
        let family = format!("selftest-{}", crate::random_hex(4));
        // What an earlier run left: one of ours, too old to be anybody's now
        let before = std::env::temp_dir().join(format!("shikisha-{family}-424242"));
        std::fs::create_dir_all(&before).unwrap();
        let left = before.join("left-behind");
        std::fs::write(&left, "from a run that is over").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 60 * 60);
        aged(&left, old);
        // The folder itself last: writing the file inside it just touched it
        aged(&before, old);

        // ...and this run's own, which the same id may well have made before
        let ours = std::env::temp_dir().join(format!("shikisha-{family}-{}", std::process::id()));
        std::fs::create_dir_all(&ours).unwrap();
        std::fs::write(ours.join("stale"), "from the run before this one").unwrap();

        let at = crate::test_temp(&family);
        assert_eq!(at, ours);
        assert!(!at.join("stale").exists(), "this run began in the last run's folder");
        assert!(!before.exists(), "an old run's folder was left on the machine");

        // Asked for again, it is the same folder with the same things in it:
        // a test that names its own folder twice must not lose its work
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(at.join("mine"), "written by this test").unwrap();
        assert_eq!(crate::test_temp(&family), at);
        assert!(at.join("mine").exists(), "the second ask emptied the folder");
        let _ = std::fs::remove_dir_all(&at);
    }
}

#[cfg(all(test, unix))]
pub(crate) use farterms::b64 as farterms_b64;

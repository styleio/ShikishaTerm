//! The bridge's resident process, and the door the app comes in by.
//!
//! The bridge (`shikisha-bridge`, see `farlink`) is this app's one program on
//! another machine, and it takes on every job the app needs done there (the
//! far-keep plan, §3.1): carrying the tabs' `shikisha` command, the small
//! operations of [`crate::farops`], and more as they come. So what runs there
//! is a **resident process that holds jobs**, not a program shaped around any
//! one of them:
//!
//!   - the resident process ([`daemon`]) is started once for the account and
//!     outlives the line it was started for. It owns the sockets, the key the
//!     app's door has to show, and the lines the app has open to it. It knows
//!     nothing about any one job: each is a [`Job`], found by name, and says for
//!     itself whether it wants the process to stay
//!   - the door ([`serve_port`]) is what the app runs over SSH, or on a MicroVM
//!     the way a terminal is run: it starts the resident process when there is
//!     none, shows the key, and carries the line between its own input and
//!     output and the resident process. The app hears exactly what it heard
//!     when the bridge was one program per line ([`crate::farlink::Frame`])
//!
//! Two sockets, because two kinds of caller speak first in two ways:
//!
//!   - `run/keep.sock`, the resident process's own: it names itself first, one
//!     line ([`Frame::Hello`]), in a shape no version changes, and then hears
//!     what the caller is (the app's door, with the key; or a question of
//!     which version is here)
//!   - `run/tabs.sock`, the tabs' `shikisha` command: it speaks first, as it
//!     does to the pipe on the app's own machine, and the resident process
//!     carries it to the app that started that tab. A tab of an older bridge
//!     keeps `run/shikisha.sock`, which nothing here touches
//!
//! **Who may come in (§4.7):** the account's own programs. The folder is 0700,
//! the sockets and the key file 0600, and each caller's user is checked on the
//! socket. The key is made after the sockets are bound, so a process that lost
//! the race to become resident never replaces the key of the one that won.
//! No network port is opened: the app always comes in from its side, over a
//! line it made.
//!
//! **How long it lives:** while an app is connected, and after that while any
//! job wants it to. With no job holding anything (today: always), it ends a
//! short while after the last app went, which is what the bridge did before.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

// The doors are sockets on a unix machine and named pipes on Windows (the
// local-keeper plan §2); the names below are what the code has always said
use crate::keepipe::{Conn as UnixStream, Listener as UnixListener, same_user};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Value, json};

pub use crate::farlink::{KEEP_SOCK, TABS_SOCK};
use crate::farlink::Frame;

/// What the app's door has to show, written after the sockets are bound
const KEY_FILE: &str = "keep.key";
/// Held while deciding who becomes resident (on Windows the pipe's name is
/// the lock, and there is no file)
#[cfg(unix)]
const LOCK_FILE: &str = "keep.lock";
/// What the resident process says, and how large it may grow before the
/// next start sets it aside
const LOG_FILE: &str = "keep.log";
const LOG_MOST: u64 = 1024 * 1024;
/// How long the resident process stays after the last app went, when no job
/// wants it to: an app that reconnects in a moment finds it still there
const GRACE: Duration = Duration::from_secs(10);
/// How long a line may be silent before the app on it is taken to be gone.
/// The app says it is there every fifteen seconds ([`Frame::Tick`]); on a
/// MicroVM the door's input stays open after the app is gone, so silence is
/// the sign
const SILENCE: Duration = Duration::from_secs(60);

// ── What a job is ─────────────────────────────────────────────────────────

/// One job the resident process holds. The process routes the app's frames
/// to its jobs and asks them whether to stay; it knows no job by what it does
pub trait Job: Send + Sync {
    /// The job's name, as the app is told it in [`Frame::Hello`]
    fn name(&self) -> &'static str;
    /// A frame from the app on `line`. `true` when this job took it; a job
    /// may look at a frame and leave it to another
    fn frame(&self, core: &Arc<Core>, line: u64, frame: &Frame) -> bool;
    /// The app on `line` went: whatever of it the job holds ends
    fn line_gone(&self, _core: &Arc<Core>, _line: u64) {}
    /// Whether the resident process should stay with no app connected
    fn wants_to_stay(&self) -> bool {
        false
    }
    /// Whether it is still working on something the app on `line` asked
    /// for. An app that stops talking is answered what it asked before it
    /// stopped: its line is kept until no job holds anything of it
    fn holds(&self, _line: u64) -> bool {
        false
    }
    /// The resident process is ending: whatever the job holds ends with it
    fn end(&self) {}
    /// Once a second, whatever the job does by the clock
    fn tick(&self, _core: &Arc<Core>) {}
}

/// How long an app that stopped talking waits, at most, for the answers to
/// what it asked before it stopped
const ANSWERS_WAIT: Duration = Duration::from_secs(120);

/// One app, connected through a door
struct Line {
    out: Mutex<UnixStream>,
    heard: Mutex<Instant>,
    /// It said it is going (`Frame::Bye`): nothing new is carried to it,
    /// while what it asked before is still answered
    leaving: std::sync::atomic::AtomicBool,
}

/// What every job reaches through: the lines to the apps
pub struct Core {
    lines: Mutex<HashMap<u64, Arc<Line>>>,
    next_line: AtomicU64,
    /// When the last app went, while none is connected
    alone_since: Mutex<Option<Instant>>,
    jobs: Mutex<Vec<Arc<dyn Job>>>,
}

impl Core {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            lines: Mutex::default(),
            next_line: AtomicU64::new(0),
            alone_since: Mutex::new(Some(Instant::now())),
            jobs: Mutex::default(),
        })
    }

    /// Say a frame to the app on `line`. `false` when it cannot be said
    pub fn say(&self, line: u64, frame: &Frame) -> bool {
        let Some(l) = self.lines.lock().ok().and_then(|m| m.get(&line).cloned()) else { return false };
        let mut text = serde_json::to_string(frame).unwrap_or_default();
        text.push('\n');
        let mut out = l.out.lock().unwrap_or_else(|e| e.into_inner());
        out.write_all(text.as_bytes()).is_ok() && out.flush().is_ok()
    }

    /// The apps connected now, those that said they are going left out:
    /// nothing new is for them
    pub fn lines(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = self
            .lines
            .lock()
            .map(|m| m.iter().filter(|(_, l)| !l.leaving.load(Ordering::SeqCst)).map(|(id, _)| *id).collect())
            .unwrap_or_default();
        ids.sort_unstable();
        ids
    }

    /// Whether the app on `line` is connected and staying
    pub fn is_up(&self, line: u64) -> bool {
        self.lines.lock().is_ok_and(|m| m.get(&line).is_some_and(|l| !l.leaving.load(Ordering::SeqCst)))
    }

    /// The app on `line` said it is going: from now on it is away to every
    /// job that asks, while its line stays for the answers it is owed
    fn leaving(&self, line: u64) {
        if let Some(l) = self.lines.lock().ok().and_then(|m| m.get(&line).cloned()) {
            l.leaving.store(true, Ordering::SeqCst);
        }
    }

    fn jobs(&self) -> Vec<Arc<dyn Job>> {
        self.jobs.lock().map(|j| j.clone()).unwrap_or_default()
    }

    fn add(self: &Arc<Self>, out: UnixStream) -> u64 {
        let id = self.next_line.fetch_add(1, Ordering::SeqCst) + 1;
        let line = Arc::new(Line {
            out: Mutex::new(out),
            heard: Mutex::new(Instant::now()),
            leaving: std::sync::atomic::AtomicBool::new(false),
        });
        if let Ok(mut m) = self.lines.lock() {
            m.insert(id, line);
        }
        if let Ok(mut a) = self.alone_since.lock() {
            *a = None;
        }
        id
    }

    fn heard(&self, line: u64) {
        if let Some(l) = self.lines.lock().ok().and_then(|m| m.get(&line).cloned())
            && let Ok(mut h) = l.heard.lock()
        {
            *h = Instant::now();
        }
    }

    /// The app on `line` is gone: its door is shut, and every job lets go
    fn drop_line(self: &Arc<Self>, line: u64) {
        let gone = self.lines.lock().ok().and_then(|mut m| {
            let l = m.remove(&line);
            if m.is_empty()
                && let Ok(mut a) = self.alone_since.lock()
            {
                *a = Some(Instant::now());
            }
            l
        });
        let Some(gone) = gone else { return };
        let _ = gone.out.lock().map(|o| o.shutdown(std::net::Shutdown::Both));
        for job in self.jobs() {
            job.line_gone(self, line);
        }
        log(&format!("an app went (line {line})"));
    }

    /// The lines whose app has said nothing for too long
    fn silent(&self) -> Vec<u64> {
        self.lines
            .lock()
            .map(|m| {
                m.iter()
                    .filter(|(_, l)| l.heard.lock().map(|h| h.elapsed() > SILENCE).unwrap_or(true))
                    .map(|(id, _)| *id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the resident process has nothing left to do: no app, and no job
    /// wanting to stay, for a while
    fn done(&self) -> bool {
        let alone = self.alone_since.lock().ok().and_then(|a| *a).is_some_and(|since| since.elapsed() >= GRACE);
        alone && !self.jobs().iter().any(|j| j.wants_to_stay())
    }
}

pub(crate) fn log(text: &str) {
    eprintln!("shikisha-bridge: {text}");
}

// ── Who may come in ──────────────────────────────────────────────────────

/// A file read only when it is this account's and nobody else's
#[cfg(unix)]
fn read_private(file: &Path) -> Result<String> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(file).with_context(|| format!("{} is not there", file.display()))?;
    // SAFETY: getuid cannot fail
    if meta.uid() != unsafe { libc::getuid() } || meta.mode() & 0o077 != 0 {
        bail!("{} is not this account's alone; it is not read", file.display());
    }
    Ok(std::fs::read_to_string(file)?.trim().to_string())
}

/// The same on Windows, where the folder is the account's own under
/// `%LOCALAPPDATA%` (local-keeper plan §2): what guards it is the profile's
/// own permissions, which give the account and the system alone
#[cfg(windows)]
fn read_private(file: &Path) -> Result<String> {
    Ok(std::fs::read_to_string(file).with_context(|| format!("{} is not there", file.display()))?.trim().to_string())
}

/// Written so that only this account can read it, and never half written:
/// made under another name first, then moved into place
#[cfg(windows)]
pub(crate) fn write_private(file: &Path, text: &str) -> Result<()> {
    let part = file.with_extension(format!("part{}", std::process::id()));
    let _ = std::fs::remove_file(&part);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&part)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&part, file)?;
    Ok(())
}

/// Written so that only this account can read it, and never half written:
/// made under another name first, then moved into place
#[cfg(unix)]
pub(crate) fn write_private(file: &Path, text: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let part = file.with_extension(format!("part{}", std::process::id()));
    let _ = std::fs::remove_file(&part);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&part)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&part, file)?;
    Ok(())
}

/// The key the resident process in `home` asks an app's door to show
pub fn door_key(home: &Path) -> Result<String> {
    read_private(&home.join("run").join(KEY_FILE))
}

/// Who is on a socket
pub enum Found {
    /// A resident process, as it names itself
    Named(Frame),
    /// Something took the connection and has not named itself yet: a resident
    /// process slow to answer is still a resident process, and its socket is
    /// not taken from it (the connection itself is the sign, as the reference
    /// implementation holds)
    Silent,
    /// Nobody: no socket, or one nothing listens on any more
    Nobody,
    /// The socket is there and could not be reached (every instance stayed
    /// busy, or it was refused): somebody may well be there, and is not to
    /// be taken for nobody -- that would start a second resident process, or
    /// count the terminals the first one holds as none
    Unreachable(String),
}

/// Who is on `sock`
pub fn find(sock: &Path) -> Found {
    let conn = match crate::keepipe::connect(sock) {
        Ok(conn) => conn,
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => return Found::Nobody,
        Err(e) => return Found::Unreachable(e.to_string()),
    };
    if conn.set_read_timeout(Some(Duration::from_secs(3))).is_err() {
        return Found::Silent;
    }
    let mut first = String::new();
    let read = conn.try_clone().map(|c| BufReader::new(c).read_line(&mut first));
    match (read, serde_json::from_str::<Frame>(first.trim())) {
        (Ok(Ok(n)), Ok(hello @ Frame::Hello { .. })) if n > 0 => {
            let _ = (&conn).write_all(b"{\"role\":\"probe\"}\n");
            Found::Named(hello)
        }
        _ => Found::Silent,
    }
}

/// The resident process as it names itself, if one answers on `sock`
pub fn probe(sock: &Path) -> Option<Frame> {
    match find(sock) {
        Found::Named(hello) => Some(hello),
        _ => None,
    }
}

/// Say, for as long as this process runs, that this program's file is in
/// use (far-keep plan §4.5): a shared lock on its mark beside it, which
/// whoever clears old builds away tries to take whole, without waiting, and
/// deletes only a build nobody holds. Held until the process ends.
///
/// Windows needs no mark: a running program's file cannot be deleted there,
/// only moved aside (which is how this app's own copies are replaced)
#[cfg(windows)]
fn hold_program(_home: &Path) {}

#[cfg(unix)]
fn hold_program(home: &Path) {
    use std::os::unix::io::AsRawFd as _;
    let mark = home.join(format!(".{}.lock", own_program()));
    let Ok(file) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&mark) else { return };
    // SAFETY: the descriptor is the mark's, and stays open (it is kept below)
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } == 0 {
        std::mem::forget(file);
    }
}

/// The file name of this program, which carries its build (`farlink::program_name`)
fn own_program() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_default()
}

/// What a command in the middle of a call hears when its app goes
const CUT: &str = "The SHIKISHA-TERM app that started this tab went away while this was being handled \
    (the PC is away). It may or may not have been carried out: check before doing it again.";

fn hello(core: &Core) -> Frame {
    Frame::Hello {
        version: env!("CARGO_PKG_VERSION").into(),
        rev: crate::build_rev().into(),
        jobs: core.jobs().iter().map(|j| j.name().to_string()).collect(),
        program: own_program(),
    }
}

// ── The resident process ──────────────────────────────────────────────────

/// Become the account's resident bridge process, unless one is already there.
///
/// Who becomes it is decided under a lock, by who binds the socket: a second
/// one started at the same moment finds the first answering and leaves
/// without touching anything. A socket left by one that died is only removed
/// by the one holding the lock, after it did not answer
/// Set when the resident process is told to end (SIGTERM): it leaves its
/// loop as if nothing were left to hold, so every job ends what it holds --
/// the terminals' programs with it -- before it goes
static TOLD_TO_END: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_term(_: libc::c_int) {
    TOLD_TO_END.store(true, Ordering::SeqCst);
}

/// Tell the resident process in this one to end what it holds and go: what
/// SIGTERM does on a unix machine, asked on Windows over its own door
pub fn tell_to_end() {
    TOLD_TO_END.store(true, Ordering::SeqCst);
}

pub fn daemon(home: PathBuf) -> Result<()> {
    hold_program(&home);
    // SAFETY: the handler only stores to an atomic, which is all a signal
    // handler may do
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTERM, on_term as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
    crate::farops::set_home(home.clone());
    let run = home.join("run");
    std::fs::create_dir_all(&run)?;
    #[cfg(unix)]
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700))?;
    let keep = run.join(KEEP_SOCK);
    let tabs = run.join(TABS_SOCK);

    // Who becomes resident. On a unix machine: whoever, under the lock, finds
    // nobody on the socket and binds it. On Windows the pipe's name is the
    // lock: the first instance is one process's alone, so a second one is
    // refused the door and leaves without touching anything
    #[cfg(unix)]
    let lock = {
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(run.join(LOCK_FILE))?;
        use std::os::unix::io::AsRawFd as _;
        // SAFETY: the descriptor is the lock file's, open for as long as `lock`
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            bail!("could not take the lock on {}", run.display());
        }
        lock
    };
    if !matches!(find(&keep), Found::Nobody) {
        // Somebody else is resident already -- or holds the socket and is slow
        // to say so, which is the same: the lock goes with the file
        return Ok(());
    }
    #[cfg(unix)]
    for stale in [&keep, &tabs] {
        let _ = std::fs::remove_file(stale);
    }
    let keep_listener = match UnixListener::bind(&keep) {
        Ok(l) => l,
        // Taken between the look and the bind: the other one is resident
        #[cfg(windows)]
        Err(_) => return Ok(()),
        #[cfg(unix)]
        Err(e) => return Err(e.into()),
    };
    let tabs_listener = UnixListener::bind(&tabs)?;
    #[cfg(unix)]
    for s in [&keep, &tabs] {
        std::fs::set_permissions(s, std::fs::Permissions::from_mode(0o600))?;
    }
    // The key, only now: bound, this process is the resident one
    let key = crate::random_hex(32);
    write_private(&run.join(KEY_FILE), &key)?;
    let key_kept = key.clone();
    let mine = [inode(&keep), inode(&tabs)];
    #[cfg(unix)]
    drop(lock);
    log(&format!("resident ({}, {})", env!("CARGO_PKG_VERSION"), crate::build_rev()));

    let core = Core::new();
    let tabs_job = Arc::new(TabsJob { home: Some(home.clone()), ..TabsJob::default() });
    let _ = tabs_job.me.set(Arc::downgrade(&tabs_job));
    if let Ok(mut j) = core.jobs.lock() {
        j.push(Arc::new(HostJob));
        j.push(tabs_job.clone());
        j.push(Arc::new(OpsJob::default()));
        j.push(Arc::new(crate::farterms::Terms::at(Some(home.clone()))));
    }

    {
        let core = Arc::clone(&core);
        std::thread::spawn(move || {
            for conn in keep_listener.incoming().flatten() {
                let (core, key) = (Arc::clone(&core), key.clone());
                std::thread::spawn(move || door(&core, conn, &key));
            }
        });
    }
    {
        let core = Arc::clone(&core);
        std::thread::spawn(move || {
            for conn in tabs_listener.incoming().flatten() {
                let (core, job) = (Arc::clone(&core), Arc::clone(&tabs_job));
                std::thread::spawn(move || job.command(&core, conn));
            }
        });
    }

    loop {
        std::thread::sleep(Duration::from_secs(1));
        for job in core.jobs() {
            job.tick(&core);
        }
        for line in core.silent() {
            log(&format!("line {line} has been silent too long"));
            core.drop_line(line);
        }
        if core.done() {
            break;
        }
        if TOLD_TO_END.load(Ordering::SeqCst) {
            log("told to end; ending what is held");
            break;
        }
    }
    for job in core.jobs() {
        job.end();
    }
    // Only what is still this process's own: a socket another resident process
    // has since bound in its place is left alone
    for (path, ino) in [(&keep, mine[0]), (&tabs, mine[1])] {
        if ino.is_some() && inode(path) == ino {
            let _ = std::fs::remove_file(path);
        }
    }
    // The key too, only while it is still this process's: one that became
    // resident after this one has a key of its own there
    if read_private(&run.join(KEY_FILE)).is_ok_and(|k| k == key_kept) {
        let _ = std::fs::remove_file(run.join(KEY_FILE));
    }
    log("nothing left to hold; ending");
    Ok(())
}

#[cfg(unix)]
fn inode(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::symlink_metadata(path).ok().map(|m| m.ino())
}

/// A pipe leaves no file behind to be tidied: it goes with its process
#[cfg(windows)]
fn inode(_path: &Path) -> Option<u64> {
    None
}

/// One caller of the resident process's own socket: the app's door, or a
/// question of which version is here
fn door(core: &Arc<Core>, conn: UnixStream, key: &str) {
    if !same_user(&conn) {
        return;
    }
    let hi = hello(core);
    let mut text = serde_json::to_string(&hi).unwrap_or_default();
    text.push('\n');
    if (&conn).write_all(text.as_bytes()).is_err() {
        return;
    }
    let Ok(reader) = conn.try_clone() else { return };
    let mut lines = BufReader::new(reader).lines();
    let Some(Ok(first)) = lines.next() else { return };
    let said: Value = serde_json::from_str(&first).unwrap_or_default();
    if said.get("role").and_then(|r| r.as_str()) != Some("port") {
        return;
    }
    let shown = said.get("key").and_then(|k| k.as_str()).unwrap_or_default();
    if !crate::crypto::token_eq(key, shown) {
        log("a door came with the wrong key");
        return;
    }
    let line = core.add(conn);
    log(&format!("an app came (line {line})"));
    for text in lines {
        let Ok(text) = text else { break };
        core.heard(line);
        let Ok(frame) = serde_json::from_str::<Frame>(&text) else { continue };
        // The app is going: its line ends here, without waiting for the
        // silence its going would otherwise be known by
        if matches!(frame, Frame::Bye) {
            log(&format!("the app on line {line} said it is going"));
            core.leaving(line);
            break;
        }
        for job in core.jobs() {
            if job.frame(core, line, &frame) {
                break;
            }
        }
    }
    // It stopped talking: what it asked before that is still answered
    let until = Instant::now() + ANSWERS_WAIT;
    while Instant::now() < until && core.jobs().iter().any(|j| j.holds(line)) {
        std::thread::sleep(Duration::from_millis(50));
    }
    core.drop_line(line);
}

// ── The jobs there are today ──────────────────────────────────────────────

/// Carrying each tab's `shikisha` command to the app that started the tab.
///
/// Which app that is, is known from the key it gave the tab ([`crate::farops`]'s
/// `put_key`, seen on its way past). A command whose app is not connected is
/// answered at once that it is away, and not held (far-keep plan §4.6) -- and
/// written down ([`crate::farmissed`]), as is one cut in the middle by its app
/// going, so the person is told when the app is back
#[derive(Default)]
struct TabsJob {
    /// Each command connection: the line its app is on, the socket, the tab
    /// it is from, and the call it is in the middle of
    conns: Mutex<HashMap<u64, Conn>>,
    next: AtomicU64,
    /// Each tab key given, and the line of the app that gave it
    owners: Mutex<HashMap<String, u64>>,
    /// Each tab key given, and the tab's name
    names: Mutex<HashMap<String, String>>,
    /// The calls written down, kept with the file
    book: Mutex<Option<crate::farmissed::Book>>,
    /// Itself, for the threads that hand kept calls over
    me: std::sync::OnceLock<std::sync::Weak<TabsJob>>,
    /// The tabs whose kept calls are being handed over now
    handing: Mutex<std::collections::HashSet<String>>,
    /// The resident process's own folder, where the calls are written down
    home: Option<PathBuf>,
}

struct Conn {
    line: u64,
    /// Where the app's answers go: the tab's command, or a kept call being
    /// handed over
    out: Out,
    tab: String,
    /// The last call carried, not yet answered
    call: Option<String>,
}

enum Out {
    Socket(UnixStream),
    Kept(std::sync::mpsc::Sender<String>),
}

/// How long the app may take to answer a kept call handed over to it
const HAND_WAIT: Duration = Duration::from_secs(120);

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl TabsJob {
    /// The tab a key was given for, as far as this resident process saw
    fn tab_of(&self, token: &str) -> String {
        self.names
            .lock()
            .ok()
            .and_then(|n| n.iter().find(|(k, _)| crate::crypto::token_eq(k, token)).map(|(_, t)| t.clone()))
            .unwrap_or_default()
    }

    /// The calls written down, read from the file the first time
    fn with_book<R>(&self, f: impl FnOnce(&mut crate::farmissed::Book) -> R) -> R {
        self.with_book_saved(f).0
    }

    /// The same, and whether what changed is in the file
    fn with_book_saved<R>(&self, f: impl FnOnce(&mut crate::farmissed::Book) -> R) -> (R, bool) {
        let path = self.home.as_ref().map(|h| h.join(crate::farmissed::FILE));
        let mut held = self.book.lock().unwrap_or_else(|e| e.into_inner());
        let book = held.get_or_insert_with(|| {
            path.as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or_default()
        });
        let before = book.clone();
        let out = f(book);
        if *book == before {
            return (out, true);
        }
        let saved = match &path {
            Some(p) => write_private(p, &serde_json::to_string(book).unwrap_or_default()),
            None => Err(anyhow!("no folder of its own")),
        };
        if let Err(e) = &saved {
            log(&format!("the calls that did not get through could not be written down: {e:#}"));
        }
        (out, saved.is_ok())
    }

    /// Keep a call to be handed over. Only a call that is in the file counts
    /// as kept -- the AI is told it is, and a resident process that ends
    /// must not take it along -- so one not written is taken back out
    fn keep(&self, tab: &str, line: &str) -> bool {
        let (id, saved) = self.with_book_saved(|b| b.keep(tab, line, now_secs()));
        match (id, saved) {
            (Some(_), true) => true,
            (Some(id), false) => {
                self.with_book(|b| b.handed(id));
                false
            }
            (None, _) => {
                log(&format!("a call of {tab} is too long to keep; it is written down instead"));
                false
            }
        }
    }

    /// A call whose app is away: kept, when it asks nothing back and its tab
    /// is known -- answered so, to be handed over when the app is back; else
    /// written down, and answered that the PC is away (`None`)
    fn away_call(&self, core: &Arc<Core>, tab: &str, line: &str) -> Option<Value> {
        let (method, _) = crate::farmissed::read_call(line);
        if crate::farmissed::OF_THE_MOMENT.contains(&method.as_str()) {
            return None;
        }
        if tab.is_empty() || !crate::farmissed::KEPT.contains(&method.as_str()) {
            self.missed(core, tab, line, false);
            return None;
        }
        if !self.keep(tab, line) {
            self.missed(core, tab, line, false);
            return None;
        }
        let id = serde_json::from_str::<Value>(line).ok().and_then(|v| v.get("id").cloned()).unwrap_or(Value::Null);
        const KEPT: &str = "The SHIKISHA-TERM app that started this tab is not connected to this machine right now \
            (the PC is away). This was kept, and is handed to it when it is back.";
        Some(json!({ "id": id, "ok": true, "result": KEPT }))
    }

    /// Hand a tab's kept calls to the app that just gave it its key, oldest
    /// first, each on a connection of its own as the tab's command would --
    /// under the key given now, and under an id of its own the app runs once.
    /// Struck out once answered; on a line that goes, the rest wait for the
    /// next time
    fn hand_over(&self, core: &Arc<Core>, line: u64, tab: &str, key: &str) {
        let (uid, kept) = self.with_book(|b| (b.uid.clone(), b.kept_for(tab)));
        for k in kept {
            let c = self.next.fetch_add(1, Ordering::SeqCst) + 1;
            let (tx, rx) = std::sync::mpsc::channel::<String>();
            if let Ok(mut m) = self.conns.lock() {
                m.insert(c, Conn { line, out: Out::Kept(tx), tab: tab.to_string(), call: None });
            }
            let mut call: Value = serde_json::from_str(&k.line).unwrap_or_default();
            let id = crate::farmissed::kept_id(&uid, k.id);
            call["id"] = json!(id);
            let ok = |a: &str| serde_json::from_str::<Value>(a).is_ok_and(|v| v["ok"] == json!(true));
            // The key taken first; a key refused is the app not knowing the
            // tab any more, and nothing of it is handed over now
            let took_key = core.say(line, &Frame::Open { c })
                && core.say(line, &Frame::Line { c, l: json!({ "token": key }).to_string() })
                && rx.recv_timeout(Duration::from_secs(30)).is_ok_and(|a| ok(&a));
            // Its own answer: under its id, any other line is not one
            let answer = if took_key && core.say(line, &Frame::Line { c, l: call.to_string() }) {
                let until = std::time::Instant::now() + HAND_WAIT;
                loop {
                    let left = until.saturating_duration_since(std::time::Instant::now());
                    match rx.recv_timeout(left) {
                        Ok(a) => match serde_json::from_str::<Value>(&a) {
                            Ok(v) if v["id"] == json!(id) => break Some(v),
                            _ => continue,
                        },
                        Err(_) => break None,
                    }
                }
            } else {
                None
            };
            core.say(line, &Frame::Close { c });
            if let Ok(mut m) = self.conns.lock() {
                m.remove(&c);
            }
            match answer {
                None => {
                    log(&format!("a kept call of {tab} was not answered; it waits for the next time"));
                    break;
                }
                Some(a) if a["ok"] == json!(true) => {
                    self.with_book(|b| b.handed(k.id));
                    log(&format!("a kept call of {tab} was handed over"));
                }
                Some(a) => {
                    let given_up = self.with_book(|b| b.refused(k.id));
                    log(&format!(
                        "a kept call of {tab} was refused ({}){}",
                        a["error"].as_str().unwrap_or("no reason"),
                        if given_up { "; given up" } else { "; it is tried again the next time" }
                    ));
                }
            }
        }
    }

    /// Write down a call that did not reach its app, and tell every app
    /// connected now: the person hears of it without waiting for a reconnect
    fn missed(&self, core: &Arc<Core>, tab: &str, line: &str, cut: bool) {
        let (method, to) = crate::farmissed::read_call(line);
        if method.is_empty() {
            return;
        }
        let call = crate::farmissed::Missed { id: 0, at: now_secs(), tab: tab.to_string(), method, to, cut };
        let book = self.with_book(|b| {
            b.add(call, now_secs());
            b.clone()
        });
        for l in core.lines() {
            core.say(l, &Frame::Job { job: "tabs".into(), m: json!({ "did": "missed", "book": book }) });
        }
    }

    fn command(&self, core: &Arc<Core>, conn: UnixStream) {
        if !same_user(&conn) {
            return;
        }
        let Ok(reader) = conn.try_clone() else { return };
        let mut lines = BufReader::new(reader).lines();
        let Some(Ok(first)) = lines.next() else { return };
        let token = serde_json::from_str::<Value>(&first)
            .ok()
            .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(str::to_string))
            .unwrap_or_default();
        let tab = self.tab_of(&token);
        let Some(line) = self.owner_of(core, &token) else {
            away(conn, lines, |call| self.away_call(core, &tab, call));
            return;
        };
        // What the tab says now comes after what it said while the app was
        // away: a report kept, then the next one, in that order. A kept call
        // the app refused is the exception: it is tried again the next time
        // and does not hold up the rest, since a call refused once is mostly
        // refused every time, and the line behind it would wait five returns
        while !tab.is_empty() && core.is_up(line) && self.handing.lock().is_ok_and(|h| h.contains(&tab)) {
            std::thread::sleep(Duration::from_millis(100));
        }
        // The app may have gone while it waited: then it is away to this call
        // too, which is kept or written down as any call to an app away is
        let Some(line) = (if core.is_up(line) { Some(line) } else { self.owner_of(core, &token) }) else {
            away(conn, lines, |call| self.away_call(core, &tab, call));
            return;
        };
        let c = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        if let Ok(mut m) = self.conns.lock() {
            m.insert(c, Conn { line, out: Out::Socket(conn), tab, call: None });
        }
        let carried = core.say(line, &Frame::Open { c }) && core.say(line, &Frame::Line { c, l: first });
        if carried {
            for text in lines {
                let Ok(l) = text else { break };
                if let Some(conn) = self.conns.lock().ok().as_mut().and_then(|m| m.get_mut(&c)) {
                    conn.call = Some(l.clone());
                }
                if !core.say(line, &Frame::Line { c, l }) {
                    break;
                }
            }
        }
        core.say(line, &Frame::Close { c });
        if let Ok(mut m) = self.conns.lock() {
            m.remove(&c);
        }
    }

    /// The app a tab's command goes to: the one that gave its key. A key
    /// nobody here saw given -- a tab started before this resident process --
    /// goes to the one app connected, when there is only one, as it did before
    fn owner_of(&self, core: &Arc<Core>, token: &str) -> Option<u64> {
        let known = self
            .owners
            .lock()
            .ok()
            .and_then(|o| o.iter().find(|(k, _)| crate::crypto::token_eq(k, token)).map(|(_, l)| *l));
        match known {
            Some(line) => core.is_up(line).then_some(line),
            None => match core.lines().as_slice() {
                [only] => Some(*only),
                _ => None,
            },
        }
    }
}

/// The answer to a command whose app is away: the handshake taken, and every
/// call answered at once, in words the AI there can read and act on. Each is
/// handed to `keep` first, which answers it itself when it keeps it
fn away(mut conn: UnixStream, lines: impl Iterator<Item = std::io::Result<String>>, keep: impl Fn(&str) -> Option<Value>) {
    const AWAY: &str = "The SHIKISHA-TERM app that started this tab is not connected to this machine right now \
        (the PC is away). Nothing was sent. Hand over what you meant to report when it is back.";
    let _ = writeln!(conn, r#"{{"ok":true,"result":"hello"}}"#);
    for text in lines {
        let Ok(l) = text else { break };
        let id = serde_json::from_str::<Value>(&l).ok().and_then(|v| v.get("id").cloned()).unwrap_or(Value::Null);
        let answer = keep(&l).unwrap_or_else(|| json!({ "id": id, "ok": false, "error": AWAY }));
        if writeln!(conn, "{answer}").is_err() {
            break;
        }
    }
}

impl Job for TabsJob {
    fn name(&self) -> &'static str {
        "tabs"
    }

    fn frame(&self, core: &Arc<Core>, line: u64, frame: &Frame) -> bool {
        match frame {
            Frame::Line { c, l } => {
                if let Some(conn) = self.conns.lock().ok().as_mut().and_then(|m| m.get_mut(c)) {
                    // Answered: nothing of it is in the middle any more
                    conn.call = None;
                    match &mut conn.out {
                        Out::Socket(s) => {
                            let _ = writeln!(s, "{l}");
                        }
                        Out::Kept(tx) => {
                            let _ = tx.send(l.clone());
                        }
                    }
                }
                true
            }
            Frame::Close { c } => {
                if let Some(Conn { out: Out::Socket(s), .. }) = self.conns.lock().ok().and_then(|mut m| m.remove(c)) {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                }
                true
            }
            // The calls written down: read by the app, and struck out once
            // the person has looked at them
            Frame::Job { job, m } if job == "tabs" => {
                match m["do"].as_str().unwrap_or_default() {
                    "missed" => {}
                    "seen" => {
                        let list: Option<Vec<crate::farmissed::Missed>> = m.get("calls").and_then(|c| serde_json::from_value(c.clone()).ok());
                        self.with_book(|b| b.seen(list.as_deref()));
                    }
                    _ => return true,
                }
                let book = self.with_book(|b| {
                    b.trim(now_secs());
                    b.clone()
                });
                let said = json!({ "did": "missed", "ref": m["ref"], "book": book });
                core.say(line, &Frame::Job { job: "tabs".into(), m: said });
                true
            }
            // Seen on the way past, and left to the operations job to do
            Frame::Op { op, p, .. } => {
                let key = p.get("key").and_then(|k| k.as_str());
                if let (Ok(mut owners), Some(key)) = (self.owners.lock(), key) {
                    match op.as_str() {
                        "put_key" => {
                            owners.insert(key.to_string(), line);
                            if let (Ok(mut names), Some(tab)) = (self.names.lock(), p.get("tab").and_then(|t| t.as_str())) {
                                names.insert(key.to_string(), tab.to_string());
                                // The app is back for this tab: what was kept
                                // for it while it was away is handed over, on a
                                // thread, since its answers come by this loop
                                let waiting = self.with_book(|b| !b.kept_for(tab).is_empty());
                                let first = self.handing.lock().is_ok_and(|mut h| h.insert(tab.to_string()));
                                if waiting && first
                                    && let Some(me) = self.me.get().and_then(std::sync::Weak::upgrade)
                                {
                                    let (core, tab, key) = (Arc::clone(core), tab.to_string(), key.to_string());
                                    std::thread::spawn(move || {
                                        me.hand_over(&core, line, &tab, &key);
                                        if let Ok(mut h) = me.handing.lock() {
                                            h.remove(&tab);
                                        }
                                    });
                                } else if first && let Ok(mut h) = self.handing.lock() {
                                    h.remove(tab);
                                }
                            }
                        }
                        "drop_key" => {
                            owners.remove(key);
                            if let Ok(mut names) = self.names.lock() {
                                names.remove(key);
                            }
                        }
                        _ => {}
                    }
                }
                false
            }
            _ => false,
        }
    }

    fn line_gone(&self, core: &Arc<Core>, line: u64) {
        // Its commands end with it; the app that answers them is gone
        // Told first, in words the command prints, since what it asked may
        // have been done, or not, as the app went: it cannot know which
        let mut cut: Vec<(String, String)> = Vec::new();
        if let Ok(mut m) = self.conns.lock() {
            m.retain(|_, conn| {
                let keep = conn.line != line;
                if !keep {
                    // A kept call being handed over is not cut: it stays kept
                    if let Out::Socket(s) = &mut conn.out {
                        let _ = writeln!(s, "{}", json!({ "id": null, "ok": false, "error": CUT }));
                        let _ = s.shutdown(std::net::Shutdown::Both);
                        if let Some(call) = conn.call.take() {
                            cut.push((conn.tab.clone(), call));
                        }
                    }
                }
                keep
            });
        }
        for (tab, call) in cut {
            self.missed(core, &tab, &call, true);
        }
    }
}

/// What the resident process says about itself when asked: how many apps are
/// connected to it (`host_lines`), so that one app does not take the bridge
/// off a machine another is using
struct HostJob;

impl Job for HostJob {
    fn name(&self) -> &'static str {
        "host"
    }

    fn frame(&self, core: &Arc<Core>, line: u64, frame: &Frame) -> bool {
        match frame {
            Frame::Op { id, op, .. } if op == "host_lines" => {
                // Every line not leaving is counted, a quiet one too: taking
                // the bridge off a machine an app is still on is what this
                // guards against, and a line that is really gone is ended by
                // its silence (`SILENCE`) soon enough to be asked again
                core.say(line, &Frame::Re { id: *id, r: json!({ "lines": core.lines().len() }), e: None });
                true
            }
            // Asked to end everything it holds and go. A unix machine is sent
            // SIGTERM for this; Windows has no such signal, and the door is
            // already this account's alone and keyed
            Frame::Op { id, op, .. } if op == "end_resident" => {
                log(&format!("the app on line {line} asked the resident process to end"));
                core.say(line, &Frame::Re { id: *id, r: json!({ "ending": true }), e: None });
                tell_to_end();
                true
            }
            _ => false,
        }
    }
}

/// The operations the app asks for by name ([`crate::farops`]), each on a
/// thread of its own so that a slow one holds up nothing else
#[derive(Default)]
struct OpsJob {
    /// How many operations each line has asked for and not yet been answered
    working: Arc<Mutex<HashMap<u64, usize>>>,
}

impl Job for OpsJob {
    fn name(&self) -> &'static str {
        "ops"
    }

    fn frame(&self, core: &Arc<Core>, line: u64, frame: &Frame) -> bool {
        let Frame::Op { id, op, p } = frame else { return false };
        if let Ok(mut w) = self.working.lock() {
            *w.entry(line).or_default() += 1;
        }
        let (core, id, op, p, working) = (Arc::clone(core), *id, op.clone(), p.clone(), Arc::clone(&self.working));
        std::thread::spawn(move || {
            let (r, e) = match crate::farops::run(&op, &p) {
                Ok(r) => (r, None),
                Err(e) => (Value::Null, Some(e)),
            };
            core.say(line, &Frame::Re { id, r, e });
            if let Ok(mut w) = working.lock()
                && let Some(n) = w.get_mut(&line)
            {
                *n = n.saturating_sub(1);
                if *n == 0 {
                    w.remove(&line);
                }
            }
        });
        true
    }

    fn holds(&self, line: u64) -> bool {
        self.working.lock().is_ok_and(|w| w.contains_key(&line))
    }
}

// ── The door ──────────────────────────────────────────────────────────────

/// The app's way in: start the resident process if there is none, show it the
/// key, and carry the line between this program's input and output and it.
///
/// Ends when either side does. The resident process stays after this does,
/// for as long as its jobs want it to
pub fn serve_port(home: PathBuf, input: impl std::io::Read, mut output: impl Write + Send + 'static) -> Result<()> {
    hold_program(&home);
    let run = home.join("run");
    let keep = run.join(KEEP_SOCK);
    // Started only when nobody is there: one slow to answer is waited for,
    // never replaced by a second
    let started = matches!(find(&keep), Found::Nobody);
    if started {
        start_daemon(&run)?;
    }
    let until = Instant::now() + Duration::from_secs(30);
    while probe(&keep).is_none() {
        if Instant::now() >= until {
            bail!(if started {
                "the resident bridge process did not start"
            } else {
                "the resident bridge process is there but did not name itself in time"
            });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let conn = crate::keepipe::connect(&keep).context("the resident bridge process did not answer")?;
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    if !matches!(serde_json::from_str::<Frame>(first.trim()), Ok(Frame::Hello { .. })) {
        bail!("the resident bridge process did not name itself");
    }
    let key = read_private(&run.join(KEY_FILE))?;
    (&conn).write_all(format!("{}\n", json!({ "role": "port", "key": key })).as_bytes())?;
    // The app hears the resident process name itself, as it always heard the
    // bridge do first
    output.write_all(first.as_bytes())?;
    output.flush()?;

    // From it to the app. When it shuts the line, the app is gone as far as
    // it knows: this ends, whatever the input is doing
    let back = std::thread::spawn(move || {
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) | Err(_) => std::process::exit(0),
                Ok(_) => {
                    if output.write_all(buf.as_bytes()).is_err() || output.flush().is_err() {
                        std::process::exit(0);
                    }
                }
            }
        }
    });
    // From the app to it. When the app stops talking, only this half is
    // shut: what it asked before that is still answered, and this ends when
    // the resident process has said all of it and shuts the line
    let mut to = conn;
    for line in BufReader::new(input).lines() {
        let Ok(l) = line else { break };
        if writeln!(to, "{l}").is_err() {
            break;
        }
    }
    let _ = to.shutdown(std::net::Shutdown::Write);
    let _ = back.join();
    Ok(())
}

/// Start the resident process, cut loose from the line that started it: in a
/// session of its own, so that the line ending does not end it. What it says
/// goes to `run/keep.log`, the one place to look when it misbehaves
#[cfg(windows)]
fn start_daemon(run: &Path) -> Result<()> {
    let home = run.parent().ok_or_else(|| anyhow!("{} has no folder above it", run.display()))?;
    start_resident(home, &["--keeper".into(), home.to_string_lossy().into_owned()])
}

/// Start this program as the resident process on Windows, cut loose from the
/// one starting it (local-keeper plan §4): no console, a process group of its
/// own, and out of any job the starter is in when the job lets it go -- so
/// ending the app, or the app's whole process tree, does not end it. Started
/// in its own folder, so the folder the app runs from can go away
#[cfg(windows)]
pub fn start_resident(home: &Path, args: &[String]) -> Result<()> {
    use std::os::windows::process::CommandExt as _;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let run = home.join("run");
    std::fs::create_dir_all(&run)?;
    let log_file = run.join(LOG_FILE);
    if std::fs::metadata(&log_file).is_ok_and(|m| m.len() > LOG_MOST) {
        let _ = std::fs::rename(&log_file, run.join(format!("{LOG_FILE}.1")));
    }
    let exe = std::env::current_exe()?;
    let spawn = |flags: u32| -> std::io::Result<std::process::Child> {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_file)?;
        std::process::Command::new(&exe)
            .args(args)
            .current_dir(home)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(log))
            .creation_flags(flags)
            .spawn()
    };
    let loose = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
    match spawn(loose | CREATE_BREAKAWAY_FROM_JOB) {
        Ok(_) => Ok(()),
        // A job that does not let its processes leave refuses the breakaway:
        // started inside it instead, and said, since ending that job ends it
        Err(e) if e.raw_os_error() == Some(5) => {
            log("the app is in a job that keeps its processes; the resident process starts inside it");
            spawn(loose).map(|_| ()).map_err(|e| anyhow!("could not start {}: {e}", exe.display()))
        }
        Err(e) => Err(anyhow!("could not start {}: {e}", exe.display())),
    }
}

#[cfg(unix)]
fn start_daemon(run: &Path) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt as _;
    use std::os::unix::process::CommandExt as _;
    std::fs::create_dir_all(run)?;
    std::fs::set_permissions(run, std::fs::Permissions::from_mode(0o700))?;
    let log_file = run.join(LOG_FILE);
    // Kept short: a log that only grows fills a small machine in the end
    if std::fs::metadata(&log_file).is_ok_and(|m| m.len() > LOG_MOST) {
        let _ = std::fs::rename(&log_file, run.join(format!("{LOG_FILE}.1")));
    }
    let log = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(&log_file)?;
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(log));
    // SAFETY: setsid is async-signal-safe and touches nothing of the parent
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_| ()).map_err(|e| anyhow!("could not start {}: {e}", exe.display()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// An app's door, as the resident process sees one: the key shown, and
    /// frames both ways
    struct App {
        to: UnixStream,
        from: BufReader<UnixStream>,
        hello: Frame,
    }

    impl App {
        fn open(run: &Path) -> App {
            let conn = UnixStream::connect(run.join(KEEP_SOCK)).unwrap();
            conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut from = BufReader::new(conn.try_clone().unwrap());
            let mut first = String::new();
            from.read_line(&mut first).unwrap();
            let hello = serde_json::from_str(first.trim()).unwrap();
            let key = read_private(&run.join(KEY_FILE)).unwrap();
            let mut to = conn;
            writeln!(to, "{}", json!({ "role": "port", "key": key })).unwrap();
            App { to, from, hello }
        }

        fn say(&mut self, f: &Frame) {
            writeln!(self.to, "{}", serde_json::to_string(f).unwrap()).unwrap();
        }

        /// The next frame, skipping none; `None` when nothing comes in time
        fn hear(&mut self) -> Option<Frame> {
            let mut l = String::new();
            match self.from.read_line(&mut l) {
                Ok(n) if n > 0 => serde_json::from_str(l.trim()).ok(),
                _ => None,
            }
        }

        /// Hear until a frame `want` says yes to
        fn hear_until(&mut self, want: impl Fn(&Frame) -> bool) -> Option<Frame> {
            (0..20).find_map(|_| self.hear().filter(|f| want(f)))
        }
    }

    /// A tab's `shikisha` command: the handshake with its key, and one call
    fn command(run: &Path, key: &str) -> (UnixStream, BufReader<UnixStream>) {
        let conn = UnixStream::connect(run.join(TABS_SOCK)).unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let from = BufReader::new(conn.try_clone().unwrap());
        let mut to = conn;
        writeln!(to, "{}", json!({ "token": key })).unwrap();
        (to, from)
    }

    /// While the app is away, a call that asks nothing back is kept (far-keep
    /// plan §4.6, the later version) and answered so; one that asks something
    /// back is answered that the PC is away. The app back gives the tab a new
    /// key, and the kept call is handed to it under that key and an id of its
    /// own, and struck out once answered
    #[test]
    fn a_report_made_while_away_is_kept_and_handed_over_when_the_app_is_back() {
        let home = std::env::temp_dir().join(format!("sk-fardaemon-kept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let run = home.join("run");
        std::fs::create_dir_all(&run).unwrap();
        let h = home.clone();
        let resident = std::thread::spawn(move || daemon(h));
        while probe(&run.join(KEEP_SOCK)).is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut pc = App::open(&run);
        pc.say(&Frame::Op { id: 1, op: "put_key".into(), p: json!({ "tab": "t1", "key": "k-old" }) });
        assert!(pc.hear_until(|f| matches!(f, Frame::Re { id: 1, .. })).is_some());
        pc.say(&Frame::Bye);
        std::thread::sleep(Duration::from_millis(300));

        let (mut to, mut from) = command(&run, "k-old");
        let mut hi = String::new();
        from.read_line(&mut hi).unwrap();
        writeln!(to, r#"{{"id":"7","method":"report","params":["job-1","all done"]}}"#).unwrap();
        let mut kept = String::new();
        from.read_line(&mut kept).unwrap();
        assert!(kept.contains("\"ok\":true") && kept.contains("kept"), "a report was not kept: {kept}");
        writeln!(to, r#"{{"id":"8","method":"tab_list","params":[]}}"#).unwrap();
        let mut away = String::new();
        from.read_line(&mut away).unwrap();
        assert!(away.contains("\"ok\":false") && away.contains("away"), "a call that asks back was kept: {away}");
        // A hook asking whether this turn is held: answered, and not written
        // down -- one comes every turn
        writeln!(to, r#"{{"id":"9","method":"confer_stop","params":["done"]}}"#).unwrap();
        let mut hook = String::new();
        from.read_line(&mut hook).unwrap();
        assert!(hook.contains("\"ok\":false"), "a hook was not answered at once: {hook}");
        // A second one kept, which the app will refuse
        writeln!(to, r#"{{"id":"10","method":"note","params":["t1","refused"]}}"#).unwrap();
        let mut kept2 = String::new();
        from.read_line(&mut kept2).unwrap();
        assert!(kept2.contains("\"ok\":true"), "a second report was not kept: {kept2}");
        drop((to, from, pc));

        // The app back, giving the tab a new key: the kept call comes to it
        let mut back = App::open(&run);
        back.say(&Frame::Op { id: 2, op: "put_key".into(), p: json!({ "tab": "t1", "key": "k-new" }) });
        let Some(Frame::Open { c }) = back.hear_until(|f| matches!(f, Frame::Open { .. })) else { panic!("nothing was handed over") };
        let Some(Frame::Line { l: hello, .. }) = back.hear_until(|f| matches!(f, Frame::Line { .. })) else { panic!("no handshake") };
        assert!(hello.contains("k-new"), "not under the key given now: {hello}");
        back.say(&Frame::Line { c, l: r#"{"ok":true,"result":"hello"}"#.into() });
        let Some(Frame::Line { l: call, .. }) = back.hear_until(|f| matches!(f, Frame::Line { .. })) else { panic!("no call") };
        let call: Value = serde_json::from_str(&call).unwrap();
        assert_eq!(call["method"], "report");
        assert_eq!(call["params"][1], "all done", "what it said was not kept whole");
        let id = call["id"].as_str().unwrap().to_string();
        assert!(id.starts_with("kept-"), "{id}");
        // A line under another id is not its answer
        back.say(&Frame::Line { c, l: json!({ "id": "other", "ok": true, "result": null }).to_string() });
        back.say(&Frame::Line { c, l: json!({ "id": id, "ok": true, "result": null }).to_string() });

        // The second: refused by the app, so it stays kept, tried once
        let Some(Frame::Open { c }) = back.hear_until(|f| matches!(f, Frame::Open { .. })) else { panic!("the second was not handed over") };
        assert!(back.hear_until(|f| matches!(f, Frame::Line { .. })).is_some(), "no handshake for the second");
        back.say(&Frame::Line { c, l: r#"{"ok":true,"result":"hello"}"#.into() });
        let Some(Frame::Line { l: call, .. }) = back.hear_until(|f| matches!(f, Frame::Line { .. })) else { panic!("no second call") };
        let call: Value = serde_json::from_str(&call).unwrap();
        assert_eq!(call["params"][1], "refused");
        back.say(&Frame::Line { c, l: json!({ "id": call["id"], "ok": false, "error": "no" }).to_string() });
        std::thread::sleep(Duration::from_millis(500));
        back.say(&Frame::Job { job: "tabs".into(), m: json!({ "do": "missed" }) });
        let Some(Frame::Job { m, .. }) = back.hear_until(|f| matches!(f, Frame::Job { job, m } if job == "tabs" && m["did"] == "missed"))
        else {
            panic!("no book")
        };
        assert_eq!(m["book"]["kept"].as_array().map(Vec::len), Some(1), "the one answered is not struck out alone: {m}");
        assert_eq!((m["book"]["kept"][0]["tries"].as_u64(), m["book"]["kept"][0]["line"].as_str().is_some_and(|l| l.contains("refused"))), (Some(1), true), "{m}");
        assert_eq!(m["book"]["calls"][0]["method"], "tab_list", "the call that asked back was not written down: {m}");
        assert_eq!(m["book"]["calls"].as_array().map(Vec::len), Some(1), "a hook of the moment was written down: {m}");
        drop(back);
        let _ = resident.join();
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The app says it is going (far-keep plan §4.6): its line ends at once, a
    /// tab's call after that is answered that the PC is away and written down
    /// -- the command, the tab it names and the tab that made it, never what
    /// it said -- and an app connected then hears of it
    #[test]
    fn a_call_after_the_app_went_is_answered_and_written_down() {
        let home = std::env::temp_dir().join(format!("sk-fardaemon-bye-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let run = home.join("run");
        std::fs::create_dir_all(&run).unwrap();
        let h = home.clone();
        let resident = std::thread::spawn(move || daemon(h));
        while probe(&run.join(KEEP_SOCK)).is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut pc = App::open(&run);
        pc.say(&Frame::Op { id: 1, op: "put_key".into(), p: json!({ "tab": "t1", "key": "k-bye" }) });
        assert!(pc.hear_until(|f| matches!(f, Frame::Re { id: 1, .. })).is_some(), "the key was not taken");
        let mut other = App::open(&run);
        pc.say(&Frame::Bye);
        std::thread::sleep(Duration::from_millis(300));

        let (mut to, mut from) = command(&run, "k-bye");
        let mut hi = String::new();
        from.read_line(&mut hi).unwrap();
        writeln!(to, r#"{{"id":"7","method":"ask_tab","params":["teal","the secret words"]}}"#).unwrap();
        let mut answer = String::new();
        from.read_line(&mut answer).unwrap();
        assert!(answer.contains("\"ok\":false") && answer.contains("away"), "not told the PC is away at once: {answer}");

        let heard = other
            .hear_until(|f| matches!(f, Frame::Job { job, m } if job == "tabs" && m["did"] == "missed"))
            .expect("the app connected was not told");
        let Frame::Job { m, .. } = heard else { unreachable!() };
        let book: crate::farmissed::Book = serde_json::from_value(m["book"].clone()).unwrap();
        assert_eq!(book.calls.len(), 1, "{book:?}");
        let c = &book.calls[0];
        assert_eq!((c.method.as_str(), c.to.as_str(), c.tab.as_str(), c.cut), ("ask_tab", "teal", "t1", false));
        assert!(!m.to_string().contains("secret"), "what the call said was written down: {m}");

        // Looked at: struck out, and the app hears what is left
        other.say(&Frame::Job { job: "tabs".into(), m: json!({ "do": "seen" }) });
        let left = other
            .hear_until(|f| matches!(f, Frame::Job { job, m } if job == "tabs" && m["did"] == "missed"))
            .expect("no answer to having looked");
        let Frame::Job { m, .. } = left else { unreachable!() };
        assert_eq!(m["book"]["calls"].as_array().map(Vec::len), Some(0), "{m}");
        drop((to, from, pc, other));
        let _ = resident.join();
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The resident process of far-keep plan §4.3, stage 2 of §10: one for the
    /// account however many start at once, the tabs' commands carried to the
    /// app that started each tab and answered at once when it is away, the
    /// older bridge's socket untouched, and gone -- sockets and key with it --
    /// a short while after the last app went
    #[test]
    fn the_resident_process_holds_its_jobs_and_leaves_when_nothing_is_left() {
        let home = std::env::temp_dir().join(format!("sk-fardaemon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let run = home.join("run");
        std::fs::create_dir_all(&run).unwrap();
        // The older bridge's socket, which must come through untouched
        let old = run.join("shikisha.sock");
        let _old_listener = UnixListener::bind(&old).unwrap();

        // Two started at the same moment: one becomes resident, the other
        // finds it and leaves
        let (a, b) = (home.clone(), home.clone());
        let first = std::thread::spawn(move || daemon(a));
        let second = std::thread::spawn(move || daemon(b));
        let until = Instant::now() + Duration::from_secs(10);
        while probe(&run.join(KEEP_SOCK)).is_none() {
            assert!(Instant::now() < until, "nobody became resident");
            std::thread::sleep(Duration::from_millis(50));
        }
        let loser_left = (0..100).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            first.is_finished() || second.is_finished()
        });
        assert!(loser_left, "both stayed: two resident processes");
        let winner = if first.is_finished() { second } else { first };

        // An app that asks and stops talking at once is still answered
        let mut brief = App::open(&run);
        brief.say(&Frame::Op { id: 7, op: "ping".into(), p: json!({}) });
        brief.to.shutdown(std::net::Shutdown::Write).unwrap();
        assert!(brief.hear_until(|f| matches!(f, Frame::Re { id: 7, .. })).is_some(), "asked, then silent: never answered");
        drop(brief);
        // Its line is let go once its answers are out, a moment later
        std::thread::sleep(Duration::from_millis(500));

        let mut pc = App::open(&run);
        let mut server = App::open(&run);
        match &pc.hello {
            Frame::Hello { jobs, program, .. } => {
                assert!(jobs.contains(&"tabs".into()) && jobs.contains(&"ops".into()) && jobs.contains(&"host".into()), "{jobs:?}");
                assert!(!program.is_empty(), "it does not say which build it runs");
            }
            other => panic!("not a hello: {other:?}"),
        }
        // How many apps are connected: one app does not take the bridge off
        // a machine another is using
        // Asked until both doors are in: an app is counted once the resident
        // process has read its key, a moment after the door is open
        let mut lines = None;
        for id in 100..150 {
            pc.say(&Frame::Op { id, op: "host_lines".into(), p: json!({}) });
            lines = pc.hear_until(|f| matches!(f, Frame::Re { id: i, .. } if *i == id));
            if matches!(lines, Some(Frame::Re { ref r, .. }) if r["lines"] == 2) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(matches!(lines, Some(Frame::Re { ref r, .. }) if r["lines"] == 2), "{lines:?}");

        // A key given by the PC: that tab's command goes to the PC alone
        pc.say(&Frame::Op { id: 1, op: "put_key".into(), p: json!({ "tab": "t1", "key": "k-pc" }) });
        assert!(pc.hear_until(|f| matches!(f, Frame::Re { id: 1, e: None, .. })).is_some(), "the key was not kept");
        let (mut to, _from) = command(&run, "k-pc");
        writeln!(to, r#"{{"id":"1","method":"state","params":[]}}"#).unwrap();
        let opened = pc.hear_until(|f| matches!(f, Frame::Open { .. }));
        assert!(opened.is_some(), "the command did not reach the app that started its tab");
        let token = pc.hear_until(|f| matches!(f, Frame::Line { .. }));
        assert!(matches!(token, Some(Frame::Line { ref l, .. }) if l.contains("k-pc")), "{token:?}");
        server.to.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        server.from.get_ref().set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        assert!(server.hear_until(|f| matches!(f, Frame::Open { .. })).is_none(), "another app heard a tab that is not its");
        drop(to);

        // A command in the middle of a call when its app goes is told it may
        // or may not have been done
        let (mut to, mut midway) = command(&run, "k-pc");
        writeln!(to, r#"{{"id":"9","method":"state","params":[]}}"#).unwrap();
        assert!(pc.hear_until(|f| matches!(f, Frame::Line { l, .. } if l.contains("\"9\""))).is_some());

        // The PC goes: its tab's command is answered at once that it is away
        drop(pc);
        let mut told = String::new();
        midway.read_line(&mut told).unwrap();
        assert!(told.contains("may or may not have been carried out"), "{told}");
        drop((to, midway));
        std::thread::sleep(Duration::from_millis(300));
        let (mut to, mut from) = command(&run, "k-pc");
        let mut hi = String::new();
        from.read_line(&mut hi).unwrap();
        writeln!(to, r#"{{"id":"2","method":"state","params":[]}}"#).unwrap();
        let mut answer = String::new();
        from.read_line(&mut answer).unwrap();
        assert!(answer.contains("\"ok\":false") && answer.contains("away"), "{answer}");
        drop((to, from));

        // The last app goes: after a short while the process ends, taking
        // its sockets and key, and leaving the older bridge's socket
        drop(server);
        let gone = (0..300).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            winner.is_finished()
        });
        assert!(gone, "it stayed with nothing to hold");
        assert!(!run.join(KEEP_SOCK).exists() && !run.join(TABS_SOCK).exists() && !run.join(KEY_FILE).exists());
        assert!(old.exists(), "the older bridge's socket was touched");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A resident process slow to name itself is still resident: a second
    /// one leaves its socket as it is rather than taking it over
    #[test]
    fn a_slow_resident_process_is_not_taken_for_a_dead_one() {
        let home = std::env::temp_dir().join(format!("sk-fardaemon-slow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let run = home.join("run");
        std::fs::create_dir_all(&run).unwrap();
        // Takes connections and says nothing
        let silent = UnixListener::bind(&run.join(KEEP_SOCK)).unwrap();
        let held = std::thread::spawn(move || {
            let kept: Vec<UnixStream> = silent.incoming().take(1).flatten().collect();
            std::thread::sleep(Duration::from_secs(5));
            drop(kept);
        });
        let before = inode(&run.join(KEEP_SOCK));
        assert!(matches!(find(&run.join(KEEP_SOCK)), Found::Silent));
        daemon(home.clone()).unwrap();
        assert_eq!(inode(&run.join(KEEP_SOCK)), before, "the slow one's socket was taken");
        assert!(!run.join(TABS_SOCK).exists(), "a second resident process started");
        let _ = held.join();
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A job message to the terminals job
    fn term(app: &mut App, m: Value) {
        app.say(&Frame::Job { job: crate::farterms::NAME.into(), m });
    }

    /// The next message of the terminals job that `want` says yes to
    fn term_said(app: &mut App, want: impl Fn(&Value) -> bool) -> Option<Value> {
        (0..400).find_map(|_| match app.hear()? {
            Frame::Job { m, .. } if want(&m) => Some(m),
            _ => None,
        })
    }

    fn unb64(v: &Value) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap_or_default()).unwrap_or_default()
    }

    /// What a terminal shows, put together the way a window does: the state
    /// it was handed, its held-back tail, and the output after it
    fn screen_of(attached: &Value, outs: &[Value]) -> String {
        let mut p = vt100::Parser::new(24, 80, 0);
        p.restore(vt100::Screen::from_snapshot(&unb64(&attached["state"])).unwrap());
        p.process(&unb64(&attached["pending"]));
        let from = attached["seq"].as_u64().unwrap();
        for o in outs.iter().filter(|o| o["seq"].as_u64().unwrap() > from) {
            p.process(&unb64(&o["b"]));
        }
        p.screen().contents()
    }

    /// What a terminal does while no app owns it is what the app said when it
    /// opened it (far-keep plan §4.3): end after the short while a dropped
    /// line gets, end after a set time, or go on -- and the resident process
    /// stays while any is to be kept, keeps the code of one that ended for its
    /// app to come back for, and ends once there is nothing left to keep
    #[test]
    fn what_a_terminal_does_while_nobody_owns_it_is_the_apps_to_say() {
        // SAFETY: tests that read SHELL do not run beside this one
        unsafe { std::env::set_var("SHELL", "/bin/bash") };
        let home = std::env::temp_dir().join(format!("sk-farterms-away-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let run = home.join("run");
        std::fs::create_dir_all(&run).unwrap();
        let h = home.clone();
        let resident = std::thread::spawn(move || daemon(h));
        while probe(&run.join(KEEP_SOCK)).is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
        // Three terminals, one of each: ends with its app (after the short
        // while a dropped line gets), kept for 4 seconds, kept for good
        let mut pc = App::open(&run);
        let mut ids = Vec::new();
        for (n, away) in [json!("stop"), json!({ "seconds": 4 }), json!("always")].into_iter().enumerate() {
            term(&mut pc, json!({ "do": "open", "ref": n + 1, "tab": format!("t{n}"), "rows": 24, "cols": 80,
                "then": "exec cat", "away": away }));
            let opened = term_said(&mut pc, |m| m["did"] == "opened").expect("not opened");
            ids.push((opened["term"].as_u64().unwrap(), opened["gen"].as_str().unwrap().to_string()));
        }
        let generation = ids[0].1.clone();
        // The app goes, and comes back to look at each in turn
        drop(pc);
        let alive = |id: u64, tab: &str| -> &'static str {
            let mut a = App::open(&run);
            term(&mut a, json!({ "do": "list", "ref": 99 }));
            let listed = term_said(&mut a, |m| m["did"] == "list").expect("no list");
            let me = listed["terms"].as_array().unwrap().iter().find(|t| t["term"] == id && t["tab"] == tab).cloned();
            match me {
                None => "gone",
                Some(t) if t["ended"] == true => "ended",
                Some(_) => "running",
            }
        };
        std::thread::sleep(Duration::from_millis(1000));
        assert_eq!(alive(ids[0].0, "t0"), "running", "ended before a dropped line could come back");
        std::thread::sleep(Duration::from_millis(2500));
        assert_eq!(alive(ids[0].0, "t0"), "ended", "kept after its app went for good");
        assert_eq!(alive(ids[1].0, "t1"), "running", "the one kept for a while was not kept");
        std::thread::sleep(Duration::from_millis(3500));
        assert_eq!(alive(ids[1].0, "t1"), "ended", "kept past the time it was to be kept for");
        assert_eq!(alive(ids[2].0, "t2"), "running", "the one kept for good was ended");
        // With nobody connected, the resident process stays while one is
        // still to be kept, and the code of the one kept for a while waits
        // for its app to come back for it
        std::thread::sleep(Duration::from_millis(12_000));
        assert!(!resident.is_finished(), "the resident process went with a terminal still to be kept");
        let mut back = App::open(&run);
        term(&mut back, json!({ "do": "attach", "term": ids[1].0, "gen": generation, "tab": "t1" }));
        assert!(term_said(&mut back, |m| m["did"] == "over").is_some(), "its end was not kept for its app");
        // Stopped by the person from the list of what runs there: by its
        // identity, without being owned (far-keep plan §7.6)
        term(&mut back, json!({ "do": "end", "term": ids[2].0, "gen": generation }));
        assert!(term_said(&mut back, |m| m["did"] == "ending").is_some(), "not told it is ending");
        let ended = (0..50).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            alive(ids[2].0, "t2") == "ended"
        });
        assert!(ended, "the one kept for good was not stopped");
        // The app has both codes: nothing is kept for it any more
        for (id, _) in &ids[1..] {
            term(&mut back, json!({ "do": "forget", "term": id }));
        }
        drop(back);
        // Nothing left to keep: it ends on its own
        let gone = (0..300).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            resident.is_finished()
        });
        assert!(gone, "it stayed with nothing to keep");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// Two apps attach to one terminal at the same moment (far-keep plan
    /// §10.1 "reconnecting at once"): one of them owns it in the end, the
    /// other is told it was taken, and only the owner's keys go in
    #[test]
    fn two_apps_attaching_at_once_leave_one_owner() {
        // SAFETY: tests that read SHELL do not run beside this one
        unsafe { std::env::set_var("SHELL", "/bin/bash") };
        let home = std::env::temp_dir().join(format!("sk-farterms-once-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let run = home.join("run");
        std::fs::create_dir_all(&run).unwrap();
        let h = home.clone();
        let resident = std::thread::spawn(move || daemon(h));
        while probe(&run.join(KEEP_SOCK)).is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
        // Opened by an app that then goes, and a second app kept on the
        // line so the resident process stays while the two come
        let keeper = App::open(&run);
        let mut pc = App::open(&run);
        term(&mut pc, json!({ "do": "open", "ref": 1, "tab": "t1", "rows": 24, "cols": 80, "then": "exec cat" }));
        let opened = term_said(&mut pc, |m| m["did"] == "opened").expect("not opened");
        let (id, generation) = (opened["term"].as_u64().unwrap(), opened["gen"].as_str().unwrap().to_string());
        assert!(term_said(&mut pc, |m| m["did"] == "attached").is_some());
        drop(pc);

        let (mut a, mut b) = (App::open(&run), App::open(&run));
        let attach = json!({ "do": "attach", "term": id, "gen": generation, "tab": "t1", "rows": 24, "cols": 80 });
        term(&mut a, attach.clone());
        term(&mut b, attach);
        let at_a = term_said(&mut a, |m| m["did"] == "attached").expect("a was not attached");
        let at_b = term_said(&mut b, |m| m["did"] == "attached").expect("b was not attached");
        let (owner_a, owner_b) = (at_a["owner"].as_u64().unwrap(), at_b["owner"].as_u64().unwrap());
        assert_ne!(owner_a, owner_b, "both were given the same generation");
        // The later generation owns it; the one before is told
        let ((mut winner, at_w, owner_w), (mut loser, owner_l)) =
            if owner_a > owner_b { ((a, at_a, owner_a), (b, owner_b)) } else { ((b, at_b, owner_b), (a, owner_a)) };
        assert!(term_said(&mut loser, |m| m["did"] == "taken").is_some(), "the earlier one was not told");
        term(&mut loser, json!({ "do": "in", "term": id, "owner": owner_l, "b": crate::farterms_b64(b"from-loser\r") }));
        assert!(term_said(&mut loser, |m| m["did"] == "refused").is_some(), "the earlier one's keys went in");
        term(&mut winner, json!({ "do": "in", "term": id, "owner": owner_w, "b": crate::farterms_b64(b"from-winner\r") }));
        let mut outs = Vec::new();
        while !screen_of(&at_w, &outs).contains("from-winner") {
            outs.push(term_said(&mut winner, |m| m["did"] == "out" && m["term"] == id).expect("the owner's keys did not go in"));
        }
        assert!(!screen_of(&at_w, &outs).contains("from-loser"), "{:?}", screen_of(&at_w, &outs));

        term(&mut winner, json!({ "do": "stop", "term": id, "owner": owner_w }));
        assert!(term_said(&mut winner, |m| m["did"] == "ended").is_some());
        drop((winner, loser, keeper));
        let _ = resident.join();
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The terminals job (far-keep plan §4.4): a terminal opened by one app,
    /// taken by another with its state, the first told and refused, asked
    /// about by the wrong tab or generation answered "unknown", stopped with
    /// its code handed over and then forgotten -- and, with nobody owning it,
    /// its program's question answered here
    #[test]
    fn a_terminal_is_held_taken_over_and_answered_for() {
        // SAFETY: tests that read SHELL do not run beside this one
        unsafe { std::env::set_var("SHELL", "/bin/bash") };
        let home = std::env::temp_dir().join(format!("sk-farterms-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let run = home.join("run");
        std::fs::create_dir_all(&run).unwrap();
        let h = home.clone();
        let resident = std::thread::spawn(move || daemon(h));
        while probe(&run.join(KEEP_SOCK)).is_none() {
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut pc = App::open(&run);
        term(&mut pc, json!({ "do": "open", "ref": 1, "tab": "t1", "rows": 24, "cols": 80,
            "then": "printf 'ready\\n'; exec cat" }));
        let opened = term_said(&mut pc, |m| m["did"] == "opened").expect("not opened");
        let (id, generation) = (opened["term"].as_u64().unwrap(), opened["gen"].as_str().unwrap().to_string());
        let first = term_said(&mut pc, |m| m["did"] == "attached").expect("not attached");
        let owner = first["owner"].as_u64().unwrap();
        let mut outs = Vec::new();
        let seen = |outs: &mut Vec<Value>, app: &mut App, what: &str| {
            for _ in 0..200 {
                if screen_of(&first, outs).contains(what) {
                    return true;
                }
                match term_said(app, |m| m["did"] == "out") {
                    Some(o) => outs.push(o),
                    None => return false,
                }
            }
            false
        };
        assert!(seen(&mut outs, &mut pc, "ready"), "the program's first words: {:?}", screen_of(&first, &outs));
        term(&mut pc, json!({ "do": "in", "term": id, "owner": owner, "b": crate::farterms_b64(b"hello there\r") }));
        assert!(seen(&mut outs, &mut pc, "hello there"), "typing did not arrive: {:?}", screen_of(&first, &outs));

        // Another app takes it: handed the same screen, the first told
        let mut server = App::open(&run);
        term(&mut server, json!({ "do": "attach", "term": id, "gen": generation, "tab": "t1", "rows": 24, "cols": 80 }));
        let taken = term_said(&mut server, |m| m["did"] == "attached").expect("not handed over");
        let shown = screen_of(&taken, &[]);
        assert!(shown.contains("ready") && shown.contains("hello there"), "{shown:?}");
        assert!(term_said(&mut pc, |m| m["did"] == "taken").is_some(), "the first owner was not told");
        term(&mut pc, json!({ "do": "in", "term": id, "owner": owner, "b": crate::farterms_b64(b"late\r") }));
        assert!(term_said(&mut pc, |m| m["did"] == "refused").is_some(), "an old owner's keys went in");

        // The wrong tab, the wrong generation: not known, never "ended"
        term(&mut server, json!({ "do": "attach", "term": id, "gen": generation, "tab": "t2" }));
        assert!(term_said(&mut server, |m| m["did"] == "unknown").is_some());
        term(&mut server, json!({ "do": "attach", "term": id, "gen": "another", "tab": "t1" }));
        assert!(term_said(&mut server, |m| m["did"] == "unknown").is_some());

        // Stopped: its code comes, is kept until said to be had, then forgotten
        let new_owner = taken["owner"].as_u64().unwrap();
        term(&mut server, json!({ "do": "stop", "term": id, "owner": new_owner }));
        assert!(term_said(&mut server, |m| m["did"] == "ended").is_some(), "no end said");
        term(&mut server, json!({ "do": "attach", "term": id, "gen": generation, "tab": "t1" }));
        assert!(term_said(&mut server, |m| m["did"] == "over").is_some(), "an ended terminal is not said to be over");
        term(&mut server, json!({ "do": "forget", "term": id }));
        std::thread::sleep(Duration::from_millis(200));
        term(&mut server, json!({ "do": "attach", "term": id, "gen": generation, "tab": "t1" }));
        assert!(term_said(&mut server, |m| m["did"] == "unknown").is_some(), "kept after it was had");

        // Nobody owns it: the program's question is answered here
        term(&mut server, json!({ "do": "open", "ref": 2, "tab": "t3", "rows": 24, "cols": 80, "away": "always",
            "then": "sleep 2; printf '\\033[6n'; IFS= read -r -s -t 5 -d R x; printf 'answer:%s\\n' \"${x#*[}\"; exec cat" }));
        let o2 = term_said(&mut server, |m| m["did"] == "opened").unwrap();
        let id2 = o2["term"].as_u64().unwrap();
        drop(server);
        std::thread::sleep(Duration::from_millis(4000));
        let mut again = App::open(&run);
        term(&mut again, json!({ "do": "attach", "term": id2, "gen": generation, "tab": "t3", "rows": 24, "cols": 80 }));
        let back = term_said(&mut again, |m| m["did"] == "attached").expect("not attached again");
        let shown = screen_of(&back, &[]);
        assert!(shown.contains("answer:"), "the question was not answered while nobody owned it: {shown:?}");

        // Owned, the program's question is the owner's to answer: the
        // resident process keeps quiet, so the program hears one answer, the
        // owner's (here a cursor at 9;9, which is not where it is)
        term(&mut again, json!({ "do": "open", "ref": 3, "tab": "t4", "rows": 24, "cols": 80,
            "then": "sleep 1; printf '\\033[6n'; IFS= read -r -s -t 5 -d R x; printf 'got:%s\\n' \"${x#*[}\"; exec cat" }));
        let o3 = term_said(&mut again, |m| m["did"] == "opened").unwrap();
        let id3 = o3["term"].as_u64().unwrap();
        let first3 = term_said(&mut again, |m| m["did"] == "attached" && m["term"] == id3).unwrap();
        let owner3 = first3["owner"].as_u64().unwrap();
        let mut outs3: Vec<Value> = Vec::new();
        let mut answered = false;
        for _ in 0..300 {
            let Some(o) = term_said(&mut again, |m| m["did"] == "out" && m["term"] == id3) else { break };
            if !answered && unb64(&o["b"]).windows(4).any(|w| w == b"\x1b[6n") {
                term(&mut again, json!({ "do": "in", "term": id3, "owner": owner3, "b": crate::farterms_b64(b"\x1b[9;9R") }));
                answered = true;
            }
            outs3.push(o);
            // The answer the program printed, on a line of its own (the command
            // typed to ask it has the same word in it)
            if screen_of(&first3, &outs3).lines().any(|l| l.trim_start().starts_with("got:")) {
                break;
            }
        }
        let shown3 = screen_of(&first3, &outs3);
        assert!(
            answered,
            "the question never came: {shown3:?} after {} outputs: {:?}",
            outs3.len(),
            outs3.iter().map(|o| String::from_utf8_lossy(&unb64(&o["b"])).into_owned()).collect::<Vec<_>>()
        );
        assert!(shown3.lines().any(|l| l.trim_start().starts_with("got:9;9")), "answered by someone other than the owner: {shown3:?}");
        // The one kept for good is stopped, so the resident process has
        // nothing left to keep and ends
        term(&mut again, json!({ "do": "stop", "term": id2, "owner": back["owner"] }));
        assert!(term_said(&mut again, |m| m["did"] == "ended" && m["term"] == id2).is_some(), "the kept one did not end");
        term(&mut again, json!({ "do": "forget", "term": id2 }));
        drop((pc, again));
        let _ = resident.join();
        let _ = std::fs::remove_dir_all(&home);
    }
}

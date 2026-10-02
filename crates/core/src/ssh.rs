//! Talking SSH ourselves, instead of running `ssh.exe` in a terminal.
//!
//! `ssh.exe` is a fine program, and a tab can still run it. But it asks for a
//! password by printing `user@host's password:` and reading the keyboard --
//! which means a stored password cannot be handed over without pretending to
//! type it, and the prompt appears on screen either way. Every program that
//! remembers a password for you (PuTTY, WinSCP, Termius) speaks the protocol
//! itself, because the password belongs in the *authentication* that happens
//! before a terminal exists, not in the terminal.
//!
//! So this module opens the connection: it authenticates, checks the server is
//! the one we met last time, and then hands out two things over the same
//! connection --
//!
//!   - a **terminal**, dressed as a [`portable_pty::MasterPty`] so that a tab
//!     cannot tell the difference between it and a local program. Everything a
//!     tab does with bytes -- the screen, the state detector, the automation
//!     hooks, the recording -- keeps working with nothing changed.
//!   - a **file connection** for the SFTP tab, which is a separate tab because
//!     transferring files and typing commands are separate jobs, whatever the
//!     wire underneath happens to be.
//!
//! Everything to do with the network runs on one background thread with its
//! own runtime. The window thread never waits for a socket, and nothing async
//! leaks into the rest of the program: what leaves this module is byte queues
//! and plain function calls.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Result, anyhow, bail};

/// How long to wait for a server to answer at all
const CONNECT_MS: u64 = 20_000;
/// How long an idle connection is kept after its last tab has gone
const IDLE_KEEP_MS: u64 = 60_000;
/// How many keepalives may go unanswered before the connection is given up.
/// Three, so that one lost packet on a bad minute is not a disconnection
const KEEPALIVE_MISSES: usize = 3;

/// Where to connect, as who, and with what.
///
/// It carries the *name* a credential is filed under, never the credential:
/// the same rule the rest of the program follows, so that a tab's settings can
/// be read, written, exported and looked at without a password being in them.
/// The value is fetched at the moment of connecting, through [`use_secrets`]
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Spec {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// The name the password is stored under (`ssh/<desk>/<tab>/password`)
    pub password_key: Option<String>,
    /// A private key file, and the name its passphrase is stored under
    pub key: Option<String>,
    pub passphrase_key: Option<String>,
    /// The server this one is reached *through*, when it cannot be reached
    /// directly. Its own address and its own credential -- and it may in turn
    /// be reached through another, which is why this is a Spec and not a pair
    /// of strings
    pub jump: Option<Box<Spec>>,
    /// Seconds between keepalive packets. None means none are sent, which is
    /// right for a network that leaves a quiet connection alone and wrong for
    /// one that cuts it after a few minutes
    pub keepalive: Option<u64>,
    /// What to run on the far end to serve files, instead of asking the server
    /// for its own file service. For a machine where reading the files that
    /// matter means being somebody else (`sudo su -`)
    pub file_command: Option<String>,
}

/// The connection credentials, handed over whenever the settings are read.
///
/// A copy, and deliberately a small one: only the `ssh/` names, and only
/// because the store itself lives on the window's thread and cannot be reached
/// from this one. Nothing else is copied, and a reload replaces the lot rather
/// than adding to it -- a password taken out of the settings must not go on
/// working because this thread still remembers it.
static SECRETS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn store() -> &'static Mutex<HashMap<String, String>> {
    SECRETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Tell the connection thread what the ssh credentials are now. Called when
/// the settings are read and again whenever they are read afresh
pub fn use_secrets(mut all: HashMap<String, String>) {
    all.retain(|k, _| k.starts_with("ssh/"));
    if let Ok(mut m) = store().lock() {
        *m = all;
    }
}

fn secret(key: &Option<String>) -> Option<String> {
    let key = key.as_ref()?;
    store().lock().ok()?.get(key).cloned()
}

/// One credential put in, the rest left as they are: tests run at once, and
/// one replacing the lot would take another's password away mid-test
#[cfg(test)]
fn set_secret(key: &str, value: &str) {
    if let Ok(mut m) = store().lock() {
        m.insert(key.to_string(), value.to_string());
    }
}

impl Spec {
    /// What the person sees this connection called, and what its remembered
    /// host key is filed under. The user is part of it only in the sense that
    /// the same machine is the same machine: the key belongs to the address
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// What a live connection is filed under while the program runs.
    ///
    /// Not the same question as [`Spec::address`]. A host key belongs to the
    /// machine, so it is remembered by address alone. A *connection* is also
    /// who signed in and which way it was reached -- two people on one server,
    /// or the same server reached directly and through a bastion, are two
    /// connections and must not be handed each other's
    pub fn route(&self) -> String {
        let mut at = format!("{}@{}", self.user, self.address());
        if let Some(j) = &self.jump {
            at = format!("{}>{at}", j.route());
        }
        at
    }

    /// Which server this is, for the name and colour a person gave it
    /// ("Production", "Staging").
    ///
    /// A third question, between the other two. Not [`Spec::route`]: who signs
    /// in does not change which server it is -- `deploy@` and `root@` on
    /// production are both production, and a name given from one tab has to
    /// be worn by every tab that reaches the same machine. Not
    /// [`Spec::address`] either: the way there *is* part of it, because the
    /// same private address behind two bastions is two machines. That is the
    /// usual shape of a production and a staging network, and filing them
    /// together would put one's name on the other.
    ///
    /// Written one way whatever case the address was typed in, since a host
    /// name is not case-sensitive and the person typed it twice in two tabs
    pub fn machine(&self) -> String {
        let mut at = machine_key(&self.address());
        if let Some(j) = &self.jump {
            at = format!("{}>{at}", j.machine());
        }
        at
    }
}

/// A server's name for its mark, spelled the one way [`Spec::machine`] spells
/// it. For reading one back out of the settings, where a person may have
/// written it by hand
pub fn machine_key(written: &str) -> String {
    written.split('>').map(|hop| hop.trim().to_lowercase()).collect::<Vec<_>>().join(">")
}

/// The fingerprints of the servers we have met, by address.
///
/// The same promise `known_hosts` makes, kept in the shape everything else
/// here is kept in. A server whose key has changed is refused rather than
/// asked about: at that moment there is no way to tell "they reinstalled it"
/// from "somebody is standing in the middle", and the second one is the one
/// that costs a password.
// A test run never writes into the real one. The probe server makes a new key
// every time and listens on whatever port the machine hands out, so a kept file
// fills up with dead ports -- and the day the machine hands out one of them
// again, a test fails saying the key changed. What the real answer would be is
// checked by a test of its own
#[cfg(test)]
fn known_hosts_path() -> std::path::PathBuf {
    crate::test_temp("known-hosts").join("known-hosts.json")
}
#[cfg(not(test))]
fn known_hosts_path() -> std::path::PathBuf {
    real_known_hosts_path()
}

/// Beside everything else this program keeps.
fn real_known_hosts_path() -> std::path::PathBuf {
    crate::config::root_dir().join("data").join("known-hosts.json")
}

fn known_hosts() -> HashMap<String, String> {
    std::fs::read_to_string(known_hosts_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// A server that answered with a key other than the one remembered for it,
/// held until a person says whether to trust the new one.
///
/// Asked, never decided here. At the moment a key changes there is no telling
/// a reinstalled server from somebody standing in the middle, and only the
/// person knows whether the server was reinstalled. What they are shown is
/// both fingerprints; what they trust is the one they were shown, and nothing
/// that answered after it
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct KeyChange {
    /// The server, as its key is filed ([`Spec::machine`])
    pub machine: String,
    /// The fingerprint remembered from before
    pub before: String,
    /// The fingerprint that answered this time
    pub now: String,
}

fn key_change_list() -> &'static Mutex<Vec<KeyChange>> {
    static CHANGED: OnceLock<Mutex<Vec<KeyChange>>> = OnceLock::new();
    CHANGED.get_or_init(Default::default)
}

/// The servers whose key changed and nobody has answered for yet, in the
/// order they were met
pub fn key_changes() -> Vec<KeyChange> {
    key_change_list().lock().map(|l| l.clone()).unwrap_or_default()
}

fn key_changed(change: KeyChange) {
    if let Ok(mut l) = key_change_list().lock() {
        match l.iter_mut().find(|c| c.machine == change.machine) {
            Some(was) => *was = change,
            None => l.push(change),
        }
    }
}

fn key_settled(machine: &str) {
    if let Ok(mut l) = key_change_list().lock() {
        l.retain(|c| c.machine != machine);
    }
}

/// A person's answer about a changed key. `trust` remembers `fingerprint` as
/// the server's key from now on -- only when it is the one that answered, the
/// one they were shown; a key that changed again since is refused, and asked
/// about afresh the next time. Not trusting puts the question away until the
/// server is next reached. Answers whether a key was trusted
pub fn answer_key_change(machine: &str, fingerprint: &str, trust: bool) -> Result<bool> {
    let Some(change) = key_changes().into_iter().find(|c| c.machine == machine) else {
        return Ok(false);
    };
    if !trust {
        key_settled(machine);
        return Ok(false);
    }
    if change.now != fingerprint {
        bail!(crate::i18n::tp("err.ssh.key_moved", &[("host", machine)]));
    }
    remember_host(machine, fingerprint)?;
    key_settled(machine);
    crate::append_hook_log(&format!("ssh: the new key at {machine} was trusted: {} -> {fingerprint}", change.before));
    Ok(true)
}

fn remember_host(addr: &str, fingerprint: &str) -> Result<()> {
    // Read, changed and written whole: two servers met at the same moment
    // must not each write the file without the other
    static WRITING: Mutex<()> = Mutex::new(());
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = known_hosts();
    all.insert(addr.to_string(), fingerprint.to_string());
    let path = known_hosts_path();
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    crate::crypto::write_atomic(&path, &serde_json::to_string_pretty(&all)?)
}

/// One thing in a folder -- on the far end, or on this machine. The same shape
/// either way, so a listing means one thing wherever it is read
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Entry {
    pub name: String,
    pub dir: bool,
    pub size: u64,
    /// Seconds since the epoch, as the far end reports it. 0 when it says nothing
    pub modified: u64,
}

/// What to do with files on the far end.
///
/// Reading, writing and rearranging, each one thing. Copying a whole folder is
/// not here: it is a loop over these, written by whoever wants it, in the same
/// way `split_pane` and `show` stayed two commands
#[derive(Debug, Clone)]
pub enum FileJob {
    List { path: String },
    Stat { path: String },
    /// Bring a file here. `overwrite` is asked for the same way `Put` asks for
    /// it: this end may be the only copy just as easily as the other one
    Get { from: String, to: std::path::PathBuf, overwrite: bool },
    /// Read a file out, without writing it down anywhere. What a script wants
    /// when it is going to look at the contents rather than keep them
    Read { path: String },
    /// Send a file there
    Put { from: std::path::PathBuf, to: String, overwrite: bool },
    /// Put these contents in a file there, whatever was in it. What the editor
    /// saves as: the text is in hand, not in a file here, and whether it may
    /// replace what is there was settled by whoever read it first
    Write { to: String, bytes: Vec<u8> },
    MakeDir { path: String },
    Rename { from: String, to: String },
    /// A file, or a folder with nothing in it. Never a folder with things in it:
    /// one wrong path would take everything under it, and there is no undo on
    /// the far end
    Remove { path: String },
}

/// What came back. `Nothing` is a job that either worked or said why
#[derive(Debug, Clone)]
pub enum FileAnswer {
    Nothing,
    Listing(Vec<Entry>),
    One(Entry),
    /// A file's contents, as they were. Bytes rather than text because that is
    /// what was on the far end, and deciding it is not text is the caller's
    /// to make
    Bytes(Vec<u8>),
}

/// What the connection thread is asked to do: each one a piece of work that
/// runs on its own the moment it arrives, so a slow one -- a clone, a large
/// file, a server that takes its time to answer -- holds up nothing but
/// itself. What is typed into a terminal is not here at all: it goes to that
/// terminal's own queue (see [`ToShell`])
enum Job {
    /// Open a terminal on `spec` and report where its bytes will arrive
    Shell {
        spec: Spec,
        rows: u16,
        cols: u16,
        /// Where it should stand once it is open. A shell over there starts
        /// where the far end puts it and the asking carries no folder, so the
        /// only way to say is to type it -- which is done here, on the channel
        /// this module already holds. Done anywhere else it would need the
        /// writer, and the writer belongs to whoever is at the keyboard
        cwd: Option<String>,
        /// What to run once it stands there, typed the same way: the tab's
        /// command when it is a program (`claude`), nothing for a terminal
        then: Option<String>,
        /// Raised when the connection went without the far end ending the
        /// shell -- no exit, no close, the line simply gone
        lost: Arc<AtomicBool>,
        out: Sender<Vec<u8>>,
        reply: Sender<Result<ShellTx>>,
    },
    /// Something to do with files, on the same connection a terminal uses.
    /// Given up, and its channel closed, once `wait_ms` has gone by
    Files {
        spec: Spec,
        job: FileJob,
        wait_ms: u64,
        reply: Sender<Result<FileAnswer>>,
    },
    /// One command, run to the end, on the same connection everything else
    /// uses. Stopped, and its channel closed, once `wait_ms` has gone by
    Exec {
        spec: Spec,
        command: String,
        wait_ms: u64,
        reply: Sender<Result<Ran>>,
    },
    /// A connection made to this PC, carried to a port on the server as if it
    /// had been made there (see [`forward`])
    Tunnel {
        spec: Spec,
        port: u16,
        stream: std::net::TcpStream,
    },
    /// A program run over there with its input and output carried both ways
    /// through a socket here, for as long as either end keeps it (see [`pipe`])
    Pipe {
        spec: Spec,
        command: String,
        stream: std::net::TcpStream,
    },
}

/// What is said to one open terminal.
///
/// Each terminal has a queue of its own, read by a task of its own, so a
/// keystroke waits for nothing but the keystrokes before it in the same
/// terminal -- never for a clone on another server, never for another
/// terminal whose far end has stopped reading
enum ToShell {
    Data(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    Close,
}

type ShellTx = tokio::sync::mpsc::UnboundedSender<ToShell>;

/// How much longer than a job's own limit its caller waits for the answer.
/// The job answers for itself when its time is up, in words that say which
/// part ran late, after closing what it had open; this is only for a thread
/// that has stopped answering altogether
const ANSWER_GRACE_MS: u64 = 5_000;

/// What a command on the far end did.
///
/// Kept apart from a terminal on purpose: a terminal is for a person to read
/// and has no ending, while this is for the program to act on and has one. Git
/// run over there is the first caller, and the exit code is the whole point --
/// a worktree that could not be made has to fail here, not look like output
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    pub code: i32,
    pub out: String,
    pub err: String,
}

impl Ran {
    pub fn ok(&self) -> bool {
        self.code == 0
    }
    /// What went wrong, in whatever the far end was willing to say
    pub fn said(&self) -> String {
        let said = self.err.trim();
        match said.is_empty() {
            false => said.to_string(),
            true => self.out.trim().to_string(),
        }
    }
}

/// The way in. One thread, one runtime, started the first time anything here
/// is asked for -- the same shape the HTTP gateway uses, and for the same
/// reason: the window thread must never wait on a socket
fn hub() -> &'static tokio::sync::mpsc::UnboundedSender<Job> {
    static HUB: OnceLock<tokio::sync::mpsc::UnboundedSender<Job>> = OnceLock::new();
    HUB.get_or_init(|| {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Job>();
        std::thread::Builder::new()
            .name("ssh".into())
            .spawn(move || run_hub(rx))
            .expect("could not start the ssh thread");
        tx
    })
}

fn run_hub(mut rx: tokio::sync::mpsc::UnboundedReceiver<Job>) {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            crate::append_hook_log(&format!("ssh: could not start the runtime: {e}"));
            return;
        }
    };
    rt.block_on(async move {
        tokio::spawn(async {
            let mut every = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                every.tick().await;
                close_idle().await;
            }
        });
        // Each job is a task of its own. Taken one after another, the first
        // slow one -- a clone over a thin line -- held every other server's
        // terminals, transfers and commands behind it until it was done
        while let Some(job) = rx.recv().await {
            tokio::spawn(run_job(job));
        }
    });
}

async fn run_job(job: Job) {
    match job {
        Job::Shell { spec, rows, cols, cwd, then, out, lost, reply } => {
            let opened = tokio::time::timeout(
                std::time::Duration::from_millis(CONNECT_MS + ANSWER_GRACE_MS),
                open_shell(&spec, rows, cols, cwd, then, out, lost),
            )
            .await
            .unwrap_or_else(|_| Err(anyhow!(crate::i18n::tp("err.ssh.timeout", &[("host", &spec.address())]))));
            // Nobody waiting for it any more is a terminal nobody will type
            // into: the queue comes back with the refusal and is dropped, and
            // the terminal's task closes it on the far end
            let _ = reply.send(opened);
        }
        Job::Files { spec, job, wait_ms, reply } => {
            let _ = reply.send(do_file_job(&spec, job, deadline(wait_ms)).await);
        }
        Job::Exec { spec, command, wait_ms, reply } => {
            let _ = reply.send(do_exec(&spec, &command, deadline(wait_ms)).await);
        }
        Job::Tunnel { spec, port, stream } => {
            let opened = async {
                let lease = lease(&spec).await?;
                let ch = lease.handle.channel_open_direct_tcpip("127.0.0.1", port as u32, "127.0.0.1", 0).await?;
                stream.set_nonblocking(true)?;
                Ok::<_, anyhow::Error>((lease, ch, tokio::net::TcpStream::from_std(stream)?))
            }
            .await;
            match opened {
                // Carried both ways until either end closes. The connection
                // is held for as long as this is -- a page loading is many of
                // these at once, and none may find it let go under it
                Ok((_lease, ch, mut here)) => {
                    let mut there = ch.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut here, &mut there).await;
                }
                Err(e) => crate::append_hook_log(&format!("ssh: could not carry a connection to port {port} on {}: {e:#}", spec.address())),
            }
        }
        Job::Pipe { spec, command, stream } => {
            let opened = async {
                let lease = lease(&spec).await?;
                let ch = lease.handle.channel_open_session().await?;
                ch.exec(true, command.as_str()).await?;
                stream.set_nonblocking(true)?;
                Ok::<_, anyhow::Error>((lease, ch, tokio::net::TcpStream::from_std(stream)?))
            }
            .await;
            match opened {
                // Held for as long as the program runs: the connection is in
                // use the whole time, and is not let go of under it
                Ok((_lease, ch, mut here)) => {
                    let mut there = ch.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut here, &mut there).await;
                }
                Err(e) => crate::append_hook_log(&format!("ssh: could not run a piped program on {}: {e:#}", spec.address())),
            }
        }
    }
}

fn deadline(wait_ms: u64) -> tokio::time::Instant {
    tokio::time::Instant::now() + std::time::Duration::from_millis(wait_ms)
}

// ── Connections ─────────────────────────────────────────────────────────────

/// One connection this thread has made, or is making, or had until lately.
struct Conn {
    /// Held while the connection is being made, so that two asks for one
    /// server at once make one connection, and asks for other servers are
    /// not held up by it at all
    gate: Arc<tokio::sync::Mutex<Option<Open>>>,
    /// How many terminals, commands, transfers and connections through it
    /// are on it now. The one thing that says whether it is in use
    users: usize,
    /// When the last of them went, so an unused one is let go rather than
    /// held open for the life of the program
    idle_since: Option<std::time::Instant>,
}

/// A connection that is open and signed in.
struct Open {
    handle: Arc<russh::client::Handle<Client>>,
    /// The bastion it is reached through, held for exactly as long as it is.
    /// Without this a bastion with no terminal of its own was let go while
    /// the server behind it was still being used, and took it along
    _via: Option<Lease>,
}

/// Every connection, by [`conn_key`]. A plain lock, taken only for a moment
/// and never across a wait on the network
fn conns() -> &'static Mutex<HashMap<String, Conn>> {
    static CONNS: OnceLock<Mutex<HashMap<String, Conn>>> = OnceLock::new();
    CONNS.get_or_init(Default::default)
}

/// A connection in use, handed to whoever uses it. Dropping it is how it is
/// given back: when the last one goes, the connection starts its idle clock
struct Lease {
    key: String,
    handle: Arc<russh::client::Handle<Client>>,
}

/// Being counted as a user of a connection before it is open, so it is not
/// let go while it is being made
struct Claim(String);

impl Claim {
    fn take(key: &str) -> (Claim, Arc<tokio::sync::Mutex<Option<Open>>>) {
        let mut all = conns().lock().unwrap_or_else(|e| e.into_inner());
        let c = all.entry(key.to_string()).or_insert_with(|| Conn {
            gate: Arc::new(tokio::sync::Mutex::new(None)),
            users: 0,
            idle_since: None,
        });
        c.users += 1;
        c.idle_since = None;
        (Claim(key.to_string()), Arc::clone(&c.gate))
    }
}

fn give_back(key: &str) {
    let mut all = conns().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = all.get_mut(key) {
        c.users = c.users.saturating_sub(1);
        if c.users == 0 {
            c.idle_since = Some(std::time::Instant::now());
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        // Turned into a lease, the lease gives it back instead
        if !self.0.is_empty() {
            give_back(&self.0);
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        give_back(&self.key);
    }
}

impl Claim {
    fn into_lease(mut self, handle: Arc<russh::client::Handle<Client>>) -> Lease {
        Lease { key: std::mem::take(&mut self.0), handle }
    }
}

/// Let go of every connection nobody has used for [`IDLE_KEEP_MS`]
async fn close_idle() {
    let now = std::time::Instant::now();
    // Taken out under the lock and let go after it: letting go of one drops
    // the lease on its bastion, and giving that back takes the same lock
    let gone: Vec<Open> = {
        let mut all = conns().lock().unwrap_or_else(|e| e.into_inner());
        let old: Vec<String> = all
            .iter()
            .filter(|(_, c)| {
                c.users == 0 && c.idle_since.is_some_and(|t| now.duration_since(t).as_millis() as u64 > IDLE_KEEP_MS)
            })
            .map(|(k, _)| k.clone())
            .collect();
        let mut gone = Vec::new();
        for k in old {
            let Some(c) = all.get(&k) else { continue };
            // Nobody is using it, so nobody holds this; a connection being
            // made is counted as used and never gets here
            let Ok(mut open) = c.gate.try_lock() else { continue };
            if let Some(o) = open.take() {
                gone.push(o);
            }
            drop(open);
            all.remove(&k);
        }
        gone
    };
    for o in gone {
        let _ = o.handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
    }
}

/// What a connection is filed under while the program runs: which way it was
/// reached and who signed in ([`Spec::route`]), and with what.
///
/// The credential is part of it because a connection is only as good as what
/// it was signed in with. Filed by route alone, a password changed in the
/// settings went unused for as long as the old connection lived -- trying it
/// seemed to work, and the first reconnection found out it did not -- and a
/// password taken out went on working. With it, the next thing asked for signs
/// in afresh with what the settings say now, and the terminals already open
/// keep the connection they have until they close. Kept as a keyed hash, never
/// as the secret, and the key file's contents count, not only its name
fn conn_key(spec: &Spec) -> String {
    use std::hash::BuildHasher;
    static SALT: OnceLock<std::collections::hash_map::RandomState> = OnceLock::new();
    let key_file = spec.key.as_ref().map(|p| std::fs::read(p).unwrap_or_default());
    let with = SALT.get_or_init(Default::default).hash_one((
        secret(&spec.password_key),
        &spec.key,
        key_file,
        secret(&spec.passphrase_key),
        spec.jump.as_deref().map(conn_key),
    ));
    format!("{}#{with:016x}", spec.route())
}

/// A connection to this server, signed in, for as long as the lease is held.
/// The one place "have we met this server" and "who are we" are answered, so
/// that a terminal and a file transfer agree about both
fn lease<'a>(spec: &'a Spec) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Lease>> + Send + 'a>> {
    // A bastion is reached the same way its server is, so this calls itself.
    // An async function that does that has a size nobody can write down, which
    // is what the box is for
    Box::pin(async move {
        let (claim, gate) = Claim::take(&conn_key(spec));
        let mut open = gate.lock().await;
        if let Some(o) = open.as_ref().filter(|o| !o.handle.is_closed()) {
            let handle = Arc::clone(&o.handle);
            return Ok(claim.into_lease(handle));
        }
        // Gone, or never made
        *open = None;
        let made = connect(spec).await?;
        let handle = Arc::clone(&made.handle);
        *open = Some(made);
        Ok(claim.into_lease(handle))
    })
}

/// The fingerprint remembered for this server, by [`Spec::machine`].
///
/// By machine and not by address, because the address alone is not a machine:
/// one private address behind two bastions is two servers with two keys, and
/// filed as one, the second of them was refused as a key that had changed.
/// A file written before carries a server reached directly under its address
/// as typed; that is still the same machine and is read as it. A server behind
/// a bastion is never read from such a line -- that line is the very one that
/// could not tell two networks apart
fn remembered(spec: &Spec) -> Option<String> {
    let all = known_hosts();
    let name = spec.machine();
    if let Some(fp) = all.get(&name) {
        return Some(fp.clone());
    }
    if spec.jump.is_some() {
        return None;
    }
    all.iter().find(|(k, _)| !k.contains('>') && machine_key(k) == name).map(|(_, fp)| fp.clone())
}

/// Open and sign in to one server
async fn connect(spec: &Spec) -> Result<Open> {
    let addr = spec.address();
    let name = spec.machine();
    let config = Arc::new(russh::client::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
        // A quiet connection is cut by some networks after a few minutes. When
        // the settings ask for it, something is said on the wire often enough
        // that nothing in between decides the connection is over
        keepalive_interval: spec
            .keepalive
            .filter(|n| *n > 0)
            .map(std::time::Duration::from_secs),
        keepalive_max: KEEPALIVE_MISSES,
        ..Default::default()
    });
    let seen = remembered(spec);
    let met = Arc::new(Mutex::new(None::<String>));
    let handler = Client { expected: seen.clone(), met: Arc::clone(&met) };
    // Through a bastion, the way in is a channel on that bastion's own
    // connection rather than a socket this machine opened. It is made first,
    // and on its own, so that "the bastion would not have us" is said in the
    // bastion's own words instead of arriving as a failure to reach the server
    // behind it. Everything after this point -- the host key included -- is the
    // far server answering for itself
    let (via, hop) = match &spec.jump {
        None => (None, None),
        Some(through) => {
            let via = lease(through).await?;
            let open = via
                .handle
                .channel_open_direct_tcpip(spec.host.clone(), spec.port as u32, "127.0.0.1", 0)
                .await
                .map_err(|e| {
                    anyhow!(crate::i18n::tp(
                        "err.ssh.jump",
                        &[("jump", &through.address()), ("host", &addr), ("e", &e.to_string())]
                    ))
                })?;
            (Some(via), Some(open))
        }
    };
    let connect = async {
        match hop {
            None => {
                russh::client::connect(config, (spec.host.as_str(), spec.port), handler).await
            }
            Some(hop) => {
                russh::client::connect_stream(config, hop.into_stream(), handler).await
            }
        }
    };
    let mut handle = match tokio::time::timeout(
        std::time::Duration::from_millis(CONNECT_MS),
        connect,
    )
    .await
    {
        Err(_) => bail!(crate::i18n::tp("err.ssh.timeout", &[("host", &addr)])),
        Ok(Err(e)) => {
            // The one failure worth its own words: we did meet this server
            // before, and what answered is not it. Everything else is "could
            // not reach it", which is what the message says
            let now = met.lock().ok().and_then(|m| m.clone());
            if let Some(now) = now.filter(|now| seen.as_ref() != Some(now)) {
                key_changed(KeyChange { machine: name.clone(), before: seen.clone().unwrap_or_default(), now });
                let message = if seen.is_none() { "err.ssh.host_unknown" } else { "err.ssh.host_changed" };
                bail!(crate::i18n::tp(message, &[("host", &addr)]));
            }
            bail!(crate::i18n::tp(
                "err.ssh.connect",
                &[("host", &addr), ("e", &e.to_string())]
            ))
        }
        Ok(Ok(h)) => h,
    };
    // Migrate a previously trusted legacy entry to its current name. A
    // persistence failure must stop us before any credential is sent.
    if let Some(fp) = met.lock().ok().and_then(|m| m.clone())
        && known_hosts().get(&name) != Some(&fp)
    {
        remember_host(&name, &fp)?;
    }
    // It answered with the key we know, so any question about it is over
    key_settled(&name);

    // A key if one is named, and the stored password otherwise. Asked for now
    // rather than kept: this is the only moment it is needed
    let password = secret(&spec.password_key);
    let ok = match (&spec.key, &password) {
        (None, None) => bail!(crate::i18n::tp(
            "err.ssh.no_credential",
            &[("host", &addr)]
        )),
        (None, Some(pw)) => handle
            .authenticate_password(spec.user.clone(), pw.clone())
            .await
            .map_err(|e| anyhow!("{e}"))?
            .success(),
        (Some(path), _) => {
            let phrase = secret(&spec.passphrase_key);
            let key = russh::keys::load_secret_key(path, phrase.as_deref())
                .map_err(|e| anyhow!(crate::i18n::tp("err.ssh.key", &[("e", &e.to_string())])))?;
            let alg = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
            handle
                .authenticate_publickey(
                    spec.user.clone(),
                    russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), alg),
                )
                .await
                .map_err(|e| anyhow!("{e}"))?
                .success()
        }
    };
    if !ok {
        bail!(crate::i18n::tp(
            "err.ssh.refused",
            &[("user", &spec.user), ("host", &addr)]
        ));
    }
    Ok(Open { handle: Arc::new(handle), _via: via })
}

async fn open_shell(
    spec: &Spec,
    rows: u16,
    cols: u16,
    cwd: Option<String>,
    then: Option<String>,
    out: Sender<Vec<u8>>,
    lost: Arc<AtomicBool>,
) -> Result<ShellTx> {
    let lease = lease(spec).await?;
    let channel = lease.handle.channel_open_session().await?;
    channel
        .request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await?;
    channel.request_shell(true).await?;
    let (mut reading, writing) = channel.split();
    // Typed, so it is on screen like anything else typed, and so that
    // nothing else has to know it happened. The folder first, and the
    // program in it only if the folder is there: a program started
    // somewhere else would work on the wrong files
    if let Some(line) = typed_first(cwd.as_deref(), then.as_deref()) {
        let _ = writing.data(line.as_bytes()).await;
    }
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ToShell>();
    // What is typed, in the order it was typed, on a task that holds the
    // connection for as long as the terminal is open. It ends when the tab
    // closes the terminal, when the tab has let go of every way to reach it,
    // or when the far end will take no more -- and then the next keystroke
    // is refused rather than said to have arrived
    tokio::spawn(async move {
        let _held = lease;
        while let Some(said) = rx.recv().await {
            match said {
                ToShell::Data(data) => {
                    if let Err(e) = writing.data(&data[..]).await {
                        crate::append_hook_log(&format!("ssh: could not send: {e}"));
                        break;
                    }
                }
                ToShell::Resize { rows, cols } => {
                    let _ = writing.window_change(cols as u32, rows as u32, 0, 0).await;
                }
                ToShell::Close => break,
            }
        }
        rx.close();
        let _ = writing.eof().await;
        let _ = writing.close().await;
    });
    // Everything the far end says goes straight to the tab's queue, from a task
    // of its own, so one quiet connection never holds up a busy one
    tokio::spawn(async move {
        // Whether the far end said the shell is over. A stream that stops
        // without that is a connection that went -- a keepalive nobody
        // answered, a router that forgot it -- and the shell with it
        let mut ended = false;
        while let Some(msg) = reading.wait().await {
            let chunk = match msg {
                // A terminal has one screen. What a program writes to its
                // error output belongs on it, in the order it arrived, exactly
                // as it would locally -- keeping them apart here would put the
                // two halves of a compiler's opinion in different places
                russh::ChannelMsg::Data { data } => data.to_vec(),
                russh::ChannelMsg::ExtendedData { data, .. } => data.to_vec(),
                russh::ChannelMsg::ExitStatus { .. } | russh::ChannelMsg::ExitSignal { .. } => {
                    ended = true;
                    continue;
                }
                russh::ChannelMsg::Eof | russh::ChannelMsg::Close => {
                    ended = true;
                    break;
                }
                _ => continue,
            };
            if out.send(chunk).is_err() {
                ended = true;
                break;
            }
        }
        // Said on the screen, where the person is looking, before the tab
        // is seen to end; the tab is opened again once the server answers
        if !ended {
            lost.store(true, Ordering::SeqCst);
            let said = format!("\r\n\x1b[33m{}\x1b[0m\r\n", crate::i18n::t("msg.ssh.lost"));
            let _ = out.send(said.into_bytes());
        }
    });
    Ok(tx)
}

/// The words for a job that ran out of time: not answering at all is one
/// thing, answering and taking too long is another
fn late(spec: &Spec, connected: bool, wait_ms: u64) -> anyhow::Error {
    match connected {
        false => anyhow!(crate::i18n::tp("err.ssh.timeout", &[("host", &spec.address())])),
        true => anyhow!(crate::i18n::tp(
            "err.ssh.too_long",
            &[("host", &spec.address()), ("secs", &wait_ms.div_ceil(1000).to_string())]
        )),
    }
}

/// Run one command over there and wait for it to finish.
///
/// Its own channel, closed when the command ends, so nothing of it is left on
/// the connection a terminal is using. What is collected is both streams and
/// the exit code, because the caller is a program: "it printed something" is
/// not the same answer as "it worked".
///
/// Stopped when its time is up, not merely stopped being waited for: the
/// command is told to end and its channel is closed, so a caller that gave up
/// does not leave it running on the far end, holding the connection open
async fn do_exec(spec: &Spec, command: &str, until: tokio::time::Instant) -> Result<Ran> {
    use russh::ChannelMsg;
    let wait_ms = until.saturating_duration_since(tokio::time::Instant::now()).as_millis() as u64;
    let lease = tokio::time::timeout_at(until, lease(spec)).await.map_err(|_| late(spec, false, wait_ms))??;
    let mut channel = tokio::time::timeout_at(until, lease.handle.channel_open_session())
        .await
        .map_err(|_| late(spec, true, wait_ms))??;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    // A server may send the code and keep talking, so the streams are drained
    // to the end rather than stopping at the first word about the ending
    let mut code: Option<i32> = None;
    let ran = tokio::time::timeout_at(until, async {
        channel.exec(true, command).await?;
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { ref data } => out.extend_from_slice(data),
                ChannelMsg::ExtendedData { ref data, .. } => err.extend_from_slice(data),
                ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status as i32),
                // OpenSSH ends the output first and says how the command ended
                // after: stopping at the end of the output lost the code, and every
                // command that worked read as one that did not
                ChannelMsg::Eof => {}
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        Ok::<_, anyhow::Error>(())
    })
    .await;
    if ran.is_err() {
        let _ = channel.signal(russh::Sig::TERM).await;
    }
    let _ = channel.close().await;
    match ran {
        Err(_) => Err(late(spec, true, wait_ms)),
        Ok(Err(e)) => Err(e),
        Ok(Ok(())) => Ok(Ran {
            // No word about the ending is not the same as a clean one: a server
            // that closed on us has not told us the command worked
            code: code.unwrap_or(-1),
            out: String::from_utf8_lossy(&out).to_string(),
            err: String::from_utf8_lossy(&err).to_string(),
        }),
    }
}

/// One file job, on the connection the terminals are already using.
///
/// A file session is opened for the job and closed after it. Holding one open
/// would be faster and would also mean a tab that transfers nothing keeps a
/// channel open on the far end for as long as the app runs; a transfer is not
/// something that happens hundreds of times a second. Closed the same way
/// when its time is up, so a transfer given up on is not still going
async fn do_file_job(spec: &Spec, job: FileJob, until: tokio::time::Instant) -> Result<FileAnswer> {
    let wait_ms = until.saturating_duration_since(tokio::time::Instant::now()).as_millis() as u64;
    let lease = tokio::time::timeout_at(until, lease(spec)).await.map_err(|_| late(spec, false, wait_ms))??;
    let sftp = tokio::time::timeout_at(until, async {
        let channel = lease.handle.channel_open_session().await?;
        // Asking for a reply, so a server that has no file service says so here
        // rather than leaving the first packet unanswered
        match spec.file_command.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            // The ordinary way: the server runs its own file service
            None => channel.request_subsystem(true, "sftp").await?,
            // A command instead, for a machine where the files that matter belong
            // to somebody else. What it prints has to be the file protocol and
            // nothing else -- a shell that greets you first will not be understood
            Some(cmd) => channel.exec(true, cmd).await?,
        }
        Ok::<_, anyhow::Error>(russh_sftp::client::SftpSession::new(channel.into_stream()).await?)
    })
    .await
    .map_err(|_| late(spec, true, wait_ms))??;
    let out = tokio::time::timeout_at(until, run_file_job(&sftp, job)).await;
    // Bounded too: a far end that stopped answering does not answer this
    // either, and the channel closes when the session is dropped regardless
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), sftp.close()).await;
    out.map_err(|_| late(spec, true, wait_ms))?
}

async fn run_file_job(
    sftp: &russh_sftp::client::SftpSession,
    job: FileJob,
) -> Result<FileAnswer> {
    match job {
        FileJob::List { path } => {
            let mut out = Vec::new();
            for e in sftp.read_dir(&path).await? {
                let m = e.metadata();
                out.push(Entry {
                    name: e.file_name(),
                    dir: m.is_dir(),
                    size: m.size.unwrap_or(0),
                    modified: m.mtime.unwrap_or(0) as u64,
                });
            }
            // Folders first and then by name, which is the order a person
            // reading a list expects and not the order a server happens to
            // answer in
            out.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));
            Ok(FileAnswer::Listing(out))
        }
        FileJob::Stat { path } => {
            let m = sftp.metadata(&path).await?;
            Ok(FileAnswer::One(Entry {
                name: path.rsplit('/').next().unwrap_or(&path).to_string(),
                dir: m.is_dir(),
                size: m.size.unwrap_or(0),
                modified: m.mtime.unwrap_or(0) as u64,
            }))
        }
        FileJob::Read { path } => Ok(FileAnswer::Bytes(sftp.read(&path).await?)),
        FileJob::Get { from, to, overwrite } => {
            // Asked before the reading, not after: after is too late, and the
            // file it would have asked about is already gone
            if !overwrite && to.exists() {
                bail!(crate::i18n::tp(
                    "err.ssh.file_exists",
                    &[("path", &to.display().to_string())]
                ));
            }
            let bytes = sftp.read(&from).await?;
            // Written off this thread: a large file on a slow disk would
            // otherwise hold every other server's work while it is written
            tokio::task::spawn_blocking(move || {
                if let Some(d) = to.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&to, bytes)
            })
            .await??;
            Ok(FileAnswer::Nothing)
        }
        FileJob::Put { from, to, overwrite } => {
            if !overwrite && sftp.try_exists(&to).await.unwrap_or(false) {
                bail!(crate::i18n::tp("err.ssh.file_exists", &[("path", &to)]));
            }
            let bytes = tokio::task::spawn_blocking(move || std::fs::read(&from)).await??;
            write_whole(sftp, &to, &bytes).await?;
            Ok(FileAnswer::Nothing)
        }
        FileJob::Write { to, bytes } => {
            write_whole(sftp, &to, &bytes).await?;
            Ok(FileAnswer::Nothing)
        }
        FileJob::MakeDir { path } => {
            sftp.create_dir(&path).await?;
            Ok(FileAnswer::Nothing)
        }
        FileJob::Rename { from, to } => {
            sftp.rename(&from, &to).await?;
            Ok(FileAnswer::Nothing)
        }
        FileJob::Remove { path } => {
            // A folder is only let go when it is empty, and the far end is the
            // one that decides that -- asking here and deleting after would be
            // a race with whoever else is on that machine
            match sftp.metadata(&path).await?.is_dir() {
                true => sftp.remove_dir(&path).await?,
                false => sftp.remove_file(&path).await?,
            }
            Ok(FileAnswer::Nothing)
        }
    }
}

/// A file there that holds exactly these bytes afterwards.
///
/// Opened to be made if it is not there and emptied if it is. The library's
/// own `write` opens for writing and nothing else, which a real server takes
/// literally: a file that is not there yet cannot be opened, so nothing new
/// could ever be sent, and a file that is there is written over from the start
/// and not cut short -- so a shorter file sent over a longer one kept the
/// longer one's tail. The test server this was first checked against was
/// kinder than OpenSSH, which is why neither showed until a real one did
async fn write_whole(sftp: &russh_sftp::client::SftpSession, to: &str, bytes: &[u8]) -> Result<()> {
    let mut file = sftp.create(to).await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, bytes).await?;
    file.close().await?;
    Ok(())
}

/// Whether the server is the one we met before.
///
/// Unknown and changed keys both end the handshake before credentials are
/// requested. The person must confirm the displayed fingerprint first.
struct Client {
    expected: Option<String>,
    met: Arc<Mutex<Option<String>>>,
}

impl russh::client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fp = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => {
                key.fingerprint(Default::default()).to_string()
            }
            russh::keys::PublicKeyOrCertificate::Certificate(c) => {
                c.public_key().fingerprint(Default::default()).to_string()
            }
        };
        if let Ok(mut m) = self.met.lock() {
            *m = Some(fp.clone());
        }
        Ok(match &self.expected {
            None => false,
            Some(seen) => seen == &fp,
        })
    }
}

// ── What a tab is handed ────────────────────────────────────────────────────

/// The reading half of a terminal on the far end.
///
/// A queue rather than a socket, because everything above it reads with
/// `std::io::Read` on a thread of its own and knows nothing about runtimes
struct ShellReader {
    rx: Receiver<Vec<u8>>,
    /// What was read but did not fit in the caller's buffer last time
    rest: Vec<u8>,
    at: usize,
}

impl Read for ShellReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at >= self.rest.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.rest = chunk;
                    self.at = 0;
                }
                // The far end has gone. Read returning 0 is how every reader
                // above this says "that was the end", the same as a local
                // program exiting
                Err(_) => return Ok(0),
            }
        }
        let n = (self.rest.len() - self.at).min(buf.len());
        buf[..n].copy_from_slice(&self.rest[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// The writing half. Typing goes to this terminal's own queue on the
/// connection thread (see [`ToShell`])
struct ShellWriter {
    tx: ShellTx,
}

impl Write for ShellWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // A terminal whose far end will take no more says so, like a local
        // program that has exited, instead of swallowing what was typed
        self.tx
            .send(ToShell::Data(buf.to_vec()))
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the connection to the server has closed"))?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A terminal on another machine, wearing the same face as a local one.
///
/// Implementing `MasterPty` is what lets the rest of the program stay exactly
/// as it is: a tab reads bytes, writes bytes and says how big it is, and never
/// asks which of those crossed a network
#[derive(Debug)]
pub struct SshPty {
    tx: ShellTx,
    size: Mutex<portable_pty::PtySize>,
    reader: Mutex<Option<ShellReader>>,
    writer_taken: AtomicBool,
}

impl portable_pty::MasterPty for SshPty {
    fn resize(&self, size: portable_pty::PtySize) -> Result<(), anyhow::Error> {
        if let Ok(mut s) = self.size.lock() {
            *s = size;
        }
        let _ = self.tx.send(ToShell::Resize { rows: size.rows, cols: size.cols });
        Ok(())
    }

    fn get_size(&self) -> Result<portable_pty::PtySize, anyhow::Error> {
        Ok(*self.size.lock().map_err(|_| anyhow!("size"))?)
    }

    fn try_clone_reader(&self) -> Result<Box<dyn Read + Send>, anyhow::Error> {
        // Once, like the writer: there is one queue of bytes from the far end,
        // and two readers of it would each get half a screen
        match self.reader.lock().map_err(|_| anyhow!("reader"))?.take() {
            Some(r) => Ok(Box::new(r)),
            None => bail!("the reader for this connection has already been taken"),
        }
    }

    fn take_writer(&self) -> Result<Box<dyn Write + Send>, anyhow::Error> {
        if self.writer_taken.swap(true, Ordering::SeqCst) {
            bail!("the writer for this connection has already been taken");
        }
        Ok(Box::new(ShellWriter { tx: self.tx.clone() }))
    }

    /// Three questions a unix caller may ask of a local pty, none of which has
    /// an answer for a terminal on another machine.
    ///
    /// There is no descriptor here -- the bytes arrive over a network
    /// connection, not through a device -- and the far end's process group is
    /// the far end's business. Saying so is the honest answer; inventing a
    /// number would have something try to signal a process on this machine that
    /// happens to share it.
    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<i32> {
        None
    }

    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::fd::RawFd> {
        None
    }

    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf> {
        None
    }
}

impl std::fmt::Debug for ShellReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShellReader")
    }
}

/// Ending a remote terminal: there is no process here to kill, so this closes
/// the channel and lets the far end tidy up after its own shell
#[derive(Debug, Clone)]
pub struct SshKiller {
    tx: ShellTx,
}

impl portable_pty::ChildKiller for SshKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        let _ = self.tx.send(ToShell::Close);
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

/// Open a terminal on another machine.
///
/// Returns the pair a tab needs and nothing else, so that the tab's own code
/// reads the same whether the shell is here or on the other side of the world
pub fn shell(
    spec: &Spec,
    rows: u16,
    cols: u16,
    cwd: Option<&str>,
    then: Option<&str>,
) -> Result<(Box<dyn portable_pty::MasterPty + Send>, Box<dyn portable_pty::ChildKiller + Send + Sync>, Arc<AtomicBool>)>
{
    let (out_tx, out_rx) = channel::<Vec<u8>>();
    let (reply_tx, reply_rx) = channel::<Result<ShellTx>>();
    let lost = Arc::new(AtomicBool::new(false));
    hub()
        .send(Job::Shell {
            spec: spec.clone(),
            rows,
            cols,
            cwd: cwd.map(str::to_string),
            then: then.map(str::to_string),
            out: out_tx,
            lost: lost.clone(),
            reply: reply_tx,
        })
        .map_err(|_| anyhow!(crate::i18n::t("err.ssh.no_thread")))?;
    let tx = reply_rx
        .recv_timeout(std::time::Duration::from_millis(CONNECT_MS + 2 * ANSWER_GRACE_MS))
        .map_err(|_| anyhow!(crate::i18n::tp("err.ssh.timeout", &[("host", &spec.address())])))??;
    let pty = SshPty {
        tx: tx.clone(),
        size: Mutex::new(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }),
        reader: Mutex::new(Some(ShellReader { rx: out_rx, rest: Vec::new(), at: 0 })),
        writer_taken: AtomicBool::new(false),
    };
    Ok((Box::new(pty), Box::new(SshKiller { tx }), lost))
}

/// The first line typed into a shell that just opened over there: the folder
/// to stand in, the program to run in it, both, or nothing.
///
/// One line, joined with `&&`, so the program runs only if the folder was
/// there. The folder is quoted the way a server's shell wants; the program
/// already is (see [`crate::worktree::for_a_shell`])
pub fn typed_first(cwd: Option<&str>, then: Option<&str>) -> Option<String> {
    let at = cwd.map(str::trim).filter(|a| !a.is_empty()).map(|a| format!("cd {}", sh_quote(a)));
    let run = then.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    match (at, run) {
        (Some(at), Some(run)) => Some(format!("{at} && {run}\n")),
        (Some(at), None) => Some(format!("{at}\n")),
        (None, Some(run)) => Some(format!("{run}\n")),
        (None, None) => None,
    }
}

/// One word, exactly as written, in the words a server's shell wants: inside
/// single quotes, where nothing is special, and a single quote in it closed and
/// reopened the way sh wants. Always quoted, because what is being quoted may
/// be something a person typed, and a `$` or a `;` in it must stay text
pub fn sh_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// Do something with files on another machine.
///
/// Blocks until the far end answers, like every other file call in the program.
/// Whoever calls it decides how long they are willing to wait, and the job is
/// stopped when that time is up rather than left going
pub fn files(spec: &Spec, job: FileJob, wait_ms: u64) -> Result<FileAnswer> {
    let (reply_tx, reply_rx) = channel::<Result<FileAnswer>>();
    hub()
        .send(Job::Files { spec: spec.clone(), job, wait_ms, reply: reply_tx })
        .map_err(|_| anyhow!(crate::i18n::t("err.ssh.no_thread")))?;
    reply_rx
        .recv_timeout(std::time::Duration::from_millis(wait_ms + ANSWER_GRACE_MS))
        .map_err(|_| anyhow!(crate::i18n::tp("err.ssh.timeout", &[("host", &spec.address())])))?
}

/// Run one command on another machine, and wait for the answer.
///
/// A port on the server, reachable from this PC: an address on this PC's own
/// loopback that carries every connection made to it over the connection the
/// terminals use, to that port over there. What a server started in a
/// terminal there (`npm run dev` on 3000) is opened in a browser tab here as
/// if it ran here, and nothing on the server is opened to anyone else.
///
/// The same number here when it is free, since a page often names its own
/// address; another when it is not. Made once for each server and port and
/// kept while the app runs. Answers the port here
pub fn forward(spec: &Spec, port: u16) -> Result<u16> {
    let made = forwards();
    let key = (spec.route(), port);
    if let Some(here) = made.lock().map_err(|_| anyhow!("forward"))?.get(&key) {
        return Ok(*here);
    }
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).or_else(|_| std::net::TcpListener::bind(("127.0.0.1", 0)))?;
    let here = listener.local_addr()?.port();
    let spec = spec.clone();
    std::thread::Builder::new().name(format!("ssh forward {port}")).spawn(move || {
        for stream in listener.incoming().flatten() {
            if hub().send(Job::Tunnel { spec: spec.clone(), port, stream }).is_err() {
                break;
            }
        }
    })?;
    made.lock().map_err(|_| anyhow!("forward"))?.insert(key, here);
    Ok(here)
}

/// Run `command` on the server with its input and output carried through a
/// socket: what is written to the answer goes to the program, and what the
/// program writes comes back on it. Over the same connection the terminals
/// use. The program ends when the socket is closed here, or ends by itself
pub fn pipe(spec: &Spec, command: &str) -> Result<std::net::TcpStream> {
    let (here, there) = socket_pair()?;
    hub()
        .send(Job::Pipe { spec: spec.clone(), command: command.to_string(), stream: there })
        .map_err(|_| anyhow!(crate::i18n::t("err.ssh.no_thread")))?;
    Ok(here)
}

/// Two ends of one connection on this PC's loopback: one to hand to whatever
/// carries it on, one to use here
pub fn socket_pair() -> Result<(std::net::TcpStream, std::net::TcpStream)> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let here = std::net::TcpStream::connect(listener.local_addr()?)?;
    let (there, from) = listener.accept()?;
    // Only the connection made just above: anything else that raced to the
    // port is not ours to carry
    if from != here.local_addr()? {
        bail!("another program connected first");
    }
    here.set_nodelay(true)?;
    there.set_nodelay(true)?;
    Ok((here, there))
}

/// The ports here each server's ports are carried to, by route and port there
fn forwards() -> &'static Mutex<HashMap<(String, u16), u16>> {
    static MADE: OnceLock<Mutex<HashMap<(String, u16), u16>>> = OnceLock::new();
    MADE.get_or_init(Default::default)
}

/// The port here a server's port is already carried to, if it is
pub fn forwarded(spec: &Spec, port: u16) -> Option<u16> {
    forwards().lock().ok()?.get(&(spec.route(), port)).copied()
}

/// The primitive everything remote is built from: git on the far side, asking
/// whether a folder is there, finding out what is installed. Blocks, like every
/// other call here; the caller decides how long it is willing to wait, and the
/// command is stopped over there when that time is up
pub fn exec(spec: &Spec, command: &str, wait_ms: u64) -> Result<Ran> {
    let (reply_tx, reply_rx) = channel::<Result<Ran>>();
    hub()
        .send(Job::Exec { spec: spec.clone(), command: command.to_string(), wait_ms, reply: reply_tx })
        .map_err(|_| anyhow!(crate::i18n::t("err.ssh.no_thread")))?;
    reply_rx
        .recv_timeout(std::time::Duration::from_millis(wait_ms + ANSWER_GRACE_MS))
        .map_err(|_| anyhow!(crate::i18n::tp("err.ssh.timeout", &[("host", &spec.address())])))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first thing typed into a shell that opened over there: the folder,
    /// then the program in it, and the program only if the folder was there
    #[test]
    fn the_folder_is_typed_first_and_the_program_only_in_it() {
        assert_eq!(typed_first(Some("/srv/proj"), Some("claude")).as_deref(), Some("cd '/srv/proj' && claude\n"));
        assert_eq!(typed_first(Some("/srv/proj"), None).as_deref(), Some("cd '/srv/proj'\n"));
        assert_eq!(typed_first(None, Some("claude")).as_deref(), Some("claude\n"));
        assert_eq!(typed_first(Some(" "), Some("")), None);
        assert_eq!(typed_first(None, None), None);
        assert_eq!(
            typed_first(Some("/srv/it's"), Some("claude")).as_deref(),
            Some("cd '/srv/it'\\''s' && claude\n"),
            "a quote in the folder is closed and reopened the way sh wants"
        );
    }

    /// The isolation a test run gets must not be the answer a person gets. The
    /// servers somebody has met are kept beside everything else this program
    /// keeps, so they are still known tomorrow
    #[test]
    fn the_servers_we_have_met_are_kept_with_our_other_things() {
        let real = real_known_hosts_path();
        assert_eq!(real, crate::config::root_dir().join("data").join("known-hosts.json"));
        assert_ne!(real, known_hosts_path(), "a test run writes somewhere of its own");
    }

    /// A host key belongs to a machine; a connection belongs to a person and a
    /// route. Filing both under the same name would hand one person's session
    /// to another, or a direct connection to something that asked for a bastion
    #[test]
    fn one_machine_can_be_two_connections() {
        let at = |user: &str| Spec {
            host: "example.com".into(),
            port: 22,
            user: user.into(),
            ..Default::default()
        };
        assert_eq!(at("a").address(), at("b").address(), "the key belongs to the machine");
        assert_ne!(at("a").route(), at("b").route(), "a different person is a different connection");

        let mut through = at("a");
        through.jump = Some(Box::new(at("gate")));
        assert_eq!(through.address(), at("a").address(), "a different route is still the same machine");
        assert_ne!(through.route(), at("a").route(), "through a jump host is a different connection");
        assert!(through.route().contains("gate"), "it cannot tell which way it went");
    }

    /// The name a person gives a server belongs to the server: whoever signs
    /// in, however the address was capitalised. The way there is part of it,
    /// because one private address behind two bastions is two machines
    #[test]
    fn a_server_is_one_server_whoever_signs_in() {
        let at = |user: &str, host: &str| Spec {
            host: host.into(),
            port: 22,
            user: user.into(),
            ..Default::default()
        };
        assert_eq!(at("deploy", "Prod.Example.com").machine(), "prod.example.com:22");
        assert_eq!(at("deploy", "prod.example.com").machine(), at("root", "PROD.example.com").machine());

        let behind = |gate: &str| {
            let mut s = at("deploy", "10.0.0.5");
            s.jump = Some(Box::new(at("me", gate)));
            s
        };
        assert_eq!(behind("gw-prod.example.com").machine(), "gw-prod.example.com:22>10.0.0.5:22");
        assert_ne!(
            behind("gw-prod.example.com").machine(),
            behind("gw-staging.example.com").machine(),
            "two networks with the same private address were filed as one machine"
        );
        assert_ne!(behind("gw-prod.example.com").machine(), at("deploy", "10.0.0.5").machine());
        // Read back from a file somebody edited by hand, it is still the same key
        assert_eq!(machine_key(" GW-Prod.example.com:22 > 10.0.0.5:22 "), behind("gw-prod.example.com").machine());
    }

    /// The reader hands out exactly what arrived, in order, however the caller
    /// happens to divide it into buffers -- a screen is a stream of bytes, and
    /// one that loses a byte at a buffer edge is a screen with a hole in it
    #[test]
    fn what_the_far_end_says_arrives_whole() {
        let (tx, rx) = channel::<Vec<u8>>();
        tx.send(b"hello ".to_vec()).unwrap();
        tx.send("せかい".as_bytes().to_vec()).unwrap();
        drop(tx);
        let mut r = ShellReader { rx, rest: Vec::new(), at: 0 };
        let mut got = Vec::new();
        let mut small = [0u8; 4];
        loop {
            match r.read(&mut small).unwrap() {
                0 => break,
                n => got.extend_from_slice(&small[..n]),
            }
        }
        assert_eq!(String::from_utf8(got).unwrap(), "hello せかい");
    }

    /// The far end going away has to look like a program ending, because that
    /// is the only thing the reader above knows how to notice
    #[test]
    fn a_closed_connection_reads_as_the_end() {
        let (tx, rx) = channel::<Vec<u8>>();
        drop(tx);
        let mut r = ShellReader { rx, rest: Vec::new(), at: 0 };
        assert_eq!(r.read(&mut [0u8; 8]).unwrap(), 0);
    }

    /// One queue, one reader. Two would each get part of the screen
    #[test]
    fn the_stream_is_handed_out_once() {
        let (_tx, rx) = channel::<Vec<u8>>();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<ToShell>();
        let pty = SshPty {
            tx,
            size: Mutex::new(portable_pty::PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }),
            reader: Mutex::new(Some(ShellReader { rx, rest: Vec::new(), at: 0 })),
            writer_taken: AtomicBool::new(false),
        };
        use portable_pty::MasterPty;
        assert!(pty.try_clone_reader().is_ok());
        assert!(pty.try_clone_reader().is_err(), "a second one gets half of the screen");
        assert!(pty.take_writer().is_ok());
        assert!(pty.take_writer().is_err());
    }

    /// A whole conversation with a real server, over a real socket.
    ///
    /// There is no pretending here: a server is started on the loopback with a
    /// key of its own, and the client half of this module signs in with a
    /// stored password, asks for a terminal, types into it, and reads back
    /// what comes out. It is the only way to know that the thing a tab holds
    /// really is a terminal -- every piece of it (the password never being
    /// typed, the pty request, the two directions of bytes, the size) fails
    /// separately and silently otherwise.
    #[test]
    fn a_terminal_on_another_machine_reads_and_writes_like_any_other() {
        use std::io::Write as _;

        let port = fake_server();

        // The password is not in the settings: it is a name, and this is the
        // store standing in for the real one
        set_secret("ssh/ws/prod/password", "hunter2");
        let spec = Spec {
            host: "127.0.0.1".into(),
            port,
            user: "tester".into(),
            password_key: Some("ssh/ws/prod/password".into()),
            key: None,
            passphrase_key: None,
            ..Default::default()
        };
        // A first meeting: nothing is remembered about this server, and the
        // test must not write into the real settings folder either
        let (pty, mut killer, _) = shell(&spec, 24, 80, None, None).expect("the terminal did not open");
        let mut reader = pty.try_clone_reader().expect("reader");
        let mut writer = pty.take_writer().expect("writer");

        let mut seen = String::new();
        let mut buf = [0u8; 1024];
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        // The far end greets, then echoes. Both have to arrive
        writer.write_all(b"hello").expect("typing");
        writer.flush().expect("flush");
        while std::time::Instant::now() < deadline && !seen.contains("echo:hello") {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
                Err(_) => break,
            }
        }
        assert!(seen.contains("welcome"), "nothing reached the screen: {seen:?}");
        assert!(seen.contains("echo:hello"), "the typed characters did not arrive: {seen:?}");
        // Resizing is a message to the far end, not a local setting
        pty.resize(portable_pty::PtySize { rows: 40, cols: 120, pixel_width: 0, pixel_height: 0 })
            .expect("resize");
        assert_eq!(pty.get_size().expect("size").cols, 120);
        killer.kill().expect("kill");
    }

    /// A command on the far side comes back whole: what it printed, what it
    /// complained about, and whether it worked.
    ///
    /// All three matter separately. Git writes "Preparing worktree" to the
    /// error half on a run that succeeded, so reading the error half as failure
    /// would call every success a failure; and a git that could not make the
    /// folder still prints nothing on the other half, so reading "it printed
    /// something" as success would call every failure a success. The exit code
    /// is the only one of the three that answers the question, which is why it
    /// is carried rather than thrown away.
    ///
    /// Proven against a real server on a real socket, the same one the terminal
    /// test uses -- and end to end against `sshd_probe`, where this made a git
    /// worktree on the far side and the folder was really there afterwards.
    #[test]
    fn a_command_on_another_machine_comes_back_whole() {
        let port = fake_server();
        set_secret("ssh/ws/prod/password", "hunter2");
        let spec = Spec {
            host: "127.0.0.1".into(),
            port,
            user: "tester".into(),
            password_key: Some("ssh/ws/prod/password".into()),
            key: None,
            passphrase_key: None,
            ..Default::default()
        };

        let good = exec(&spec, "git --version", 15_000).expect("the command did not run");
        assert!(good.ok(), "it worked but counts as a failure: {good:?}");
        assert_eq!(good.code, 0);
        assert!(good.out.contains("ran:git --version"), "{good:?}");

        let bad = exec(&spec, "please fail", 15_000).expect("the command did not run");
        assert!(!bad.ok(), "it failed but counts as a success: {bad:?}");
        assert_eq!(bad.code, 3, "the exit code did not arrive");
        assert_eq!(bad.said(), "it went wrong", "what it said was not picked up");
        // The two halves do not run into each other
        assert!(bad.out.is_empty(), "{bad:?}");
    }

    /// A server of our own on the loopback, with a key made for it, that takes
    /// `tester` / `hunter2` and gives out an echoing terminal. Answers its port
    fn fake_server() -> u16 { fake_server_trusted(true).0 }

    fn fake_server_trusted(trusted: bool) -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
        let (port_tx, port_rx) = channel::<u16>();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&attempts);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            rt.block_on(async move {
                let config = Arc::new(russh::server::Config {
                    inactivity_timeout: Some(std::time::Duration::from_secs(30)),
                    auth_rejection_time: std::time::Duration::from_millis(1),
                    keys: vec![
                        russh::keys::PrivateKey::random(
                            &mut rand::rng(),
                            russh::keys::Algorithm::Ed25519,
                        )
                        .expect("host key"),
                    ],
                    ..Default::default()
                });
                let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                    .await
                    .expect("listen");
                let port = listener.local_addr().expect("addr").port();
                if trusted {
                    remember_host(&fake_spec(port, "").machine(), &config.keys[0].public_key().fingerprint(Default::default()).to_string()).unwrap();
                }
                let _ = port_tx.send(port);
                let mut server = Fake { attempts: counted };
                use russh::server::Server as _;
                let _ = server.run_on_socket(config, &listener).await;
            });
        });
        let port = port_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the test server did not start");
        (port, attempts)
    }

    fn fake_spec(port: u16, password_key: &str) -> Spec {
        Spec {
            host: "127.0.0.1".into(),
            port,
            user: "tester".into(),
            password_key: Some(password_key.into()),
            ..Default::default()
        }
    }

    /// A long piece of work on one connection holds up nobody else.
    ///
    /// Everything used to go through one queue, taken one after another: while
    /// a clone ran, what was typed into any terminal on any server waited
    /// behind it -- and the typing was said to have been sent. Here a command
    /// that takes three seconds is running while a terminal is opened and
    /// typed into, and the echo has to come back long before the command ends
    #[test]
    fn a_long_job_holds_up_no_terminal() {
        use std::io::Write as _;
        let port = fake_server();
        set_secret("ssh/ws/slow/password", "hunter2");
        let spec = fake_spec(port, "ssh/ws/slow/password");
        // Signed in first, so the clock below measures the waiting and not
        // the meeting
        exec(&spec, "true", 15_000).expect("the first command did not run");
        let slow = {
            let spec = spec.clone();
            std::thread::spawn(move || exec(&spec, "slow", 15_000))
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        let started = std::time::Instant::now();
        let (pty, mut killer, _) = shell(&spec, 24, 80, None, None).expect("the terminal did not open");
        let mut reader = pty.try_clone_reader().expect("reader");
        let mut writer = pty.take_writer().expect("writer");
        writer.write_all(b"quick").expect("typing");
        let mut seen = String::new();
        let mut buf = [0u8; 1024];
        while !seen.contains("echo:quick") {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => seen.push_str(&String::from_utf8_lossy(&buf[..n])),
            }
        }
        assert!(seen.contains("echo:quick"), "the typing did not arrive: {seen:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(2_000),
            "the terminal waited for the long command: {:?}",
            started.elapsed()
        );
        let done = slow.join().expect("thread").expect("the long command did not finish");
        assert_eq!(done.out, "done");
        killer.kill().expect("kill");
    }

    /// A caller that stops waiting stops the work. Otherwise a command given
    /// up on went on over there, and a transfer went on filling a file, with
    /// nobody to hear how it ended
    #[test]
    fn a_job_out_of_time_is_stopped_and_says_so() {
        let port = fake_server();
        set_secret("ssh/ws/late/password", "hunter2");
        let spec = fake_spec(port, "ssh/ws/late/password");
        exec(&spec, "true", 15_000).expect("the first command did not run");
        let started = std::time::Instant::now();
        let late = exec(&spec, "slow", 500).expect_err("a command past its time came back as done");
        assert!(started.elapsed() < std::time::Duration::from_millis(2_500), "{:?}", started.elapsed());
        assert_eq!(
            late.to_string(),
            crate::i18n::tp("err.ssh.too_long", &[("host", &spec.address()), ("secs", "1")]),
            "a server that answered and ran long is not one that did not answer"
        );
        // And the connection is still good for the next one
        assert!(exec(&spec, "true", 15_000).expect("the next command did not run").ok());
    }

    /// A connection is only as good as what it signed in with. A password
    /// changed in the settings is used by the next thing asked for, and the
    /// old connection is not handed out as if it were signed in with it
    #[test]
    fn a_changed_password_signs_in_again() {
        let port = fake_server();
        set_secret("ssh/ws/change/password", "hunter2");
        let spec = fake_spec(port, "ssh/ws/change/password");
        assert!(exec(&spec, "true", 15_000).expect("signed in").ok());
        set_secret("ssh/ws/change/password", "wrong");
        let refused = exec(&spec, "true", 15_000).expect_err("the old connection was used for a new password");
        assert!(refused.to_string().contains(&spec.address()), "{refused}");
        set_secret("ssh/ws/change/password", "hunter2");
        assert!(exec(&spec, "true", 15_000).expect("signed in again").ok());
    }

    /// Two servers at one private address, behind two bastions, are two
    /// servers with two keys. Filed by address, the second was refused as the
    /// first one with its key changed
    #[test]
    fn one_private_address_behind_two_bastions_is_two_keys() {
        let at = |host: &str| Spec { host: host.into(), port: 22, user: "me".into(), ..Default::default() };
        let behind = |gate: &str| {
            let mut s = at("10.0.0.1");
            s.jump = Some(Box::new(at(gate)));
            s
        };
        remember_host(&behind("gw-prod.example.com").machine(), "SHA256:prod").expect("written");
        assert_eq!(remembered(&behind("gw-prod.example.com")).as_deref(), Some("SHA256:prod"));
        assert_eq!(remembered(&behind("gw-staging.example.com")), None, "staging was checked against production's key");
    }

    /// A file written before this was filed by address as typed. A server
    /// reached directly is still found in it; one behind a bastion is not,
    /// since that line could not say which network it was about
    #[test]
    fn a_key_filed_the_old_way_is_found_where_it_can_be() {
        remember_host("Old-Direct.example.com:2201", "SHA256:old").expect("written");
        let direct = Spec { host: "old-direct.example.com".into(), port: 2201, user: "me".into(), ..Default::default() };
        assert_eq!(remembered(&direct).as_deref(), Some("SHA256:old"));
        remember_host("10.9.9.9:22", "SHA256:someone").expect("written");
        let mut behind = Spec { host: "10.9.9.9".into(), port: 22, user: "me".into(), ..Default::default() };
        behind.jump = Some(Box::new(Spec { host: "gw.example.com".into(), port: 22, user: "me".into(), ..Default::default() }));
        assert_eq!(remembered(&behind), None);
    }

    /// What a connection is filed under changes with what it signs in with,
    /// and with what its bastion signs in with -- and never holds the secret
    #[test]
    fn a_connection_is_filed_with_its_credential() {
        set_secret("ssh/ws/key/password", "one");
        let spec = fake_spec(22, "ssh/ws/key/password");
        let before = conn_key(&spec);
        assert!(!before.contains("one"));
        assert!(before.starts_with(&spec.route()));
        set_secret("ssh/ws/key/password", "two");
        assert_ne!(conn_key(&spec), before, "a new password was handed the old connection");
        let mut through = fake_spec(22, "ssh/ws/key/password");
        through.jump = Some(Box::new(fake_spec(2222, "ssh/ws/key/gate")));
        let first = conn_key(&through);
        set_secret("ssh/ws/key/gate", "changed");
        assert_ne!(conn_key(&through), first, "the bastion's new password was not asked for");
    }

    /// A server whose key is not the one remembered is refused, and asked
    /// about: both fingerprints are there to be read, and trusting takes only
    /// the one that answered. Afterwards it connects with it
    #[test]
    fn a_changed_key_is_asked_about_and_trusted_only_as_shown() {
        let port = fake_server();
        set_secret("ssh/ws/rekey/password", "hunter2");
        let spec = fake_spec(port, "ssh/ws/rekey/password");
        remember_host(&spec.machine(), "SHA256:the-old-one").expect("written");
        let refused = exec(&spec, "true", 15_000).expect_err("a server with another key was let in");
        assert_eq!(refused.to_string(), crate::i18n::tp("err.ssh.host_changed", &[("host", &spec.address())]));
        let change = key_changes().into_iter().find(|c| c.machine == spec.machine()).expect("nobody was asked");
        assert_eq!(change.before, "SHA256:the-old-one");
        assert_ne!(change.now, change.before);

        assert!(answer_key_change(&spec.machine(), "SHA256:another", true).is_err(), "a key nobody was shown was trusted");
        assert_eq!(remembered(&spec).as_deref(), Some("SHA256:the-old-one"));
        assert_eq!(answer_key_change(&spec.machine(), &change.now, true).expect("trusted"), true);
        assert!(key_changes().iter().all(|c| c.machine != spec.machine()), "the question stayed up");
        assert!(exec(&spec, "true", 15_000).expect("it did not connect with the trusted key").ok());
    }

    #[test]
    fn a_first_key_requires_explicit_trust_before_signing_in() {
        let (port, attempts) = fake_server_trusted(false);
        let spec = fake_spec(port, "ssh/ws/first/password");
        set_secret("ssh/ws/first/password", "hunter2");
        let why = exec(&spec, "true", 15_000).expect_err("an unverified server was allowed");
        assert_eq!(why.to_string(), crate::i18n::tp("err.ssh.host_unknown", &[("host", &spec.address())]));
        assert!(remembered(&spec).is_none());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 0, "credentials reached an unverified server");
        let question = key_changes().into_iter().find(|c| c.machine == spec.machine()).unwrap();
        assert!(question.before.is_empty());
        assert!(answer_key_change(&spec.machine(), "not-the-shown-key", true).is_err());
        assert!(answer_key_change(&spec.machine(), &question.now, true).unwrap());
        assert!(exec(&spec, "true", 15_000).unwrap().ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Not trusting it puts the question away and remembers nothing
    #[test]
    fn a_changed_key_not_trusted_is_left_as_it_was() {
        let port = fake_server();
        set_secret("ssh/ws/nokey/password", "hunter2");
        let spec = fake_spec(port, "ssh/ws/nokey/password");
        remember_host(&spec.machine(), "SHA256:kept").expect("written");
        assert!(exec(&spec, "true", 15_000).is_err());
        let change = key_changes().into_iter().find(|c| c.machine == spec.machine()).expect("nobody was asked");
        assert_eq!(answer_key_change(&spec.machine(), &change.now, false).expect("answered"), false);
        assert!(key_changes().iter().all(|c| c.machine != spec.machine()));
        assert_eq!(remembered(&spec).as_deref(), Some("SHA256:kept"));
    }

    /// The far side of that conversation. It asks for a password, insists on
    /// the one it was told, and gives out a terminal that echoes
    #[derive(Clone)]
    struct Fake { attempts: Arc<std::sync::atomic::AtomicUsize> }

    impl russh::server::Server for Fake {
        type Handler = Self;
        fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
            self.clone()
        }
    }

    impl russh::server::Handler for Fake {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            user: &str,
            password: &str,
        ) -> Result<russh::server::Auth, Self::Error> {
            self.attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(match (user, password) {
                ("tester", "hunter2") => russh::server::Auth::Accept,
                _ => russh::server::Auth::reject(),
            })
        }

        async fn channel_open_session(
            &mut self,
            _channel: russh::Channel<russh::server::Msg>,
            reply: russh::server::ChannelOpenHandle,
            _session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }

        async fn pty_request(
            &mut self,
            channel: russh::ChannelId,
            _term: &str,
            _cols: u32,
            _rows: u32,
            _pw: u32,
            _ph: u32,
            _modes: &[(russh::Pty, u32)],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            Ok(())
        }

        /// One command: both streams and an ending, which is the whole of what
        /// a program needs and none of what a terminal needs
        async fn exec_request(
            &mut self,
            channel: russh::ChannelId,
            command: &[u8],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            let line = String::from_utf8_lossy(command).to_string();
            session.channel_success(channel)?;
            // A command that takes its time, the way a clone over a thin line
            // does: it answers after three seconds, from a task of its own
            if line.contains("slow") {
                let h = session.handle();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let _ = h.data(channel, b"done".to_vec()).await;
                    let _ = h.eof(channel).await;
                    let _ = h.exit_status_request(channel, 0).await;
                    let _ = h.close(channel).await;
                });
                return Ok(());
            }
            // In the order OpenSSH sends them: the output ends, then how the
            // command ended, then the channel closes
            let code = if line.contains("fail") {
                session.extended_data(channel, 1, russh::keys::ssh_encoding::bytes::Bytes::from_static(b"it went wrong"))?;
                3
            } else {
                session.data(channel, russh::keys::ssh_encoding::bytes::Bytes::from(format!("ran:{line}").into_bytes()))?;
                0
            };
            session.eof(channel)?;
            session.exit_status_request(channel, code)?;
            session.close(channel)?;
            Ok(())
        }

        async fn shell_request(
            &mut self,
            channel: russh::ChannelId,
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            session.channel_success(channel)?;
            session.data(channel, russh::keys::ssh_encoding::bytes::Bytes::from_static(b"welcome\r\n"))?;
            Ok(())
        }

        async fn data(
            &mut self,
            channel: russh::ChannelId,
            data: &[u8],
            session: &mut russh::server::Session,
        ) -> Result<(), Self::Error> {
            let back = format!("echo:{}\r\n", String::from_utf8_lossy(data));
            session.data(channel, russh::keys::ssh_encoding::bytes::Bytes::from(back.into_bytes()))?;
            Ok(())
        }
    }

    /// The address is what a host key is filed under, and it has to tell two
    /// servers apart even when they share a name on different ports
    #[test]
    fn a_server_is_known_by_its_address() {
        let a = Spec { host: "example.com".into(), port: 22, ..Default::default() };
        let b = Spec { host: "example.com".into(), port: 2222, ..Default::default() };
        assert_eq!(a.address(), "example.com:22");
        assert_ne!(a.address(), b.address());
    }
}

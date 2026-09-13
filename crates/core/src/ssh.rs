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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
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
}

/// The fingerprints of the servers we have met, by address.
///
/// The same promise `known_hosts` makes, kept in the shape everything else
/// here is kept in. A server whose key has changed is refused rather than
/// asked about: at that moment there is no way to tell "they reinstalled it"
/// from "somebody is standing in the middle", and the second one is the one
/// that costs a password.
fn known_hosts_path() -> std::path::PathBuf {
    // A test run never writes into the real one. The probe server makes a new
    // key every time and listens on whatever port the machine hands out, so a
    // kept file fills up with dead ports -- and the day the machine hands out
    // one of them again, a test fails saying the key changed. What the real
    // answer would be is checked by a test of its own
    if cfg!(test) {
        return std::env::temp_dir()
            .join(format!("shikisha-known-hosts-{}", std::process::id()))
            .join("known-hosts.json");
    }
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

fn remember_host(addr: &str, fingerprint: &str) -> Result<()> {
    let mut all = known_hosts();
    all.insert(addr.to_string(), fingerprint.to_string());
    let path = known_hosts_path();
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    crate::crypto::write_atomic(&path, &serde_json::to_string_pretty(&all)?)
}

/// One thing in a folder on the far end
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Bring a file here
    Get { from: String, to: std::path::PathBuf },
    /// Send a file there
    Put { from: std::path::PathBuf, to: String, overwrite: bool },
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
}

/// What the connection thread is asked to do. One enum, because one thread
/// answers all of it and a second queue would be a second order of events
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
        out: Sender<Vec<u8>>,
        reply: Sender<Result<u64>>,
    },
    /// Type at a terminal that is already open
    Write { id: u64, data: Vec<u8> },
    /// The window changed shape
    Resize { id: u64, rows: u16, cols: u16 },
    /// Nobody is looking at this terminal any more
    Close { id: u64 },
    /// Something to do with files, on the same connection a terminal uses
    Files {
        spec: Spec,
        job: FileJob,
        reply: Sender<Result<FileAnswer>>,
    },
    /// One command, run to the end, on the same connection everything else uses
    Exec {
        spec: Spec,
        command: String,
        reply: Sender<Result<Ran>>,
    },
}

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
fn hub() -> &'static Sender<Job> {
    static HUB: OnceLock<Sender<Job>> = OnceLock::new();
    HUB.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        std::thread::Builder::new()
            .name("ssh".into())
            .spawn(move || run_hub(rx))
            .expect("could not start the ssh thread");
        tx
    })
}

/// Numbers handed to terminals, so that a message about one cannot be about
/// another after a restart
fn next_id() -> u64 {
    static N: AtomicU64 = AtomicU64::new(1);
    N.fetch_add(1, Ordering::Relaxed)
}

/// What the thread keeps: connections by address, and open terminals by id.
///
/// A terminal is kept as its writing half only. The reading half goes to a
/// task of its own the moment it exists, because the two directions have
/// nothing to do with each other -- a person typing must not wait for the far
/// end to say something, and a busy screen must not wait for a keystroke
struct Live {
    sessions: HashMap<String, russh::client::Handle<Client>>,
    shells: HashMap<u64, russh::ChannelWriteHalf<russh::client::Msg>>,
    /// When each connection last had a terminal on it, so an unused one can be
    /// let go rather than held open for the life of the program
    idle_since: HashMap<String, std::time::Instant>,
}

fn run_hub(rx: Receiver<Job>) {
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            crate::append_hook_log(&format!("ssh: could not start the runtime: {e}"));
            return;
        }
    };
    let mut live = Live {
        sessions: HashMap::new(),
        shells: HashMap::new(),
        idle_since: HashMap::new(),
    };
    rt.block_on(async move {
        loop {
            // The queue is a blocking one, and this thread is the only one that
            // reads it. Waiting on it inside the runtime would stop the timers
            // the connections need, so it is drained without blocking and the
            // thread sleeps between looks
            match rx.try_recv() {
                Ok(job) => handle(&mut live, job).await,
                Err(TryRecvError::Empty) => {
                    tokio::time::sleep(std::time::Duration::from_millis(8)).await;
                    close_idle(&mut live).await;
                }
                Err(TryRecvError::Disconnected) => break,
            }
        }
    });
}

async fn close_idle(live: &mut Live) {
    let now = std::time::Instant::now();
    let gone: Vec<String> = live
        .idle_since
        .iter()
        .filter(|(_, since)| now.duration_since(**since).as_millis() as u64 > IDLE_KEEP_MS)
        .map(|(a, _)| a.clone())
        .collect();
    for addr in gone {
        live.idle_since.remove(&addr);
        if let Some(h) = live.sessions.remove(&addr) {
            let _ = h
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        }
    }
}

async fn handle(live: &mut Live, job: Job) {
    match job {
        Job::Shell { spec, rows, cols, cwd, out, reply } => {
            let r = open_shell(live, &spec, rows, cols, out).await;
            // Typed, so it is on screen like anything else typed, and so that
            // nothing else has to know it happened
            if let (Ok(id), Some(at)) = (&r, cwd.as_deref().map(str::trim).filter(|a| !a.is_empty()))
                && let Some(ch) = live.shells.get(id) {
                    let line = format!("cd '{}'\n", at.replace('\'', "'\\''"));
                    let _ = ch.data(line.as_bytes()).await;
                }
            let _ = reply.send(r);
        }
        Job::Write { id, data } => {
            if let Some(ch) = live.shells.get(&id)
                && let Err(e) = ch.data(&data[..]).await {
                    crate::append_hook_log(&format!("ssh: could not send: {e}"));
                }
        }
        Job::Resize { id, rows, cols } => {
            if let Some(ch) = live.shells.get(&id) {
                let _ = ch.window_change(cols as u32, rows as u32, 0, 0).await;
            }
        }
        Job::Files { spec, job, reply } => {
            let r = do_file_job(live, &spec, job).await;
            let _ = reply.send(r);
        }
        Job::Exec { spec, command, reply } => {
            let r = do_exec(live, &spec, &command).await;
            let _ = reply.send(r);
        }
        Job::Close { id } => {
            if let Some(ch) = live.shells.remove(&id) {
                let _ = ch.eof().await;
                let _ = ch.close().await;
            }
            // A connection with nothing left on it starts its clock
            for addr in live.sessions.keys() {
                live.idle_since.insert(addr.clone(), std::time::Instant::now());
            }
        }
    }
}

/// Make sure there is a connection to this server, and say what it is filed
/// under. The one place "have we met this server" and "who are we" are
/// answered, so that a terminal and a file transfer agree about both
fn session<'a>(
    live: &'a mut Live,
    spec: &'a Spec,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + 'a>> {
    // A bastion is reached the same way its server is, so this calls itself.
    // An async function that does that has a size nobody can write down, which
    // is what the box is for
    Box::pin(open_session(live, spec))
}

async fn open_session(live: &mut Live, spec: &Spec) -> Result<String> {
    let addr = spec.address();
    let route = spec.route();
    if let Some(h) = live.sessions.get(&route) {
        if !h.is_closed() {
            live.idle_since.remove(&route);
            return Ok(route);
        }
        live.sessions.remove(&route);
    }
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
    let seen = known_hosts().get(&addr).cloned();
    let met = Arc::new(Mutex::new(None::<String>));
    let handler = Client { expected: seen.clone(), met: Arc::clone(&met) };
    // Through a bastion, the way in is a channel on that bastion's own
    // connection rather than a socket this machine opened. It is made first,
    // and on its own, so that "the bastion would not have us" is said in the
    // bastion's own words instead of arriving as a failure to reach the server
    // behind it. Everything after this point -- the host key included -- is the
    // far server answering for itself
    let hop = match &spec.jump {
        None => None,
        Some(via) => {
            let through = session(live, via).await?;
            let open = live
                .sessions
                .get(&through)
                .ok_or_else(|| {
                    anyhow!(crate::i18n::tp(
                        "err.ssh.connect",
                        &[("host", &through), ("e", "gone")]
                    ))
                })?
                .channel_open_direct_tcpip(spec.host.clone(), spec.port as u32, "127.0.0.1", 0)
                .await;
            Some(open.map_err(|e| {
                anyhow!(crate::i18n::tp(
                    "err.ssh.jump",
                    &[("jump", &through), ("host", &addr), ("e", &e.to_string())]
                ))
            })?)
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
            if let (Some(before), Some(now)) = (&seen, &now)
                && before != now {
                    crate::append_hook_log(&format!(
                        "ssh: the key at {addr} changed: {before} -> {now}"
                    ));
                    bail!(crate::i18n::tp("err.ssh.host_changed", &[("host", &addr)]));
                }
            bail!(crate::i18n::tp(
                "err.ssh.connect",
                &[("host", &addr), ("e", &e.to_string())]
            ))
        }
        Ok(Ok(h)) => h,
    };
    // A server we had not met is remembered now, with its fingerprint, so that
    // the next time it changes we are able to say so
    if seen.is_none()
        && let Some(fp) = met.lock().ok().and_then(|m| m.clone()) {
            let _ = remember_host(&addr, &fp);
            crate::append_hook_log(&format!("ssh: first time at {addr}, key {fp}"));
        }

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
    live.sessions.insert(route.clone(), handle);
    live.idle_since.remove(&route);
    Ok(route)
}

async fn open_shell(
    live: &mut Live,
    spec: &Spec,
    rows: u16,
    cols: u16,
    out: Sender<Vec<u8>>,
) -> Result<u64> {
    let route = session(live, spec).await?;
    let handle = live
        .sessions
        .get(&route)
        .ok_or_else(|| anyhow!(crate::i18n::tp("err.ssh.connect", &[("host", &route), ("e", "gone")])))?;
    let channel = handle.channel_open_session().await?;
    channel
        .request_pty(true, "xterm-256color", cols as u32, rows as u32, 0, 0, &[])
        .await?;
    channel.request_shell(true).await?;
    let id = next_id();
    let (mut reading, writing) = channel.split();
    live.shells.insert(id, writing);
    // Everything the far end says goes straight to the tab's queue, from a task
    // of its own, so one quiet connection never holds up a busy one
    tokio::spawn(async move {
        while let Some(msg) = reading.wait().await {
            let chunk = match msg {
                // A terminal has one screen. What a program writes to its
                // error output belongs on it, in the order it arrived, exactly
                // as it would locally -- keeping them apart here would put the
                // two halves of a compiler's opinion in different places
                russh::ChannelMsg::Data { data } => data.to_vec(),
                russh::ChannelMsg::ExtendedData { data, .. } => data.to_vec(),
                russh::ChannelMsg::Eof | russh::ChannelMsg::Close => break,
                _ => continue,
            };
            if out.send(chunk).is_err() {
                break;
            }
        }
    });
    Ok(id)
}

/// One file job, on the connection the terminals are already using.
///
/// A file session is opened for the job and closed after it. Holding one open
/// would be faster and would also mean a tab that transfers nothing keeps a
/// channel open on the far end for as long as the app runs; a transfer is not
/// something that happens hundreds of times a second
/// Run one command over there and wait for it to finish.
///
/// Its own channel, closed when the command ends, so nothing of it is left on
/// the connection a terminal is using. What is collected is both streams and
/// the exit code, because the caller is a program: "it printed something" is
/// not the same answer as "it worked"
async fn do_exec(live: &mut Live, spec: &Spec, command: &str) -> Result<Ran> {
    use russh::ChannelMsg;
    let route = session(live, spec).await?;
    let handle = live
        .sessions
        .get(&route)
        .ok_or_else(|| anyhow!(crate::i18n::tp("err.ssh.connect", &[("host", &route), ("e", "gone")])))?;
    let mut channel = handle.channel_open_session().await?;
    channel.exec(true, command).await?;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    // A server may send the code and keep talking, so the streams are drained
    // to the end rather than stopping at the first word about the ending
    let mut code: Option<i32> = None;
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { ref data } => out.extend_from_slice(data),
            ChannelMsg::ExtendedData { ref data, .. } => err.extend_from_slice(data),
            ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status as i32),
            ChannelMsg::Eof | ChannelMsg::Close => break,
            _ => {}
        }
    }
    let _ = channel.close().await;
    Ok(Ran {
        // No word about the ending is not the same as a clean one: a server
        // that closed on us has not told us the command worked
        code: code.unwrap_or(-1),
        out: String::from_utf8_lossy(&out).to_string(),
        err: String::from_utf8_lossy(&err).to_string(),
    })
}

async fn do_file_job(live: &mut Live, spec: &Spec, job: FileJob) -> Result<FileAnswer> {
    let route = session(live, spec).await?;
    let handle = live
        .sessions
        .get(&route)
        .ok_or_else(|| anyhow!(crate::i18n::tp("err.ssh.connect", &[("host", &route), ("e", "gone")])))?;
    let channel = handle.channel_open_session().await?;
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
    let sftp = russh_sftp::client::SftpSession::new(channel.into_stream()).await?;
    let out = run_file_job(&sftp, job).await;
    let _ = sftp.close().await;
    out
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
        FileJob::Get { from, to } => {
            let bytes = sftp.read(&from).await?;
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&to, bytes)?;
            Ok(FileAnswer::Nothing)
        }
        FileJob::Put { from, to, overwrite } => {
            if !overwrite && sftp.try_exists(&to).await.unwrap_or(false) {
                bail!(crate::i18n::tp("err.ssh.file_exists", &[("path", &to)]));
            }
            let bytes = std::fs::read(&from)?;
            sftp.write(&to, &bytes).await?;
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

/// Whether the server is the one we met before.
///
/// The first time, whatever answers is taken as the truth and written down --
/// there is nothing to compare against, and refusing would mean nobody could
/// ever connect to anything. Every time after that the fingerprint has to
/// match, and a mismatch ends the connection rather than asking: the moment a
/// key changes is the moment you cannot tell a reinstall from somebody
/// standing in the middle, and only one of those costs you a password.
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
            None => true,
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

/// The writing half. Typing goes to the thread, which is the only thing that
/// touches the connection
struct ShellWriter {
    id: u64,
}

impl Write for ShellWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = hub().send(Job::Write { id: self.id, data: buf.to_vec() });
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
    id: u64,
    size: Mutex<portable_pty::PtySize>,
    reader: Mutex<Option<ShellReader>>,
    writer_taken: AtomicBool,
}

impl portable_pty::MasterPty for SshPty {
    fn resize(&self, size: portable_pty::PtySize) -> Result<(), anyhow::Error> {
        if let Ok(mut s) = self.size.lock() {
            *s = size;
        }
        let _ = hub().send(Job::Resize {
            id: self.id,
            rows: size.rows,
            cols: size.cols,
        });
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
        Ok(Box::new(ShellWriter { id: self.id }))
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
    id: u64,
}

impl portable_pty::ChildKiller for SshKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        let _ = hub().send(Job::Close { id: self.id });
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
) -> Result<(Box<dyn portable_pty::MasterPty + Send>, Box<dyn portable_pty::ChildKiller + Send + Sync>)>
{
    let (out_tx, out_rx) = channel::<Vec<u8>>();
    let (reply_tx, reply_rx) = channel::<Result<u64>>();
    hub()
        .send(Job::Shell {
            spec: spec.clone(),
            rows,
            cols,
            cwd: cwd.map(str::to_string),
            out: out_tx,
            reply: reply_tx,
        })
        .map_err(|_| anyhow!(crate::i18n::t("err.ssh.no_thread")))?;
    let id = reply_rx
        .recv_timeout(std::time::Duration::from_millis(CONNECT_MS + 5_000))
        .map_err(|_| anyhow!(crate::i18n::tp("err.ssh.timeout", &[("host", &spec.address())])))??;
    let pty = SshPty {
        id,
        size: Mutex::new(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }),
        reader: Mutex::new(Some(ShellReader { rx: out_rx, rest: Vec::new(), at: 0 })),
        writer_taken: AtomicBool::new(false),
    };
    Ok((Box::new(pty), Box::new(SshKiller { id })))
}

/// Do something with files on another machine.
///
/// Blocks until the far end answers, like every other file call in the program.
/// Whoever calls it decides how long they are willing to wait
pub fn files(spec: &Spec, job: FileJob, wait_ms: u64) -> Result<FileAnswer> {
    let (reply_tx, reply_rx) = channel::<Result<FileAnswer>>();
    hub()
        .send(Job::Files { spec: spec.clone(), job, reply: reply_tx })
        .map_err(|_| anyhow!(crate::i18n::t("err.ssh.no_thread")))?;
    reply_rx
        .recv_timeout(std::time::Duration::from_millis(wait_ms))
        .map_err(|_| anyhow!(crate::i18n::tp("err.ssh.timeout", &[("host", &spec.address())])))?
}

/// Run one command on another machine, and wait for the answer.
///
/// The primitive everything remote is built from: git on the far side, asking
/// whether a folder is there, finding out what is installed. Blocks, like every
/// other call here; the caller decides how long it is willing to wait
pub fn exec(spec: &Spec, command: &str, wait_ms: u64) -> Result<Ran> {
    let (reply_tx, reply_rx) = channel::<Result<Ran>>();
    hub()
        .send(Job::Exec { spec: spec.clone(), command: command.to_string(), reply: reply_tx })
        .map_err(|_| anyhow!(crate::i18n::t("err.ssh.no_thread")))?;
    reply_rx
        .recv_timeout(std::time::Duration::from_millis(wait_ms))
        .map_err(|_| anyhow!(crate::i18n::tp("err.ssh.timeout", &[("host", &spec.address())])))?
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(at("a").address(), at("b").address(), "鍵は機械のもの");
        assert_ne!(at("a").route(), at("b").route(), "人が違えば別の接続");

        let mut through = at("a");
        through.jump = Some(Box::new(at("gate")));
        assert_eq!(through.address(), at("a").address(), "経路が変わっても機械は同じ");
        assert_ne!(through.route(), at("a").route(), "踏み台越しは別の接続");
        assert!(through.route().contains("gate"), "どこを通ったか読めない");
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
        let pty = SshPty {
            id: 1,
            size: Mutex::new(portable_pty::PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }),
            reader: Mutex::new(Some(ShellReader { rx, rest: Vec::new(), at: 0 })),
            writer_taken: AtomicBool::new(false),
        };
        use portable_pty::MasterPty;
        assert!(pty.try_clone_reader().is_ok());
        assert!(pty.try_clone_reader().is_err(), "二人目に画面の半分が渡る");
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

        let (port_tx, port_rx) = channel::<u16>();
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
                let _ = port_tx.send(listener.local_addr().expect("addr").port());
                let mut server = Fake;
                use russh::server::Server as _;
                let _ = server.run_on_socket(config, &listener).await;
            });
        });
        let port = port_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the test server did not start");

        // The password is not in the settings: it is a name, and this is the
        // store standing in for the real one
        use_secrets(HashMap::from([(
            "ssh/ws/prod/password".to_string(),
            "hunter2".to_string(),
        )]));
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
        let (pty, mut killer) = shell(&spec, 24, 80, None).expect("the terminal did not open");
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
        assert!(seen.contains("welcome"), "画面に何も届いていない: {seen:?}");
        assert!(seen.contains("echo:hello"), "打った文字が届いていない: {seen:?}");
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
        let (port_tx, port_rx) = channel::<u16>();
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
                let _ = port_tx.send(listener.local_addr().expect("addr").port());
                let mut server = Fake;
                use russh::server::Server as _;
                let _ = server.run_on_socket(config, &listener).await;
            });
        });
        let port = port_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the test server did not start");
        use_secrets(HashMap::from([(
            "ssh/ws/prod/password".to_string(),
            "hunter2".to_string(),
        )]));
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
        assert!(good.ok(), "動いたのに失敗になっている: {good:?}");
        assert_eq!(good.code, 0);
        assert!(good.out.contains("ran:git --version"), "{good:?}");

        let bad = exec(&spec, "please fail", 15_000).expect("the command did not run");
        assert!(!bad.ok(), "失敗したのに成功になっている: {bad:?}");
        assert_eq!(bad.code, 3, "終了コードが届いていない");
        assert_eq!(bad.said(), "it went wrong", "言い分が拾えていない");
        // The two halves do not run into each other
        assert!(bad.out.is_empty(), "{bad:?}");
    }

    /// The far side of that conversation. It asks for a password, insists on
    /// the one it was told, and gives out a terminal that echoes
    #[derive(Clone)]
    struct Fake;

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
            if line.contains("fail") {
                session.extended_data(channel, 1, russh::keys::ssh_encoding::bytes::Bytes::from_static(b"it went wrong"))?;
                session.exit_status_request(channel, 3)?;
            } else {
                session.data(channel, russh::keys::ssh_encoding::bytes::Bytes::from(format!("ran:{line}").into_bytes()))?;
                session.exit_status_request(channel, 0)?;
            }
            session.eof(channel)?;
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

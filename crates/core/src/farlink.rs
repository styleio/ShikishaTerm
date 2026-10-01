//! The bridge: this app's small helper on another machine, and the line to it.
//!
//! A tab on a server or a MicroVM runs its AI over there, where this app's
//! `shikisha` command and the pipe it talks through do not exist. So the AI
//! there could not report a task, ask its lead, or read its inbox. The bridge
//! (`shikisha-bridge`, one small program) closes that gap:
//!
//!   - **over there** it opens a socket the tabs' `shikisha` command connects
//!     to, exactly as it would to the pipe here, and carries each connection
//!     down one line to this app. The command is this crate's own `cli`, so it
//!     is the same command on both machines
//!   - **here** each connection that comes down the line is served by the same
//!     code as the pipe (`api::serve_elsewhere`): the same keys, the same
//!     queue, the same answers
//!   - it also does small jobs on its machine when asked ([`crate::farops`]),
//!     above all reading a conversation record there instead of fetching it a
//!     piece at a time with remote commands
//!
//! **The line is always made from here.** Over SSH it is a program run on the
//! connection the terminals already use; on a MicroVM it is a program started
//! the way terminals are, without a terminal. Nothing over there ever connects
//! to this PC, and nothing is opened to anyone else -- so a private MicroVM
//! works exactly like a public one.
//!
//! **It runs only while the line is up.** When this app lets go (it quits, the
//! machine is not in use any more) the bridge's input ends and it exits. It is
//! never left running, and nothing makes it start again by itself.
//!
//! **It is only there if the person said so.** Putting it on a machine is asked
//! first, per machine, and taking it off is one press (see [`install`] and
//! [`remove`], and the host's card in the settings). Nothing here puts it
//! anywhere on its own.
//!
//! The wire: one JSON object per line ([`Frame`]), both ways.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One thing said on the line
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Frame {
    /// The bridge is up, which version it is, and the jobs it holds (far-keep
    /// plan §3.1). An older bridge names no jobs; an older app ignores them
    Hello {
        version: String,
        rev: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        jobs: Vec<String>,
        /// The file name of the program it runs, which carries its build
        /// (`program_name`): the app can tell a resident process of an older
        /// build of the same version from its own
        #[serde(default, skip_serializing_if = "String::is_empty")]
        program: String,
    },
    /// A `shikisha` command over there connected (`c` names the connection)
    Open { c: u64 },
    /// A line of one connection, either way
    Line { c: u64, l: String },
    /// One side of a connection is done with it
    Close { c: u64 },
    /// An operation this PC asks for ([`crate::farops`])
    Op { id: u64, op: String, p: Value },
    /// This PC is still there. Said every [`TICK`] while the line is up:
    /// on a MicroVM the program's input stays open after this PC is gone, so
    /// the end of the input cannot be the only sign
    Tick,
    /// This app is going: its line ends now. Said as it quits and as it lets
    /// go of a machine, because on a MicroVM the end of the input is never
    /// seen there, and until the silence runs out the bridge would carry the
    /// tabs' calls to an app that is gone. A bridge that does not know it
    /// goes by the silence, as before
    Bye,
    /// A message of one job, by the job's name (far-keep plan §3.1): a job
    /// added later speaks through this, both ways, and the line needs no new
    /// kind of frame for it. A job the other side does not hold ignores it
    Job {
        job: String,
        #[serde(default)]
        m: Value,
    },
    /// Its answer
    Re {
        id: u64,
        #[serde(default)]
        r: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        e: Option<String>,
    },
}

impl Frame {
    fn line(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_default();
        s.push('\n');
        s
    }
}

/// How often this PC says it is still there
const TICK: Duration = Duration::from_secs(15);

/// The folder the bridge lives in on its machine, below the account's home.
/// Everything of it is here, so taking it off is one folder
pub const HOME_DIR: &str = ".local/share/shikisha/bridge";

// ── Over there ──────────────────────────────────────────────────────────────

/// The socket the tabs' `shikisha` command connects to, in the bridge's
/// `run` folder (see `fardaemon`). A tab of an older bridge has
/// `shikisha.sock` instead, served by that bridge for as long as it runs
pub const TABS_SOCK: &str = "tabs.sock";
/// The resident process's own socket there, beside the older bridge's
/// (`shikisha.sock`), which nothing of the resident process touches
pub const KEEP_SOCK: &str = "keep.sock";

/// The `shikisha` command over there: this crate's own command, pointed at the
/// bridge's socket, with the tab's key read from the file the bridge was given
/// (`farops::put_key`). The tab says which file through its environment
pub fn far_cli(args: &[String]) -> i32 {
    // A hook of the AI's CLI that waits for its answer (`--hook line`): the
    // CLI is never left waiting, whatever happens, so it is answered before
    // anything here can fail
    let hook = args.first().map(String::as_str) == Some("--hook");
    let (Ok(sock), Ok(key_file)) = (std::env::var(ENV_SOCK), std::env::var(ENV_KEY)) else {
        if hook {
            println!("{{}}");
            return 0;
        }
        eprintln!("[shikisha] This command works in a SHIKISHA-TERM tab; this terminal was not opened by the app.");
        return 1;
    };
    let key = std::fs::read_to_string(&key_file).unwrap_or_default();
    // SAFETY: set before anything else runs on this thread; the command is a
    // single-threaded program that has only just started
    unsafe {
        std::env::set_var(crate::api::ENV_PIPE, sock);
        std::env::set_var(crate::api::ENV_TOKEN, key.trim());
    }
    if hook {
        return far_hook(args.get(1).map(String::as_str).unwrap_or_default());
    }
    crate::cli::run(args)
}

/// How long a hook of the AI's CLI waits for the app, a little less than the
/// CLI waits for the hook (10 s), so the hook always answers first
const HOOK_WAIT: Duration = Duration::from_secs(8);

/// A hook of the AI's CLI on this machine that waits for the app's answer,
/// made as the tab whose terminal the CLI runs in -- the key in its
/// environment -- and printed as the CLI takes it. Anything that goes wrong
/// -- the app away, too slow, a hook it does not know -- answers `{}`, so a
/// turn is never held up by this machine.
///
///   `shikisha --hook line`  the Stop event on its input; the app's
///   `confer_stop` with the turn's last message decides whether the turn is
///   held for a line said to the chat (`{"decision":"block","reason":...}`)
fn far_hook(which: &str) -> i32 {
    let mut input = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
    let event: Value = serde_json::from_str(&input).unwrap_or_default();
    let answer = match which {
        "line" => {
            let said = event.get("last_assistant_message").and_then(|m| m.as_str()).unwrap_or_default().to_string();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(crate::api::ApiClient::from_env().and_then(|mut c| c.call("confer_stop", vec![said.into()])));
            });
            match rx.recv_timeout(HOOK_WAIT) {
                Ok(Ok(a)) if a["ok"] == json!(true) => a["result"]["hold"]
                    .as_str()
                    .filter(|r| !r.trim().is_empty())
                    .map(|reason| json!({ "decision": "block", "reason": reason })),
                _ => None,
            }
        }
        _ => None,
    };
    println!("{}", answer.unwrap_or_else(|| json!({})));
    0
}

/// Where the `shikisha` command over there finds the bridge, and its tab's key
pub const ENV_SOCK: &str = "SHIKISHA_BRIDGE_SOCK";
pub const ENV_KEY: &str = "SHIKISHA_KEY_FILE";
/// The program of the resident process that opened a terminal, which the
/// `shikisha` command in it runs (`shim`)
pub const ENV_PROGRAM: &str = "SHIKISHA_BRIDGE_PROGRAM";

// ── Here ────────────────────────────────────────────────────────────────────

/// The line to one machine's bridge
pub struct Link {
    out: Mutex<Box<dyn Write + Send>>,
    /// The socket itself, to be closed when the line is let go: other threads
    /// hold the link, so dropping it here would close nothing
    socket: Option<std::net::TcpStream>,
    waiting: Mutex<HashMap<u64, Sender<Result<Value, String>>>>,
    conns: Mutex<HashMap<u64, Sender<Vec<u8>>>>,
    next: AtomicU64,
    up: AtomicBool,
    /// The bridge's own version, once it has said hello
    pub version: Mutex<Option<String>>,
    /// The jobs it said it holds (far-keep plan §3.1): a job this app would
    /// ask for and the bridge does not name is not asked for
    pub jobs: Mutex<Vec<String>>,
    /// Who here hears each job's messages (see [`Frame::Job`])
    job_listeners: Mutex<HashMap<String, Sender<Value>>>,
    /// Its folder over there (`$HOME/.local/share/shikisha/bridge`)
    pub home: String,
}

impl Link {
    fn say(&self, f: &Frame) -> bool {
        let mut o = self.out.lock().unwrap_or_else(|e| e.into_inner());
        let ok = o.write_all(f.line().as_bytes()).is_ok() && o.flush().is_ok();
        if !ok {
            self.up.store(false, Ordering::SeqCst);
        }
        ok
    }

    pub fn is_up(&self) -> bool {
        self.up.load(Ordering::SeqCst)
    }

    /// Ask the bridge to do something over there, and wait for its answer
    pub fn call(&self, op: &str, p: Value, wait: Duration) -> Result<Value, String> {
        if !self.is_up() {
            return Err("the bridge is not connected".into());
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = channel();
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
        if !self.say(&Frame::Op { id, op: op.to_string(), p }) {
            self.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            return Err("the bridge's line broke".into());
        }
        let got = rx.recv_timeout(wait).map_err(|_| format!("the bridge did not answer {op} in time"));
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        got?
    }

    /// Whether the bridge there holds `job` (it said so in its hello)
    pub fn holds(&self, job: &str) -> bool {
        self.jobs.lock().is_ok_and(|j| j.iter().any(|n| n == job))
    }

    /// Hear `job`'s messages from now on. One listener a job: a second takes
    /// the place of the first
    pub fn listen_job(&self, job: &str) -> Receiver<Value> {
        let (tx, rx) = channel();
        self.job_listeners.lock().unwrap_or_else(|e| e.into_inner()).insert(job.to_string(), tx);
        rx
    }

    /// Say something to `job` there. `false` when it could not be said
    pub fn to_job(&self, job: &str, m: Value) -> bool {
        let line = Frame::Job { job: job.to_string(), m }.line();
        let mut out = self.out.lock().unwrap_or_else(|e| e.into_inner());
        out.write_all(line.as_bytes()).is_ok() && out.flush().is_ok()
    }

    /// Read what comes down the line, until it ends. One thread per link
    fn listen(self: &Arc<Self>, input: impl Read) {
        for line in BufReader::new(input).lines() {
            let Ok(line) = line else { break };
            let Ok(frame) = serde_json::from_str::<Frame>(&line) else { continue };
            match frame {
                Frame::Hello { version, jobs, program, .. } => {
                    // A resident process of another build of this version,
                    // still holding what it holds: it is used as it is, for
                    // the jobs it names, and said in the log
                    let mine = program_name(env!("CARGO_PKG_VERSION"));
                    if !program.is_empty() && program != mine {
                        crate::append_hook_log(&format!(
                            "bridge: the resident process there runs {program}, not {mine}; using it for the jobs it names ({})",
                            jobs.join(", ")
                        ));
                    }
                    *self.version.lock().unwrap_or_else(|e| e.into_inner()) = Some(version);
                    *self.jobs.lock().unwrap_or_else(|e| e.into_inner()) = jobs;
                    self.up.store(true, Ordering::SeqCst);
                }
                Frame::Open { c } => self.open(c),
                Frame::Line { c, l } => {
                    if let Some(tx) = self.conns.lock().unwrap_or_else(|e| e.into_inner()).get(&c) {
                        let mut bytes = l.into_bytes();
                        bytes.push(b'\n');
                        let _ = tx.send(bytes);
                    }
                }
                Frame::Close { c } => {
                    self.conns.lock().unwrap_or_else(|e| e.into_inner()).remove(&c);
                }
                Frame::Re { id, r, e } => {
                    if let Some(tx) = self.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
                        let _ = tx.send(match e {
                            Some(e) => Err(e),
                            None => Ok(r),
                        });
                    }
                }
                // To whoever here listens for that job ([`Link::listen_job`])
                Frame::Job { job, m } => {
                    let mut to = self.job_listeners.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(tx) = to.get(&job)
                        && tx.send(m).is_err()
                    {
                        to.remove(&job);
                    }
                }
                Frame::Op { .. } | Frame::Tick | Frame::Bye => {}
            }
        }
        self.up.store(false, Ordering::SeqCst);
        // Whoever listens for a job hears the line end, and does not wait on
        // a line that is gone
        self.job_listeners.lock().unwrap_or_else(|e| e.into_inner()).clear();
        // Everybody still waiting is told, rather than left to time out
        for (_, tx) in self.waiting.lock().unwrap_or_else(|e| e.into_inner()).drain() {
            let _ = tx.send(Err("the bridge's line ended".into()));
        }
        self.conns.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// A `shikisha` command over there connected: served here like the pipe
    fn open(self: &Arc<Self>, c: u64) {
        let (tx, rx) = channel::<Vec<u8>>();
        self.conns.lock().unwrap_or_else(|e| e.into_inner()).insert(c, tx);
        let link = Arc::clone(self);
        std::thread::spawn(move || {
            let reader = Incoming { rx, held: Vec::new(), at: 0 };
            let writer = Outgoing { link: Arc::clone(&link), c, held: Vec::new() };
            if !crate::api::serve_elsewhere(reader, writer) {
                let _ = link.say(&Frame::Line {
                    c,
                    l: r#"{"ok":false,"error":"the app's external API is off"}"#.into(),
                });
            }
            let _ = link.say(&Frame::Close { c });
        });
    }
}

/// What a connection over there sent, read as a stream
struct Incoming {
    rx: Receiver<Vec<u8>>,
    held: Vec<u8>,
    at: usize,
}

impl Read for Incoming {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at >= self.held.len() {
            match self.rx.recv() {
                Ok(b) => {
                    self.held = b;
                    self.at = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = buf.len().min(self.held.len() - self.at);
        buf[..n].copy_from_slice(&self.held[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// What is said back to that connection, sent a line at a time
struct Outgoing {
    link: Arc<Link>,
    c: u64,
    held: Vec<u8>,
}

impl Write for Outgoing {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.held.extend_from_slice(buf);
        while let Some(i) = self.held.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.held.drain(..=i).collect();
            let l = String::from_utf8_lossy(&line[..line.len() - 1]).to_string();
            if !self.link.say(&Frame::Line { c: self.c, l }) {
                return Err(std::io::Error::other("the bridge's line broke"));
            }
        }
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Every line to a bridge this app holds, by the machine it goes to
/// (`Elsewhere::machine_key`)
fn links() -> &'static Mutex<HashMap<String, Arc<Link>>> {
    static L: OnceLock<Mutex<HashMap<String, Arc<Link>>>> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// The line to a machine's bridge, when it is up
pub fn link(at: &crate::elsewhere::Elsewhere) -> Option<Arc<Link>> {
    links().lock().ok()?.get(&at.machine_key()).filter(|l| l.is_up()).cloned()
}

pub fn is_up(at: &crate::elsewhere::Elsewhere) -> bool {
    link(at).is_some()
}

/// Ask a machine's bridge to do something, if its line is up
pub fn call(at: &crate::elsewhere::Elsewhere, op: &str, p: Value) -> Result<Value, String> {
    link(at)
        .ok_or_else(|| "the bridge is not connected".to_string())?
        .call(op, p, Duration::from_secs(60))
}

/// How many times the bridge is put there and started before it is left to
/// the next round
const BRING_UP_TRIES: u32 = 3;

/// Put this build of the bridge on a machine if it is not there, and start
/// it. Agreed to for this machine's entry: a machine of it without this build
/// gets it now, as the person said.
///
/// What was found there is not taken as settled (far-keep plan §4.5): between
/// looking and starting, another app of another build may have cleared this
/// build away, as nothing ran it. A start that fails is tried again from the
/// look, a few times
fn bring_up(at: &crate::elsewhere::Elsewhere) -> Result<()> {
    let mut tried = 0;
    loop {
        tried += 1;
        let done = (|| -> Result<()> {
            if installed(at)? != Installed::Current {
                install(at)?;
            }
            connect(at).map(drop)
        })();
        match done {
            Ok(()) => return Ok(()),
            Err(e) if tried >= BRING_UP_TRIES => return Err(e),
            Err(e) => {
                crate::append_hook_log(&format!("bridge: {} did not start ({e:#}); trying again", at.address()));
                std::thread::sleep(Duration::from_secs(u64::from(tried)));
            }
        }
    }
}

/// Start a machine's bridge over the line this app already has to it, and
/// keep it. Blocks until it has said hello (or not); call from a thread
pub fn connect(at: &crate::elsewhere::Elsewhere) -> Result<Arc<Link>> {
    if let Some(l) = link(at) {
        return Ok(l);
    }
    let home = far_home(at)?;
    let program = format!("{home}/{}", program_name(env!("CARGO_PKG_VERSION")));
    let stream = match at {
        crate::elsewhere::Elsewhere::Ssh(spec) => {
            crate::ssh::pipe(spec, &format!("exec {} serve", crate::ssh::sh_quote(&program)))?
        }
        crate::elsewhere::Elsewhere::Cloud(host) => crate::e2b::pipe(host, &program, &["serve".to_string()])?,
    };
    let input = stream.try_clone()?;
    let socket = stream.try_clone().ok();
    let link = Arc::new(Link {
        out: Mutex::new(Box::new(stream)),
        socket,
        waiting: Mutex::default(),
        conns: Mutex::default(),
        next: AtomicU64::new(0),
        up: AtomicBool::new(false),
        version: Mutex::new(None),
        jobs: Mutex::new(Vec::new()),
        job_listeners: Mutex::default(),
        home,
    });
    {
        let l = Arc::clone(&link);
        std::thread::Builder::new()
            .name(format!("bridge {}", at.address()))
            .spawn(move || l.listen(input))?;
    }
    // Said until the line goes down; the bridge exits once it stops hearing it
    {
        let l = Arc::clone(&link);
        std::thread::Builder::new().name(format!("bridge tick {}", at.address())).spawn(move || {
            loop {
                std::thread::sleep(TICK);
                if !l.is_up() || !l.say(&Frame::Tick) {
                    break;
                }
            }
        })?;
    }
    let end = Instant::now() + Duration::from_secs(20);
    while !link.is_up() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(100));
    }
    if !link.is_up() {
        bail!("the bridge on {} did not start", at.address());
    }
    links()
        .lock()
        .map_err(|_| anyhow!("bridge"))?
        .insert(at.machine_key(), Arc::clone(&link));
    crate::append_hook_log(&format!("bridge: connected to {}", at.address()));
    hear_missed(at, &link);
    Ok(link)
}

/// Let go of every machine's bridge, as the app quits: each is told this app
/// is going, so what its tabs ask from now on is answered at once that the PC
/// is away, rather than carried to an app that is gone
pub fn disconnect_all() {
    let all: Vec<(String, Arc<Link>)> = links().lock().map(|mut m| m.drain().collect()).unwrap_or_default();
    for (key, l) in all {
        l.say(&Frame::Bye);
        l.up.store(false, Ordering::SeqCst);
        if let Some(s) = &l.socket {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        crate::append_hook_log(&format!("bridge: let go of {key} on the way out"));
    }
}

// ── What did not get through while the app was away ──────────────────────

/// The calls each machine's tabs made while this app was away (far-keep plan
/// §4.6), as the bridge there last said: by machine, with the name the
/// person knows the machine by
static MISSED: Mutex<std::collections::BTreeMap<String, (String, crate::farmissed::Book)>> =
    Mutex::new(std::collections::BTreeMap::new());

/// Words for the person, from the lines' own threads, for the loop to show
static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The tabs the person asked, from the settings, to see the conversation of
/// (far-keep plan §4.6): by the name each tab's key file has
static CONVO_WANTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Ask the board to show a tab's conversation
pub fn want_convo(tab_key: &str) {
    if let Ok(mut w) = CONVO_WANTED.lock() {
        w.push(tab_key.to_string());
    }
}

/// The tabs asked for, taken once
pub fn take_convo_wanted() -> Vec<String> {
    CONVO_WANTED.lock().map(|mut w| std::mem::take(&mut *w)).unwrap_or_default()
}

/// What the bridges have to tell the person, taken once
pub fn take_said() -> Vec<String> {
    SAID.lock().map(|mut s| std::mem::take(&mut *s)).unwrap_or_default()
}

/// Every machine's calls that did not get through, as last heard: the machine
/// (`Elsewhere::machine_key`), its name, and the calls
pub fn missed() -> Vec<(String, String, crate::farmissed::Book)> {
    MISSED
        .lock()
        .map(|m| m.iter().filter(|(_, (_, b))| !b.calls.is_empty() || b.dropped > 0).map(|(k, (n, b))| (k.clone(), n.clone(), b.clone())).collect())
        .unwrap_or_default()
}

/// Listen for what a machine's bridge says of the calls its tabs made while
/// this app was away, and ask for them now: the person is told how many once
/// the line is up, and they stay listed until the person has looked
fn hear_missed(at: &crate::elsewhere::Elsewhere, link: &Arc<Link>) {
    if !link.holds("tabs") {
        return;
    }
    let from = link.listen_job("tabs");
    let (key, name) = (at.machine_key(), at.address());
    std::thread::spawn(move || {
        // How many the person has been told of: the bridge says again when
        // one is written down while this app is connected (a call cut as an
        // earlier line of this app went), and only a larger count is news
        let mut told = 0;
        for m in from {
            if m["did"] != "missed" {
                continue;
            }
            let Ok(book) = serde_json::from_value::<crate::farmissed::Book>(m["book"].clone()) else { continue };
            let n = book.calls.len() as u64 + book.dropped;
            if n > told {
                told = n;
                crate::append_hook_log(&format!("bridge: {n} call(s) from {name}'s tabs did not get through while this app was away"));
                if let Ok(mut s) = SAID.lock() {
                    s.push(crate::i18n::tp("msg.bridge.missed", &[("host", &name), ("n", &n.to_string())]));
                }
            }
            if let Ok(mut all) = MISSED.lock() {
                all.insert(key.clone(), (name.clone(), book));
            }
        }
    });
    link.to_job("tabs", json!({ "do": "missed" }));
}

/// Strike out the calls the person looked at on a machine (`None`: all of
/// them). Said to the bridge there, which answers with what is left
pub fn seen_missed(machine: &str, calls: Option<Vec<crate::farmissed::Missed>>) -> Result<(), String> {
    let link = links()
        .lock()
        .ok()
        .and_then(|m| m.get(machine).cloned())
        .filter(|l| l.is_up())
        .ok_or_else(|| "the bridge is not connected".to_string())?;
    let said = match calls {
        Some(c) => json!({ "do": "seen", "calls": c }),
        None => json!({ "do": "seen" }),
    };
    if link.to_job("tabs", said) { Ok(()) } else { Err("the bridge's line is down".into()) }
}

/// Let go of a machine's bridge: it is told this app is going, its input
/// ends, and the door exits over there
pub fn disconnect(at: &crate::elsewhere::Elsewhere) {
    if let Some(l) = links().lock().ok().and_then(|mut m| m.remove(&at.machine_key())) {
        l.say(&Frame::Bye);
        l.up.store(false, Ordering::SeqCst);
        // Closing the socket ends the program's input over there, and it exits
        if let Some(s) = &l.socket {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        crate::append_hook_log(&format!("bridge: let go of {}", at.address()));
    }
}

/// The bridge's file name for one version. A new version is a new file beside
/// the old, so one being replaced is never the one running.
///
/// With a piece of the hash of the bridges this copy of the app carries: two
/// builds of one version are two programs (far-keep plan §4.5), and a machine
/// holding the older of them has to be given the newer, not told it has it
pub fn program_name(version: &str) -> String {
    match bundled_stamp() {
        Some(stamp) => format!("shikisha-bridge-{version}-{stamp}"),
        None => format!("shikisha-bridge-{version}"),
    }
}

/// The hash of every bridge this copy carries, by name and content, cut to a
/// dozen hex digits: the same for every machine, whatever its processor, so
/// the name is known before the machine is asked what it is
fn bundled_stamp() -> Option<String> {
    use sha2::Digest as _;
    static STAMP: OnceLock<Option<String>> = OnceLock::new();
    STAMP
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            let dir = bridge_dirs(exe.parent()?).into_iter().find(|d| d.is_dir())?;
            let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
                .ok()?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("shikisha-bridge-")))
                .collect();
            if files.is_empty() {
                return None;
            }
            files.sort();
            let mut h = sha2::Sha256::new();
            for f in &files {
                h.update(f.file_name()?.to_string_lossy().as_bytes());
                h.update(std::fs::read(f).ok()?);
            }
            Some(h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect())
        })
        .clone()
}

/// Where the bridge lives on a machine, as an absolute path there
fn far_home(at: &crate::elsewhere::Elsewhere) -> Result<String> {
    static KNOWN: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(h) = known.lock().ok().and_then(|m| m.get(&at.machine_key()).cloned()) {
        return Ok(h);
    }
    let ran = crate::elsewhere::exec(at, "printf %s \"$HOME\"", 30_000)?;
    let home = ran.out.trim();
    if home.is_empty() {
        bail!("the machine did not say where its home is");
    }
    let dir = format!("{home}/{HOME_DIR}");
    if let Ok(mut m) = known.lock() {
        m.insert(at.machine_key(), dir.clone());
    }
    Ok(dir)
}

// ── Putting it on, and taking it off ──────────────────────────────────────

/// What a machine has: which bridge, if any
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Installed {
    /// None there
    No,
    /// This app's version
    Current,
    /// Another version (this app puts its own beside it when it connects)
    Other(String),
}

/// The bridge's program for a machine's processor, from the folder beside this
/// app's program. `uname -m` is what the machine said
pub fn bundled_for(machine: &str) -> Result<std::path::PathBuf> {
    let arch = match machine.trim() {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        other => bail!("there is no bridge for a {other} machine"),
    };
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| anyhow!("no folder beside the program"))?;
    let name = format!("shikisha-bridge-{arch}-linux");
    let places = bridge_dirs(dir);
    places
        .iter()
        .map(|d| d.join(&name))
        .find(|f| f.exists())
        .ok_or_else(|| {
            let looked: Vec<String> = places.iter().map(|d| d.join(&name).display().to_string()).collect();
            anyhow!("this copy of the app has no bridge for {arch} machines (looked for {})", looked.join(", "))
        })
}

/// Where a copy of the app keeps the bridges it puts on other machines: the
/// `bridge` folder beside its program (the Windows zip, the Linux tar and the
/// installer that unpacks it), and where the server version's package puts
/// them (far-keep plan §8)
fn bridge_dirs(beside: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut dirs = vec![beside.join("bridge")];
    if cfg!(target_os = "linux") {
        dirs.push(std::path::PathBuf::from("/usr/lib/shikisha/bridge"));
    }
    dirs
}

/// Whether a machine has the bridge, and which
pub fn installed(at: &crate::elsewhere::Elsewhere) -> Result<Installed> {
    let home = far_home(at)?;
    let ran = crate::elsewhere::exec(at, &format!("ls -1 {} 2>/dev/null", crate::ssh::sh_quote(&home)), 30_000)?;
    let mine = program_name(env!("CARGO_PKG_VERSION"));
    let found: Vec<&str> = ran.out.lines().map(str::trim).filter(|l| l.starts_with("shikisha-bridge-")).collect();
    Ok(if found.contains(&mine.as_str()) {
        Installed::Current
    } else if let Some(other) = found.first() {
        Installed::Other(other.trim_start_matches("shikisha-bridge-").to_string())
    } else {
        Installed::No
    })
}

/// Put the bridge on a machine. Only ever called because the person said so
/// (the settings, or the question asked where it is needed). The tabs'
/// command is written beside it. Of the other builds there, only those
/// nothing runs are cleared away (`clear_old_builds`): a resident process of
/// one may still be running, holding what it holds, and a door an older app
/// opens runs one by its name
pub fn install(at: &crate::elsewhere::Elsewhere) -> Result<()> {
    let home = far_home(at)?;
    let machine = crate::elsewhere::exec(at, "uname -m", 30_000)?.out;
    let from = bundled_for(&machine)?;
    let program = format!("{home}/{}", program_name(env!("CARGO_PKG_VERSION")));
    let q = |s: &str| crate::ssh::sh_quote(s);
    crate::elsewhere::exec(at, &format!("mkdir -p {}/bin && chmod 700 {}", q(&home), q(&home)), 30_000)?;
    let part = format!("{program}.part");
    crate::elsewhere::files(
        at,
        crate::ssh::FileJob::Put { from, to: part.clone(), overwrite: true },
        10 * 60_000,
    )?;
    let shim = shim(&home, &program);
    let bin = format!("{home}/bin/shikisha");
    let ran = crate::elsewhere::exec(
        at,
        &format!(
            "chmod 700 {part} && mv -f {part} {prog} && printf %s {shim} > {bin} && chmod 700 {bin} \
             && {prog} --version",
            part = q(&part),
            prog = q(&program),
            shim = q(&shim),
            bin = q(&bin),
        ),
        60_000,
    )?;
    if !ran.out.contains(env!("CARGO_PKG_VERSION")) {
        bail!("the bridge was put on {} but did not run there: {}", at.address(), ran.err.trim());
    }
    clear_old_builds(at, &home, &program);
    crate::append_hook_log(&format!("bridge: installed on {}", at.address()));
    Ok(())
}

/// The tabs' command, `bin/shikisha`: the bridge's own, under the name the
/// tabs call. Every build writes the same script, so a tab of an older
/// resident process that calls it after a newer build was put there still
/// reaches a program (far-keep plan §4.5): the resident process's own, which
/// it puts in the environment of the terminals it opens; else the build put
/// there last; else any build there. What the command then says goes to the
/// socket in the tab's environment in the API's own lines, which no build
/// changes
fn shim(home: &str, program: &str) -> String {
    let q = crate::ssh::sh_quote;
    format!(
        "#!/bin/sh\n\
         for p in \"${ENV_PROGRAM}\" {program} {home}/shikisha-bridge-*; do\n\
         \x20 case \"$p\" in ''|*.part) continue ;; esac\n\
         \x20 [ -x \"$p\" ] && exec \"$p\" cli \"$@\"\n\
         done\n\
         echo 'shikisha: the SHIKISHA bridge program is not on this machine; connect SHIKISHA-TERM to it again' >&2\n\
         exit 127\n",
        program = q(program),
        home = q(home),
    )
}

/// Delete the other builds on a machine that nothing runs any more (far-keep
/// plan §4.5): each resident process and door holds a shared lock on its
/// build's mark while it runs, and a build is deleted only while its mark
/// can be locked whole, without waiting -- and with the lock held, so none
/// starts in between. A build with no mark is older than the marks and is
/// left where it is. A machine without `flock` keeps them all
fn clear_old_builds(at: &crate::elsewhere::Elsewhere, home: &str, keep: &str) {
    let q = |s: &str| crate::ssh::sh_quote(s);
    let name = keep.rsplit('/').next().unwrap_or(keep);
    let script = format!(
        "command -v flock >/dev/null || exit 0; cd {home} || exit 0; \
         for f in shikisha-bridge-*; do case \"$f\" in {name}|*.part) continue;; esac; \
         [ -e \".$f.lock\" ] || continue; \
         flock -n -x \".$f.lock\" -c \"rm -f -- '$f' '.$f.lock'\" && echo \"cleared $f\"; done",
        home = q(home),
        name = name,
    );
    match crate::elsewhere::exec(at, &script, 60_000) {
        Ok(ran) => {
            for line in ran.out.lines().filter(|l| l.starts_with("cleared ")) {
                crate::append_hook_log(&format!("bridge: {} on {}", line, at.address()));
            }
        }
        Err(e) => crate::append_hook_log(&format!("bridge: old builds on {} were not cleared: {e:#}", at.address())),
    }
}

/// Take the bridge off a machine: its line is let go, and its folder -- the
/// program, the tabs' command, the keys -- is deleted
pub fn remove(at: &crate::elsewhere::Elsewhere) -> Result<()> {
    // Not from under another app: the folder, its sockets and its key are
    // every app's on that machine, and one app's "no" is not theirs
    let others = match link(at) {
        Some(l) if l.jobs.lock().is_ok_and(|j| j.iter().any(|n| n == "host")) => l
            .call("host_lines", json!({}), Duration::from_secs(20))
            .ok()
            .and_then(|v| v.get("lines").and_then(|n| n.as_u64()))
            .map(|n| n.saturating_sub(1))
            .unwrap_or(0),
        Some(_) => 0,
        // Not connected from here: a resident process there is holding some
        // other app's line (it leaves a few seconds after the last one goes)
        None => {
            let home = far_home(at)?;
            let sock = format!("{home}/run/{}", KEEP_SOCK);
            let ran = crate::elsewhere::exec(at, &format!("test -S {} && echo held", crate::ssh::sh_quote(&sock)), 30_000)?;
            u64::from(ran.out.contains("held"))
        }
    };
    if others > 0 {
        bail!(crate::i18n::tp("err.bridge.in_use", &[("host", &at.address()), ("n", &others.to_string())]));
    }
    // What it holds ends with it (far-keep plan §7.7): the person was told
    // how many AIs run there before they took it off
    crate::farterm::end_all(at);
    disconnect(at);
    let home = far_home(at)?;
    if !home.ends_with(HOME_DIR) {
        bail!("refusing to delete {home}");
    }
    // A resident process this app had no line to -- one holding AIs while the
    // app was away -- is told to end too (it ends its terminals' programs
    // first), and the folder goes only once it is seen gone: one that does
    // not go leaves the removal undone, to be tried again
    let daemon = crate::ssh::sh_quote(&format!("{home}/shikisha-bridge-[^ ]* daemon"));
    let ran = crate::elsewhere::exec(
        at,
        &format!(
            "pkill -TERM -f {daemon} 2>/dev/null; i=0; \
             while pgrep -f {daemon} >/dev/null && [ $i -lt 20 ]; do sleep 0.5; i=$((i+1)); done; \
             if pgrep -f {daemon} >/dev/null; then echo still-running; else rm -rf {home}; fi",
            home = crate::ssh::sh_quote(&home)
        ),
        60_000,
    )?;
    if ran.out.contains("still-running") {
        bail!("the bridge's resident process on {} did not end; its folder is left, to be taken off again", at.address());
    }
    // Its terminals went with it: none is to be gone back to
    crate::farterm::forget_machine(at);
    crate::append_hook_log(&format!("bridge: removed from {}", at.address()));
    Ok(())
}

/// What a tab over there needs in its environment for `shikisha` to reach this
/// app through the bridge: the command on its PATH, the bridge's socket, and
/// where its key is. Written as a line typed before the tab's program starts
/// (the environment of a login over SSH is the server's to decide)
pub fn tab_env(home: &str, tab: &str) -> Vec<(String, String)> {
    vec![
        ("PATH".into(), format!("{home}/bin:$PATH")),
        (ENV_SOCK.into(), format!("{home}/run/{}", TABS_SOCK)),
        (ENV_KEY.into(), format!("{home}/keys/{}", key_name(tab))),
    ]
}

/// The file a tab's key is kept in over there: its name in letters any file
/// system and any shell take as they are, and a short hash of the whole name so
/// two tabs whose names differ only in other letters do not share one
pub fn key_name(tab: &str) -> String {
    let plain: String = tab
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(40)
        .collect();
    let mut h: u32 = 0x811c_9dc5;
    for b in tab.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{}-{h:08x}", plain.trim_start_matches('_'))
}

/// Hand a tab's key to the bridge on its machine, for its `shikisha` command
pub fn give_key(at: &crate::elsewhere::Elsewhere, tab: &str, key: &str) -> Result<(), String> {
    call(at, "put_key", json!({"tab": key_name(tab), "key": key})).map(|_| ())
}

// ── Keeping the lines ─────────────────────────────────────────────────────

/// The machines the person agreed to, as last read from the settings
static AGREED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The machines this app keeps lines to, by machine, with the name of the
/// settings entry each is of: how the settings find a machine entry's lines
/// (a MicroVM entry has one machine a worktree)
static WANTED: Mutex<Vec<(String, String, crate::elsewhere::Elsewhere)>> = Mutex::new(Vec::new());

/// The machines of a settings entry this app has a line to now
pub fn up_for(host: &str) -> Vec<crate::elsewhere::Elsewhere> {
    WANTED
        .lock()
        .map(|w| w.iter().filter(|(_, h, _)| h == host).map(|(_, _, at)| at.clone()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(is_up)
        .collect()
}

/// The settings entry a machine is of, as this app last knew it
pub fn host_of(machine: &str) -> Option<String> {
    WANTED.lock().ok().and_then(|w| w.iter().find(|(k, _, _)| k == machine).map(|(_, h, _)| h.clone()))
}

/// Whether the person agreed to the bridge on the machine this entry names
pub fn agreed(host: &str) -> bool {
    AGREED.lock().is_ok_and(|a| a.iter().any(|h| h == host))
}

/// A machine this app wants a line to right now: a tab on it runs an AI, the
/// person agreed to the bridge there, and (a MicroVM) it is awake anyway
pub struct Want {
    pub at: crate::elsewhere::Elsewhere,
    /// Its entry's name in the settings
    pub host: String,
    /// The keys of its tabs, by the name each tab's key file has
    pub keys: Vec<(String, String)>,
}

/// Holds the lines the app wants and lets go of the rest. Everything that
/// touches the network is done on a thread; the loop only asks, a few times a
/// minute, what should be up
#[derive(Default)]
pub struct Keeper {
    busy: Arc<Mutex<std::collections::HashSet<String>>>,
    given: Arc<Mutex<std::collections::HashSet<String>>>,
    tried: HashMap<String, Instant>,
    held: HashMap<String, crate::elsewhere::Elsewhere>,
    agreed: Option<Vec<String>>,
    /// Machines already looked over for a bridge nobody agreed to (this run)
    swept: std::collections::HashSet<String>,
    /// Machines the person took the bridge off whose folder is not deleted
    /// yet, by machine, with the entry's name: tried again until it is
    unremoved: Arc<Mutex<HashMap<String, (String, crate::elsewhere::Elsewhere)>>>,
    /// When each of those was last tried
    remove_tried: HashMap<String, Instant>,
}

/// How long after a failed try a machine is tried again
const RETRY: Duration = Duration::from_secs(60);

impl Keeper {
    pub fn tend(&mut self, wanted: Vec<Want>) {
        let now = Instant::now();
        if let Ok(mut w) = WANTED.lock() {
            *w = wanted.iter().map(|w| (w.at.machine_key(), w.host.clone(), w.at.clone())).collect();
        }
        self.retry_removals(now);
        let keys: std::collections::HashSet<String> = wanted.iter().map(|w| w.at.machine_key()).collect();
        // Nothing on it wants the line any more: let go, and the bridge there exits
        let gone: Vec<String> = self.held.keys().filter(|k| !keys.contains(*k)).cloned().collect();
        for k in gone {
            if let Some(at) = self.held.remove(&k) {
                disconnect(&at);
            }
            // Let go of, not failed: wanted again -- a tab waiting for its
            // line to open its terminal -- it is made again at once
            self.tried.remove(&k);
        }
        for w in wanted {
            let key = w.at.machine_key();
            if self.busy.lock().is_ok_and(|b| b.contains(&key)) {
                continue;
            }
            let up = is_up(&w.at);
            // A line that is not up is a bridge that may have started over in
            // a folder made again -- taken off and put back, or deleted by
            // someone over there -- with none of the keys it was given. Every
            // key goes again then; over a line that stayed up, only new ones
            let fresh: Vec<(String, String)> = if up {
                let given = self.given.lock().unwrap_or_else(|e| e.into_inner());
                w.keys.iter().filter(|(t, k)| !given.contains(&format!("{key}|{t}|{k}"))).cloned().collect()
            } else {
                w.keys.clone()
            };
            if up && fresh.is_empty() {
                continue;
            }
            if !up && self.tried.get(&key).is_some_and(|t| now.duration_since(*t) < RETRY) {
                continue;
            }
            self.tried.insert(key.clone(), now);
            self.held.insert(key.clone(), w.at.clone());
            if let Ok(mut b) = self.busy.lock() {
                b.insert(key.clone());
            }
            let (busy, given) = (Arc::clone(&self.busy), Arc::clone(&self.given));
            let _ = std::thread::Builder::new().name(format!("bridge keep {}", w.at.address())).spawn(move || {
                let done = (|| -> Result<()> {
                    if !is_up(&w.at) {
                        bring_up(&w.at)?;
                    }
                    for (tab, k) in fresh {
                        give_key(&w.at, &tab, &k).map_err(|e| anyhow!(e))?;
                        if let Ok(mut g) = given.lock() {
                            g.insert(format!("{}|{tab}|{k}", w.at.machine_key()));
                        }
                    }
                    Ok(())
                })();
                if let Err(e) = done {
                    crate::append_hook_log(&format!("bridge: {} ({}): {e:#}", w.at.address(), w.host));
                }
                if let Ok(mut b) = busy.lock() {
                    b.remove(&w.at.machine_key());
                }
            });
        }
    }

    /// The folders the person asked to have deleted that could not be yet,
    /// tried again a minute apart until they are
    fn retry_removals(&mut self, now: Instant) {
        let due: Vec<(String, crate::elsewhere::Elsewhere)> = {
            let pending = self.unremoved.lock().unwrap_or_else(|e| e.into_inner());
            pending
                .iter()
                .filter(|(k, _)| self.remove_tried.get(*k).is_none_or(|t| now.duration_since(*t) >= RETRY))
                .map(|(k, (_, at))| (k.clone(), at.clone()))
                .collect()
        };
        for (key, at) in due {
            // Being put there or taken off right now: tried once that is done,
            // so two threads never work on one machine's folder at once
            if self.busy.lock().is_ok_and(|b| b.contains(&key)) {
                continue;
            }
            // A MicroVM is never woken to have its folder deleted: it is tried
            // while it is awake, which is when this app is using it
            if let crate::elsewhere::Elsewhere::Cloud(h) = &at
                && !h.instance.as_deref().is_some_and(crate::e2b::awake)
            {
                continue;
            }
            self.remove_tried.insert(key.clone(), now);
            self.take_off(key, at);
        }
    }

    /// Delete the bridge's folder on a machine, on a thread. Until it is gone
    /// the machine stays on the list to try again, and the keys it was given
    /// are forgotten -- a bridge put back there starts with none
    fn take_off(&self, key: String, at: crate::elsewhere::Elsewhere) {
        if let Ok(mut g) = self.given.lock() {
            g.retain(|k| !k.starts_with(&format!("{key}|")));
        }
        if let Ok(mut b) = self.busy.lock() {
            b.insert(key.clone());
        }
        let (busy, unremoved) = (Arc::clone(&self.busy), Arc::clone(&self.unremoved));
        let _ = std::thread::Builder::new().name("bridge remove".into()).spawn(move || {
            // Agreed to again since it was asked for: it stays
            if !unremoved.lock().is_ok_and(|u| u.get(&key).is_some_and(|(name, _)| !agreed(name))) {
                if let Ok(mut b) = busy.lock() {
                    b.remove(&key);
                }
                return;
            }
            match remove(&at) {
                Ok(()) => {
                    if let Ok(mut u) = unremoved.lock() {
                        u.remove(&key);
                    }
                }
                Err(e) => crate::append_hook_log(&format!("bridge: taking it off {} failed, trying again in a minute: {e:#}", at.address())),
            }
            if let Ok(mut b) = busy.lock() {
                b.remove(&key);
            }
        });
    }

    /// Machines this app is using anyway whose entry the person has not agreed
    /// to (any more): a bridge left there -- unticked while the machine was
    /// paused, or before this app last started -- is taken off. Looked at once
    /// per machine per run, and never by waking anything. A machine that could
    /// not be looked at, or whose folder could not be deleted, joins the ones
    /// tried again every minute
    pub fn sweep(&mut self, awake_not_agreed: Vec<(String, crate::elsewhere::Elsewhere)>) {
        for (name, at) in awake_not_agreed {
            let key = at.machine_key();
            // Being put there or taken off right now: looked at once that is
            // done, never at the same time
            if self.swept.contains(&key) || self.busy.lock().is_ok_and(|b| b.contains(&key)) {
                continue;
            }
            self.swept.insert(key.clone());
            if let Ok(mut b) = self.busy.lock() {
                b.insert(key.clone());
            }
            let (busy, unremoved) = (Arc::clone(&self.busy), Arc::clone(&self.unremoved));
            let _ = std::thread::Builder::new().name("bridge sweep".into()).spawn(move || {
                // Agreed to again while this was on its way: it stays, and
                // nothing is left behind to take it off later
                let still_off = || !agreed(&name);
                let later = |why: String| {
                    crate::append_hook_log(&format!("bridge: {why}, trying again in a minute"));
                    if let Ok(mut u) = unremoved.lock()
                        && still_off()
                    {
                        u.insert(at.machine_key(), (name.clone(), at.clone()));
                    }
                };
                match installed(&at) {
                    Ok(Installed::No) => {}
                    Ok(_) if still_off() => {
                        if let Err(e) = remove(&at) {
                            later(format!("taking it off {} failed: {e:#}", at.address()));
                        }
                    }
                    Ok(_) => {}
                    Err(e) => later(format!("could not look at {}: {e:#}", at.address())),
                }
                if let Ok(mut b) = busy.lock() {
                    b.remove(&key);
                }
            });
        }
    }

    /// The machines the person agreed to, as the settings say now. One taken
    /// off the list has the bridge taken off it, wherever this app can reach
    /// without waking anything: servers, and MicroVMs that are awake
    pub fn agreed_now(&mut self, agreed: &[String], hosts: &[crate::config::HostSpec], awake: &[crate::elsewhere::Elsewhere]) {
        if let Ok(mut a) = AGREED.lock() {
            *a = agreed.to_vec();
        }
        // Agreed to again before its folder could be deleted: it stays
        if let Ok(mut u) = self.unremoved.lock() {
            u.retain(|_, (name, _)| !agreed.contains(name));
        }
        let before = self.agreed.replace(agreed.to_vec());
        let Some(before) = before else { return };
        for name in before.iter().filter(|b| !agreed.contains(b)) {
            let mut targets: Vec<crate::elsewhere::Elsewhere> = awake
                .iter()
                .filter(|at| matches!(at, crate::elsewhere::Elsewhere::Cloud(h) if &h.name == name))
                .cloned()
                .collect();
            if let Some(h) = hosts.iter().find(|h| &h.name == name && !h.is_made())
                && let Ok(at) = crate::elsewhere::Elsewhere::of(h)
            {
                targets.push(at);
            }
            for at in targets {
                let key = at.machine_key();
                self.held.remove(&key);
                if let Ok(mut u) = self.unremoved.lock() {
                    u.insert(key.clone(), (name.clone(), at.clone()));
                }
                self.remove_tried.remove(&key);
            }
        }
        // Taken off at once -- or, on a machine the bridge is being put on at
        // this moment, as soon as that is done, never at the same time
        self.retry_removals(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tabs' command runs the build of the resident process that opened
    /// the tab; without one, the build put there last; without that, any
    /// build there; and with none, says so. Each made-up build here says its
    /// name and what it was asked
    #[cfg(unix)]
    #[test]
    fn the_tabs_command_finds_a_build_whichever_was_put_there_last() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = std::env::temp_dir().join(format!("shikisha shim {}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("bin")).unwrap();
        let build = |name: &str| {
            let p = home.join(name);
            std::fs::write(&p, format!("#!/bin/sh\necho {name} \"$@\"\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
            p
        };
        let (older, newer) = (build("shikisha-bridge-1-aaaa"), build("shikisha-bridge-2-bbbb"));
        build("shikisha-bridge-3-cccc.part");
        let bin = home.join("bin/shikisha");
        std::fs::write(&bin, shim(&home.to_string_lossy(), &newer.to_string_lossy())).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let run = |program: Option<&std::path::Path>| {
            let mut c = std::process::Command::new(&bin);
            c.args(["tab_list", "a b"]).env_remove(ENV_PROGRAM);
            if let Some(p) = program {
                c.env(ENV_PROGRAM, p);
            }
            let o = c.output().unwrap();
            (o.status.code(), String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        assert_eq!(run(Some(&older)), (Some(0), "shikisha-bridge-1-aaaa cli tab_list a b".to_string()));
        assert_eq!(run(None), (Some(0), "shikisha-bridge-2-bbbb cli tab_list a b".to_string()));
        std::fs::remove_file(&newer).unwrap();
        assert_eq!(run(Some(&newer)), (Some(0), "shikisha-bridge-1-aaaa cli tab_list a b".to_string()));
        std::fs::remove_file(&older).unwrap();
        assert_eq!(run(None).0, Some(127), "a build half put there is never run");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// The bridges are looked for beside the program first, and on Linux also
    /// where the server version's package puts them
    #[test]
    fn the_bridges_are_looked_for_where_every_kind_of_install_puts_them() {
        let beside = std::path::Path::new("/opt/app");
        let dirs = bridge_dirs(beside);
        assert_eq!(dirs[0], beside.join("bridge"), "beside the program comes first");
        assert_eq!(
            dirs.iter().any(|d| d == std::path::Path::new("/usr/lib/shikisha/bridge")),
            cfg!(target_os = "linux"),
            "{dirs:?}"
        );
    }

    #[test]
    fn a_frame_is_one_line_of_json() {
        for f in [
            Frame::Hello { version: "1".into(), rev: "x".into(), jobs: vec!["ops".into()], program: "shikisha-bridge-1-abc".into() },
            Frame::Open { c: 3 },
            Frame::Line { c: 3, l: "{\"id\":\"1\"}".into() },
            Frame::Close { c: 3 },
            Frame::Op { id: 9, op: "ping".into(), p: json!({}) },
            Frame::Tick,
            Frame::Re { id: 9, r: json!({"a": 1}), e: None },
            Frame::Re { id: 9, r: Value::Null, e: Some("no".into()) },
        ] {
            let line = f.line();
            assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "{line}");
            assert_eq!(serde_json::from_str::<Frame>(line.trim()).unwrap(), f);
        }
    }

    #[test]
    fn a_program_for_every_machine_or_a_reason() {
        assert!(bundled_for("riscv64").unwrap_err().to_string().contains("riscv64"));
    }

    #[test]
    fn a_tab_over_there_finds_the_command_the_socket_and_its_key() {
        let env = tab_env("/home/u/.local/share/shikisha/bridge", "coder");
        assert_eq!(env[0], ("PATH".to_string(), "/home/u/.local/share/shikisha/bridge/bin:$PATH".to_string()));
        assert!(env.iter().any(|(k, v)| k == ENV_KEY && v.ends_with(&format!("/keys/{}", key_name("coder")))));
    }

    #[test]
    fn every_tab_name_gives_a_key_file_the_bridge_accepts() {
        let dir = std::path::Path::new("/h");
        for name in ["coder", "Claude 2", "レビュー", "../etc", "a/b", ".x", ""] {
            let k = key_name(name);
            assert!(crate::farops::key_file(dir, &k).is_some(), "{name} -> {k}");
        }
        assert_ne!(key_name("a b"), key_name("a_b"), "two names, two files");
    }

    /// The line itself, both ends in this process: a connection over there is
    /// carried here and served; an operation is asked and answered
    #[test]
    fn a_call_goes_down_the_line_and_comes_back() {
        let (here, there) = crate::ssh::socket_pair().unwrap();
        let far_in = there.try_clone().unwrap();
        // Over there: answers operations the way the bridge does
        std::thread::spawn(move || {
            let mut out = there;
            let _ = out.write_all(Frame::Hello { version: "t".into(), rev: "t".into(), jobs: Vec::new(), program: String::new() }.line().as_bytes());
            for line in BufReader::new(far_in).lines() {
                let Ok(line) = line else { break };
                if let Ok(Frame::Op { id, op, p }) = serde_json::from_str::<Frame>(&line) {
                    let (r, e) = match crate::farops::run(&op, &p) {
                        Ok(r) => (r, None),
                        Err(e) => (Value::Null, Some(e)),
                    };
                    let _ = out.write_all(Frame::Re { id, r, e }.line().as_bytes());
                }
            }
        });
        let input = here.try_clone().unwrap();
        let link = Arc::new(Link {
            out: Mutex::new(Box::new(here)),
            socket: None,
            waiting: Mutex::default(),
            conns: Mutex::default(),
            next: AtomicU64::new(0),
            up: AtomicBool::new(false),
            version: Mutex::new(None),
            jobs: Mutex::new(Vec::new()),
            job_listeners: Mutex::default(),
            home: String::new(),
        });
        let l = Arc::clone(&link);
        std::thread::spawn(move || l.listen(input));
        let end = Instant::now() + Duration::from_secs(5);
        while !link.is_up() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(link.is_up());
        assert_eq!(link.call("ping", Value::Null, Duration::from_secs(5)).unwrap()["version"], env!("CARGO_PKG_VERSION"));
        assert!(link.call("teleport", Value::Null, Duration::from_secs(5)).unwrap_err().contains("older version"));
    }
}

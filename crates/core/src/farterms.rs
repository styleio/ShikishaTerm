//! Holding terminals on the bridge's machine (far-keep plan §4.4): the job
//! `terms` of the resident process ([`crate::fardaemon`]).
//!
//! A tab whose machine keeps its AI while the app is away has its terminal
//! opened here, by the resident process, rather than by the app over its own
//! line: the shell and the AI in it are the resident process's children and
//! outlive the line. The app attaches to a terminal it opened -- again, after
//! it was away -- and is handed the terminal's state ([`crate::termstate`])
//! and then its output as it comes.
//!
//! What this job promises the app:
//!
//!   - **Who is asking about which terminal.** Every terminal has an id the
//!     resident process never gives twice, and the resident process a
//!     generation of its own made when it started. Asked about a terminal,
//!     it answers one of three things: alive (and here is its state), ended
//!     (it saw the program end, and here is the code), or unknown (another
//!     generation, an id it never gave, or another tab's). Unknown is never
//!     taken for ended: an app that cannot tell does not start a second AI
//!   - **One owner.** Attaching takes the terminal: the one who had it is
//!     told, and what it sends after that -- keys, a size, a stop -- is
//!     refused. The owner is a number handed out on each attach
//!   - **The state and then what came after it, in that order.** The state
//!     is taken, and the output after it counted, under the terminal's lock,
//!     where nothing is sent; what is sent goes through the terminal's one
//!     queue, the state first
//!   - **Answers to the program's questions.** A program that asks where its
//!     cursor is waits for the answer. While nobody owns the terminal this
//!     job answers; while somebody does, the owner's parser does, and this
//!     one keeps quiet -- the switch is the attach, taken under the lock
//!   - **The code of a program that ended is kept** until the app says it
//!     has it, so "ended" can be answered after the app was away
//!   - **What happens while no app owns it is the app's to say** (far-keep
//!     plan §4.3): each terminal is opened, and attached to, with what to do
//!     when its owner goes -- end it (after a short while, so a line that
//!     drops and comes back finds it), keep it for a set time, or keep it
//!     for as long as its program runs ([`Away`])

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};

use crate::fardaemon::{Core, Job};
use crate::farlink::Frame;

/// The job's name on the line
pub const NAME: &str = "terms";
pub use crate::termstate::SCROLLBACK_SENT;
/// Scrollback kept by the resident process's own parser
const SCROLLBACK_KEPT: usize = 5000;
/// How much output may wait in a terminal's queue for an owner that is not
/// taking it -- a line gone slow -- before the owner is let go of. Output is
/// never dropped from the terminal: the owner, attaching again, is handed
/// the state with all of it in
const QUEUED_MOST: u64 = 32 * 1024 * 1024;
/// How long a terminal whose owner went is kept when it is to end with its
/// owner: long enough for a line that dropped to come back and attach again,
/// short enough that an app that is gone does not leave its AI running
const STOP_GRACE: Duration = if cfg!(test) { Duration::from_secs(2) } else { Duration::from_secs(60) };
/// How long the code of a terminal that ended while its owner was away is
/// kept for the owner to come back for, when it was to be kept running
const ENDED_KEPT: Duration = Duration::from_secs(24 * 60 * 60);

/// What a terminal does while no app owns it (far-keep plan §4.3), as the
/// app says on the line: `"stop"`, `{"seconds": n}`, or `"always"`. Three
/// states, and no number that means one of them. A terminal opened by an app
/// that says nothing -- an older one -- ends with it, as everything did
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Away {
    Stop,
    For(Duration),
    Always,
}

impl Away {
    pub fn read(v: &Value) -> Option<Self> {
        match v {
            Value::String(s) if s == "stop" => Some(Self::Stop),
            Value::String(s) if s == "always" => Some(Self::Always),
            Value::Object(o) => o.get("seconds").and_then(Value::as_u64).map(|n| Self::For(Duration::from_secs(n))),
            _ => None,
        }
    }

    pub fn write(self) -> Value {
        match self {
            Self::Stop => json!("stop"),
            Self::For(d) => json!({ "seconds": d.as_secs() }),
            Self::Always => json!("always"),
        }
    }

    /// How long after its owner went the terminal is ended. None: never
    fn ends_after(self) -> Option<Duration> {
        match self {
            Self::Stop => Some(STOP_GRACE),
            Self::For(d) => Some(d),
            Self::Always => None,
        }
    }
}

pub(crate) fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(v: &Value) -> Vec<u8> {
    v.as_str()
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .unwrap_or_default()
}

/// What a terminal's program asked of the terminal, answered here while
/// nobody owns it, and kept either way so an owner who attaches carries on
/// with it: the keyboard it asked for, its title, where it says it is
struct Held {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Whether this job answers the program's questions: nobody owns it
    answering: bool,
    keyboard: Vec<u8>,
    title: String,
    cwd: String,
}

impl Held {
    fn reply(&self, bytes: &[u8]) {
        if !self.answering {
            return;
        }
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }
}

impl vt100::Callbacks for Held {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        if let Some(t) = crate::tab::title_of(params) {
            self.title = t;
        } else if let Some(c) = crate::tab::cwd_of(params) {
            self.cwd = c;
        }
    }

    // The same answers a tab gives (`tab::QueryResponder`), so a program
    // cannot tell whether the app is there
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let p0 = params.first().and_then(|p| p.first()).copied();
        match (i1, c, p0) {
            (None, 'n', Some(6)) => {
                let (row, col) = screen.cursor_position();
                self.reply(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            (None, 'n', Some(5)) => self.reply(b"\x1b[0n"),
            (None, 'c', _) => self.reply(b"\x1b[?6c"),
            (Some(b'>'), 'c', _) => self.reply(b"\x1b[>0;0;0c"),
            (Some(b'>'), 'u', _) => {
                if self.keyboard.len() < crate::tab::KEYBOARD_STACK_MAX {
                    self.keyboard.push(crate::tab::supported_keyboard_flags(p0.unwrap_or(0)));
                }
            }
            (Some(b'<'), 'u', _) => {
                let n = usize::from(p0.unwrap_or(1).max(1));
                let keep = self.keyboard.len().saturating_sub(n);
                self.keyboard.truncate(keep);
            }
            (Some(b'='), 'u', _) => {
                let flags = crate::tab::supported_keyboard_flags(p0.unwrap_or(0));
                match self.keyboard.last_mut() {
                    Some(top) => *top = flags,
                    None => self.keyboard.push(flags),
                }
            }
            (Some(b'?'), 'u', _) => {
                let flags = self.keyboard.last().copied().unwrap_or(0);
                self.reply(format!("\x1b[?{flags}u").as_bytes());
            }
            _ => {}
        }
    }
}

/// The terminal's picture and where its output stands, under one lock
struct Seen {
    parser: vt100::Parser<Held>,
    edge: crate::termstate::Boundary,
    /// How many bytes of output there have been
    seq: u64,
    /// Who owns it now: (the line, the owner number)
    owner: Option<(u64, u64)>,
    /// When its last owner went, while nobody owns it
    left: Option<Instant>,
    /// What it does while nobody owns it
    away: Away,
}

/// One terminal held here
struct Term {
    /// Which tab it is for, as the app said when it opened it: asked about by
    /// another tab, it is not that tab's
    tab: String,
    /// The folder it was opened in, as the app asked: with the tab, what an
    /// app that lost its note of it finds it again by (far-keep plan §7.4)
    cwd: String,
    seen: Mutex<Seen>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// The pseudo terminal. Let go of once the program ended on Windows, where
    /// a pseudo console's output stays open after its program is gone until
    /// the console itself is closed -- the reader would wait on it forever
    master: Mutex<Option<Box<dyn portable_pty::MasterPty + Send>>>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    /// Everything the program started, ended with it (`crate::job`): a tab's
    /// AI started through a `.cmd` is a cmd.exe holding a node, and ending the
    /// one leaves the other. Closed when the terminal goes, and when this
    /// process does, crash or not
    #[cfg(windows)]
    _job: Option<crate::job::Job>,
    /// The terminal's one queue out: what goes to the app, in the order put
    queue: Sender<(u64, Frame)>,
    /// How many bytes of output are in the queue, not yet sent
    queued: Arc<AtomicU64>,
    next_owner: AtomicU64,
    /// The program's exit code, once it ended, and when
    ended: Mutex<Option<i32>>,
    ended_at: Mutex<Option<Instant>>,
    /// When it was opened
    since: Instant,
    /// The session its shell leads (the shell's process id)
    session: Option<u32>,
}

/// The file the terminals held are written down in, in the bridge's folder:
/// which generation, which id, which tab, and the session its program runs
/// in. Read by the next resident process, so that one asked about a
/// terminal of the one before can tell whether its program still runs
const HELD_FILE: &str = "held.json";

/// A terminal of an earlier resident process, as it wrote it down
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Before {
    #[serde(rename = "gen")]
    generation: String,
    term: u64,
    tab: String,
    /// The session its shell leads: every process of the program in it
    session: u32,
}

/// Whether the program a terminal started still runs. On Windows every
/// process a terminal started is in its job, closed when the resident process
/// that held it ended -- so what is left to ask is whether its first process
/// is still there (the process id is the one written down)
#[cfg(windows)]
fn session_runs(session: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: opened only to read whether it ended, and closed below
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, session);
        if p.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(p, &mut code) != 0;
        CloseHandle(p);
        !ok || code == STILL_ACTIVE as u32
    }
}

/// Whether any process of a session still runs: the shell leads it, and
/// the AI in it, and whatever that started, are in it unless they left it
#[cfg(unix)]
fn session_runs(session: u32) -> bool {
    let Ok(dir) = std::fs::read_dir("/proc") else { return true };
    dir.flatten().any(|e| {
        e.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
            && std::fs::read_to_string(e.path().join("stat")).ok().is_some_and(|stat| {
                // pid (comm) state ppid pgrp session ...: after the last ')'
                stat.rsplit_once(')')
                    .and_then(|(_, rest)| rest.split_whitespace().nth(3)?.parse::<u32>().ok())
                    == Some(session)
            })
    })
}

/// What is answered about a terminal of an earlier resident process: ended
/// when nothing of its session runs, not known while something does
fn answer_before(before: &[Before], id: u64, generation: &str, tab: &str, runs: impl Fn(u32) -> bool) -> Option<Value> {
    let b = before.iter().find(|b| b.generation == generation && b.term == id && b.tab == tab)?;
    Some(if runs(b.session) {
        json!({ "did": "unknown", "term": id, "why": "another generation of the resident process, and its program still runs" })
    } else {
        json!({ "did": "over", "term": id, "code": -1, "why": "its resident process ended, and it with it" })
    })
}

/// The job
pub struct Terms {
    /// This resident process's generation: an id asked about under another
    /// was not given by this one
    generation: String,
    terms: Mutex<HashMap<u64, Arc<Term>>>,
    next: AtomicU64,
    /// The terminals the resident process before this one wrote down
    before: Vec<Before>,
    /// How many of its terminals had ended when they were last written down
    ended_written: AtomicU64,
}

impl Terms {
    pub fn new() -> Self {
        let before = crate::farops::home()
            .ok()
            .and_then(|h| std::fs::read_to_string(h.join(HELD_FILE)).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Self {
            generation: crate::random_hex(8),
            terms: Mutex::default(),
            next: AtomicU64::new(0),
            before,
            ended_written: AtomicU64::new(0),
        }
    }

    /// Write down the terminals held now, for the resident process after this
    /// one. The ones the one before wrote down stay until their programs end,
    /// so a resident process started twice in a row still knows them
    fn write_held(&self) {
        let Ok(home) = crate::farops::home() else { return };
        // One writer at a time, from the list as it is when its turn comes,
        // and the file replaced whole: the next resident process reads either
        // the list before or the list after, never an older one over a newer
        static WRITING: Mutex<()> = Mutex::new(());
        let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        let mut all: Vec<Before> = self.before.iter().filter(|b| session_runs(b.session)).cloned().collect();
        if let Ok(t) = self.terms.lock() {
            for (id, term) in t.iter() {
                if let Some(session) = term.session
                    && term.ended.lock().is_ok_and(|e| e.is_none())
                {
                    all.push(Before { generation: self.generation.clone(), term: *id, tab: term.tab.clone(), session });
                }
            }
        }
        let text = serde_json::to_string(&all).unwrap_or_default();
        if let Err(e) = crate::fardaemon::write_private(&home.join(HELD_FILE), &text) {
            crate::fardaemon::log(&format!("the terminals held could not be written down: {e:#}"));
        }
    }

    /// A terminal of an earlier resident process, asked about: ended when
    /// nothing of its session runs any more (the resident process ended and
    /// took it with it), else not known -- it may still run, out of reach
    fn of_before(&self, id: u64, generation: &str, tab: &str) -> Option<Value> {
        answer_before(&self.before, id, generation, tab, session_runs)
    }

    fn say(core: &Arc<Core>, line: u64, m: Value) {
        core.say(line, &Frame::Job { job: NAME.into(), m });
    }

    /// Open a terminal and attach to it. "opened" goes through the
    /// terminal's queue before its state does, so the app knows the id first
    fn open(&self, core: &Arc<Core>, line: u64, m: &Value) -> Option<Value> {
        match self.start(core, m) {
            Ok(id) => {
                self.write_held();
                let term = self.terms.lock().ok().and_then(|t| t.get(&id).cloned())?;
                let opened = json!({ "did": "opened", "ref": m["ref"], "gen": self.generation, "term": id });
                let _ = term.queue.send((line, Frame::Job { job: NAME.into(), m: opened }));
                let refused = self.attach(core, line, &json!({ "term": id, "gen": self.generation, "tab": m["tab"],
                    "rows": m["rows"], "cols": m["cols"], "away": m["away"] }));
                (refused["did"] != "attaching").then_some(refused)
            }
            Err(e) => Some(json!({ "did": "failed", "ref": m["ref"], "why": format!("{e:#}") })),
        }
    }

    /// A shell in `cwd`, with `env` in its environment, and `then` typed into
    /// it once it is up -- the way a tab over SSH is started
    fn start(&self, core: &Arc<Core>, m: &Value) -> anyhow::Result<u64> {
        let rows = m["rows"].as_u64().and_then(|v| u16::try_from(v).ok()).filter(|v| *v > 0).unwrap_or(24);
        let cols = m["cols"].as_u64().and_then(|v| u16::try_from(v).ok()).filter(|v| *v > 0).unwrap_or(80);
        let pty = portable_pty::native_pty_system().openpty(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        let mut cmd = command_of(m)?;
        if let Some(cwd) = m["cwd"].as_str().filter(|c| !c.is_empty()) {
            cmd.cwd(cwd);
        } else if let Ok(home) = std::env::var("HOME") {
            cmd.cwd(home);
        }
        if let Some(env) = m["env"].as_object() {
            for (k, v) in env {
                if let Some(v) = v.as_str() {
                    cmd.env(k, v);
                }
            }
        }
        // The `shikisha` command in it runs this build, whatever build was
        // put on the machine after (far-keep plan §4.5)
        if let Ok(exe) = std::env::current_exe() {
            cmd.env(crate::farlink::ENV_PROGRAM, exe);
        }
        #[cfg_attr(unix, allow(unused_mut))]
        let mut child = pty.slave.spawn_command(cmd)?;
        let session = child.process_id();
        drop(pty.slave);
        #[cfg(windows)]
        let job = crate::job::Job::new().filter(|j| session.is_some_and(|p| j.take(p)));
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pty.master.take_writer()?));
        let mut reader = pty.master.try_clone_reader()?;
        let held = Held { writer: Arc::clone(&writer), answering: true, keyboard: Vec::new(), title: String::new(), cwd: String::new() };
        let (queue, out) = channel::<(u64, Frame)>();
        let queued = Arc::new(AtomicU64::new(0));
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let term = Arc::new(Term {
            tab: m["tab"].as_str().unwrap_or_default().to_string(),
            cwd: m["cwd"].as_str().unwrap_or_default().to_string(),
            seen: Mutex::new(Seen {
                parser: vt100::Parser::new_with_callbacks(rows, cols, SCROLLBACK_KEPT, held),
                edge: crate::termstate::Boundary::default(),
                seq: 0,
                owner: None,
                left: None,
                away: Away::read(&m["away"]).unwrap_or(Away::Stop),
            }),
            writer,
            master: Mutex::new(Some(pty.master)),
            killer: Mutex::new(child.clone_killer()),
            #[cfg(windows)]
            _job: job,
            queue,
            queued: Arc::clone(&queued),
            next_owner: AtomicU64::new(0),
            ended: Mutex::new(None),
            ended_at: Mutex::new(None),
            since: Instant::now(),
            session,
        });
        if let Ok(mut t) = self.terms.lock() {
            t.insert(id, Arc::clone(&term));
        }
        // The one sender: what is queued goes out in order, and never under
        // the terminal's lock
        {
            let core = Arc::clone(core);
            std::thread::spawn(move || {
                for (line, frame) in out {
                    if let Frame::Job { m, .. } = &frame
                        && m["did"] == "out"
                    {
                        let n = m["b"].as_str().map_or(0, |b| b.len() as u64);
                        queued.fetch_sub(n.min(queued.load(Ordering::SeqCst)), Ordering::SeqCst);
                    }
                    core.say(line, &frame);
                }
            });
        }
        // The reader: the output is looked at, taken in, and handed to the
        // owner at the time it was taken in -- decided under the lock, so an
        // attach either sees it in the state or gets it after, never both
        {
            let term = Arc::clone(&term);
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let bytes = &buf[..n];
                    let Ok(mut seen) = term.seen.lock() else { break };
                    let whole = seen.edge.feed(bytes);
                    seen.parser.process(&whole);
                    seen.seq += n as u64;
                    if let Some((line, _)) = seen.owner {
                        let b = b64(bytes);
                        if term.queued.fetch_add(b.len() as u64, Ordering::SeqCst) > QUEUED_MOST {
                            // Not taken for too long: let go of, and answering
                            // the program here until it attaches again
                            seen.owner = None;
                            seen.left = Some(Instant::now());
                            seen.parser.callbacks_mut().answering = true;
                            let m = json!({ "did": "taken", "term": id, "why": "the line did not keep up" });
                            let _ = term.queue.send((line, Frame::Job { job: NAME.into(), m }));
                        } else {
                            let m = json!({ "did": "out", "term": id, "seq": seen.seq, "b": b });
                            let _ = term.queue.send((line, Frame::Job { job: NAME.into(), m }));
                        }
                    }
                }
                // The program ended: its code is kept until the app has it
                #[cfg(unix)]
                ended(&term, id, child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1));
            });
        }
        // On Windows the output does not end when the program does: the end
        // is the program's, waited on here. What it wrote last is read before
        // the end is said (the pseudo console is given a moment to hand it
        // over), and then the console is closed, which ends the reader
        #[cfg(windows)]
        {
            let term = Arc::clone(&term);
            std::thread::spawn(move || {
                let code = child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1);
                std::thread::sleep(LAST_WORDS);
                ended(&term, id, code);
                if let Ok(mut m) = term.master.lock() {
                    m.take();
                }
            });
        }
        if let Some(then) = m["then"].as_str().filter(|t| !t.is_empty())
            && let Ok(mut w) = term.writer.lock()
        {
            let _ = w.write_all(format!("{then}\n").as_bytes());
            let _ = w.flush();
        }
        Ok(id)
    }

    /// Take a terminal, and be handed its state
    fn attach(&self, _core: &Arc<Core>, line: u64, m: &Value) -> Value {
        let id = m["term"].as_u64().unwrap_or(0);
        let unknown = |why: &str| json!({ "did": "unknown", "term": id, "why": why });
        if m["gen"].as_str() != Some(self.generation.as_str()) {
            let (generation, tab) = (m["gen"].as_str().unwrap_or_default(), m["tab"].as_str().unwrap_or_default());
            return self.of_before(id, generation, tab).unwrap_or_else(|| unknown("another generation of the resident process"));
        }
        let Some(term) = self.terms.lock().ok().and_then(|t| t.get(&id).cloned()) else {
            return unknown("no such terminal here");
        };
        if m["tab"].as_str().unwrap_or_default() != term.tab {
            return unknown("another tab's terminal");
        }
        if let Some(code) = term.ended.lock().ok().and_then(|e| *e) {
            return json!({ "did": "over", "term": id, "code": code });
        }
        // The size first, so the state is taken at the size it is sent with
        let rows = m["rows"].as_u64().and_then(|v| u16::try_from(v).ok()).filter(|v| *v > 0);
        let cols = m["cols"].as_u64().and_then(|v| u16::try_from(v).ok()).filter(|v| *v > 0);
        let owner = term.next_owner.fetch_add(1, Ordering::SeqCst) + 1;
        let Ok(mut seen) = term.seen.lock() else { return unknown("the terminal could not be read") };
        if let (Some(rows), Some(cols)) = (rows, cols) {
            if let Ok(master) = term.master.lock()
                && let Some(master) = master.as_ref()
            {
                let _ = master.resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
            }
            seen.parser.screen_mut().set_size(rows, cols);
        }
        let before = seen.owner.replace((line, owner));
        seen.left = None;
        // The one attaching says what it is to do while nobody owns it
        if let Some(away) = Away::read(&m["away"]) {
            seen.away = away;
        }
        seen.parser.callbacks_mut().answering = false;
        let held = seen.parser.callbacks();
        let state = json!({
            "did": "attached",
            "term": id,
            "owner": owner,
            "seq": seen.seq,
            "state": b64(&seen.parser.screen().snapshot(SCROLLBACK_SENT)),
            "pending": b64(seen.edge.pending()),
            "keyboard": held.keyboard,
            "title": held.title,
            "cwd": held.cwd,
        });
        // Queued while the lock is held: nothing of the output after this
        // point can go out before it
        let _ = term.queue.send((line, Frame::Job { job: NAME.into(), m: state }));
        if let Some((was, _)) = before.filter(|(was, _)| *was != line) {
            let _ = term.queue.send((was, Frame::Job { job: NAME.into(), m: json!({ "did": "taken", "term": id }) }));
        }
        json!({ "did": "attaching", "term": id, "owner": owner })
    }

    /// Every terminal held here: which tab and folder it is for, whether an
    /// app owns it now, and whether it ended
    fn list(&self, m: &Value) -> Value {
        let terms: Vec<Value> = self
            .terms
            .lock()
            .map(|t| {
                t.iter()
                    .map(|(id, term)| {
                        let seen = term.seen.lock().ok().map(|s| (s.owner.is_some(), s.away, s.left));
                        json!({
                            "term": id,
                            "tab": term.tab,
                            "cwd": term.cwd,
                            "owned": seen.is_some_and(|(owned, _, _)| owned),
                            "away": seen.map_or(Away::Stop, |(_, a, _)| a).write(),
                            "ended": term.ended.lock().is_ok_and(|e| e.is_some()),
                            // How long ago it was opened, and its last owner went
                            "for": term.since.elapsed().as_secs(),
                            "left": seen.and_then(|(_, _, l)| l).map(|l| l.elapsed().as_secs()),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({ "did": "list", "ref": m["ref"], "gen": self.generation, "terms": terms })
    }

    /// The terminal, when `m` comes from its owner now; otherwise the refusal
    fn owned(&self, line: u64, m: &Value) -> Result<Arc<Term>, Value> {
        let id = m["term"].as_u64().unwrap_or(0);
        let term = self
            .terms
            .lock()
            .ok()
            .and_then(|t| t.get(&id).cloned())
            .ok_or_else(|| json!({ "did": "unknown", "term": id, "why": "no such terminal here" }))?;
        let owner = term.seen.lock().ok().and_then(|s| s.owner);
        match owner {
            Some((l, n)) if l == line && Some(n) == m["owner"].as_u64() => Ok(term),
            _ => Err(json!({ "did": "refused", "term": id, "why": "the terminal was taken from somewhere else" })),
        }
    }
}

impl Default for Terms {
    fn default() -> Self {
        Self::new()
    }
}

impl Job for Terms {
    fn name(&self) -> &'static str {
        NAME
    }

    fn frame(&self, core: &Arc<Core>, line: u64, frame: &Frame) -> bool {
        let Frame::Job { job, m } = frame else { return false };
        if job != NAME {
            return false;
        }
        let answer = match m["do"].as_str().unwrap_or_default() {
            "open" => self.open(core, line, m),
            // Attached: the state goes through the terminal's queue
            "attach" => Some(self.attach(core, line, m)).filter(|a| a["did"] != "attaching"),
            "in" => match self.owned(line, m) {
                Ok(term) => {
                    if let Ok(mut w) = term.writer.lock() {
                        let _ = w.write_all(&unb64(&m["b"]));
                        let _ = w.flush();
                    }
                    None
                }
                Err(no) => Some(no),
            },
            "size" => match self.owned(line, m) {
                Ok(term) => {
                    let rows = m["rows"].as_u64().and_then(|v| u16::try_from(v).ok()).filter(|v| *v > 0).unwrap_or(24);
                    let cols = m["cols"].as_u64().and_then(|v| u16::try_from(v).ok()).filter(|v| *v > 0).unwrap_or(80);
                    if let Ok(master) = term.master.lock()
                        && let Some(master) = master.as_ref()
                    {
                        let _ = master.resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                    }
                    if let Ok(mut seen) = term.seen.lock() {
                        seen.parser.screen_mut().set_size(rows, cols);
                    }
                    None
                }
                Err(no) => Some(no),
            },
            "stop" => match self.owned(line, m) {
                Ok(term) => {
                    if let Ok(mut k) = term.killer.lock() {
                        let _ = k.kill();
                    }
                    None
                }
                Err(no) => Some(no),
            },
            // Every terminal held here, for an app that lost its note of
            // which were its (far-keep plan §7.4), and for the person's list
            // of what runs while the app is away (§7.6)
            "list" => Some(self.list(m)),
            // The person stops one from that list (§7.6): whoever owns it, if
            // anybody. A terminal of another generation is not this one
            "end" => {
                let id = m["term"].as_u64().unwrap_or(0);
                let term = (m["gen"].as_str() == Some(self.generation.as_str()))
                    .then(|| self.terms.lock().ok().and_then(|t| t.get(&id).cloned()))
                    .flatten();
                Some(match term {
                    Some(term) => {
                        crate::fardaemon::log(&format!("terminal {id}: stopped by the person"));
                        if let Ok(mut k) = term.killer.lock() {
                            let _ = k.kill();
                        }
                        json!({ "did": "ending", "ref": m["ref"], "term": id })
                    }
                    None => json!({ "did": "unknown", "ref": m["ref"], "term": id, "why": "no such terminal here" }),
                })
            }
            // What it does while nobody owns it, changed by its owner: the
            // person changed the machine's setting while the app runs (§4.3)
            "set_away" => match self.owned(line, m) {
                Ok(term) => {
                    if let (Some(away), Ok(mut seen)) = (Away::read(&m["away"]), term.seen.lock()) {
                        seen.away = away;
                    }
                    None
                }
                Err(no) => Some(no),
            },
            // Every one, as the bridge is taken off the machine (§7.7)
            "end_all" => {
                self.end();
                Some(json!({ "did": "ending", "ref": m["ref"] }))
            }
            // The app has the code of an ended terminal: nothing is kept
            "forget" => {
                let id = m["term"].as_u64().unwrap_or(0);
                if let Ok(mut t) = self.terms.lock()
                    && t.get(&id).is_some_and(|term| term.ended.lock().is_ok_and(|e| e.is_some()))
                {
                    t.remove(&id);
                }
                None
            }
            other => Some(json!({ "did": "failed", "ref": m["ref"], "why": format!("the terminals job has nothing called {other}") })),
        };
        if let Some(a) = answer {
            Self::say(core, line, a);
        }
        true
    }

    /// The app on `line` went: the terminals it owned have no owner, and
    /// this job answers their programs' questions until one attaches again
    fn line_gone(&self, _core: &Arc<Core>, line: u64) {
        let terms: Vec<Arc<Term>> = self.terms.lock().map(|t| t.values().cloned().collect()).unwrap_or_default();
        for term in terms {
            if let Ok(mut seen) = term.seen.lock()
                && seen.owner.is_some_and(|(l, _)| l == line)
            {
                seen.owner = None;
                seen.left = Some(Instant::now());
                seen.parser.callbacks_mut().answering = true;
            }
        }
    }

    /// Each terminal nobody owns, ended once it has been left for as long as
    /// it was to be kept; and the code of one that ended, let go of once it
    /// has been kept long enough for its app to come back for it
    fn tick(&self, _core: &Arc<Core>) {
        let terms: Vec<(u64, Arc<Term>)> = self.terms.lock().map(|t| t.iter().map(|(i, t)| (*i, Arc::clone(t))).collect()).unwrap_or_default();
        // One that ended since they were written down is struck out there
        let ended = terms.iter().filter(|(_, t)| t.ended.lock().is_ok_and(|e| e.is_some())).count() as u64;
        if self.ended_written.swap(ended, Ordering::SeqCst) != ended {
            self.write_held();
        }
        for (id, term) in terms {
            if let Some(at) = term.ended_at.lock().ok().and_then(|e| *e) {
                if at.elapsed() >= ENDED_KEPT
                    && let Ok(mut t) = self.terms.lock()
                {
                    t.remove(&id);
                }
                continue;
            }
            // Decided and done under the terminal's lock, which an attach
            // takes too: one that comes back at the last moment either finds
            // it still there and keeps it, or finds it ending -- never attached
            // and then ended under it
            let Ok(seen) = term.seen.lock() else { continue };
            let due = seen.owner.is_none()
                && seen.left.zip(seen.away.ends_after()).is_some_and(|(left, after)| left.elapsed() >= after);
            if due {
                crate::fardaemon::log(&format!("terminal {id}: left alone past what it was to be kept for ({:?}); ending it", seen.away));
                if let Ok(mut k) = term.killer.lock() {
                    let _ = k.kill();
                }
            }
            drop(seen);
        }
    }

    /// While a terminal runs, and while the code of one that was to be kept
    /// running waits for its app to come back for it
    fn wants_to_stay(&self) -> bool {
        self.terms.lock().is_ok_and(|t| {
            t.values().any(|term| match term.ended_at.lock().ok().and_then(|e| *e) {
                None => true,
                Some(at) => term.seen.lock().is_ok_and(|s| s.away != Away::Stop) && at.elapsed() < ENDED_KEPT,
            })
        })
    }

    /// Ending: every terminal's program with it (far-keep plan §4.3). The
    /// resident process ends only once no terminal is to be kept, so what
    /// is here then is left by a resident process told to go
    fn end(&self) {
        let terms: Vec<Arc<Term>> = self.terms.lock().map(|t| t.values().cloned().collect()).unwrap_or_default();
        for term in terms {
            if let Ok(mut k) = term.killer.lock() {
                let _ = k.kill();
            }
        }
    }
}

/// How long a pseudo console on Windows is given, after its program ended,
/// to hand over what the program wrote last. Measured: ConPTY passes the last
/// screen on within a frame or two of the program going (tens of
/// milliseconds); a quarter of a second is several of those, and short
/// enough that nobody waits on it
#[cfg(windows)]
const LAST_WORDS: Duration = Duration::from_millis(250);

/// The program ended: its code is kept until the app has it, and the app
/// owning the terminal is told -- after the output, through the same queue
fn ended(term: &Term, id: u64, code: i32) {
    if let Ok(mut e) = term.ended.lock() {
        *e = Some(code);
    }
    if let Ok(mut e) = term.ended_at.lock() {
        *e = Some(Instant::now());
    }
    let owner = term.seen.lock().ok().and_then(|s| s.owner);
    if let Some((line, _)) = owner {
        let m = json!({ "did": "ended", "term": id, "code": code });
        let _ = term.queue.send((line, Frame::Job { job: NAME.into(), m }));
    }
}

/// What a terminal runs. On this PC (local-keeper plan §5) the app says it in
/// full -- the program and its arguments (`argv`), and the whole environment
/// it built for the tab (`env_all`) -- since the app is the one place a tab's
/// command is put together, and the resident process was started with
/// whatever environment the app had at the time. Elsewhere it is the
/// account's login shell, as over SSH, and what is to run is typed into it
fn command_of(m: &Value) -> anyhow::Result<portable_pty::CommandBuilder> {
    let argv: Vec<std::ffi::OsString> = m["argv"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(std::ffi::OsString::from)).collect())
        .unwrap_or_default();
    let mut cmd = if argv.is_empty() {
        #[cfg(unix)]
        {
            let mut c = portable_pty::CommandBuilder::new(login_shell());
            c.arg("-l");
            c.env("TERM", "xterm-256color");
            c
        }
        #[cfg(windows)]
        anyhow::bail!("nothing to run: this PC's terminals are given their command");
    } else {
        portable_pty::CommandBuilder::from_argv(argv)
    };
    if let Some(all) = m["env_all"].as_object() {
        cmd.env_clear();
        for (k, v) in all {
            if let Some(v) = v.as_str() {
                cmd.env(k, v);
            }
        }
    }
    Ok(cmd)
}

/// The account's own shell: what its entry in the password database names,
/// as a login over SSH would start. The resident process was started without
/// a login, and has no SHELL of its own to go by
#[cfg(unix)]
fn login_shell() -> String {
    if let Some(s) = std::env::var("SHELL").ok().filter(|s| !s.is_empty()) {
        return s;
    }
    // SAFETY: getpwuid returns a pointer into static storage, read here at
    // once and copied before anything else could call it
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_shell.is_null() {
            let s = std::ffi::CStr::from_ptr((*pw).pw_shell).to_string_lossy().into_owned();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "/bin/sh".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asked about a terminal of an earlier resident process, the answer is
    /// "ended" only when nothing of its session runs any more: a resident
    /// process that went may have left its program running, out of reach,
    /// and a second AI is not started beside it (far-keep plan §4.4)
    #[test]
    fn a_terminal_of_an_earlier_resident_process_is_ended_only_once_nothing_of_it_runs() {
        let before = vec![Before { generation: "g1".into(), term: 4, tab: "t".into(), session: 77 }];
        assert_eq!(answer_before(&before, 4, "g1", "t", |_| false).unwrap()["did"], "over");
        assert_eq!(answer_before(&before, 4, "g1", "t", |_| true).unwrap()["did"], "unknown");
        assert!(answer_before(&before, 4, "g1", "other tab", |_| false).is_none(), "another tab's");
        assert!(answer_before(&before, 5, "g1", "t", |_| false).is_none(), "an id it never had");
    }

    /// A session runs while any of its processes does: this test's own does,
    /// one whose only process ended does not
    #[cfg(unix)]
    #[test]
    fn a_session_runs_while_any_of_its_processes_does() {
        // SAFETY: getsid only reads
        let mine = unsafe { libc::getsid(0) } as u32;
        assert!(session_runs(mine));
        let mut child = std::process::Command::new("setsid").arg("true").spawn().unwrap();
        let gone = child.id();
        child.wait().unwrap();
        assert!(!session_runs(gone), "a session nothing runs in");
    }
}

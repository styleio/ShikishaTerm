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

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

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
}

/// One terminal held here
struct Term {
    /// Which tab it is for, as the app said when it opened it: asked about by
    /// another tab, it is not that tab's
    tab: String,
    seen: Mutex<Seen>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Mutex<Box<dyn portable_pty::MasterPty + Send>>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    /// The terminal's one queue out: what goes to the app, in the order put
    queue: Sender<(u64, Frame)>,
    /// How many bytes of output are in the queue, not yet sent
    queued: Arc<AtomicU64>,
    next_owner: AtomicU64,
    /// The program's exit code, once it ended
    ended: Mutex<Option<i32>>,
}

/// The job
pub struct Terms {
    /// This resident process's generation: an id asked about under another
    /// was not given by this one
    generation: String,
    terms: Mutex<HashMap<u64, Arc<Term>>>,
    next: AtomicU64,
}

impl Terms {
    pub fn new() -> Self {
        Self { generation: crate::random_hex(8), terms: Mutex::default(), next: AtomicU64::new(0) }
    }

    fn say(core: &Arc<Core>, line: u64, m: Value) {
        core.say(line, &Frame::Job { job: NAME.into(), m });
    }

    /// Open a terminal and attach to it. "opened" goes through the
    /// terminal's queue before its state does, so the app knows the id first
    fn open(&self, core: &Arc<Core>, line: u64, m: &Value) -> Option<Value> {
        match self.start(core, m) {
            Ok(id) => {
                let term = self.terms.lock().ok().and_then(|t| t.get(&id).cloned())?;
                let opened = json!({ "did": "opened", "ref": m["ref"], "gen": self.generation, "term": id });
                let _ = term.queue.send((line, Frame::Job { job: NAME.into(), m: opened }));
                let refused = self.attach(core, line, &json!({ "term": id, "gen": self.generation, "tab": m["tab"],
                    "rows": m["rows"], "cols": m["cols"] }));
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
        let shell = login_shell();
        let mut cmd = portable_pty::CommandBuilder::new(&shell);
        cmd.arg("-l");
        cmd.env("TERM", "xterm-256color");
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
        let mut child = pty.slave.spawn_command(cmd)?;
        drop(pty.slave);
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pty.master.take_writer()?));
        let mut reader = pty.master.try_clone_reader()?;
        let held = Held { writer: Arc::clone(&writer), answering: true, keyboard: Vec::new(), title: String::new(), cwd: String::new() };
        let (queue, out) = channel::<(u64, Frame)>();
        let queued = Arc::new(AtomicU64::new(0));
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let term = Arc::new(Term {
            tab: m["tab"].as_str().unwrap_or_default().to_string(),
            seen: Mutex::new(Seen {
                parser: vt100::Parser::new_with_callbacks(rows, cols, SCROLLBACK_KEPT, held),
                edge: crate::termstate::Boundary::default(),
                seq: 0,
                owner: None,
            }),
            writer,
            master: Mutex::new(pty.master),
            killer: Mutex::new(child.clone_killer()),
            queue,
            queued: Arc::clone(&queued),
            next_owner: AtomicU64::new(0),
            ended: Mutex::new(None),
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
                let code = child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1);
                if let Ok(mut e) = term.ended.lock() {
                    *e = Some(code);
                }
                let owner = term.seen.lock().ok().and_then(|s| s.owner);
                if let Some((line, _)) = owner {
                    let m = json!({ "did": "ended", "term": id, "code": code });
                    let _ = term.queue.send((line, Frame::Job { job: NAME.into(), m }));
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
            return unknown("another generation of the resident process");
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
            if let Ok(master) = term.master.lock() {
                let _ = master.resize(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
            }
            seen.parser.screen_mut().set_size(rows, cols);
        }
        let before = seen.owner.replace((line, owner));
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
                    if let Ok(master) = term.master.lock() {
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
                seen.parser.callbacks_mut().answering = true;
            }
        }
    }

    /// Ending: every terminal's program with it (far-keep plan §4.3). Until
    /// the away mode is chosen (stage 6), nothing is kept once no app is here
    fn end(&self) {
        let terms: Vec<Arc<Term>> = self.terms.lock().map(|t| t.values().cloned().collect()).unwrap_or_default();
        for term in terms {
            if let Ok(mut k) = term.killer.lock() {
                let _ = k.kill();
            }
        }
    }
}

/// The account's own shell: what its entry in the password database names,
/// as a login over SSH would start. The resident process was started without
/// a login, and has no SHELL of its own to go by
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

//! A tab's terminal held by the bridge on its machine, reached from here
//! (far-keep plan §4.4): this app's side of the bridge's `terms` job
//! (`farterms`).
//!
//! To the tab it is a terminal like any other -- a [`portable_pty::MasterPty`]
//! to read from, write to and size, and a killer -- so nothing above it
//! changes. What is different is underneath: the terminal lives in the
//! bridge's resident process, and this end is attached to it. When it is
//! attached again (the line came back), the bridge hands over the
//! terminal's state, which is put into the tab's own parser in place of what
//! it held, in the reading of the output, so that nothing read before it is
//! put after it and nothing already in it is read twice.
//!
//! Used only for a machine whose terminals are held (the away mode, far-keep
//! plan §4.3). Until that can be chosen (stage 6), only a check that sets
//! `SHIKISHA_HOLD_TERMINALS=1` uses it; every tab of everybody else is
//! opened as before.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use base64::Engine as _;
use serde_json::{Value, json};

/// The job's name there
const JOB: &str = "terms";
/// How long a terminal may take to open there
const OPEN_WAIT: Duration = Duration::from_secs(30);
/// How long a terminal whose line went waits for it to come back before the
/// tab is told it ended. The line is made again by the app on its own
/// (`farlink::Keeper`), a few times a minute
const LINE_BACK_WAIT: Duration = Duration::from_secs(10 * 60);

/// The development switch this is behind until the away mode can be chosen
pub fn wanted() -> bool {
    std::env::var("SHIKISHA_HOLD_TERMINALS").is_ok_and(|v| v == "1")
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(v: &Value) -> Vec<u8> {
    v.as_str()
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .unwrap_or_default()
}

/// Every message of the terms job from one machine, handed to the terminal
/// it is about (by its id, or by the reference an open was asked with)
#[derive(Default)]
struct Router {
    by_term: HashMap<u64, Sender<Value>>,
    by_ref: HashMap<u64, Sender<Value>>,
}

static ROUTERS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Router>>>>> = OnceLock::new();
static NEXT_REF: AtomicU64 = AtomicU64::new(0);

/// The router for a machine's line, started the first time it is needed
fn router(at: &crate::elsewhere::Elsewhere, link: &crate::farlink::Link) -> Arc<Mutex<Router>> {
    let mut all = ROUTERS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let key = at.machine_key();
    if let Some(r) = all.get(&key) {
        return Arc::clone(r);
    }
    let r = Arc::new(Mutex::new(Router::default()));
    let from = link.listen_job(JOB);
    let (r2, key2) = (Arc::clone(&r), key.clone());
    std::thread::spawn(move || {
        for m in from {
            let mut rt = r2.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(reference) = m["ref"].as_u64()
                && let Some(tx) = rt.by_ref.get(&reference)
            {
                // Its id is known from now on
                if let Some(id) = m["term"].as_u64() {
                    let tx = tx.clone();
                    rt.by_term.insert(id, tx);
                }
                let _ = rt.by_ref.get(&reference).map(|tx| tx.send(m.clone()));
                if m["did"] != "failed" {
                    rt.by_ref.remove(&reference);
                }
                continue;
            }
            if let Some(id) = m["term"].as_u64()
                && let Some(tx) = rt.by_term.get(&id)
                && tx.send(m).is_err()
            {
                rt.by_term.remove(&id);
            }
        }
        // The line ended: every terminal on it is told by its channel closing
        if let Ok(mut all) = ROUTERS.get_or_init(Default::default).lock() {
            all.remove(&key2);
        }
    });
    all.insert(key, Arc::clone(&r));
    r
}

/// Who a held terminal is: what it takes to ask about it again (far-keep
/// plan §4.4, the three points of identity), and what this end holds of it
pub struct FarTerm {
    pub term: u64,
    pub generation: String,
    pub tab: String,
    owner: AtomicU64,
    /// Where it is, and the line to it now: a line that went and came back
    /// is another line
    at: crate::elsewhere::Elsewhere,
    link: Mutex<Arc<crate::farlink::Link>>,
    size: Mutex<(u16, u16)>,
    /// The tab's own parser and what beside it a program's asks are kept in,
    /// so a state handed over goes where the tab reads from
    bound: Mutex<Option<Bound>>,
    ended: AtomicBool,
    /// On a MicroVM, counted as a terminal of this app open there for as
    /// long as it is held: the machine is in use
    open_there: Mutex<Option<crate::e2b::Opened>>,
}

struct Bound {
    parser: crate::tab::SharedParser,
    keyboard: crate::tab::KeyboardMode,
    title: crate::tab::WindowTitle,
    cwd: crate::tab::ReportedCwd,
}

impl FarTerm {
    /// The tab's parser, once it has one: states handed over go into it
    pub fn bind(
        &self,
        parser: crate::tab::SharedParser,
        keyboard: crate::tab::KeyboardMode,
        title: crate::tab::WindowTitle,
        cwd: crate::tab::ReportedCwd,
    ) {
        if let Ok(mut b) = self.bound.lock() {
            *b = Some(Bound { parser, keyboard, title, cwd });
        }
    }

    fn say(&self, m: Value) -> bool {
        self.link.lock().map(|l| Arc::clone(&l)).is_ok_and(|l| l.to_job(JOB, m))
    }

    /// The line came back: be routed its messages again and attach, which
    /// hands the state over. `None` while the line is not up yet
    fn attach_again(&self) -> Option<Receiver<Value>> {
        let link = crate::farlink::link(&self.at).filter(|l| l.holds(JOB))?;
        let r = router(&self.at, &link);
        let (tx, rx) = channel();
        r.lock().unwrap_or_else(|e| e.into_inner()).by_term.insert(self.term, tx);
        if let Ok(mut l) = self.link.lock() {
            *l = Arc::clone(&link);
        }
        let (rows, cols) = self.size.lock().map(|s| *s).unwrap_or((24, 80));
        let asked = json!({ "do": "attach", "term": self.term, "gen": self.generation, "tab": self.tab,
            "rows": rows, "cols": cols });
        link.to_job(JOB, asked).then_some(rx)
    }

    /// Put a state handed over in place of the tab's own. `false` when it
    /// could not be read -- a format of another version -- and the screen
    /// is left to be drawn again by the program
    fn take_state(&self, m: &Value) -> bool {
        let Some(b) = self
            .bound
            .lock()
            .ok()
            .and_then(|b| b.as_ref().map(|b| (b.parser.clone(), b.keyboard.clone(), b.title.clone(), b.cwd.clone())))
        else {
            // Not bound yet: a fresh terminal, whose state is the empty screen
            return true;
        };
        let screen = match vt100::Screen::from_snapshot(&unb64(&m["state"])) {
            Ok(s) => s,
            Err(e) => {
                crate::append_hook_log(&format!("far terminal {}: its screen could not be taken over: {e}", self.term));
                return false;
            }
        };
        if let Ok(mut p) = b.0.lock() {
            p.restore(screen);
        }
        if let (Ok(mut k), Some(stack)) = (b.1.lock(), m["keyboard"].as_array()) {
            *k = stack.iter().filter_map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok())).collect();
        }
        // What the program called itself, and where it said it stands
        if let (Ok(mut t), Some(title)) = (b.2.lock(), m["title"].as_str().filter(|t| !t.is_empty())) {
            *t = title.to_string();
        }
        if let (Ok(mut c), Some(cwd)) = (b.3.lock(), m["cwd"].as_str().filter(|c| !c.is_empty())) {
            *c = cwd.to_string();
        }
        true
    }
}

/// Open a terminal for `tab` in the bridge's resident process on `at`: a
/// shell in `cwd`, with `then` typed into it, `env` in its environment
#[allow(clippy::type_complexity)]
pub fn open(
    at: &crate::elsewhere::Elsewhere,
    tab: &str,
    rows: u16,
    cols: u16,
    cwd: Option<&str>,
    then: Option<&str>,
) -> Result<(Box<dyn portable_pty::MasterPty + Send>, Box<dyn portable_pty::ChildKiller + Send + Sync>, Arc<FarTerm>)> {
    let link = crate::farlink::link(at).ok_or_else(|| anyhow!("the bridge on {} is not connected", at.address()))?;
    if !link.holds(JOB) {
        bail!("the bridge on {} does not hold terminals (an older version)", at.address());
    }
    let r = router(at, &link);
    let reference = NEXT_REF.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = channel::<Value>();
    r.lock().unwrap_or_else(|e| e.into_inner()).by_ref.insert(reference, tx);
    let asked = json!({ "do": "open", "ref": reference, "tab": tab, "rows": rows, "cols": cols,
        "cwd": cwd.unwrap_or_default(), "then": then.unwrap_or_default() });
    if !link.to_job(JOB, asked) {
        bail!("the bridge on {} could not be asked for a terminal", at.address());
    }
    let opened = rx.recv_timeout(OPEN_WAIT).map_err(|_| anyhow!("the bridge on {} did not open a terminal", at.address()))?;
    if opened["did"] != "opened" {
        bail!("the bridge on {} could not open a terminal: {}", at.address(), opened["why"].as_str().unwrap_or("no reason given"));
    }
    let term = Arc::new(FarTerm {
        term: opened["term"].as_u64().unwrap_or(0),
        generation: opened["gen"].as_str().unwrap_or_default().to_string(),
        tab: tab.to_string(),
        owner: AtomicU64::new(0),
        at: at.clone(),
        link: Mutex::new(link),
        size: Mutex::new((rows, cols)),
        bound: Mutex::new(None),
        ended: AtomicBool::new(false),
        open_there: Mutex::new(match at {
            crate::elsewhere::Elsewhere::Cloud(h) => h.instance.as_deref().map(crate::e2b::Opened::new),
            crate::elsewhere::Elsewhere::Ssh(_) => None,
        }),
    });
    crate::append_hook_log(&format!("far terminal {} opened on {} for {tab}", term.term, at.address()));
    let reader = FarReader { from: rx, term: Arc::clone(&term), seen: 0, rest: Vec::new(), at: 0 };
    let master = FarMaster {
        term: Arc::clone(&term),
        size: Mutex::new(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }),
        reader: Mutex::new(Some(reader)),
        writer_taken: AtomicBool::new(false),
    };
    Ok((Box::new(master), Box::new(FarKiller { term: Arc::clone(&term) }), term))
}

/// What comes from the terminal there, as bytes to read: its output, and
/// any state handed over put into the tab's parser on the way
struct FarReader {
    from: Receiver<Value>,
    term: Arc<FarTerm>,
    /// How far into the output the tab has it: output already in a state
    /// that was taken is not read again
    seen: u64,
    rest: Vec<u8>,
    at: usize,
}

impl std::io::Read for FarReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.at >= self.rest.len() {
            let Ok(m) = self.from.recv() else {
                // The line went. The terminal there may well still be there:
                // wait for the line to come back and attach to it again
                if self.term.ended.load(Ordering::SeqCst) {
                    return Ok(0);
                }
                crate::append_hook_log(&format!("far terminal {}: the line went; waiting for it to come back", self.term.term));
                let until = std::time::Instant::now() + LINE_BACK_WAIT;
                let again = loop {
                    if let Some(rx) = self.term.attach_again() {
                        break Some(rx);
                    }
                    if std::time::Instant::now() >= until {
                        break None;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                };
                match again {
                    Some(rx) => {
                        crate::append_hook_log(&format!("far terminal {}: attaching again", self.term.term));
                        self.from = rx;
                        continue;
                    }
                    None => return Ok(0),
                }
            };
            match m["did"].as_str().unwrap_or_default() {
                "attached" => {
                    self.term.owner.store(m["owner"].as_u64().unwrap_or(0), Ordering::SeqCst);
                    let taken = self.term.take_state(&m);
                    self.seen = m["seq"].as_u64().unwrap_or(0);
                    // The tail of a sequence the state was taken in the middle
                    // of, read after the state as its output was
                    self.rest = unb64(&m["pending"]);
                    self.at = 0;
                    if !taken {
                        self.rest.extend_from_slice(b"\r\n[the screen could not be brought back; it is drawn again as the program writes]\r\n");
                    }
                }
                "out" => {
                    let seq = m["seq"].as_u64().unwrap_or(0);
                    if seq <= self.seen {
                        continue;
                    }
                    self.seen = seq;
                    self.rest = unb64(&m["b"]);
                    self.at = 0;
                }
                "ended" | "over" => {
                    self.term.ended.store(true, Ordering::SeqCst);
                    if let Ok(mut o) = self.term.open_there.lock() {
                        *o = None;
                    }
                    // The code is had: nothing is kept there any more
                    self.term.say(json!({ "do": "forget", "term": self.term.term }));
                    return Ok(0);
                }
                // Let go of because this line fell behind: attached again at
                // once, and handed the state with everything in it
                "taken" if m["why"].as_str().is_some_and(|w| w.contains("keep up")) => {
                    crate::append_hook_log(&format!("far terminal {}: fell behind; attaching again", self.term.term));
                    let (rows, cols) = self.term.size.lock().map(|s| *s).unwrap_or((24, 80));
                    self.term.say(json!({ "do": "attach", "term": self.term.term, "gen": self.term.generation,
                        "tab": self.term.tab, "rows": rows, "cols": cols }));
                }
                "taken" => {
                    self.rest = b"\r\n[this terminal was opened from somewhere else]\r\n".to_vec();
                    self.at = 0;
                }
                // Asked about again and not known there: whatever it was is
                // not there any more to be attached to
                "unknown" => {
                    crate::append_hook_log(&format!(
                        "far terminal {}: not known there any more ({})",
                        self.term.term,
                        m["why"].as_str().unwrap_or_default()
                    ));
                    self.term.ended.store(true, Ordering::SeqCst);
                    return Ok(0);
                }
                "refused" => {
                    crate::append_hook_log(&format!(
                        "far terminal {}: {} ({})",
                        self.term.term,
                        m["did"].as_str().unwrap_or_default(),
                        m["why"].as_str().unwrap_or_default()
                    ));
                }
                _ => {}
            }
        }
        let n = (self.rest.len() - self.at).min(buf.len());
        buf[..n].copy_from_slice(&self.rest[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

struct FarMaster {
    term: Arc<FarTerm>,
    size: Mutex<portable_pty::PtySize>,
    reader: Mutex<Option<FarReader>>,
    writer_taken: AtomicBool,
}

impl portable_pty::MasterPty for FarMaster {
    fn resize(&self, size: portable_pty::PtySize) -> Result<()> {
        if let Ok(mut s) = self.size.lock() {
            *s = size;
        }
        if let Ok(mut s) = self.term.size.lock() {
            *s = (size.rows, size.cols);
        }
        self.term.say(json!({ "do": "size", "term": self.term.term, "owner": self.term.owner.load(Ordering::SeqCst),
            "rows": size.rows, "cols": size.cols }));
        Ok(())
    }

    fn get_size(&self) -> Result<portable_pty::PtySize> {
        Ok(*self.size.lock().map_err(|_| anyhow!("size"))?)
    }

    fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>> {
        match self.reader.lock().map_err(|_| anyhow!("reader"))?.take() {
            Some(r) => Ok(Box::new(r)),
            None => bail!("the reader for this terminal has already been taken"),
        }
    }

    fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>> {
        if self.writer_taken.swap(true, Ordering::SeqCst) {
            bail!("the writer for this terminal has already been taken");
        }
        Ok(Box::new(FarWriter { term: Arc::clone(&self.term) }))
    }

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

struct FarWriter {
    term: Arc<FarTerm>,
}

impl std::io::Write for FarWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let said = self.term.say(json!({ "do": "in", "term": self.term.term,
            "owner": self.term.owner.load(Ordering::SeqCst), "b": b64(buf) }));
        if said { Ok(buf.len()) } else { Err(std::io::Error::other("the bridge's line is down")) }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct FarKiller {
    term: Arc<FarTerm>,
}

impl std::fmt::Debug for FarKiller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FarKiller({})", self.term.term)
    }
}

impl portable_pty::ChildKiller for FarKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        if !self.term.ended.load(Ordering::SeqCst) {
            self.term.say(json!({ "do": "stop", "term": self.term.term, "owner": self.term.owner.load(Ordering::SeqCst) }));
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

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
//! Each terminal opened is written down the moment it is (far-keep plan §7.4,
//! [`Saved`]), so a start after this app ended -- closed, or killed -- goes
//! back to it rather than starting a second AI beside the first (§7.5): the
//! tab attaches to it if it is there, starts again where the conversation
//! was if its end is known, and starts nothing while neither is known.
//!
//! Used for a machine whose AIs the person chose what to do with while the
//! app is away (its entry's `away`, far-keep plan §4.3), with the bridge put
//! there. Every other machine's tabs are opened as before.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The job's name there
const JOB: &str = "terms";
/// How long a terminal may take to open there
const OPEN_WAIT: Duration = Duration::from_secs(30);
/// How long the bridge may take to say which terminals it holds
const LIST_WAIT: Duration = Duration::from_secs(10);
/// How long a terminal whose line went waits for it to come back before the
/// tab is told it ended. The line is made again by the app on its own
/// (`farlink::Keeper`), a few times a minute
const LINE_BACK_WAIT: Duration = Duration::from_secs(10 * 60);

/// Where a held terminal is: on another machine, held by the bridge there;
/// or on this PC, held by this PC's own resident process (the local-keeper
/// plan), which is the same resident process run here
#[derive(Debug, Clone)]
pub enum Place {
    Far(crate::elsewhere::Elsewhere),
    Here,
}

impl Place {
    /// What its terminals are written down under
    pub fn machine_key(&self) -> String {
        match self {
            Place::Far(at) => at.machine_key(),
            Place::Here => crate::localkeep::KEY.to_string(),
        }
    }

    /// How the log names it
    pub fn address(&self) -> String {
        match self {
            Place::Far(at) => at.address(),
            Place::Here => "this PC".to_string(),
        }
    }

    /// How a person is told it, in their language
    pub fn name(&self) -> String {
        match self {
            Place::Far(at) => at.address(),
            Place::Here => crate::i18n::t("msg.localkeep.this_pc"),
        }
    }

    /// The line to its resident process, when it is up
    pub fn link(&self) -> Option<Arc<crate::farlink::Link>> {
        match self {
            Place::Far(at) => crate::farlink::link(at),
            Place::Here => crate::localkeep::link(),
        }
    }

    /// The MicroVM's entry, when it is one: what pausing and keeping it up
    /// is about
    pub fn cloud(&self) -> Option<&crate::config::HostSpec> {
        match self {
            Place::Far(crate::elsewhere::Elsewhere::Cloud(h)) => Some(h),
            _ => None,
        }
    }
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(v: &Value) -> Vec<u8> {
    v.as_str()
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .unwrap_or_default()
}

// ── What was opened, written down ──────────────────────────────────────────

/// The file the terminals this app opened are written down in. Apart from
/// the last session's (`lastsession`), which keeps conversations, only for
/// tabs worth keeping, and only when the app closes (far-keep plan §7.4)
const SAVED_FILE: &str = "far-terminals";
/// The file's shape, so a later one can refuse to read this rather than
/// half-understand it
const SAVED_VERSION: u32 = 1;

/// One terminal this app opened on a machine's bridge, and whose end it has
/// not seen: what it takes to ask about it again (far-keep plan §4.4, the
/// three points of identity), and which tab it is
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// The machine (`Elsewhere::machine_key`)
    pub machine: String,
    /// Which tab: the folder it stands in there, and who it is (its uid; its
    /// name, in a file written before tabs had uids)
    pub cwd: String,
    pub tab: String,
    /// The resident process's generation, and the terminal's id in it
    pub generation: String,
    pub term: u64,
    /// When it was opened, in seconds since 1970
    pub since: u64,
    /// On a MicroVM: when the run it was in as this app let go of it began,
    /// as the service keeps time. Another run on return is a pause between
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<u64>,
    /// It was told to stop, and its end was not seen yet: the next start
    /// that finds it stops it again rather than going back to it
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopping: bool,
}

impl Saved {
    fn is_tab(&self, machine: &str, cwd: &str, tab: &str) -> bool {
        self.machine == machine && self.cwd == cwd && self.tab == tab
    }
}

#[derive(Serialize, Deserialize)]
struct SavedFile {
    version: u32,
    #[serde(default)]
    terms: Vec<Saved>,
}

/// One change to the file at a time: read, changed, and written whole
static SAVED_LOCK: Mutex<()> = Mutex::new(());

fn read_saved() -> Vec<Saved> {
    std::fs::read_to_string(crate::config::state_path(SAVED_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<SavedFile>(&t).ok())
        .filter(|f| f.version == SAVED_VERSION)
        .map(|f| f.terms)
        .unwrap_or_default()
}

fn change_saved(change: impl FnOnce(&mut Vec<Saved>)) {
    let _one = SAVED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read_saved();
    let before = all.clone();
    change(&mut all);
    if all == before {
        return;
    }
    let text = serde_json::to_string_pretty(&SavedFile { version: SAVED_VERSION, terms: all }).unwrap_or_default();
    if let Err(e) = crate::crypto::write_atomic(&crate::config::state_path(SAVED_FILE), &text) {
        // Said on the screen, not only in the log: what is not written down
        // is not gone back to by name after a restart (the resident process
        // still hands the tab its running terminal when asked to open again)
        crate::caps::tell(crate::i18n::tp("msg.farterm.unwritten", &[("e", &format!("{e:#}"))]));
    }
}

/// Write a terminal down, in place of whatever was written for its tab
fn put(all: &mut Vec<Saved>, s: Saved) {
    all.retain(|o| !o.is_tab(&s.machine, &s.cwd, &s.tab));
    all.push(s);
}

/// Strike a terminal out, once its end is known or it is not there
fn strike(all: &mut Vec<Saved>, machine: &str, generation: &str, term: u64) {
    all.retain(|o| !(o.machine == machine && o.generation == generation && o.term == term));
}

/// What a terminal is to do while the app is away, as the resident process
/// is told it. A MicroVM set to go on for a while is paused by the service
/// when that time is up (`e2b::keep_up_while_away`), freezing the AI where
/// it was, to be taken up again on the next start (far-keep plan §5): its
/// resident process keeps the terminal, rather than ending the AI on a clock
/// of its own that a pause stops and a start takes up again
fn on_the_line(at: &Place, away: crate::config::Away) -> Value {
    match (at.cloud(), away) {
        (Some(_), crate::config::Away::Minutes(_)) => crate::config::Away::Always.on_the_line(),
        _ => away.on_the_line(),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Every terminal written down for a machine taken off: its bridge, and
/// every terminal in it, is gone with it
pub fn forget_machine(at: &Place) {
    let machine = at.machine_key();
    change_saved(|all| all.retain(|o| o.machine != machine));
}

/// The terminal a tab left running on `at`, if there is one to go back to:
/// the one written down for it; or, with nothing written down -- the note
/// was lost -- one the bridge holds for the same tab and folder that no app
/// owns (far-keep plan §7.4). Asked of the bridge only when its line is up
pub fn left_running(at: &Place, cwd: &str, uid: &str, name: &str) -> Option<Saved> {
    let machine = at.machine_key();
    if let Some(s) = written_for(&read_saved(), &machine, cwd, uid, name) {
        return Some(s);
    }
    let said = list_held(at)?;
    let generation = said["gen"].as_str().unwrap_or_default().to_string();
    let found = said["terms"].as_array()?.iter().find(|t| {
        t["tab"].as_str().is_some_and(|tab| is_this_tab(tab, uid, name))
            && t["cwd"] == cwd
            && t["owned"] == false
            && t["ended"] == false
    })?;
    let tab = found["tab"].as_str().unwrap_or(uid).to_string();
    let s = Saved { machine, cwd: cwd.to_string(), tab, generation, term: found["term"].as_u64()?, since: now_secs(), left: None, stopping: false };
    crate::append_hook_log(&format!("far terminal {}: found on {} for {name}, with nothing written down about it", s.term, at.address()));
    change_saved(|all| put(all, s.clone()));
    Some(s)
}

/// Whether a terminal written down for `tab` is the one of the tab `uid`,
/// called `name`. By who the tab is; by its name only for what a version
/// before uids opened, which knew a tab by nothing else -- never a uid taken
/// for a name, since no tab is called something shaped like one
fn is_this_tab(tab: &str, uid: &str, name: &str) -> bool {
    tab == uid || (!crate::config::is_tab_uid(tab) && tab == name)
}

/// The terminal written down on `machine` for that tab in `cwd`: the one
/// written under who it is first, and only then one written under its name
fn written_for(saved: &[Saved], machine: &str, cwd: &str, uid: &str, name: &str) -> Option<Saved> {
    saved
        .iter()
        .find(|s| s.is_tab(machine, cwd, uid))
        .or_else(|| saved.iter().find(|s| s.machine == machine && s.cwd == cwd && is_this_tab(&s.tab, uid, name)))
        .cloned()
}

/// Ask the bridge on `at` something of its terminals job, and wait for the
/// answer: through the one router of its line, which a second listener would
/// take the line's messages from
fn ask(at: &Place, mut m: Value) -> Option<Value> {
    let link = at.link().filter(|l| l.holds(JOB))?;
    let r = router(at, &link);
    let reference = NEXT_REF.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = channel::<Value>();
    r.lock().unwrap_or_else(|e| e.into_inner()).by_ref.insert(reference, tx);
    m["ref"] = json!(reference);
    if !link.to_job(JOB, m) {
        return None;
    }
    rx.recv_timeout(LIST_WAIT).ok()
}

/// Every terminal the bridge on `at` holds, with its generation: for the
/// person's list of what runs while the app is away (far-keep plan §7.6)
pub fn list_held(at: &Place) -> Option<Value> {
    ask(at, json!({ "do": "list" })).filter(|m| m["did"] == "list")
}

/// Stop one of them, whoever owns it: the person asked to, from that list
pub fn end_held(at: &Place, term: u64, generation: &str) -> bool {
    let done = ask(at, json!({ "do": "end", "term": term, "gen": generation })).is_some_and(|m| m["did"] == "ending");
    if done {
        let machine = at.machine_key();
        change_saved(|all| strike(all, &machine, generation, term));
    }
    done
}

/// Stop every one, as the bridge is taken off the machine (§7.7). Nothing
/// is asked when the line is not up: the resident process is told to end
/// as the folder goes
pub fn end_all(at: &Place) {
    if ask(at, json!({ "do": "end_all" })).is_some() {
        crate::append_hook_log(&format!("far terminals on {}: every one stopped, the bridge being taken off", at.address()));
        std::thread::sleep(Duration::from_millis(500));
    }
}

// ── The line ────────────────────────────────────────────────────────────────

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
fn router(at: &Place, link: &crate::farlink::Link) -> Arc<Mutex<Router>> {
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

/// Ask the bridge on `at` for a terminal. Its id and generation, and where
/// what is said about it comes
fn ask_open(
    at: &Place,
    tab: &str,
    (rows, cols): (u16, u16),
    cwd: Option<&str>,
    then: Option<&str>,
    run: Option<&Value>,
    away: crate::config::Away,
) -> Result<(u64, String, Receiver<Value>, Arc<crate::farlink::Link>)> {
    let link = at.link().ok_or_else(|| anyhow!("the bridge on {} is not connected", at.address()))?;
    if !link.holds(JOB) {
        bail!("the bridge on {} does not hold terminals (an older version)", at.address());
    }
    let r = router(at, &link);
    let reference = NEXT_REF.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = channel::<Value>();
    r.lock().unwrap_or_else(|e| e.into_inner()).by_ref.insert(reference, tx);
    // "reuse": the tab's terminal, if one is already running there, rather
    // than a second AI beside it -- whatever this app wrote down or failed to
    let mut asked = json!({ "do": "open", "ref": reference, "tab": tab, "rows": rows, "cols": cols,
        "cwd": cwd.unwrap_or_default(), "then": then.unwrap_or_default(), "away": on_the_line(at, away), "reuse": true });
    // On this PC the command is said in full: the program, its arguments and
    // the environment the tab put together for it (`crate::localkeep`)
    if let Some(run) = run {
        asked["argv"] = run["argv"].clone();
        asked["env_all"] = run["env_all"].clone();
    }
    if !link.to_job(JOB, asked) {
        bail!("the bridge on {} could not be asked for a terminal", at.address());
    }
    let opened = rx.recv_timeout(OPEN_WAIT).map_err(|_| anyhow!("the bridge on {} did not open a terminal", at.address()))?;
    if opened["did"] != "opened" {
        bail!("the bridge on {} could not open a terminal: {}", at.address(), opened["why"].as_str().unwrap_or("no reason given"));
    }
    let term = opened["term"].as_u64().unwrap_or(0);
    let generation = opened["gen"].as_str().unwrap_or_default().to_string();
    crate::append_hook_log(&format!("far terminal {term} opened on {} for {tab}", at.address()));
    change_saved(|all| {
        put(
            all,
            Saved {
                machine: at.machine_key(),
                cwd: cwd.unwrap_or_default().to_string(),
                tab: tab.to_string(),
                generation: generation.clone(),
                term,
                since: now_secs(),
                left: None,
                stopping: false,
            },
        )
    });
    Ok((term, generation, rx, link))
}

// ── The terminal ────────────────────────────────────────────────────────────

/// Which terminal there this is. Changes once, when a terminal gone back to
/// after a start is found to have ended and a new one is opened in its place
struct Ident {
    term: u64,
    generation: String,
}

/// A held terminal: what it takes to ask about it again, and what this end
/// holds of it
pub struct FarTerm {
    ident: Mutex<Ident>,
    tab: String,
    /// Where the tab stands there, and what is typed into a new terminal:
    /// what a terminal found ended is opened again with
    cwd: Option<String>,
    then: Option<String>,
    /// On this PC, what is run, said in full (see `ask_open`)
    run: Option<Value>,
    /// What it does while this app is away (far-keep plan §4.3): changed
    /// when the person changes the machine's setting
    away: Mutex<crate::config::Away>,
    owner: AtomicU64,
    /// Where it is, and the line to it now: a line that went and came back
    /// is another line
    at: Place,
    link: Mutex<Option<Arc<crate::farlink::Link>>>,
    size: Mutex<(u16, u16)>,
    /// The tab's own parser and what beside it a program's asks are kept in,
    /// so a state handed over goes where the tab reads from
    bound: Mutex<Option<Bound>>,
    ended: AtomicBool,
    /// The tab let go of it: a wait for the line to come back ends
    let_go: AtomicBool,
    /// Gone back to after a start: when the app before let go of it
    left: Mutex<Option<u64>>,
    /// Gone back to after a start: it was told to stop, and its end was not
    /// seen -- stopped again if it still runs, and a new one started
    stopping: AtomicBool,
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
    fn new(
        at: &Place,
        ident: Ident,
        tab: &str,
        size: (u16, u16),
        (cwd, then): (Option<&str>, Option<&str>),
        away: crate::config::Away,
    ) -> Self {
        Self {
            ident: Mutex::new(ident),
            tab: tab.to_string(),
            cwd: cwd.map(str::to_string),
            then: then.map(str::to_string),
            run: None,
            away: Mutex::new(away),
            owner: AtomicU64::new(0),
            at: at.clone(),
            link: Mutex::new(None),
            size: Mutex::new(size),
            bound: Mutex::new(None),
            ended: AtomicBool::new(false),
            let_go: AtomicBool::new(false),
            left: Mutex::new(None),
            stopping: AtomicBool::new(false),
            open_there: Mutex::new(at.cloud().and_then(|h| h.instance.as_deref()).map(crate::e2b::Opened::new)),
        }
    }

    /// What is run on this PC, said in full (`ask_open`)
    fn with_run(mut self, run: Option<Value>) -> Self {
        self.run = run;
        self
    }

    /// Whether it is held on this PC, by this PC's own resident process
    pub fn here(&self) -> bool {
        matches!(self.at, Place::Here)
    }

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

    fn term(&self) -> u64 {
        self.ident.lock().map(|i| i.term).unwrap_or(0)
    }

    /// Whether its AI goes on once this app went (far-keep plan §4.3)
    pub fn keeps(&self) -> bool {
        self.away().keeps()
    }

    /// What it does while this app is away
    pub fn away(&self) -> crate::config::Away {
        self.away.lock().map(|a| *a).unwrap_or(crate::config::Away::Stop)
    }

    /// The person changed what its machine's AIs do while the app is away:
    /// from now on, here and there
    pub fn set_away(&self, away: crate::config::Away) {
        let was = self.away.lock().map(|mut a| std::mem::replace(&mut *a, away)).unwrap_or(away);
        if was == away {
            return;
        }
        crate::append_hook_log(&format!("far terminal {}: while the app is away, now {away:?} (was {was:?})", self.term()));
        // Said now when the line is up; the next attach -- the line back, the
        // next start -- carries it either way
        let owner = self.owner.load(Ordering::SeqCst);
        if owner == 0 || !self.say(json!({ "do": "set_away", "term": self.term(), "owner": owner, "away": on_the_line(&self.at, away) })) {
            crate::append_hook_log(&format!("far terminal {}: the change is said on its next attach", self.term()));
        }
    }

    /// Let go of it without stopping it: the line is left, and what runs
    /// there carries on as it was set to (far-keep plan §7, "disconnect").
    /// It stays written down, so the next start goes back to it
    pub fn let_go(&self) {
        self.let_go.store(true, Ordering::SeqCst);
        // When, for the next start to tell whether its machine was paused
        // since (a MicroVM freezes what runs on it)
        let run = self.at.cloud().and_then(|h| h.instance.as_deref()).and_then(crate::e2b::run_left);
        let (machine, generation, term) = (self.at.machine_key(), self.generation(), self.term());
        change_saved(|all| {
            if let Some(s) = all.iter_mut().find(|s| s.machine == machine && s.generation == generation && s.term == term) {
                s.left = run;
            }
        });
        crate::append_hook_log(&format!("far terminal {}: let go of, left running ({:?})", self.term(), self.away()));
    }

    fn generation(&self) -> String {
        self.ident.lock().map(|i| i.generation.clone()).unwrap_or_default()
    }

    fn say(&self, m: Value) -> bool {
        self.link.lock().ok().and_then(|l| l.clone()).is_some_and(|l| l.to_job(JOB, m))
    }

    fn attach_message(&self) -> Value {
        let (rows, cols) = self.size.lock().map(|s| *s).unwrap_or((24, 80));
        json!({ "do": "attach", "term": self.term(), "gen": self.generation(), "tab": self.tab, "rows": rows, "cols": cols,
            "away": on_the_line(&self.at, self.away()) })
    }

    /// The line is up: be routed its messages again and attach, which hands
    /// the state over. `None` while the line is not up yet
    fn attach_again(&self) -> Option<Receiver<Value>> {
        let link = self.at.link().filter(|l| l.holds(JOB))?;
        let r = router(&self.at, &link);
        let (tx, rx) = channel();
        r.lock().unwrap_or_else(|e| e.into_inner()).by_term.insert(self.term(), tx);
        if let Ok(mut l) = self.link.lock() {
            *l = Some(Arc::clone(&link));
        }
        link.to_job(JOB, self.attach_message()).then_some(rx)
    }

    /// Told to stop: written down as stopping until its end is seen
    fn stopping(&self) {
        let (machine, generation, term) = (self.at.machine_key(), self.generation(), self.term());
        change_saved(|all| {
            if let Some(s) = all.iter_mut().find(|s| s.machine == machine && s.generation == generation && s.term == term) {
                s.stopping = true;
            }
        });
    }

    /// Nothing of it is kept there or here any more
    fn struck_out(&self) {
        let (machine, generation, term) = (self.at.machine_key(), self.generation(), self.term());
        change_saved(|all| strike(all, &machine, &generation, term));
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
                crate::append_hook_log(&format!("far terminal {}: its screen could not be taken over: {e}", self.term()));
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

type Opened = (Box<dyn portable_pty::MasterPty + Send>, Box<dyn portable_pty::ChildKiller + Send + Sync>, Arc<FarTerm>);

fn made(term: Arc<FarTerm>, from: Receiver<Value>, again: bool, fresh: bool) -> Opened {
    let (rows, cols) = term.size.lock().map(|s| *s).unwrap_or((24, 80));
    let reader = FarReader {
        from,
        term: Arc::clone(&term),
        seen: 0,
        rest: Vec::new(),
        at: 0,
        again,
        fresh,
        attached: false,
        said_waiting: false,
        last: false,
    };
    let master = FarMaster {
        term: Arc::clone(&term),
        size: Mutex::new(portable_pty::PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }),
        reader: Mutex::new(Some(reader)),
        writer_taken: AtomicBool::new(false),
    };
    (Box::new(master), Box::new(FarKiller { term: Arc::clone(&term) }), term)
}

/// Open a terminal for `tab` in the bridge's resident process on `at`: a
/// shell in `cwd`, with `then` typed into it
pub fn open(
    at: &Place,
    tab: &str,
    (rows, cols): (u16, u16),
    (cwd, then): (Option<&str>, Option<&str>),
    run: Option<Value>,
    away: crate::config::Away,
) -> Result<Opened> {
    let (id, generation, rx, link) = ask_open(at, tab, (rows, cols), cwd, then, run.as_ref(), away)?;
    let term = Arc::new(FarTerm::new(at, Ident { term: id, generation }, tab, (rows, cols), (cwd, then), away).with_run(run));
    if let Ok(mut l) = term.link.lock() {
        *l = Some(link);
    }
    Ok(made(term, rx, false, false))
}

/// Go back to the terminal a tab left running (far-keep plan §7.5), whether
/// or not the line to its machine is up yet: the tab waits for it, then
/// attaches. Never starts anything by itself -- unless the bridge says the
/// terminal ended, when a new one is opened with `then`, the way the tab
/// starts where its conversation was
pub fn reattach(
    at: &Place,
    saved: Saved,
    (rows, cols): (u16, u16),
    (cwd, then): (Option<&str>, Option<&str>),
    run: Option<Value>,
    away: crate::config::Away,
) -> Opened {
    crate::append_hook_log(&format!("far terminal {}: going back to it on {} for {}", saved.term, at.address(), saved.tab));
    let term = Arc::new(FarTerm::new(at, Ident { term: saved.term, generation: saved.generation }, &saved.tab, (rows, cols), (cwd, then), away).with_run(run));
    if let Ok(mut l) = term.left.lock() {
        *l = saved.left;
    }
    term.stopping.store(saved.stopping, Ordering::SeqCst);
    // Read as a line that went: the reader attaches as soon as it is up
    let (_, gone) = channel();
    made(term, gone, true, false)
}

/// Open a terminal for `tab` once the line to its machine is up: a tab of a
/// machine whose AIs are held there starts there, and not the old way for
/// want of the line at the moment it was started. The tab says it is
/// waiting, and gives up -- starting nothing -- if the line never comes
pub fn open_later(
    at: &Place,
    tab: &str,
    (rows, cols): (u16, u16),
    (cwd, then): (Option<&str>, Option<&str>),
    run: Option<Value>,
    away: crate::config::Away,
) -> Opened {
    crate::append_hook_log(&format!("far terminal for {tab}: to be opened on {} once its line is up", at.address()));
    let term = Arc::new(FarTerm::new(at, Ident { term: 0, generation: String::new() }, tab, (rows, cols), (cwd, then), away).with_run(run));
    let (_, gone) = channel();
    made(term, gone, true, true)
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
    /// Gone back to after a start, and not attached to yet
    again: bool,
    /// Not opened yet: opened once the line is up (`open_later`)
    fresh: bool,
    attached: bool,
    said_waiting: bool,
    /// What is in `rest` is the last of it: the terminal ends after it
    last: bool,
}

impl FarReader {
    /// Show `text` in the tab, on a line of its own
    fn say(&mut self, text: &str) {
        self.rest = format!("\r\n[{text}]\r\n").into_bytes();
        self.at = 0;
    }

    /// Show `text`, and end after it
    fn say_last(&mut self, text: &str) {
        self.say(text);
        self.last = true;
    }

    /// The terminal gone back to ended while this app was not there: its end
    /// is known, so the tab starts where its conversation was, in a new one.
    /// Or there was none yet (`open_later`), and this is its first
    fn open_in_its_place(&mut self) -> bool {
        let (rows, cols) = self.term.size.lock().map(|s| *s).unwrap_or((24, 80));
        match ask_open(&self.term.at, &self.term.tab, (rows, cols), self.term.cwd.as_deref(), self.term.then.as_deref(), self.term.run.as_ref(), self.term.away()) {
            Ok((id, generation, rx, link)) => {
                if self.fresh {
                    crate::append_hook_log(&format!("far terminal {id} opened on {} once its line was up", self.term.at.address()));
                } else {
                    crate::append_hook_log(&format!("far terminal {}: it ended while away; opened {id} in its place", self.term.term()));
                }
                self.fresh = false;
                if let Ok(mut i) = self.term.ident.lock() {
                    *i = Ident { term: id, generation };
                }
                if let Ok(mut l) = self.term.link.lock() {
                    *l = Some(link);
                }
                self.from = rx;
                self.again = false;
                true
            }
            Err(e) => {
                crate::append_hook_log(&format!("far terminal {}: a new one could not be opened in its place: {e:#}", self.term.term()));
                self.term.ended.store(true, Ordering::SeqCst);
                if let Ok(mut o) = self.term.open_there.lock() {
                    *o = None;
                }
                self.say_last(&crate::i18n::tp("msg.farterm.not_opened", &[("e", &format!("{e:#}"))]));
                false
            }
        }
    }
}

impl std::io::Read for FarReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.at >= self.rest.len() {
            if self.last {
                return Ok(0);
            }
            let Ok(m) = self.from.recv() else {
                // The line went (or, gone back to after a start, is not up
                // yet). The terminal there may well still be there: wait for
                // the line to come back and attach to it again
                if self.term.ended.load(Ordering::SeqCst) || self.term.let_go.load(Ordering::SeqCst) {
                    return Ok(0);
                }
                let address = self.term.at.address();
                if self.again && !self.said_waiting {
                    // Said first, so the tab shows why it is empty
                    self.said_waiting = true;
                    let key = if self.fresh { "msg.farterm.waiting_line" } else { "msg.farterm.going_back" };
                    self.say(&crate::i18n::tp(key, &[("host", &address)]));
                    continue;
                }
                crate::append_hook_log(&format!("far terminal {}: the line went; waiting for it to come back", self.term.term()));
                let until = std::time::Instant::now() + LINE_BACK_WAIT;
                let again = loop {
                    if self.term.let_go.load(Ordering::SeqCst) {
                        return Ok(0);
                    }
                    // The line to this PC's resident process is made by the
                    // tabs that need it, not by the round that keeps the
                    // other machines' lines: asked again while it is down
                    if self.term.here() {
                        crate::localkeep::connect_soon();
                    }
                    if self.fresh {
                        // Nothing there yet to attach to: opened as soon as
                        // the line is up
                        if self.term.at.link().is_some_and(|l| l.holds(JOB)) {
                            self.open_in_its_place();
                            break Some(None);
                        }
                    } else if let Some(rx) = self.term.attach_again() {
                        break Some(Some(rx));
                    }
                    if std::time::Instant::now() >= until {
                        break None;
                    }
                    std::thread::sleep(Duration::from_secs(2));
                };
                match again {
                    // Opened, or said why not
                    Some(None) => continue,
                    Some(Some(rx)) => {
                        crate::append_hook_log(&format!("far terminal {}: attaching again", self.term.term()));
                        self.from = rx;
                        continue;
                    }
                    // Not reached: the terminal stays written down, and a
                    // restart of the tab asks again. Nothing new is started
                    None if self.again => {
                        self.say_last(&crate::i18n::tp("msg.farterm.not_reached", &[("host", &address)]));
                        continue;
                    }
                    None => return Ok(0),
                }
            };
            match m["did"].as_str().unwrap_or_default() {
                // It was told to stop, and still runs: stopped now, and once
                // its end is seen a new one is started in its place
                "attached" if self.again && self.term.stopping.load(Ordering::SeqCst) => {
                    let owner = m["owner"].as_u64().unwrap_or(0);
                    self.term.owner.store(owner, Ordering::SeqCst);
                    crate::append_hook_log(&format!("far terminal {}: told to stop before, still running; stopped again", self.term.term()));
                    self.term.say(json!({ "do": "stop", "term": self.term.term(), "owner": owner }));
                }
                "attached" => {
                    // A MicroVM paused while the app was away froze what ran
                    // on it: an AI in the middle of a reply may have lost it
                    // (far-keep plan §5), and the person is told so
                    let mut frozen = false;
                    if self.again && !self.attached {
                        crate::append_hook_log(&format!("far terminal {}: went back to it", self.term.term()));
                        if let (Some(h), Some(left)) = (self.term.at.cloud(), self.term.left.lock().ok().and_then(|l| *l))
                            && let Some(id) = h.instance.as_deref()
                        {
                            frozen = crate::e2b::begun_again_since(id, left).unwrap_or(false);
                        }
                    }
                    self.attached = true;
                    self.again = false;
                    self.term.owner.store(m["owner"].as_u64().unwrap_or(0), Ordering::SeqCst);
                    let taken = self.term.take_state(&m);
                    self.seen = m["seq"].as_u64().unwrap_or(0);
                    // The tail of a sequence the state was taken in the middle
                    // of, read after the state as its output was
                    self.rest = unb64(&m["pending"]);
                    self.at = 0;
                    if !taken {
                        self.rest.extend_from_slice(format!("\r\n[{}]\r\n", crate::i18n::t("msg.farterm.screen_lost")).as_bytes());
                    }
                    if frozen {
                        crate::append_hook_log(&format!("far terminal {}: its machine was paused while the app was away", self.term.term()));
                        self.rest.extend_from_slice(format!("\r\n[{}]\r\n", crate::i18n::t("msg.farterm.frozen")).as_bytes());
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
                    // The code is had: nothing is kept there any more
                    self.term.say(json!({ "do": "forget", "term": self.term.term() }));
                    self.term.struck_out();
                    if self.again && !self.term.let_go.load(Ordering::SeqCst) {
                        if self.open_in_its_place() {
                            continue;
                        }
                    } else {
                        self.term.ended.store(true, Ordering::SeqCst);
                        if let Ok(mut o) = self.term.open_there.lock() {
                            *o = None;
                        }
                        return Ok(0);
                    }
                }
                // Let go of because this line fell behind: attached again at
                // once, and handed the state with everything in it
                "taken" if m["why"].as_str().is_some_and(|w| w.contains("keep up")) => {
                    crate::append_hook_log(&format!("far terminal {}: fell behind; attaching again", self.term.term()));
                    self.term.say(self.term.attach_message());
                }
                "taken" => {
                    let text = crate::i18n::t("msg.farterm.taken");
                    self.say(&text);
                }
                // Asked about again and not known there: whatever it was is
                // not there any more to be attached to. Nothing new is
                // started for it: whether the AI there is gone is not known
                "unknown" => {
                    crate::append_hook_log(&format!(
                        "far terminal {}: not known there any more ({})",
                        self.term.term(),
                        m["why"].as_str().unwrap_or_default()
                    ));
                    self.term.struck_out();
                    self.term.ended.store(true, Ordering::SeqCst);
                    if let Ok(mut o) = self.term.open_there.lock() {
                        *o = None;
                    }
                    let text = crate::i18n::tp("msg.farterm.unknown", &[("host", &self.term.at.name())]);
                    self.say_last(&text);
                }
                "refused" => {
                    crate::append_hook_log(&format!(
                        "far terminal {}: {} ({})",
                        self.term.term(),
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
        self.term.say(json!({ "do": "size", "term": self.term.term(), "owner": self.term.owner.load(Ordering::SeqCst),
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
        let said = self.term.say(json!({ "do": "in", "term": self.term.term(),
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
        write!(f, "FarKiller({})", self.term.term())
    }
}

impl portable_pty::ChildKiller for FarKiller {
    /// Stop the terminal there. One not attached to -- gone back to after a
    /// start, and waiting for its line -- is let go of here and stays
    /// written down: nobody here can stop it, and the next start of the tab
    /// goes back to it again rather than starting a second AI beside it
    fn kill(&mut self) -> std::io::Result<()> {
        self.term.let_go.store(true, Ordering::SeqCst);
        if self.term.ended.load(Ordering::SeqCst) || self.term.term() == 0 {
            return Ok(());
        }
        // Owned from here, stopped as its owner; not attached to yet (gone
        // back to, waiting), stopped by its identity, whoever owns it
        let owner = self.term.owner.load(Ordering::SeqCst);
        let told = if owner != 0 {
            self.term.say(json!({ "do": "stop", "term": self.term.term(), "owner": owner }))
        } else {
            self.term.say(json!({ "do": "end", "term": self.term.term(), "gen": self.term.generation() }))
        };
        if told {
            // Said, which is not yet heard: written down as stopping until
            // its end comes back on the line (the reader strikes it out
            // then). A start that finds it still running stops it again
            self.term.stopping();
        } else {
            // The line is down: nothing here can stop it. It stays written
            // down, and the next start goes back to it rather than starting a
            // second AI beside it
            crate::append_hook_log(&format!(
                "far terminal {}: could not be told to stop, its line being down; it stays written down",
                self.term.term()
            ));
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(machine: &str, cwd: &str, tab: &str, generation: &str, term: u64) -> Saved {
        Saved { machine: machine.into(), cwd: cwd.into(), tab: tab.into(), generation: generation.into(), term, since: 1, left: None, stopping: false }
    }

    /// A tab goes back to the terminal it left running by who it is; one
    /// written down by a version before uids, under its name, is found by
    /// that -- and a tab that came since under another tab's name is never
    /// handed the other's terminal
    #[test]
    fn a_tab_goes_back_to_its_own_terminal_and_nobody_elses() {
        let (me, other) = ("11111111-1111-4111-8111-111111111111", "22222222-2222-4222-8222-222222222222");
        let book = vec![saved("pi", "/w", other, "g", 1), saved("pi", "/w", me, "g", 2), saved("pi", "/old", "tiger", "g", 3)];
        assert_eq!(written_for(&book, "pi", "/w", me, "tiger").map(|s| s.term), Some(2));
        assert_eq!(written_for(&book, "pi", "/w", "33333333-3333-4333-8333-333333333333", "tiger"), None, "a new tab was handed another's");
        // Written by an older version, under the name
        assert_eq!(written_for(&book, "pi", "/old", me, "tiger").map(|s| s.term), Some(3));
        // A uid is never matched as a name
        assert!(!is_this_tab(other, me, other));
        assert!(is_this_tab("tiger", me, "tiger") && is_this_tab(me, me, "tiger"));
    }

    /// A tab has one terminal written down, the last it opened; one is struck
    /// out by its generation and id, which leaves another machine's, another
    /// folder's and another generation's alone
    #[test]
    fn one_terminal_is_written_down_per_tab_and_struck_out_by_its_identity() {
        let mut all = Vec::new();
        put(&mut all, saved("vm", "/w", "claude", "g1", 1));
        put(&mut all, saved("vm", "/w", "claude", "g1", 2));
        put(&mut all, saved("vm", "/w2", "claude", "g1", 3));
        put(&mut all, saved("vps", "/w", "claude", "g1", 4));
        assert_eq!(all.iter().map(|s| s.term).collect::<Vec<_>>(), vec![2, 3, 4]);
        strike(&mut all, "vm", "g2", 2);
        assert_eq!(all.len(), 3, "another generation's terminal 2 is not this one");
        strike(&mut all, "vm", "g1", 2);
        assert_eq!(all.iter().map(|s| s.term).collect::<Vec<_>>(), vec![3, 4]);
    }

    /// The file is read back as written, and one of another version is not
    /// half-read
    #[test]
    fn the_file_keeps_its_shape() {
        let text = serde_json::to_string(&SavedFile { version: SAVED_VERSION, terms: vec![saved("vm", "/w", "t", "g", 7)] }).unwrap();
        let back: SavedFile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.terms, vec![saved("vm", "/w", "t", "g", 7)]);
        let later = text.replace(&format!("\"version\":{SAVED_VERSION}"), "\"version\":99");
        assert!(serde_json::from_str::<SavedFile>(&later).ok().filter(|f| f.version == SAVED_VERSION).is_none());
    }
}

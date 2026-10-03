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
//!
//! Routing has three distinct identities: the connection, a request's ref,
//! and a terminal's (resident generation, number). Only an open/attach
//! acknowledgement may establish an output route. Request registrations are
//! scoped to their receivers; a late reply cannot revive one. Peers which
//! predate generation-labelled output are usable within their current
//! lifetime, but cannot safely answer questions about an earlier one.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use crate::farterms::Identity as Ident;
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
    let found = said["terms"]
        .as_array()?
        .iter()
        .find(|t| t["tab"].as_str().is_some_and(|tab| is_this_tab(tab, uid, name)) && t["cwd"] == cwd && t["owned"] == false && t["ended"] == false)?;
    let tab = found["tab"].as_str().unwrap_or(uid).to_string();
    let s = Saved { machine, cwd: cwd.to_string(), tab, generation, term: found["term"].as_u64()?, since: now_secs(), left: None, stopping: false };
    crate::append_hook_log(&format!("far terminal {}: found on {} for {name}, with nothing written down about it", s.term, at.address()));
    change_saved(|all| put(all, s.clone()));
    Some(s)
}

/// Whether a terminal written down for `tab` is the one of the tab `uid`,
/// called `name`. By who the tab is; by its name only for what a version
/// before uids opened, which knew a tab by nothing else -- never a uid taken
/// for a name, since no tab is called something shaped like one, and never
/// for a tab made since, which was not there when it was opened
fn is_this_tab(tab: &str, uid: &str, name: &str) -> bool {
    tab == uid || (!crate::config::is_tab_uid(tab) && tab == name && crate::config::uid_is_worked_out(uid))
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
fn ask(at: &Place, m: Value) -> Option<Value> {
    let link = at.link().filter(|l| l.holds(JOB))?;
    let r = router(at, &link);
    r.send(&link, m, RouteKind::Reply)?.from.recv_timeout(LIST_WAIT).ok()
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

/// Whether a terminal is written down for the tab `uid` on this PC's own
/// resident process: a tab that left one running there goes back to it
/// even with the setting now off, rather than starting a second copy of
/// what is still running (local-keeper plan §3)
pub fn written_here(uid: &str) -> bool {
    let machine = Place::Here.machine_key();
    read_saved().iter().any(|s| s.machine == machine && s.tab == uid)
}

/// Tell this PC's resident process when to keep the PC up while it holds
/// terminals (the "keep awake" setting, and which of its terminals an AI is
/// working in as this app sees it), so that the setting goes on applying
/// after the app is gone. Not answered; an older resident process ignores it
pub fn tell_awake(mode: &str, working: &[u64]) {
    let Some(link) = Place::Here.link().filter(|l| l.holds(JOB)) else { return };
    let _ = link.to_job(JOB, json!({ "do": "awake", "mode": mode, "working": working }));
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

/// Each request owns its registration. A timed-out request, a closed tab,
/// and a dead connection all release their senders, even if no reply arrives.
struct Messages {
    from: Receiver<Value>,
    router: Weak<Router>,
    reference: u64,
}

impl Messages {
    fn gone() -> Self {
        let (_, from) = channel();
        Self { from, router: Weak::new(), reference: 0 }
    }
}

impl Drop for Messages {
    fn drop(&mut self) {
        if let Some(r) = self.router.upgrade() {
            r.routes.lock().unwrap_or_else(|e| e.into_inner()).remove(self.reference);
        }
    }
}

enum RouteKind {
    Reply,
    Open,
    Attach(Ident),
    Stream(Ident),
}

struct Route {
    to: Sender<Value>,
    kind: RouteKind,
}

/// Requests and streams have different lifetimes. A management reply must
/// never take over a terminal's output route just because it names a term.
#[derive(Default)]
struct Routes {
    by_term: HashMap<Ident, u64>,
    by_ref: HashMap<u64, Route>,
    peer: Option<Peer>,
}

#[derive(Clone)]
struct Peer {
    generation: String,
    identities: bool,
}

impl Routes {
    fn remove(&mut self, reference: u64) {
        self.by_ref.remove(&reference);
        self.by_term.retain(|_, r| *r != reference);
    }

    fn identity(&self, m: &Value) -> Option<Ident> {
        if let Some(id) = Ident::read(m) {
            return Some(id);
        }
        // Compatibility with an older resident process is confined to its
        // current generation, learned before any stream is registered.
        let peer = self.peer.as_ref().filter(|p| !p.identities && m.get("gen").is_none())?;
        Some(Ident { term: m["term"].as_u64().filter(|n| *n != 0)?, generation: peer.generation.clone() })
    }

    fn deliver(&mut self, m: Value) {
        let identity = self.identity(&m);
        let reference = if let Some(r) = m["ref"].as_u64() {
            // A late answer to an abandoned request cannot become output.
            r
        } else {
            // Older peers don't echo attach refs. Only current-generation
            // attaches are allowed against those peers (Router::attach).
            let Some(id) = identity.as_ref() else { return };
            let Some(r) = self.by_term.get(id) else {
                return;
            };
            *r
        };
        let Some(route) = self.by_ref.get_mut(&reference) else {
            return;
        };
        let mut terminal_reply = false;
        match &route.kind {
            RouteKind::Reply => terminal_reply = true,
            RouteKind::Open if m["did"] == "opened" && identity.is_some() => {
                let id = identity.unwrap();
                route.kind = RouteKind::Stream(id.clone());
                self.by_term.insert(id, reference);
            }
            RouteKind::Attach(expected) if identity.as_ref() == Some(expected) => {
                if m["did"] == "attached" {
                    let id = expected.clone();
                    route.kind = RouteKind::Stream(id.clone());
                    self.by_term.insert(id, reference);
                } else {
                    terminal_reply = matches!(m["did"].as_str(), Some("over" | "unknown" | "refused" | "failed"));
                    if !terminal_reply {
                        return;
                    }
                }
            }
            RouteKind::Stream(expected) if identity.as_ref() == Some(expected) => {}
            RouteKind::Open if m["did"] == "failed" => terminal_reply = true,
            _ => return,
        }
        if route.to.send(m).is_err() || terminal_reply {
            self.remove(reference);
        }
    }
}

struct Router {
    link: Weak<crate::farlink::Link>,
    routes: Mutex<Routes>,
    peer: OnceLock<Peer>,
    checking_peer: Mutex<()>,
}

impl Router {
    fn register(self: &Arc<Self>, kind: RouteKind) -> Messages {
        let reference = NEXT_REF.fetch_add(1, Ordering::SeqCst) + 1;
        let (to, from) = channel();
        self.routes.lock().unwrap_or_else(|e| e.into_inner()).by_ref.insert(reference, Route { to, kind });
        Messages { from, router: Arc::downgrade(self), reference }
    }

    fn send(self: &Arc<Self>, link: &crate::farlink::Link, mut m: Value, kind: RouteKind) -> Option<Messages> {
        let messages = self.register(kind);
        m["ref"] = json!(messages.reference);
        link.to_job(JOB, m).then_some(messages)
    }

    fn peer(self: &Arc<Self>, link: &crate::farlink::Link) -> Option<&Peer> {
        if let Some(peer) = self.peer.get() {
            return Some(peer);
        }
        let _one = self.checking_peer.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(peer) = self.peer.get() {
            return Some(peer);
        }
        let messages = self.send(link, json!({ "do": "list" }), RouteKind::Reply)?;
        let m = messages.from.recv_timeout(LIST_WAIT).ok()?;
        if m["did"] != "list" {
            return None;
        }
        let peer = Peer { generation: m["gen"].as_str().filter(|s| !s.is_empty())?.to_string(), identities: m["identities"] == true };
        self.routes.lock().unwrap_or_else(|e| e.into_inner()).peer = Some(peer.clone());
        let _ = self.peer.set(peer);
        self.peer.get()
    }

    fn attach(self: &Arc<Self>, link: &crate::farlink::Link, m: Value) -> Option<Messages> {
        let id = Ident::read(&m)?;
        let peer = self.peer(link)?;
        if !peer.identities && peer.generation != id.generation {
            // An old peer cannot label its answer about a previous lifetime.
            // Do not ask it an ambiguous question or start another AI.
            let mut answer = json!({ "did": "unknown", "why": "the older resident process cannot identify a terminal from its previous run" });
            id.stamp(&mut answer);
            let (to, from) = channel();
            let _ = to.send(answer);
            return Some(Messages { from, router: Weak::new(), reference: 0 });
        }
        let messages = self.register(RouteKind::Attach(id.clone()));
        if !peer.identities {
            self.routes.lock().unwrap_or_else(|e| e.into_inner()).by_term.insert(id, messages.reference);
        }
        let mut m = m;
        m["ref"] = json!(messages.reference);
        link.to_job(JOB, m).then_some(messages)
    }

    fn close(&self) {
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        routes.by_ref.clear();
        routes.by_term.clear();
    }
}

static ROUTERS: OnceLock<Mutex<HashMap<String, Arc<Router>>>> = OnceLock::new();
static NEXT_REF: AtomicU64 = AtomicU64::new(0);

/// A router belongs to one connection, not just to a machine. Cleanup of
/// an old connection must not remove the router of its replacement.
fn router(at: &Place, link: &Arc<crate::farlink::Link>) -> Arc<Router> {
    let mut all = ROUTERS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    let key = at.machine_key();
    if let Some(r) = all.get(&key)
        && r.link.ptr_eq(&Arc::downgrade(link))
    {
        return Arc::clone(r);
    }
    let r = Arc::new(Router { link: Arc::downgrade(link), routes: Mutex::default(), peer: OnceLock::new(), checking_peer: Mutex::new(()) });
    let from = link.listen_job(JOB);
    let (r2, key2) = (Arc::clone(&r), key.clone());
    std::thread::spawn(move || {
        for m in from {
            r2.routes.lock().unwrap_or_else(|e| e.into_inner()).deliver(m);
        }
        r2.close();
        if let Ok(mut all) = ROUTERS.get_or_init(Default::default).lock()
            && all.get(&key2).is_some_and(|r| Arc::ptr_eq(r, &r2))
        {
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
) -> Result<(u64, String, Messages, Arc<crate::farlink::Link>)> {
    let link = at.link().ok_or_else(|| anyhow!("the bridge on {} is not connected", at.address()))?;
    if !link.holds(JOB) {
        bail!("the bridge on {} does not hold terminals (an older version)", at.address());
    }
    let r = router(at, &link);
    r.peer(&link).ok_or_else(|| anyhow!("the bridge on {} did not identify its terminals", at.address()))?;
    // "reuse": the tab's terminal, if one is already running there, rather
    // than a second AI beside it -- whatever this app wrote down or failed to
    let mut asked = json!({ "do": "open", "tab": tab, "rows": rows, "cols": cols,
        "cwd": cwd.unwrap_or_default(), "then": then.unwrap_or_default(), "away": on_the_line(at, away), "reuse": true });
    // On this PC the command is said in full: the program, its arguments and
    // the environment the tab put together for it (`crate::localkeep`)
    if let Some(run) = run {
        asked["argv"] = run["argv"].clone();
        asked["env_all"] = run["env_all"].clone();
    }
    let rx = r.send(&link, asked, RouteKind::Open).ok_or_else(|| anyhow!("the bridge on {} could not be asked for a terminal", at.address()))?;
    let opened = rx.from.recv_timeout(OPEN_WAIT).map_err(|_| anyhow!("the bridge on {} did not open a terminal", at.address()))?;
    if opened["did"] != "opened" {
        bail!("the bridge on {} could not open a terminal: {}", at.address(), opened["why"].as_str().unwrap_or("no reason given"));
    }
    let term = opened["term"].as_u64().unwrap_or(0);
    let generation = opened["gen"].as_str().unwrap_or_default().to_string();
    crate::append_hook_log(&format!("far terminal {term} ({generation}) opened on {} for {tab}", at.address()));
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
    /// On this PC, the processes its job holds, as last told
    procs: Mutex<Option<Vec<u32>>>,
    /// On this PC, the program's first process (0 until told)
    root: AtomicU64,
    /// On this PC, how many of its processes are the program at rest, as an
    /// app that owned it learned and the resident process kept (0 unknown)
    rest: AtomicU64,
    /// What it does while this app is away (far-keep plan §4.3): changed
    /// when the person changes the machine's setting
    away: Mutex<crate::config::Away>,
    owner: AtomicU64,
    /// What was typed into the tab before its terminal there was attached
    /// to: sent, in order, the moment it is ([`FarTerm::owned_by`]).
    ///
    /// A tab is on screen, and taking keys, before the terminal it goes back
    /// to or opens in place of one that ended has answered -- a second or two
    /// at a start, longer on a line that is slow to come up. What was typed
    /// then went with no owner, which the resident process refuses: the
    /// person's first words to a tab after a start vanished without a trace
    held_in: Mutex<Vec<u8>>,
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
            procs: Mutex::new(None),
            root: AtomicU64::new(0),
            rest: AtomicU64::new(0),
            away: Mutex::new(away),
            owner: AtomicU64::new(0),
            held_in: Mutex::new(Vec::new()),
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

    /// Attached to, as `owner`: what was typed while it was not goes in now,
    /// ahead of anything typed after. The owner is set under the same lock
    /// the writer reads it under, and what was held is sent before that lock
    /// is let go of -- so no key typed from here on can overtake it.
    ///
    /// When the line went in that moment, what was held is kept, and the
    /// terminal is taken as not attached to: what is typed next is kept
    /// behind it, and all of it goes in at the next attach -- the first words
    /// after a start are what this is for, and a line that blinks would
    /// otherwise lose them after all
    fn owned_by(&self, owner: u64) {
        let mut held = self.held_in.lock().unwrap_or_else(|e| e.into_inner());
        self.owner.store(owner, Ordering::SeqCst);
        if owner == 0 || held.is_empty() {
            return;
        }
        if !self.say(json!({ "do": "in", "term": self.term(), "owner": owner, "b": b64(&held) })) {
            self.owner.store(0, Ordering::SeqCst);
            return;
        }
        held.clear();
    }

    /// What is run on this PC, said in full (`ask_open`)
    fn with_run(mut self, run: Option<Value>) -> Self {
        self.run = run;
        self
    }

    /// The processes its job holds, as the resident process last said: on
    /// this PC only, and `None` until it has said
    pub fn procs(&self) -> Option<Vec<u32>> {
        self.procs.lock().ok().and_then(|p| p.clone())
    }

    /// The program's first process, on this PC, once the resident process
    /// has said it: what its ports and its use of the machine are read below
    pub fn root(&self) -> Option<u32> {
        u32::try_from(self.root.load(Ordering::SeqCst)).ok().filter(|p| *p != 0)
    }

    /// How many of its processes are its program at rest, when an app that
    /// owned it learned that before: what this app counts its work behind the
    /// prompt against, instead of learning it again with that work running
    pub fn rest(&self) -> Option<u32> {
        u32::try_from(self.rest.load(Ordering::SeqCst)).ok().filter(|n| *n != 0)
    }

    /// Hand the resident process what this app learned of the program at
    /// rest, for the app after it
    pub fn learned_rest(&self, n: u32) {
        if self.rest.swap(u64::from(n), Ordering::SeqCst) != u64::from(n) {
            self.say(json!({ "do": "rest", "term": self.term(), "owner": self.owner.load(Ordering::SeqCst), "n": n }));
        }
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

    /// Its id at the resident process (0 until it is opened there)
    pub fn term_id(&self) -> u64 {
        self.term()
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

    fn say(&self, mut m: Value) -> bool {
        if let Ok(id) = self.ident.lock() {
            id.stamp(&mut m);
        } else {
            return false;
        }
        self.link.lock().ok().and_then(|l| l.clone()).is_some_and(|l| l.to_job(JOB, m))
    }

    fn attach_message(&self) -> Value {
        let (rows, cols) = self.size.lock().map(|s| *s).unwrap_or((24, 80));
        json!({ "do": "attach", "term": self.term(), "gen": self.generation(), "tab": self.tab, "rows": rows, "cols": cols,
            "away": on_the_line(&self.at, self.away()) })
    }

    /// The line is up: be routed its messages again and attach, which hands
    /// the state over. `None` while the line is not up yet
    fn attach_again(&self) -> Option<Messages> {
        let link = self.at.link().filter(|l| l.holds(JOB))?;
        let r = router(&self.at, &link);
        let rx = r.attach(&link, self.attach_message())?;
        if let Ok(mut l) = self.link.lock() {
            *l = Some(Arc::clone(&link));
        }
        Some(rx)
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

fn made(term: Arc<FarTerm>, from: Messages, again: bool, fresh: bool) -> Opened {
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
        waiting_since: None,
        ack_until: None,
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
    crate::append_hook_log(&format!("far terminal {} ({}): going back to it on {} for {}", saved.term, saved.generation, at.address(), saved.tab));
    let term = Arc::new(FarTerm::new(at, Ident { term: saved.term, generation: saved.generation }, &saved.tab, (rows, cols), (cwd, then), away).with_run(run));
    if let Ok(mut l) = term.left.lock() {
        *l = saved.left;
    }
    term.stopping.store(saved.stopping, Ordering::SeqCst);
    // Read as a line that went: the reader attaches as soon as it is up
    made(term, Messages::gone(), true, false)
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
    made(term, Messages::gone(), true, true)
}

/// What comes from the terminal there, as bytes to read: its output, and
/// any state handed over put into the tab's parser on the way
struct FarReader {
    from: Messages,
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
    /// One deadline across retries, including a connected peer which never
    /// acknowledges an attach. Reset only after its state is received.
    waiting_since: Option<std::time::Instant>,
    ack_until: Option<std::time::Instant>,
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
                self.attached = false;
                self.ack_until = None;
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
            let received = if self.attached {
                self.from.from.recv().map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                let until = *self.ack_until.get_or_insert_with(|| std::time::Instant::now() + OPEN_WAIT);
                self.waiting_since.get_or_insert_with(std::time::Instant::now);
                let now = std::time::Instant::now();
                if now >= until { Err(RecvTimeoutError::Timeout) } else { self.from.from.recv_timeout(until - now) }
            };
            let Ok(m) = received else {
                if matches!(received, Err(RecvTimeoutError::Timeout)) {
                    crate::append_hook_log(&format!("far terminal {}: no attach acknowledgement; retrying", self.term.term()));
                }
                self.from = Messages::gone();
                self.attached = false;
                self.ack_until = None;
                self.term.owner.store(0, Ordering::SeqCst);
                // The line went (or, gone back to after a start, is not up
                // yet). The terminal there may well still be there: wait for
                // the line to come back and attach to it again
                if self.term.ended.load(Ordering::SeqCst) || self.term.let_go.load(Ordering::SeqCst) {
                    return Ok(0);
                }
                // This PC's resident process was asked to end everything it
                // held (the person's "Stop them"): its terminals are over, and
                // there is no line to wait for
                if self.term.here() && crate::localkeep::ended_on_purpose() {
                    self.term.ended.store(true, Ordering::SeqCst);
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
                let until = *self.waiting_since.get_or_insert_with(std::time::Instant::now) + LINE_BACK_WAIT;
                let again = loop {
                    if self.term.let_go.load(Ordering::SeqCst) {
                        return Ok(0);
                    }
                    if std::time::Instant::now() >= until {
                        break None;
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
                    None => {
                        self.say_last(&crate::i18n::tp("msg.farterm.not_reached", &[("host", &address)]));
                        continue;
                    }
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
                    self.waiting_since = None;
                    self.ack_until = None;
                    self.again = false;
                    self.term.owned_by(m["owner"].as_u64().unwrap_or(0));
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
                    self.attached = false;
                    self.ack_until = None;
                    self.term.owner.store(0, Ordering::SeqCst);
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
                // The processes the terminal's job holds (this PC's resident
                // process says so when it changes): what the tab counts as
                // its work in the background, as it would for its own job
                "procs" => {
                    // What was learned of the program at rest first: the tab
                    // reads it once it has processes to count, never before
                    if let Some(rest) = m["rest"].as_u64() {
                        self.term.rest.store(rest, Ordering::SeqCst);
                    }
                    if let (Ok(mut p), Some(list)) = (self.term.procs.lock(), m["pids"].as_array()) {
                        *p = Some(list.iter().filter_map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok())).collect());
                    }
                    if let Some(root) = m["root"].as_u64().and_then(|n| u32::try_from(n).ok()) {
                        self.term.root.store(u64::from(root), Ordering::SeqCst);
                    }
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

/// The most typed before a terminal is attached to that is kept for it: a
/// paste or two, not a stream nobody is reading
const HELD_IN_MOST: usize = 1 << 20;

impl std::io::Write for FarWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // Not attached to yet: kept for the moment it is (`FarTerm::owned_by`).
        // Checked under the lock the flush takes, so nothing typed between
        // the owner arriving and the flush is left behind it
        let owner = {
            let mut held = self.term.held_in.lock().unwrap_or_else(|e| e.into_inner());
            let owner = self.term.owner.load(Ordering::SeqCst);
            if owner == 0 && !self.term.ended.load(Ordering::SeqCst) {
                if held.len() + buf.len() > HELD_IN_MOST {
                    return Err(std::io::Error::other("the terminal there has not been reached yet"));
                }
                held.extend_from_slice(buf);
                return Ok(buf.len());
            }
            owner
        };
        let said = self.term.say(json!({ "do": "in", "term": self.term.term(), "owner": owner, "b": b64(buf) }));
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
    #[test]
    fn what_was_typed_before_an_attach_is_kept_when_the_line_goes_as_it_is_sent() {
        use std::io::Write as _;
        let term = std::sync::Arc::new(super::FarTerm::new(
            &super::Place::Here,
            super::Ident { term: 7, generation: "g".into() },
            "otter",
            (24, 80),
            (None, None),
            crate::config::Away::Stop,
        ));
        let mut w = super::FarWriter { term: std::sync::Arc::clone(&term) };
        w.write_all(b"first words").unwrap();
        // Attached to with no line to send them on: kept, not lost
        term.owned_by(5);
        assert_eq!(term.owner.load(std::sync::atomic::Ordering::SeqCst), 0, "taken as attached with nothing sent");
        w.write_all(b" and more").unwrap();
        assert_eq!(&*term.held_in.lock().unwrap(), b"first words and more", "what was typed went missing or out of order");
    }

    use super::*;

    fn test_router(identities: bool) -> Arc<Router> {
        let peer = Peer { generation: "new".into(), identities };
        Arc::new(Router {
            link: Weak::new(),
            routes: Mutex::new(Routes { peer: Some(peer.clone()), ..Default::default() }),
            peer: OnceLock::from(peer),
            checking_peer: Mutex::new(()),
        })
    }

    fn delivered(r: &Router, m: Value) {
        r.routes.lock().unwrap().deliver(m);
    }

    fn recv(messages: &Messages) -> Value {
        messages.from.recv_timeout(Duration::from_secs(1)).expect("the intended recipient heard it")
    }

    /// A real Link over a loopback socket, with the far end controlled by
    /// the test so replies and connection shutdowns can be ordered exactly.
    struct Wire {
        at: Place,
        server: std::net::TcpStream,
        reader: std::io::BufReader<std::net::TcpStream>,
        link: Arc<crate::farlink::Link>,
    }

    impl Wire {
        fn new(at: Option<Place>) -> Self {
            use std::io::Write as _;
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            let (mut server, _) = listener.accept().unwrap();
            server.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            writeln!(server, "{}", json!({ "t": "hello", "version": "test", "rev": "test", "jobs": [JOB] })).unwrap();
            let at = at.unwrap_or_else(|| {
                Place::Far(crate::elsewhere::Elsewhere::Ssh(crate::ssh::Spec {
                    host: format!("routing-{}.invalid", crate::random_hex(8)),
                    ..Default::default()
                }))
            });
            let link = crate::farlink::link_over(
                &at.machine_key(),
                &at.address(),
                Box::new(client.try_clone().unwrap()),
                client.try_clone().unwrap(),
                Some(client),
                None,
                String::new(),
            )
            .unwrap();
            Self { at, reader: std::io::BufReader::new(server.try_clone().unwrap()), server, link }
        }

        fn hear(&mut self) -> Value {
            use std::io::BufRead as _;
            loop {
                let mut line = String::new();
                assert!(self.reader.read_line(&mut line).unwrap() > 0);
                if let crate::farlink::Frame::Job { m, .. } = serde_json::from_str(&line).unwrap() {
                    return m;
                }
            }
        }

        fn say(&mut self, m: Value) {
            use std::io::Write as _;
            writeln!(self.server, "{}", json!({ "t": "job", "job": JOB, "m": m })).unwrap();
        }
    }

    impl Drop for Wire {
        fn drop(&mut self) {
            let _ = self.server.shutdown(std::net::Shutdown::Both);
            if crate::farlink::link_by_key(&self.at.machine_key()).is_some_and(|l| Arc::ptr_eq(&l, &self.link)) {
                crate::farlink::let_go_key(&self.at.machine_key());
            }
        }
    }

    #[test]
    fn a_replaced_connections_late_messages_and_cleanup_stay_on_that_connection() {
        let mut old = Wire::new(None);
        let old_router = router(&old.at, &old.link);
        let old_messages = old_router.register(RouteKind::Reply);
        let mut new = Wire::new(Some(old.at.clone()));
        let new_router = router(&new.at, &new.link);
        assert!(!Arc::ptr_eq(&old_router, &new_router));
        let new_messages = new_router.register(RouteKind::Reply);
        old.say(json!({ "did": "list", "ref": new_messages.reference, "gen": "old" }));
        old.server.shutdown(std::net::Shutdown::Both).unwrap();
        assert_eq!(old_messages.from.recv_timeout(Duration::from_secs(2)), Err(RecvTimeoutError::Disconnected));
        assert!(Arc::ptr_eq(&new_router, &router(&new.at, &new.link)), "old cleanup removed the replacement");
        new.say(json!({ "did": "list", "ref": new_messages.reference, "gen": "new" }));
        assert_eq!(recv(&new_messages)["gen"], "new");
    }

    #[test]
    fn a_live_legacy_peer_reattaches_only_its_current_lifetime() {
        let mut wire = Wire::new(None);
        let r = router(&wire.at, &wire.link);
        std::thread::scope(|scope| {
            let link = Arc::clone(&wire.link);
            let r = Arc::clone(&r);
            let checked = scope.spawn(move || r.peer(&link).unwrap().clone());
            let request = wire.hear();
            assert_eq!(request["do"], "list");
            wire.say(json!({ "did": "list", "ref": request["ref"], "gen": "new", "terms": [] }));
            assert!(!checked.join().unwrap().identities);
        });
        let unknown = r.attach(&wire.link, json!({ "do": "attach", "gen": "old", "term": 6, "tab": "a" })).unwrap();
        assert_eq!(recv(&unknown)["did"], "unknown");
        let current = r.attach(&wire.link, json!({ "do": "attach", "gen": "new", "term": 6, "tab": "b" })).unwrap();
        // No request about old/6 crossed the line.
        let request = wire.hear();
        assert_eq!(request["gen"], "new");
        wire.say(json!({ "did": "attached", "term": 6, "owner": 1 }));
        wire.say(json!({ "did": "out", "term": 6, "b": "right" }));
        assert_eq!(recv(&current)["did"], "attached");
        assert_eq!(recv(&current)["b"], "right");
    }

    /// The observed restart: a newly opened tab is given 6 while another
    /// tab is still asking about 6 of the previous resident process. Its
    /// answer may precede, interrupt, or follow the new screen's delivery.
    #[test]
    fn a_previous_generations_end_never_closes_a_new_tabs_screen() {
        for position in 0..=3 {
            let r = test_router(true);
            let current = r.register(RouteKind::Open);
            let old = r.register(RouteKind::Attach(Ident { term: 6, generation: "old".into() }));
            let mut frames = vec![
                json!({ "did": "opened", "ref": current.reference, "term": 6, "gen": "new" }),
                json!({ "did": "attached", "term": 6, "gen": "new", "owner": 1 }),
                json!({ "did": "out", "term": 6, "gen": "new", "b": "the right tab" }),
            ];
            frames.insert(position, json!({ "did": "over", "ref": old.reference, "term": 6, "gen": "old", "code": -1 }));
            for frame in frames {
                delivered(&r, frame);
            }
            assert_eq!(recv(&old)["did"], "over");
            assert_eq!(old.from.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected));
            for did in ["opened", "attached", "out"] {
                assert_eq!(recv(&current)["did"], did);
            }
            assert_eq!(current.from.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty));
            // Even unsolicited or late messages of that lifetime go nowhere.
            delivered(&r, json!({ "did": "ended", "term": 6, "gen": "old" }));
            delivered(&r, json!({ "did": "over", "ref": old.reference, "term": 6, "gen": "new" }));
            assert_eq!(current.from.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty));
        }
    }

    #[test]
    fn replies_timeouts_and_closed_tabs_do_not_steal_or_keep_streams() {
        let r = test_router(true);
        let stream = r.register(RouteKind::Open);
        delivered(&r, json!({ "did": "opened", "ref": stream.reference, "term": 2, "gen": "new" }));
        recv(&stream);
        let query = r.register(RouteKind::Reply);
        delivered(&r, json!({ "did": "ending", "ref": query.reference, "term": 2, "gen": "new" }));
        assert_eq!(recv(&query)["did"], "ending");
        let late = r.register(RouteKind::Open);
        let reference = late.reference;
        drop(late); // The request timed out, or its send failed.
        delivered(&r, json!({ "did": "opened", "ref": reference, "term": 2, "gen": "new" }));
        delivered(&r, json!({ "did": "out", "term": 2, "gen": "new" }));
        assert_eq!(recv(&stream)["did"], "out");
        drop(stream);
        let routes = r.routes.lock().unwrap();
        assert!(routes.by_term.is_empty() && routes.by_ref.is_empty(), "abandoned registrations are retained");
    }

    #[test]
    fn an_attach_needs_the_requested_identity_and_a_dead_line_releases_every_waiter() {
        let r = test_router(true);
        let attach = r.register(RouteKind::Attach(Ident { term: 2, generation: "new".into() }));
        delivered(&r, json!({ "did": "attached", "ref": attach.reference, "term": 2, "gen": "old" }));
        delivered(&r, json!({ "did": "attached", "ref": attach.reference, "term": 3, "gen": "new" }));
        assert!(attach.from.try_recv().is_err());
        delivered(&r, json!({ "did": "attached", "ref": attach.reference, "term": 2, "gen": "new" }));
        assert_eq!(recv(&attach)["did"], "attached");
        let query = r.register(RouteKind::Reply);
        r.close();
        for rx in [&attach, &query] {
            assert_eq!(rx.from.try_recv(), Err(std::sync::mpsc::TryRecvError::Disconnected));
        }
    }

    #[test]
    fn only_legacy_peers_may_omit_the_generation_on_current_output() {
        for legacy in [false, true] {
            let r = test_router(!legacy);
            let stream = r.register(RouteKind::Open);
            delivered(&r, json!({ "did": "opened", "ref": stream.reference, "term": 1, "gen": "new" }));
            recv(&stream);
            delivered(&r, json!({ "did": "out", "term": 1 }));
            assert_eq!(stream.from.try_recv().is_ok(), legacy);
            delivered(&r, json!({ "did": "out", "term": 1, "gen": "old" }));
            assert!(stream.from.try_recv().is_err());
        }
    }

    #[test]
    fn a_reconnect_deadline_ends_the_wait_without_claiming_the_ai_ended() {
        use std::io::Read as _;
        let at = Place::Far(crate::elsewhere::Elsewhere::Ssh(crate::ssh::Spec { host: "no-connection.invalid".into(), ..Default::default() }));
        let term = Arc::new(FarTerm::new(&at, Ident { term: 6, generation: "old".into() }, "t", (24, 80), (None, None), crate::config::Away::Always));
        let mut reader = FarReader {
            from: Messages::gone(),
            term: term.clone(),
            seen: 0,
            rest: Vec::new(),
            at: 0,
            again: true,
            fresh: false,
            attached: false,
            said_waiting: true,
            last: false,
            waiting_since: Some(std::time::Instant::now() - LINE_BACK_WAIT),
            ack_until: None,
        };
        let mut buf = [0; 4096];
        let n = reader.read(&mut buf).unwrap();
        assert!(n > 0 && reader.last, "the person was not told the wait ended");
        assert_eq!(reader.read(&mut buf).unwrap(), 0);
        assert!(!term.ended.load(Ordering::SeqCst), "a missing answer was mistaken for an ended AI");
    }

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
        // Written by an older version, under the name: the tab that was there
        // then, and not one made since under the same name
        let then = crate::config::derived_tab_uid("work", "tiger");
        assert_eq!(written_for(&book, "pi", "/old", &then, "tiger").map(|s| s.term), Some(3));
        assert_eq!(written_for(&book, "pi", "/old", me, "tiger"), None, "a tab made since was handed an old terminal");
        // A uid is never matched as a name
        assert!(!is_this_tab(other, &then, other));
        assert!(is_this_tab("tiger", &then, "tiger") && is_this_tab(me, me, "tiger"));
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

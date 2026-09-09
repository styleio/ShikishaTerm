//! Remote UI for monitoring/instructing from a phone etc. DESIGN.md ch. 10.4.
//!
//! Rather than reproducing the terminal screen as-is, this focuses on
//! "look at the situation, give a one-line instruction." The implementation
//! just returns existing material (state detection, response capture, screen
//! text) as JSON — no WebSocket, no terminal emulator needed.
//!
//! Safety:
//!   - Disabled by default. Only listens when explicitly enabled in settings
//!   - Listening is restricted to private networks (netaddr.rs)
//!   - Requires a 32-byte token. Constant-time comparison
//!   - Remote input is treated as "human operation" (resets the auto chain)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use serde::Serialize;
use tiny_http::{Header, Response, Server};

/// State of a tab as shown on screen (updated from the main loop every tick)
#[derive(Clone, Serialize, Default)]
pub struct RemoteTab {
    pub index: usize,
    pub name: String,
    pub state: String,
    pub locked: bool,
    /// Latest response (or the tail of the screen if there is none)
    pub output: String,
    /// Screen contents while waiting for confirmation (to read the choices)
    pub screen: String,
    /// The tab's working folder (absolute). Where a pasted/attached file is saved
    /// so the AI running here can reach it. Empty if the tab has no folder set.
    #[serde(default)]
    pub cwd: String,
    /// Which conversation, and the pattern that finds its record. Never sent:
    /// an id names a conversation, and the phone has no use for one — it asks
    /// by tab number and the path is resolved on this side. Kept here because
    /// this snapshot is the only thing the request thread can see
    #[serde(skip)]
    pub record_id: String,
    #[serde(skip)]
    pub record_glob: String,
}

#[derive(Clone, Serialize, Default)]
pub struct Snapshot {
    /// Same state as the window. The phone draws the same page too
    #[serde(default)]
    pub ui: Option<crate::uistate::UiState>,
    /// Screen of the tab being viewed (colored HTML)
    #[serde(default)]
    pub screen_html: String,
    pub workspace: String,
    pub tabs: Vec<RemoteTab>,
    pub auto_enabled: bool,
    /// Terminal column count. The screen is drawn at this width, so
    /// the phone side uses it to pick a font size that fits without wrapping
    pub cols: u16,
}

/// Operations arriving from remote. Executed on the main loop
#[derive(Debug)]
pub enum RemoteCmd {
    /// Send an instruction to a tab (treated as human input)
    Send { tab: usize, text: String },
    /// Raw keys, e.g. an answer to a confirmation
    Keys { tab: usize, keys: String },
    /// An answer typed on a reply page, which a notification linked to.
    ///
    /// Carries the tab's own id as well as its number because the two can
    /// disagree by the time somebody answers: tabs are added and closed while
    /// a phone sits in a pocket, and a "yes" delivered to whatever is now
    /// third in the list is worse than one that arrives nowhere. `dest` is
    /// where to report back that it landed
    Reply {
        tab_id: Option<String>,
        tab: usize,
        name: String,
        dest: String,
        text: String,
    },
    /// Emergency stop / resume of automation
    SetAuto(bool),
    /// Operation from the screen (switch tab, menu, keystroke).
    /// Treated the same as one coming from the window, entering the same queue
    Ui(crate::browser::Ev),
}

/// Whether an operation is accepted from the phone.
///
/// Since the same page is served, in principle it can send the same intents
/// as the window. But some only make sense in front of the window, and some
/// would stop the window. This is written from the side that enumerates
/// what's let through. Add to it only after writing down the reason
fn allowed_from_afar(ev: &crate::browser::Ev) -> bool {
    use crate::browser::Ev;
    match ev {
        // Pick/type into/stop the tab you want to view. The core of remote control
        Ev::Select { .. } | Ev::Key { .. } | Ev::Stop => true,
        // Relaunching the tab you are looking at. A phone watching an SSH tab that
        // dropped is exactly who needs this, and it reaches no further than the
        // keystroke (Ctrl+B r) it stands for — one tab, the one on screen.
        Ev::Restart => true,
        // The same act, named by pane instead of implied. It reaches no
        // further than Ev::Restart does -- one pane, one tab -- and refusing it
        // would mean the same button worked or didn't depending on which door
        // it was pressed from
        Ev::RestartPane { .. } => true,
        // Input into the relay screen (finger trail / swipe / characters). The
        // heart of remote control, so let it through
        Ev::Inject { .. } => true,
        // Back/forward/reload/navigate. Since this remotely controls a browser,
        // the buttons on the top bar need to work too, or it's only half done.
        // It only changes the destination; it doesn't stop the window
        Ev::Go { .. } => true,
        // Copy the selected text. Keep the same manners as the window (same as PuTTY)
        Ev::Copy { .. } => true,
        // Entries only the window can carry out. The board keeps the same list
        // (crate::shell::WINDOW_ONLY_MENU), so what it offers from afar and what
        // this lets through can't drift apart. Settings is on that list yet still
        // reachable from a phone: the board doesn't send a keystroke for it, it
        // walks to the reverse-proxied /cfg page instead.
        Ev::Menu { key } => !crate::shell::WINDOW_ONLY_MENU.contains(&key.as_str()),
        // Opening the workspace switcher is allowed (it was allowed before as
        // Menu "w"). It only shows the list; picking a workspace is a separate
        // digit intent, so this alone doesn't disrupt the window.
        Ev::OpenWs => true,
        // Finishing a line in the composer from the phone is just like typing
        // into the tab it names.
        Ev::Say { .. } => true,
        // Firing one of the user's own quick actions (its Lua runs sandboxed).
        // No different in reach than typing the same instruction from the phone.
        Ev::RunAction { .. } => true,
        // Aiming the active AI at another tab and handing it a goal — the same
        // reach as typing that instruction into the AI from the phone.
        Ev::Operate { .. } => true,
        // 📼 arming the page recorder and ▶ running composer Lua. Both stay
        // inside the run_scoped jail on the shown browser — no more reach than
        // the input injection and quick actions already allowed above.
        Ev::Record { .. } => true,
        Ev::RunLua { .. } => true,
        // The git panel. Everything it can ask for is something the person can
        // do on this PC by opening the same panel, and a phone that can hand an
        // AI a whole task can certainly stage a file
        Ev::Git { .. } => true,
        // ✨ command suggestion: the reply is a draft the person still has to
        // send — same reach as typing the command from the phone themselves
        Ev::Suggest { .. } => true,
        // 🔍 the environment survey types a fixed read-only probe — the same
        // reach as the person typing that probe from the phone
        Ev::Survey => true,
        // Searching past work is a read, and reopening one is the phone asking
        // for a tab it could have asked for by hand -- the same reach as adding
        // a tab, which the person does from their own device all the time
        Ev::VaultSearch { .. } | Ev::VaultOpen { .. } => true,
        // The command palette runs an action by name -- no more reach than
        // pressing the key it stands for, which the phone can already do
        Ev::RunKey { .. } => true,
        // Scrolling back through the history is the whole point of monitoring
        // from afar — without it the phone is stuck on the current screen and
        // can't review what was said earlier. It only moves the viewport, never
        // injecting input, and scroll_by() already routes it correctly: into a
        // full-screen TUI's own scroll (Claude Code) or our kept scrollback (a
        // plain shell). Typing returns to the live screen, as it does at the window.
        Ev::Scroll { .. } => true,
        // Fit the terminal to whoever is actually looking. This is a single-person
        // setup, and the phone is often the one being used — a terminal sized to the
        // window is clipped on a phone, with its bottom input line and newest output
        // off-screen. Each side re-reports only when ITS OWN measured size changes,
        // so they don't fight in a loop: they hand off, the side that last actually
        // changed size wins, and the other holds until it changes.
        Ev::Resize { .. } => true,
        // Giving a branch its own folder, and putting a working folder back on
        // this machine when it is not here. Both run git against a folder the
        // person already works in, and both show what will run before it runs.
        //
        // Allowed from afar because a working folder that is missing is
        // precisely the thing somebody discovers while away from the PC -- and
        // because the phone can already type anything at all into an agent
        // running in that folder, which is more reach, not less. Refusing here
        // would mean the answer to "my folder is gone" is "wait until you are
        // home", which is not an answer this app is allowed to give.
        Ev::Branch { .. } | Ev::Repair { .. } => true,
        // Closing the first-run pointer. It is drawn on the phone as well, and
        // a pointer that cannot be closed from where it is seen is a nag
        Ev::Coach { .. } => true,
        // Answering the card that says a newer version is out. It installs
        // nothing; it opens the settings' Update card, which the phone reaches
        // as /cfg, or puts the card away
        Ev::Update { .. } => true,
        // Putting away a tab's usage-limit notice. Reading it is the whole act
        Ev::LimitAck { .. } => true,
        // Walking this PC's folders to open another one. The list exists
        // precisely because a phone has no folder dialog of its own
        // (uistate::BrowseState) -- refusing it here left the phone a dialog
        // that opened and stayed empty. Looking reads folder names; choosing
        // writes one folder into the settings, which is what adding a tab from
        // the phone already does.
        Ev::Browse { .. } => true,
        // Paste stays local — one long-press would flow straight into the AI's input box
        _ => false,
    }
}

/// Destinations for relay frames (one per connected WS client).
/// A line that can no longer send is cleaned up on the next frame
type FrameClients = Arc<Mutex<Vec<Sender<Vec<u8>>>>>;
/// Destinations for state pushes — the terminal screen and UI, sent over a
/// WebSocket instead of the phone polling. One text sender per connected viewer.
type StateClients = Arc<Mutex<Vec<Sender<String>>>>;

pub struct RemoteUi {
    pub url: String,
    /// The origin (scheme://host:port) without the token, kept so `url` can be
    /// rebuilt when the token is rotated. Not necessarily where the server is
    /// listening -- see `reached_at`.
    origin: String,
    /// The port actually bound, which is not always the port asked for (0 means
    /// "any"). Kept because the address the world reaches this by can change
    /// while the port cannot.
    port: u16,
    /// The same board, answering on 127.0.0.1 as well, so a proxy on this
    /// machine can reach it. `None` when the board is already on the loopback,
    /// or when something else holds that port there.
    loopback: Mutex<Option<Arc<Server>>>,
    loopback_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// The access token, shared with the server thread's request handlers so a
    /// runtime rotation takes effect immediately.
    token: Arc<Mutex<String>>,
    pub note: Option<String>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    pub rx: Receiver<RemoteCmd>,
    stop: Arc<AtomicBool>,
    /// Destinations for relay frames. JPEGs arriving from the browser flow here
    frame_clients: FrameClients,
    /// Destinations for state pushes (screen HTML / UI JSON) over /ws-state
    state_clients: StateClients,
    /// When a viewer last asked for the state over plain HTTP. Watching is
    /// normally a held-open state socket, but a phone whose network won't carry
    /// one falls back to asking for the state every second and a half, and it is
    /// watching just as much -- `watched` counts both.
    last_poll: Arc<Mutex<Option<Instant>>>,
    /// A new viewer joined. The main loop will emit one frame of the current
    /// screen at the next opportunity (so a static page doesn't stay blank
    /// waiting for the next change)
    keyframe_wanted: Arc<AtomicBool>,
    /// The local settings web server to reverse-proxy the phone's `/cfg` (and the
    /// settings `/api/*`) to, as (origin, token). The config UI stays bound to
    /// loopback and never faces the network — the phone reaches it only through
    /// this proxy, authenticated by the remote token. None until it is up.
    settings: Arc<Mutex<Option<(String, String)>>>,
    /// The HTTP server itself (owns the listening socket). Held so shutdown()
    /// can unblock the accept loop and close the socket; taken (dropped) there
    server: Mutex<Option<Arc<Server>>>,
    /// The accept thread, joined in shutdown() so the port is truly released
    /// before shutdown() returns
    accept_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// What a phone must hold besides the token, and where a cut lands (see Gate)
    gate: Arc<Gate>,
    /// Tickets for the reply pages a notification can link to. Held here
    /// because they die with the run, and because "disconnect" ends them
    /// along with every phone
    book: Arc<crate::reply::Book>,
}

/// How many sessions are remembered at once. Every re-pairing mints one and
/// only a cut removes them, so the list is bounded rather than left to grow for
/// the life of the app; the oldest falls off first. Re-presenting a session that
/// is still valid keeps it (see `Ids::keep`), so reloading a page does not use
/// the list up — it takes that many *different* devices to push one out.
const MAX_SESSIONS: usize = 64;

/// A bounded set of session ids.
struct Ids(Mutex<std::collections::VecDeque<String>>);

impl Ids {
    fn new() -> Self {
        Self(Mutex::new(std::collections::VecDeque::new()))
    }

    /// Whether this id is one we handed out and have not since cut
    fn has(&self, id: &str) -> bool {
        !id.is_empty()
            && self
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|k| crate::crypto::token_eq(k, id))
    }

    /// The id the caller should hold from here on: the one it presented if that
    /// is still valid (so a reload keeps its place in the list), a fresh one
    /// otherwise.
    fn keep(&self, presented: &str) -> String {
        if self.has(presented) {
            return presented.to_string();
        }
        let id = crate::random_hex(24);
        let mut q = self.0.lock().unwrap();
        while q.len() >= MAX_SESSIONS {
            q.pop_front();
        }
        q.push_back(id.clone());
        id
    }

    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

/// Everything a phone must present beyond the token — and the single place the
/// window's "disconnect" takes effect.
///
/// The token alone cannot carry a disconnect: it may be a fixed string the
/// person wrote into settings, and even when it is rotated, a page already
/// loaded still holds sockets that were authorised at the time they opened. So
/// admission is a *session*, minted when the pairing link is opened (the shell
/// page with a valid `?t=`) and required by every data route and every live
/// socket afterwards. Cutting empties the list: the phone's very next request —
/// or its very next touch on the relay — is refused, whatever the token says.
///
/// The password is the optional second factor. The URL token is convenient but
/// travels through notification channels in plain text; a password — entered
/// once per app run on the phone itself — never does. Empty password = the gate
/// is off (the user's own risk to accept).
struct Gate {
    password: String,
    /// Sessions handed to phones that presented the password
    pw: Ids,
    /// Sessions handed to phones that opened the pairing link
    grants: Ids,
    /// Wrong passwords in a row, and when the last one was.
    ///
    /// One score for every door the password has — the board's `/auth` and
    /// the reply page's unlock — because they open the same password, and a
    /// guesser turned away at one must not walk round to the other. The board's
    /// door sits behind the access token, so reaching it at all means holding a
    /// full-machine credential; a reply link is a short ticket posted into
    /// chat rooms on purpose. Neither may be tried at machine speed.
    misses: Mutex<Misses>,
}

/// The score of wrong passwords, and what it costs.
///
/// The first two slips cost nothing — that is a person mistyping. The third
/// shuts the door for a second, and from there the wait doubles, up to a
/// minute; ten quiet minutes forget the score. Guessing is thereby held to
/// about one try a minute, while a person who mistyped three times is in after
/// a second. Nobody is made to sleep: the answer is "come back in n seconds",
/// so the listener — one thread, serving every phone — keeps serving the phone
/// that is already in. (It used to sleep the thread; a guesser then slowed
/// everybody's screen along with their own.)
///
/// Time is handed in rather than read, so the ladder can be tested without
/// waiting on it
struct Misses {
    count: u32,
    last: Instant,
}

impl Misses {
    /// Wrong passwords in a row that cost nothing
    const FREE: u32 = 2;
    /// The longest the door stays shut after one wrong password
    const LONGEST: Duration = Duration::from_secs(60);
    /// How long a score is remembered after the last wrong password
    const FORGOTTEN_AFTER: Duration = Duration::from_secs(600);

    fn new(now: Instant) -> Self {
        Self { count: 0, last: now }
    }

    /// How long the door stays shut after `count` wrong passwords in a row
    fn shut_for(count: u32) -> Duration {
        if count <= Self::FREE {
            return Duration::ZERO;
        }
        let doublings = (count - Self::FREE - 1).min(16);
        Duration::from_secs(1u64 << doublings).min(Self::LONGEST)
    }

    /// How much longer the door is shut at `now`, or `None` if a try is welcome
    fn wait_at(&mut self, now: Instant) -> Option<Duration> {
        if now.saturating_duration_since(self.last) > Self::FORGOTTEN_AFTER {
            self.count = 0;
        }
        let shut = Self::shut_for(self.count);
        let since = now.saturating_duration_since(self.last);
        (since < shut).then(|| shut - since)
    }

    /// One more wrong password at `now`
    fn missed_at(&mut self, now: Instant) {
        if now.saturating_duration_since(self.last) > Self::FORGOTTEN_AFTER {
            self.count = 0;
        }
        self.count = self.count.saturating_add(1);
        self.last = now;
    }

    /// A right password clears the score
    fn forgive(&mut self) {
        self.count = 0;
    }
}

impl Gate {
    /// Whether the caller still holds a live session from opening the link
    fn granted(&self, id: &str) -> bool {
        self.grants.has(id)
    }

    /// Whether the password factor is satisfied (always, when none is set)
    fn unlocked(&self, id: &str) -> bool {
        self.password.is_empty() || self.pw.has(id)
    }

    /// How much longer no password will be looked at, or `None` if one may be
    /// offered now. Asked before the password is compared, so a right one
    /// offered during the wait is turned away too — that is what makes the
    /// wait count against a guesser
    fn wait(&self) -> Option<Duration> {
        self.misses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .wait_at(Instant::now())
    }

    /// Score a wrong password
    fn missed(&self) {
        self.misses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .missed_at(Instant::now());
    }

    /// A right password clears the score
    fn forgive(&self) {
        self.misses.lock().unwrap_or_else(|e| e.into_inner()).forgive();
    }

    /// The disconnect. Every session is gone, so nothing that was let in
    /// before this moment is let in again without opening the link afresh.
    fn cut(&self) {
        self.grants.clear();
        self.pw.clear();
    }
}

/// The pairing session cookie (see Gate). HttpOnly so the page it admits can
/// never read it out, SameSite=Strict so nothing but this origin can send it.
fn session_cookie(id: &str) -> String {
    format!("rs={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000")
}

/// The cookie that says this device has already given the password for a reply
/// page. Its own cookie, and deliberately not the board's.
///
/// **`Lax`, where the board's is `Strict`, and the whole feature depends on
/// it.** A reply link is followed from a chat app, which is another site; a
/// `Strict` cookie is withheld on exactly that navigation, so the password
/// would be asked for again every single time -- which is the thing this is
/// meant to stop.
///
/// It is safe to loosen here because of what it is scoped to. `Path=/r/` keeps
/// it off every other route, and the board reads `rp` and never this, so
/// holding it grants no more than the reply page a ticket already names. The
/// only method it can arrive on is the GET that opens that page.
fn reply_cookie(id: &str) -> String {
    format!("rq={id}; Path=/r/; HttpOnly; SameSite=Lax; Max-Age=2592000")
}

impl RemoteUi {
    pub fn start(
        bind: std::net::Ipv4Addr,
        port: u16,
        token: String,
        password: String,
    ) -> Result<Self> {
        Self::start_with(bind, port, token, password, false)
    }

    /// sticky: the phone keeps its pairing (token in the URL and in
    /// persistent storage) — see config::RemoteSpec::sticky_token. The
    /// flag only shapes the served shell page; a config change restarts the
    /// server, so it needs no runtime switch
    pub fn start_with(
        bind: std::net::Ipv4Addr,
        port: u16,
        token: String,
        password: String,
        sticky: bool,
    ) -> Result<Self> {
        let addr = format!("{bind}:{port}");
        let in_use = |e: &(dyn std::error::Error + Send + Sync + 'static)| {
            e.downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse)
        };
        // The previous server's socket closes a moment AFTER shutdown()
        // returns (tiny_http's accept thread exits asynchronously on drop),
        // so an off→on flip can hit AddrInUse for a few more milliseconds.
        // Wait those out; only a port that stays taken is a real error
        let mut waited_ms = 0u64;
        let server = loop {
            match Server::http((bind, port)) {
                Ok(s) => break s,
                Err(e) if in_use(e.as_ref()) && waited_ms < 1000 => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                    waited_ms += 25;
                }
                Err(e) => {
                    // "In use" is a long OS message that doesn't say what to do about
                    // it. The culprit is usually your own previous instance still running
                    return Err(if in_use(e.as_ref()) {
                        anyhow::anyhow!(crate::i18n::tp("remote.err.in_use", &[("addr", &addr)]))
                    } else {
                        anyhow::anyhow!(crate::i18n::tp(
                            "remote.err.start",
                            &[("addr", &addr), ("error", &e.to_string())]
                        ))
                    });
                }
            }
        };
        let real_port = server
            .server_addr()
            .to_ip()
            .with_context(|| crate::i18n::t("err.remote.no_port"))?
            .port();
        // The origin without the token, so the URL can be rebuilt when the token
        // is rotated (see rotate_token).
        let origin = format!("http://{bind}:{real_port}");
        let url = format!("{origin}/?t={token}");
        // Shared so a runtime rotation is seen by the server thread's handlers.
        let token = Arc::new(Mutex::new(token));
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let (tx, rx) = channel::<RemoteCmd>();
        let stop = Arc::new(AtomicBool::new(false));
        let frame_clients: FrameClients = Arc::new(Mutex::new(Vec::new()));
        let state_clients: StateClients = Arc::new(Mutex::new(Vec::new()));
        let last_poll: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let keyframe_wanted = Arc::new(AtomicBool::new(false));
        let settings = Arc::new(Mutex::new(None));
        // Who is let in, and the one thing a "disconnect" changes. In memory
        // only — an app restart re-pairs every phone from the link
        let gate = Arc::new(Gate {
            password,
            pw: Ids::new(),
            grants: Ids::new(),
            misses: Mutex::new(Misses::new(Instant::now())),
        });
        let book = Arc::new(crate::reply::Book::new());

        // A second door on the loopback, sharing this one's handler.
        //
        // `tailscale serve` is what puts HTTPS in front of this board
        // (src/tailscale.rs), and what it can proxy to is a service on the
        // loopback. Handed this machine's own tailnet address instead, it does
        // not route back to itself: every request answers 502. Measured, not
        // assumed -- a rule pointing at 100.x:8787, with the board listening
        // exactly there, got nothing through.
        //
        // So the board also answers on 127.0.0.1, at the same port. It is the
        // same handler behind the same token; what changes is that a proxy
        // running on this machine can reach it. Nothing off this machine can,
        // which is what the loopback is -- and the settings server has always
        // sat there for the same reason.
        //
        // A loopback port already taken is not an error. It costs the HTTPS
        // front door and nothing else, and the address a person was given
        // still works.
        let loopback = if bind.is_loopback() {
            None
        } else {
            match Server::http((std::net::Ipv4Addr::LOCALHOST, real_port)) {
                Ok(s) => Some(Arc::new(s)),
                Err(e) => {
                    crate::append_hook_log(&format!(
                        "remote: no second door on 127.0.0.1:{real_port} ({e});                          an HTTPS proxy in front of this will not reach it"
                    ));
                    None
                }
            }
        };

        let server = Arc::new(server);
        let accept_thread = {
            let server = Arc::clone(&server);
            let token = Arc::clone(&token);
            let snapshot = Arc::clone(&snapshot);
            let stop = Arc::clone(&stop);
            let clients = Arc::clone(&frame_clients);
            let states = Arc::clone(&state_clients);
            let polls = Arc::clone(&last_poll);
            let kf = Arc::clone(&keyframe_wanted);
            let settings = Arc::clone(&settings);
            let gate = Arc::clone(&gate);
            let book_for_thread = Arc::clone(&book);
            let tx = tx.clone();
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Err(e) =
                        handle(
                            req, &token, &snapshot, &tx, &clients, &states, &polls, &kf,
                            &settings, &gate, &book_for_thread, sticky,
                        )
                    {
                        crate::append_hook_log(&crate::i18n::tp(
                            "err.remote.hook_log",
                            &[("e", &e.to_string())],
                        ));
                    }
                }
            })
        };
        let loopback_thread = loopback.as_ref().map(|server| {
            let server = Arc::clone(server);
            let token = Arc::clone(&token);
            let snapshot = Arc::clone(&snapshot);
            let stop = Arc::clone(&stop);
            let clients = Arc::clone(&frame_clients);
            let states = Arc::clone(&state_clients);
            let polls = Arc::clone(&last_poll);
            let kf = Arc::clone(&keyframe_wanted);
            let settings = Arc::clone(&settings);
            let gate = Arc::clone(&gate);
            let book_for_thread = Arc::clone(&book);
            let tx = tx.clone();
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Err(e) = handle(
                        req, &token, &snapshot, &tx, &clients, &states, &polls, &kf,
                        &settings, &gate, &book_for_thread, sticky,
                    ) {
                        crate::append_hook_log(&crate::i18n::tp(
                            "err.remote.hook_log",
                            &[("e", &e.to_string())],
                        ));
                    }
                }
            })
        });
        Ok(Self {
            url,
            origin,
            port: real_port,
            token,
            note: None,
            snapshot,
            rx,
            stop,
            frame_clients,
            state_clients,
            last_poll,
            keyframe_wanted,
            settings,
            gate,
            book,
            server: Mutex::new(Some(server)),
            accept_thread: Mutex::new(Some(accept_thread)),
            loopback: Mutex::new(loopback),
            loopback_thread: Mutex::new(loopback_thread),
        })
    }

    /// Point the settings reverse-proxy at the local (loopback) settings web
    /// server: its origin (`http://127.0.0.1:<port>`) and its own token, which
    /// the proxy injects server-side. The phone never sees this token.
    /// Write a ticket for a reply page and hand back the whole link.
    ///
    /// The caller has the tab and what it said; this is the only place that
    /// knows the address. `None` when there is nowhere to point -- a board
    /// with no address is a link to nothing.
    pub fn reply_link(&self, t: crate::reply::Ticket) -> String {
        crate::reply::link(&self.origin, &self.book.mint(t))
    }

    /// The ticket table, for the automation to write its own reply links with.
    pub fn tickets(&self) -> Arc<crate::reply::Book> {
        Arc::clone(&self.book)
    }

    /// The port this is really listening on -- on both of its addresses.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Say that this board is reached at another address than the one it
    /// listens on -- an HTTPS front door put there by `tailscale serve`
    /// (src/tailscale.rs).
    ///
    /// Everything that hands out a link reads `origin`, so this is the only
    /// place that has to change: the QR, the reply pages a notification links
    /// to, and the board's own address as the automation sees it.
    pub fn reached_at(&mut self, origin: String) {
        self.url = format!("{origin}/?t={}", self.token.lock().unwrap());
        self.origin = origin;
    }

    /// Where the board is, without the key to it.
    ///
    /// A phone that has been paired opens this and is back on the board -- its
    /// token is in the browser's own storage. Anyone else gets a page that can
    /// read nothing. That is the difference between a link worth putting in a
    /// notification and a credential nobody should post into a chat room.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn set_settings_backend(&self, origin: String, token: String) {
        *self.settings.lock().unwrap() = Some((origin, token));
    }

    /// Cut every current remote session, honestly. Rotating the token makes the
    /// old URL (already loaded on a phone) fail auth on its next request, and
    /// dropping the client channels closes the sockets it holds open right now —
    /// so a phone that was told it's disconnected really is, and can't reconnect
    /// until it re-pairs with the new URL. `url` is rebuilt so the pairing QR,
    /// which reads it every frame, shows the new token.
    pub fn rotate_token(&mut self, new: String) {
        *self.token.lock().unwrap() = new.clone();
        self.url = format!("{}/?t={}", self.origin, new);
        self.cut_sessions();
    }

    /// End every remote session WITHOUT changing the token — the fixed-token
    /// "disconnect". The pairing survives (a bookmarked link still opens the
    /// board, and it must present the password again), but nothing that is open
    /// right now survives: the sessions are gone, so the page already loaded is
    /// refused on its next request and its next touch, and the sockets carrying
    /// the screen to it are dropped.
    ///
    /// Dropping the sockets alone was NOT a disconnect. The page reconnects on
    /// its own a second and a half later, and with a token that cannot change
    /// there was nothing to refuse it with — so a phone that had been told it
    /// was disconnected went on watching and driving a browser for as long as it
    /// liked. Admission has to be revocable on its own, which is what Gate is.
    pub fn cut_sessions(&self) {
        self.gate.cut();
        // The links sitting in a chat are somebody holding this terminal too
        self.book.cut();
        // Say it on the way out. The phone's own poll would notice within a
        // second and a half; the screen it is holding should go dark the
        // instant the person here decides it does.
        self.push_state("{\"cut\":true}".to_string());
        self.frame_clients.lock().unwrap().clear();
        self.state_clients.lock().unwrap().clear();
    }

    /// Whether a new viewer joined and we should emit one frame of the
    /// current screen (lowers the flag and returns it if it was raised)
    pub fn take_keyframe_request(&self) -> bool {
        self.keyframe_wanted.swap(false, Ordering::SeqCst)
    }

    /// Deliver a relay frame (JPEG bytes) to every connected WS client.
    /// Drop lines that can't receive it (the peer closed or is backed up)
    pub fn push_frame(&self, jpeg: Vec<u8>) {
        let mut clients = self.frame_clients.lock().unwrap();
        clients.retain(|tx| tx.send(jpeg.clone()).is_ok());
    }

    /// Whether at least one client is watching the relay (if nobody is
    /// watching, the relay can be stopped)
    pub fn has_frame_clients(&self) -> bool {
        !self.frame_clients.lock().unwrap().is_empty()
    }

    /// Whether at least one viewer is connected on the state socket. When none
    /// is, the main loop skips building and pushing state entirely.
    pub fn has_state_clients(&self) -> bool {
        !self.state_clients.lock().unwrap().is_empty()
    }

    /// How long a viewer that can only poll counts as still being there. It
    /// asks every second and a half, so this is a few missed turns.
    const POLL_LIFE: Duration = Duration::from_secs(6);

    /// Whether anyone is watching from afar at all -- over the state socket, or
    /// by polling when a socket won't hold. What the terminal is cut to hangs on
    /// this answer, so it has to count the poller too: a phone reduced to
    /// polling is still a phone-sized screen looking at the same terminal.
    pub fn watched(&self) -> bool {
        self.has_state_clients()
            || self
                .last_poll
                .lock()
                .unwrap()
                .is_some_and(|t| t.elapsed() < Self::POLL_LIFE)
    }

    /// Push one state message (a small JSON object with `ui` or `screen_html`)
    /// to every connected viewer. Drop lines whose peer has gone.
    pub fn push_state(&self, msg: String) {
        let mut clients = self.state_clients.lock().unwrap();
        clients.retain(|tx| tx.send(msg.clone()).is_ok());
    }

    /// Stop accepting and release the port before returning.
    ///
    /// Setting the flag alone left the accept thread blocked inside
    /// `incoming_requests()` — still owning the socket — until the next
    /// request happened to arrive. Toggling the feature off and on then
    /// failed to rebind (AddrInUse) until the app was restarted, with the
    /// settings toggle reporting "stopped" no matter which way it was flipped
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let server = self.server.lock().unwrap().take();
        let loopback = self.loopback.lock().unwrap().take();
        for s in [server.as_ref(), loopback.as_ref()].into_iter().flatten() {
            s.unblock();
        }
        for h in [
            self.accept_thread.lock().unwrap().take(),
            self.loopback_thread.lock().unwrap().take(),
        ]
        .into_iter()
        .flatten()
        {
            let _ = h.join();
        }
        // The threads are gone; dropping the last Arc closes each listener, so
        // both ports are free for an immediate rebind by the time this returns
        drop(server);
        drop(loopback);
    }
}

fn query_value(url: &str, key: &str) -> String {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .unwrap_or("")
        .to_string()
}

/// The caller's query with the token taken out, ready to go back on a URL.
///
/// This lands in a Location header, so a parameter carrying anything but plain
/// query text is dropped rather than escaped: the settings page reads its own
/// parameters and has no use for the rest, and a response header is no place to
/// be generous about what it will carry.
fn carried_query(url: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "-_.~%+=".contains(c);
    let kept: Vec<&str> = url
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .filter(|kv| !kv.is_empty() && !kv.starts_with("t=") && kv.chars().all(plain))
        .collect();
    if kept.is_empty() {
        String::new()
    } else {
        format!("?{}", kept.join("&"))
    }
}

fn json_response(v: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(v.to_string())
        .with_header(
            Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                .unwrap(),
        )
        .with_header(Header::from_bytes(&b"Referrer-Policy"[..], &b"no-referrer"[..]).unwrap())
        .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap())
}

/// The header that says when a turned-away password may be offered again
fn retry_after(secs: u64) -> Header {
    Header::from_bytes(&b"Retry-After"[..], secs.max(1).to_string().as_bytes()).unwrap()
}

/// Maximum accepted request-body size (see webui::read_body).
const MAX_BODY: usize = 1 << 20; // 1 MiB
/// Body cap for the attach route (a base64-encoded file, ~1.33x its raw size).
/// Comfortably covers the default 25 MB attachment; a much larger `attach.max_mb`
/// would need this raised too.
const MAX_ATTACH: usize = 96 << 20; // 96 MiB

/// Read a request body, capped at `max` bytes; None if it would exceed the cap.
fn read_body(req: &mut tiny_http::Request, max: usize) -> std::io::Result<Option<String>> {
    use std::io::Read as _;
    let mut body = String::new();
    req.as_reader().take(max as u64 + 1).read_to_string(&mut body)?;
    Ok((body.len() <= max).then_some(body))
}

#[allow(clippy::too_many_arguments)]
fn handle(
    req: tiny_http::Request,
    token: &Arc<Mutex<String>>,
    snapshot: &Arc<Mutex<Snapshot>>,
    tx: &Sender<RemoteCmd>,
    frame_clients: &FrameClients,
    state_clients: &StateClients,
    last_poll: &Arc<Mutex<Option<Instant>>>,
    keyframe_wanted: &Arc<AtomicBool>,
    settings: &Arc<Mutex<Option<(String, String)>>>,
    gate: &Arc<Gate>,
    book: &Arc<crate::reply::Book>,
    sticky: bool,
) -> Result<()> {
    // Snapshot the current token for this request. It can be rotated at runtime
    // (the PC's "disconnect" cuts every existing session by changing the token),
    // so each request compares against, and each served page embeds, the token
    // as it stands right now.
    let token = token.lock().unwrap().clone();
    // The token may arrive as the X-Token header, the `?t=` query, or the `sst`
    // cookie. The cookie is for the reverse-proxied settings page: its absolute
    // `/api/*` fetches can't carry a query token and a top-level navigation can't
    // set a header, so `/cfg` trades the URL token once for a SameSite=Strict
    // cookie and every following settings request authenticates by that cookie.
    let supplied = {
        let h = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("X-Token"))
            .map(|h| h.value.as_str().to_string())
            .unwrap_or_default();
        if !h.is_empty() {
            h
        } else {
            let q = query_value(req.url(), "t");
            if !q.is_empty() {
                q
            } else {
                cookie_value(&req, "sst")
            }
        }
    };
    let method = req.method().as_str().to_string();
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    // The pairing session this request belongs to (see Gate). Read before the
    // routes so the sockets, which consume the request when they upgrade, can
    // carry it with them.
    let session = cookie_value(&req, "rs");

    // The shell page carries no secrets and no state — it is the same inert HTML
    // for everyone, and every data route below still requires the token. Serving
    // it WITHOUT a token is what lets a paired phone drop `?t=…` from its URL (the
    // token moves into sessionStorage) and still survive a reload: the reload
    // fetches "/" with no token, gets the shell, and re-auths its data sockets
    // from storage. An unpaired visitor gets the same inert shell and can read
    // nothing — its state socket and every intent answer 403 just below.
    //
    // This is also where a session is minted: arriving here WITH the token in
    // the URL is the pairing gesture — a QR scan, a bookmark, a deliberate
    // reload — and it is the only way in. The page's own fetches and sockets
    // carry the token too, but none of them lands on this route, so a page that
    // has been cut cannot quietly let itself back in: a person has to open the
    // link again.
    if method == "GET" && (path == "/" || path == "/shell") {
        let mut resp = Response::from_string(crate::shell::page_for(sticky))
            .with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            )
            // Never show a stale page after an update
            .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap())
            // Keep any URL token out of the Referer header
            .with_header(Header::from_bytes(&b"Referrer-Policy"[..], &b"no-referrer"[..]).unwrap());
        if crate::crypto::token_eq(&query_value(req.url(), "t"), &token) {
            let id = gate.grants.keep(&session);
            resp = resp.with_header(
                Header::from_bytes(&b"Set-Cookie"[..], session_cookie(&id).as_bytes()).unwrap(),
            );
        }
        return req.respond(resp).map_err(Into::into);
    }

    // What an installed copy needs before it has anything to show: the
    // manifest and the icons.
    //
    // Ahead of the token gate, and it has to be -- a browser fetches a
    // manifest on its own account, and a launcher fetches an icon days later,
    // neither of them carrying anything the page was given. Nothing here is
    // state: a name, a colour, and a drawing. See src/pwa.rs.
    if method == "GET" && path == crate::pwa::MANIFEST_PATH {
        return req
            .respond(
                Response::from_string(crate::pwa::manifest_json())
                    .with_header(
                        Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/manifest+json; charset=utf-8"[..],
                        )
                        .unwrap(),
                    )
                    // It names the theme's colour, which the person can change.
                    .with_header(
                        Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap(),
                    ),
            )
            .map_err(Into::into);
    }
    if method == "GET" {
        if let Some(bytes) = crate::pwa::icon(&path) {
            return req
                .respond(
                    Response::from_data(bytes)
                        .with_header(
                            Header::from_bytes(&b"Content-Type"[..], &b"image/png"[..]).unwrap(),
                        )
                        // The drawing only changes when the program does, and
                        // the launcher that wants it may ask while nothing of
                        // ours is running to answer twice.
                        .with_header(
                            Header::from_bytes(
                                &b"Cache-Control"[..],
                                &b"public, max-age=604800, immutable"[..],
                            )
                            .unwrap(),
                        ),
                )
                .map_err(Into::into);
        }
    }

    // The service worker: the piece of this program that runs inside the
    // phone's browser with the page shut (src/push.rs).
    //
    // Token-free for the same reason the manifest is, and one more: a browser
    // re-fetches this file on its own schedule to see whether the worker has
    // changed, long after whatever page installed it has gone. It is inert
    // code that can read nothing -- every route it might call still wants the
    // token, and it never has one.
    //
    // Served from the root so its scope covers the whole board. A worker
    // registered from a deeper path can only ever control that path.
    if method == "GET" && path == "/sw.js" {
        return req
            .respond(
                Response::from_string(crate::push::SERVICE_WORKER)
                    .with_header(
                        Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/javascript; charset=utf-8"[..],
                        )
                        .unwrap(),
                    )
                    // Never a stale worker after an update: the browser asks,
                    // and the answer has to be this build's.
                    .with_header(
                        Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap(),
                    ),
            )
            .map_err(Into::into);
    }

    // The reply page a notification links to.
    //
    // Ahead of the token gate, and on purpose: the ticket in the path IS the
    // credential, and it is a far smaller one than the token. It can say one
    // thing to one tab. It cannot read the board, open the settings, reach a
    // file, or survive the disconnect that ends every phone.
    //
    // A configured password is the one thing that stops it. Somebody who set a
    // second factor decided the token alone was not enough; quietly letting a
    // link in a chat past that decision is not ours to do.
    if let Some(rest) = path.strip_prefix("/r/") {
        let (id, verb) = match rest.split_once('/') {
            Some((id, v)) => (id, v),
            None => (rest, ""),
        };
        let html = |body: String| {
            Response::from_string(body)
                .with_header(
                    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..])
                        .unwrap(),
                )
                .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap())
                .with_header(
                    Header::from_bytes(&b"Referrer-Policy"[..], &b"no-referrer"[..]).unwrap(),
                )
        };
        let Some(ticket) = book.get(id) else {
            // Expired, torn up, or never ours. One answer for all three: a
            // page that says the link is finished. Which of the three it was
            // is not the visitor's business to learn by trying ids
            return req
                .respond(html(crate::reply::gone_page()).with_status_code(404))
                .map_err(Into::into);
        };
        // The second factor, when one is set. Asked on this page's own terms
        // and remembered in this page's own cookie, so it is asked once per
        // device rather than once per notification.
        let unlocked = gate.unlocked(&cookie_value(&req, "rq"));
        match (method.as_str(), verb) {
            ("POST", "unlock") => {
                let mut req = req;
                let Some(body) = read_body(&mut req, MAX_BODY)? else {
                    req.respond(Response::from_string("payload too large").with_status_code(413))?;
                    return Ok(());
                };
                // The door is shut for a while after wrong passwords: said
                // before the password is looked at, so a right one offered
                // during the wait is turned away too (see Misses)
                if let Some(left) = gate.wait() {
                    let secs = left.as_secs_f64().ceil() as u64;
                    req.respond(
                        json_response(serde_json::json!({"ok": false, "wait": secs}))
                            .with_status_code(429)
                            .with_header(retry_after(secs)),
                    )?;
                    return Ok(());
                }
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let given = v.get("password").and_then(|p| p.as_str()).unwrap_or("");
                if !crate::crypto::token_eq(given, &gate.password) {
                    gate.missed();
                    req.respond(json_response(serde_json::json!({"ok": false})))?;
                    return Ok(());
                }
                gate.forgive();
                let id = gate.pw.keep("");
                req.respond(
                    json_response(serde_json::json!({"ok": true})).with_header(
                        Header::from_bytes(&b"Set-Cookie"[..], reply_cookie(&id).as_bytes())
                            .unwrap(),
                    ),
                )?;
                return Ok(());
            }
            // Nothing below here happens until that is settled. The asking
            // page carries nothing about the tab: whoever is looking has not
            // said who they are yet.
            (_, _) if !unlocked => {
                return req
                    .respond(html(crate::reply::ask_page(id, false)))
                    .map_err(Into::into);
            }
            ("GET", "") => {
                return req
                    .respond(html(crate::reply::page(id, &ticket)))
                    .map_err(Into::into);
            }
            ("POST", "say") => {
                let mut req = req;
                let Some(body) = read_body(&mut req, MAX_BODY)? else {
                    req.respond(Response::from_string("payload too large").with_status_code(413))?;
                    return Ok(());
                };
                let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let text = v.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if text.trim().is_empty() {
                    req.respond(json_response(serde_json::json!({"ok": false})))?;
                    return Ok(());
                }
                let _ = tx.send(RemoteCmd::Reply {
                    tab_id: ticket.tab_id.clone(),
                    tab: ticket.tab_index,
                    name: ticket.tab_name.clone(),
                    dest: ticket.dest.clone(),
                    text,
                });
                req.respond(json_response(serde_json::json!({"ok": true})))?;
                return Ok(());
            }
            _ => {
                return req
                    .respond(Response::from_string("not found").with_status_code(404))
                    .map_err(Into::into);
            }
        }
    }

    // Everything past here is data or control — the token first.
    if !crate::crypto::token_eq(&supplied, &token) {
        return req
            .respond(Response::from_string("forbidden").with_status_code(403))
            .map_err(Into::into);
    }

    // Then the session, which is what the window's "disconnect" takes away. A
    // caller holding the right token but no live session was cut (or never
    // opened the link): the body says so, and the phone puts up its ⛔ screen
    // rather than sitting in front of a picture that stopped being true.
    if !gate.granted(&session) {
        return req
            .respond(Response::from_string("cut").with_status_code(403))
            .map_err(Into::into);
    }

    // Then the optional password. A phone that has the token and a session but
    // hasn't presented the password yet may do exactly one thing: trade the
    // password for a session cookie at /auth. Everything else answers 403 with
    // the body "password", which the shell reads as "prompt the person and try
    // again".
    //
    // The password travels in a POST body, never in the address: an address is
    // what proxies, browsers and screenshots keep. And the door is shut for a
    // while after wrong passwords — the same score as the reply page's, so a
    // guesser cannot alternate doors — with "come back in n seconds" (429)
    // rather than a slept thread, which would have slowed every phone
    let unlocked = gate.unlocked(&cookie_value(&req, "rp"));
    if !unlocked {
        if method == "POST" && path == "/auth" {
            if let Some(left) = gate.wait() {
                let secs = left.as_secs_f64().ceil() as u64;
                return req
                    .respond(
                        Response::from_string("wait")
                            .with_status_code(429)
                            .with_header(retry_after(secs))
                            .with_header(
                                Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..])
                                    .unwrap(),
                            ),
                    )
                    .map_err(Into::into);
            }
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let given = v.get("password").and_then(|p| p.as_str()).unwrap_or("");
            if crate::crypto::token_eq(given, &gate.password) {
                gate.forgive();
                let id = gate.pw.keep("");
                let cookie = format!(
                    "rp={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age=2592000"
                );
                return req
                    .respond(
                        Response::from_string("ok")
                            .with_header(
                                Header::from_bytes(&b"Set-Cookie"[..], cookie.as_bytes()).unwrap(),
                            )
                            .with_header(
                                Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..])
                                    .unwrap(),
                            ),
                    )
                    .map_err(Into::into);
            }
            gate.missed();
            return req
                .respond(Response::from_string("forbidden").with_status_code(403))
                .map_err(Into::into);
        }
        return req
            .respond(Response::from_string("password").with_status_code(403))
            .map_err(Into::into);
    }

    // The settings screen, reverse-proxied to the loopback config server. The
    // phone reaches the config UI only through here, on the same admission as
    // the board; the config server itself never faces the network. Handled
    // before the routes below so its `/api/*` don't collide with the remote's
    // own verbs.
    if is_settings_path(&path) {
        return proxy_settings(req, settings, &token, &method, &path);
    }

    match (method.as_str(), path.as_str()) {
        ("GET", "/api/state") => {
            // Asking for the state IS watching, and this is the only trace a
            // viewer without a socket leaves (see `watched`).
            *last_poll.lock().unwrap() = Some(Instant::now());
            let snap = snapshot.lock().unwrap().clone();
            req.respond(json_response(serde_json::to_value(snap)?))?;
        }
        // The tab's conversation, as text, newest turns last. What the phone's
        // reader shows instead of the terminal grid: a full-screen TUI keeps no
        // scrollback, so the screen relay simply has no older text to give, and
        // the CLI's own record is the only complete copy (reader.rs).
        //
        // The phone names a tab and a byte offset to read back from — never a
        // path. Which file that is stays on this side, resolved from the id the
        // app itself handed the CLI at launch
        ("GET", "/api/read") => {
            let tab: usize = query_value(req.url(), "tab").parse().unwrap_or(0);
            let before: u64 = query_value(req.url(), "before").parse().unwrap_or(u64::MAX);
            let want: usize = query_value(req.url(), "want")
                .parse()
                .unwrap_or(6)
                .clamp(1, 40);
            let record = snapshot
                .lock()
                .unwrap()
                .tabs
                .iter()
                .find(|t| t.index == tab)
                .map(|t| (t.record_id.clone(), t.record_glob.clone()));
            let found = record
                .filter(|(id, glob)| !id.is_empty() && !glob.is_empty())
                .and_then(|(id, glob)| crate::sessionfind::locate(&glob, &id));
            let answer = match found {
                // No record yet is the ordinary state of a tab nobody has
                // spoken to. Said plainly, so the reader can show a line
                // instead of an error
                None => serde_json::json!({"ok": false, "empty": true}),
                Some(path) => match crate::reader::read_back(&path, before, want) {
                    Ok(page) => serde_json::json!({
                        "ok": true,
                        "turns": page.turns,
                        "from": page.from,
                        "more": page.more,
                    }),
                    Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
                },
            };
            req.respond(json_response(answer))?;
        }
        // The latest run's durable replay script (css/xpath anchors, no
        // digest refs) — the 🎯 panel's download button on the phone.
        // 404 while no run has recorded anything replayable yet
        ("GET", "/api/replay") => {
            let found = crate::exchange::latest_run().and_then(|dir| {
                let text = std::fs::read_to_string(dir.join("replay.lua")).unwrap_or_default();
                let name = dir.file_name().and_then(|s| s.to_str()).unwrap_or("run").to_string();
                let live = text
                    .lines()
                    .any(|l| !l.trim().is_empty() && !l.trim_start().starts_with("--"));
                live.then_some((text, name))
            });
            match found {
                Some((text, name)) => {
                    let cd = format!("attachment; filename=\"shikisha-macro-{name}.lua\"");
                    let resp = Response::from_string(text)
                        .with_header(
                            Header::from_bytes(
                                &b"Content-Type"[..],
                                &b"text/plain; charset=utf-8"[..],
                            )
                            .unwrap(),
                        )
                        .with_header(
                            Header::from_bytes(&b"Content-Disposition"[..], cd.as_bytes())
                                .unwrap(),
                        );
                    req.respond(resp)?;
                }
                None => {
                    req.respond(Response::from_string("no replay").with_status_code(404))?;
                }
            }
        }
        // State push. Same data as /api/state, but sent over a WebSocket the
        // moment it changes (the main loop calls push_state) instead of the
        // phone polling. Download-only; the write thread owns the socket.
        ("GET", "/ws-state") => {
            let key = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Sec-WebSocket-Key"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            if key.is_empty() {
                return req
                    .respond(Response::from_string("expected websocket").with_status_code(400))
                    .map_err(Into::into);
            }
            let accept = crate::ws::accept_key(&key);
            let resp = Response::empty(101).with_header(
                Header::from_bytes(&b"Sec-WebSocket-Accept"[..], accept.as_bytes()).unwrap(),
            );
            let stream = req.upgrade("websocket", resp);
            let (stx, srx) = channel::<String>();
            // Give the new viewer the current screen and UI right away, so it
            // isn't blank until something next changes.
            {
                let snap = snapshot.lock().unwrap();
                if let Ok(ui) = serde_json::to_string(&snap.ui) {
                    let _ = stx.send(format!("{{\"ui\":{ui}}}"));
                }
                if let Ok(scr) = serde_json::to_string(&snap.screen_html) {
                    let _ = stx.send(format!("{{\"screen_html\":{scr}}}"));
                }
            }
            state_clients.lock().unwrap().push(stx);
            std::thread::spawn(move || {
                let mut w = crate::ws::WsWriter::new(stream);
                while let Ok(msg) = srx.recv() {
                    if w.send_text(&msg).is_err() {
                        break;
                    }
                }
                let _ = w.send_close();
            });
        }
        // Entry point for the screen relay. Handshake, upgrade to WebSocket,
        // and from then on JPEG frames flow over this line (download-only;
        // the write thread owns the socket)
        ("GET", "/ws") => {
            let key = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Sec-WebSocket-Key"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            if key.is_empty() {
                return req
                    .respond(Response::from_string("expected websocket").with_status_code(400))
                    .map_err(Into::into);
            }
            let accept = crate::ws::accept_key(&key);
            let resp = Response::empty(101).with_header(
                Header::from_bytes(&b"Sec-WebSocket-Accept"[..], accept.as_bytes()).unwrap(),
            );
            let stream = req.upgrade("websocket", resp);
            let (ftx, frx) = channel::<Vec<u8>>();
            frame_clients.lock().unwrap().push(ftx);
            // New viewer. Tell the main loop "emit one frame of the current screen"
            keyframe_wanted.store(true, Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut w = crate::ws::WsWriter::new(stream);
                while let Ok(jpeg) = frx.recv() {
                    if w.send_binary(&jpeg).is_err() {
                        break;
                    }
                }
                let _ = w.send_close();
            });
        }
        // Upload path for input. Carries the finger trail with low latency,
        // so it's a separate one-way WS from the download path (avoids
        // splitting one socket for read/write; each line stays single-threaded)
        ("GET", "/ws-in") => {
            let key = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Sec-WebSocket-Key"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            if key.is_empty() {
                return req
                    .respond(Response::from_string("expected websocket").with_status_code(400))
                    .map_err(Into::into);
            }
            let accept = crate::ws::accept_key(&key);
            let resp = Response::empty(101).with_header(
                Header::from_bytes(&b"Sec-WebSocket-Accept"[..], accept.as_bytes()).unwrap(),
            );
            let mut stream = req.upgrade("websocket", resp);
            let tx = tx.clone();
            let gate = Arc::clone(gate);
            let session = session.clone();
            std::thread::spawn(move || {
                loop {
                    let frame = crate::ws::read_frame(&mut stream);
                    // Admission is re-checked on every touch, not once at the
                    // handshake. This line stays open for as long as the phone
                    // holds it, so a disconnect that only dropped the picture
                    // left the hand still on the controls — blind, but driving.
                    if !gate.granted(&session) {
                        break;
                    }
                    match frame {
                        Ok((crate::ws::Op::Text, payload)) => {
                            let Ok(text) = String::from_utf8(payload) else {
                                continue;
                            };
                            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                                continue;
                            };
                            if let Some(ev) = crate::browser::parse_intent(&v) {
                                if allowed_from_afar(&ev) {
                                    let _ = tx.send(RemoteCmd::Ui(ev));
                                }
                            }
                        }
                        Ok((crate::ws::Op::Close, _)) | Err(_) => break,
                        Ok(_) => {} // ping/pong/binary are ignored
                    }
                }
            });
        }
        ("POST", "/api/send") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let tab = v.get("tab").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            if let Some(text) = v.get("text").and_then(|x| x.as_str()) {
                let _ = tx.send(RemoteCmd::Send {
                    tab,
                    text: text.to_string(),
                });
            } else if let Some(keys) = v.get("keys").and_then(|x| x.as_str()) {
                let _ = tx.send(RemoteCmd::Keys {
                    tab,
                    keys: keys.to_string(),
                });
            }
            req.respond(json_response(serde_json::json!({"ok": true})))?;
        }
        // Operation from the screen. Received with the same intent, the
        // same vocabulary, as the window
        ("POST", "/api/intent") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let mut took = false;
            if let Some(ev) = crate::browser::parse_intent(&v) {
                if allowed_from_afar(&ev) {
                    let _ = tx.send(RemoteCmd::Ui(ev));
                    took = true;
                }
            }
            req.respond(json_response(serde_json::json!({"ok": took})))?;
        }
        ("POST", "/api/auto") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_BODY)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let on = v.get("on").and_then(|x| x.as_bool()).unwrap_or(false);
            let _ = tx.send(RemoteCmd::SetAuto(on));
            req.respond(json_response(serde_json::json!({"ok": true})))?;
        }
        // A file pasted/dropped/attached in the sub-input bar. Saved beside the
        // target tab (so its AI can read it) and the saved path handed back to
        // type into the prompt. Larger cap than the other routes — this carries a
        // base64 file, not a short command.
        ("POST", "/api/attach") => {
            let mut req = req;
            let Some(body) = read_body(&mut req, MAX_ATTACH)? else {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            };
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let tab = v.get("tab").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("file");
            let data = v.get("data").and_then(|x| x.as_str()).unwrap_or("");
            // The target tab's working folder, as it stands in the current snapshot.
            let cwd = snapshot
                .lock()
                .unwrap()
                .tabs
                .iter()
                .find(|t| t.index == tab)
                .map(|t| t.cwd.clone())
                .unwrap_or_default();
            req.respond(json_response(attach_save(&cwd, name, data)))?;
        }
        _ => {
            req.respond(Response::from_string("not found").with_status_code(404))?;
        }
    }
    Ok(())
}

/// Decode a base64 attachment and save it beside the target tab, returning a
/// JSON result (`{ok, path}` or `{ok:false, error}`). The size cap and allowed
/// extensions come from config; nothing here runs the file (see `attach`). Shared
/// by the phone's HTTP route and the desktop composer's ipc path.
pub(crate) fn attach_save(cwd: &str, name: &str, data_b64: &str) -> serde_json::Value {
    use base64::Engine as _;
    if cwd.is_empty() {
        return serde_json::json!({ "ok": false, "error": crate::i18n::t("attach.err.no_folder") });
    }
    let bytes = match base64::engine::general_purpose::STANDARD.decode(data_b64.as_bytes()) {
        Ok(b) => b,
        Err(_) => {
            return serde_json::json!({ "ok": false, "error": crate::i18n::t("attach.err.empty") })
        }
    };
    let cfg = crate::config::load().unwrap_or_default();
    let limits = crate::attach::Limits {
        max_bytes: (cfg.attach.max_mb as usize).saturating_mul(1024 * 1024),
        allowed_ext: cfg.attach.extensions,
    };
    match crate::attach::save(std::path::Path::new(cwd), name, &bytes, &limits) {
        Ok(path) => serde_json::json!({ "ok": true, "path": path.to_string_lossy() }),
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}

/// Read a named cookie from the request's `Cookie` header (empty if absent).
fn cookie_value(req: &tiny_http::Request, name: &str) -> String {
    req.headers()
        .iter()
        .find(|h| h.field.equiv("Cookie"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default()
        .split(';')
        .find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k.trim() == name).then(|| v.trim().to_string())
        })
        .unwrap_or_default()
}

/// Paths served by reverse-proxy to the loopback settings server. The remote UI
/// owns exactly its own four `/api/*` verbs; every other `/api/*`, plus the
/// settings / result / help pages, belongs to the config server.
/// The verbs the remote UI answers itself. Everything else under `/api/` belongs
/// to the config server and is reached through the proxy.
///
/// One list, checked against the routes themselves by
/// `every_verb_this_file_serves_is_claimed`. It was written apart from them
/// once, and the cost was silent: `/api/replay` was added below and not here,
/// so the phone's download button asked the settings proxy for it and the code
/// that actually serves it — a dozen lines away — was never once reached.
const OWN_VERBS: [&str; 7] = [
    "state", "send", "auto", "intent", "attach", "read", "replay",
];

fn is_settings_path(path: &str) -> bool {
    if path == "/cfg" || path == "/help" || path == "/result" {
        return true;
    }
    if let Some(rest) = path.strip_prefix("/api/") {
        let seg = rest.split(['/', '?']).next().unwrap_or("");
        return !OWN_VERBS.contains(&seg);
    }
    false
}

/// Reverse-proxy one request to the loopback settings server, injecting its token
/// server-side so the phone never holds it. `/cfg` maps to the settings root;
/// every other path is kept as-is. On the first hop the URL still carries `?t=`,
/// which is traded for a SameSite=Strict cookie and a clean redirect.
fn proxy_settings(
    mut req: tiny_http::Request,
    settings: &Arc<Mutex<Option<(String, String)>>>,
    remote_token: &str,
    method: &str,
    path: &str,
) -> Result<()> {
    // First hop: swap the URL token for a cookie, then bounce to a `/cfg` with the
    // credential gone, so it never lingers in the address bar or history. What the
    // caller asked to land on rides along — `?section=` / `?addtab=` / `?ret=`
    // say which screen to open. Dropping them here landed every walk to the
    // settings on a plain page, which is how the tab bar's + came to do nothing.
    if path == "/cfg" && !query_value(req.url(), "t").is_empty() {
        let cookie = format!("sst={remote_token}; Path=/; HttpOnly; SameSite=Strict");
        let location = format!("/cfg{}", carried_query(req.url()));
        let resp = Response::empty(302)
            .with_header(Header::from_bytes(&b"Location"[..], location.as_bytes()).unwrap())
            .with_header(Header::from_bytes(&b"Set-Cookie"[..], cookie.as_bytes()).unwrap());
        return req.respond(resp).map_err(Into::into);
    }

    let Some((origin, sett_token)) = settings.lock().unwrap().clone() else {
        return req
            .respond(Response::from_string("settings unavailable").with_status_code(503))
            .map_err(Into::into);
    };

    let query = req.url().split_once('?').map(|(_, q)| q.to_string());
    let sub = if path == "/cfg" { "/" } else { path };
    let url = match &query {
        Some(q) if !q.is_empty() => format!("{origin}{sub}?{q}"),
        _ => format!("{origin}{sub}"),
    };
    let req_ctype = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Content-Type"))
        .map(|h| h.value.as_str().to_string());
    let body = if method == "GET" {
        None
    } else {
        match read_body(&mut req, MAX_BODY)? {
            Some(b) => Some(b),
            None => {
                req.respond(Response::from_string("payload too large").with_status_code(413))?;
                return Ok(());
            }
        }
    };

    // Loopback only. Treat any HTTP status as a normal response so an upstream 4xx
    // body (e.g. a validation error) reaches the phone rather than being swallowed.
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .build()
        .new_agent();
    // Tell the settings server the operator is on a phone, not at this PC's screen.
    // Built here rather than forwarded, so the phone can never claim otherwise.
    let remote_hdr = crate::webui::REMOTE_CLIENT_HEADER;
    let result = if method == "GET" {
        agent
            .get(&url)
            .header("X-Token", &sett_token)
            .header(remote_hdr, "1")
            .call()
    } else {
        let mut rb = agent
            .post(&url)
            .header("X-Token", &sett_token)
            .header(remote_hdr, "1");
        if let Some(ct) = &req_ctype {
            rb = rb.header("Content-Type", ct);
        }
        rb.send(body.unwrap_or_default())
    };

    match result {
        Ok(mut resp) => {
            let status = resp.status().as_u16();
            let ctype = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("application/octet-stream")
                .to_string();
            let mut text = resp.body_mut().read_to_string().unwrap_or_default();
            // The settings HTML embeds its own token; scrub it so the phone never
            // sees it. (Getting back to the board is the page's own sticky "Close",
            // which navigates to "/" on the phone — with the unsaved-changes guard.)
            if path == "/cfg" && ctype.contains("text/html") {
                text = text.replace(&sett_token, "");
            }
            let resp = Response::from_string(text)
                .with_status_code(status)
                .with_header(Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap())
                .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap());
            req.respond(resp).map_err(Into::into)
        }
        Err(e) => req
            .respond(
                Response::from_string(format!("settings proxy error: {e}")).with_status_code(502),
            )
            .map_err(Into::into),
    }
}


#[cfg(test)]
mod tests {
    use super::*;


    /// Operations sendable from afar are counted on the allow side.
    ///
    /// Since the same page is served, it can in principle send the same
    /// intents as the window. The master password, asked inside the window,
    /// would block the app until the person in front of it answers, so that
    /// stays local. Sizing, by contrast, is allowed — a phone needs a terminal
    /// that fits it, and the two sides hand off rather than oscillate.
    #[test]
    fn the_phone_cannot_reach_what_only_the_window_can_answer() {
        use crate::browser::Ev;
        let menu = |k: &str| super::allowed_from_afar(&Ev::Menu { key: k.into() });
        assert!(super::allowed_from_afar(&Ev::Select { tab: 2 }));
        assert!(super::allowed_from_afar(&Ev::Stop));
        // Relaunching the tab on screen: a phone watching an SSH session that
        // dropped is precisely who needs it, and it reaches no further than the
        // keystroke it stands for
        assert!(super::allowed_from_afar(&Ev::Restart), "見ているタブを遠くから直せない");
        // Back/forward/reload/navigate must work from remote, or the top bar is just decoration
        assert!(super::allowed_from_afar(&Ev::Go { go: crate::browser::Go::Back }));
        assert!(super::allowed_from_afar(&Ev::Go {
            go: crate::browser::Go::To("example.com".into())
        }));
        assert!(menu("a") && menu("?") && menu("w"), "普通の操作が通らない");
        // The board and this gate read one list, so nothing the window alone can
        // do slips through, and nothing the phone can reach gets refused.
        for k in crate::shell::WINDOW_ONLY_MENU {
            assert!(!menu(k), "{k} は窓専用なのに遠隔から通ってしまう");
        }
        // Scrolling back to review earlier output is core to monitoring from afar
        assert!(
            super::allowed_from_afar(&Ev::Scroll { by: 3, row: 0, col: 0 }),
            "遠くから履歴を遡れない"
        );

        // The folder list is the phone's only way to open another folder;
        // the window has the same list, so nothing is let through that the
        // person could not do by adding a tab
        assert!(
            super::allowed_from_afar(&Ev::Browse { path: String::new(), open: false }),
            "スマホからフォルダ一覧を歩けない"
        );
        assert!(
            super::allowed_from_afar(&Ev::Browse { path: r"C:\work".into(), open: true }),
            "スマホから選んだフォルダを開けない"
        );

        assert!(!menu("k"), "マスターパスワードを遠くから呼べてしまう");
        assert!(!menu("e") && !menu("o"), "窓の中にしか出ないものを呼べる");
        // Sizing from afar is intentionally allowed: a phone needs a terminal that
        // fits it, and the two sides hand off (each re-reports only on its own size
        // change) rather than fight in a loop.
        assert!(
            super::allowed_from_afar(&Ev::Resize {
                full: (0, 0, 0, 0),
                rows: 10,
                cols: 20,
                area: (0, 0, 0, 0),
                panes: Vec::new()
            }),
            "スマホから端末をスマホ寸法に合わせられない"
        );
        assert!(
            !super::allowed_from_afar(&Ev::Paste),
            "長押しひとつでAIの入力欄に流れ込む"
        );
    }

    #[test]
    fn token_is_required_and_compared_safely() {
        assert!(crate::crypto::token_eq("abc", "abc"));
        assert!(!crate::crypto::token_eq("abc", "abd"));
        assert!(!crate::crypto::token_eq("abc", "abcd"));
        assert_eq!(query_value("/?t=xyz", "t"), "xyz");
        assert_eq!(query_value("/api/state", "t"), "");
    }

    /// The window's "disconnect" rotates the token: a phone still holding the old
    /// URL stops working on its very next request, only the new URL does, and the
    /// published URL (which the pairing QR reads) carries the new token.
    /// A phone as the server sees it: it opens the pairing link once, keeps the
    /// session cookie it is handed, and presents it on everything afterwards
    /// (exactly what a browser does with a same-origin cookie).
    struct Phone {
        agent: ureq::Agent,
        base: String,
        cookie: String,
    }

    impl Phone {
        fn new(base: &str) -> Self {
            Self {
                agent: ureq::Agent::config_builder()
                    // A refusal is an answer here, not a transport error — and a
                    // redirect is something to look at rather than to follow
                    .http_status_as_error(false)
                    .max_redirects(0)
                    .build()
                    .new_agent(),
                base: base.to_string(),
                cookie: String::new(),
            }
        }

        /// Open the link: the pairing gesture (a QR scan, a bookmark, a reload).
        /// Everything below it needs the session this hands back.
        fn pair(&mut self, token: &str) {
            let r = self.get(&format!("/?t={token}"));
            let set = r
                .headers()
                .get("set-cookie")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let rs = set
                .split(';')
                .find(|kv| kv.trim_start().starts_with("rs="))
                .unwrap_or_else(|| panic!("no session handed to a phone that opened the link: {set}"));
            // The new session replaces the old one; anything else this browser
            // holds it keeps, as a browser does
            let kept = self
                .cookie
                .split(';')
                .map(str::trim)
                .filter(|c| !c.is_empty() && !c.starts_with("rs="))
                .map(str::to_string);
            self.cookie = std::iter::once(rs.trim().to_string())
                .chain(kept)
                .collect::<Vec<_>>()
                .join("; ");
        }

        /// Keep another cookie alongside the session (the password gate's, the
        /// settings hop's) — a browser holds them all at once
        fn also(&mut self, cookie: &str) {
            self.cookie = format!("{}; {}", self.cookie, cookie);
        }

        fn get(&self, path: &str) -> ureq::http::Response<ureq::Body> {
            self.agent
                .get(&format!("{}{path}", self.base))
                .header("Cookie", &self.cookie)
                .call()
                .expect("no answer at all")
        }

        /// A POST whose answer is worth reading, not only counting
        fn said_post(&self, path: &str, body: &str) -> (u16, String) {
            let mut r = self.post(path, body);
            (r.status().as_u16(), r.body_mut().read_to_string().unwrap_or_default())
        }

        fn post(&self, path: &str, body: &str) -> ureq::http::Response<ureq::Body> {
            self.agent
                .post(&format!("{}{path}", self.base))
                .header("Cookie", &self.cookie)
                .header("Content-Type", "application/json")
                .send(body)
                .expect("no answer at all")
        }

        fn status(&self, path: &str) -> u16 {
            self.get(path).status().as_u16()
        }

        /// The status and body together — the body is how the phone tells a
        /// disconnect ("cut") from a locked device ("password")
        fn said(&self, path: &str) -> (u16, String) {
            let mut r = self.get(path);
            let code = r.status().as_u16();
            (code, r.body_mut().read_to_string().unwrap_or_default())
        }

        fn text(&self, path: &str) -> String {
            self.get(path).body_mut().read_to_string().unwrap()
        }

        /// The status of one data request, as the page's own poll would make it
        fn state(&self, token: &str) -> u16 {
            self.status(&format!("/api/state?t={token}"))
        }
    }

    /// The reply link is a door of its own, and the whole design rests on how
    /// small it is: it opens without the board's token, and it opens nothing
    /// but itself.
    #[test]
    fn a_reply_ticket_opens_its_own_page_and_nothing_else() {
        let ui = RemoteUi::start(
            "127.0.0.1".parse().unwrap(),
            0,
            "board-token-0000".into(),
            String::new(),
        )
        .unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let link = ui.reply_link(crate::reply::Ticket::new(
            Some("coder".into()),
            1,
            "レビュワー".into(),
            "終わりました".into(),
            String::new(),
        ));
        let id = link.rsplit('/').next().unwrap().to_string();
        let phone = Phone::new(&base);

        // No token, no cookie, no pairing -- and the page is there
        let r = phone.get(&format!("/r/{id}"));
        assert_eq!(r.status(), 200, "切符だけで開ける");
        let body = phone.text(&format!("/r/{id}"));
        assert!(body.contains("レビュワー"), "そのタブの話だと分かる");
        assert!(!body.contains("board-token-0000"), "盤面の鍵は載らない");

        // What it cannot do is everything else. The board's own doors still
        // want the board's token, and the ticket is not one
        // A path that tries to climb out of the ticket's own area is not
        // served by anything else: it is still inside /r/, where the only
        // verb is "say"
        assert_eq!(phone.status(&format!("/r/{id}/../api/state")), 404);
        assert_eq!(phone.status("/api/state"), 403);
        assert_eq!(phone.status("/cfg"), 403);

        // A ticket that was never ours reads the same as one that has expired
        assert_eq!(phone.status("/r/ZZZZZZZZZZZZ"), 404);
        assert_eq!(phone.status("/r/"), 404);

        // Saying nothing is not saying something
        let (code, said) = phone.said_post(&format!("/r/{id}/say"), "{\"text\":\"   \"}");
        assert_eq!(code, 200);
        assert!(said.contains("false"), "空文は送らない: {said}");

        // ...and what is said arrives as it was written. The message this
        // feature exists for is a sentence typed on a phone, which in this
        // house means Japanese: it travels as JSON, through a body read by
        // length, into a command the terminal will type. Every one of those
        // steps has a way of turning it into mojibake, and none of them says
        // so -- the reply simply arrives wrong.
        let words = "2でお願いします。ドキュメントを直してから、まとめてコミットしてください。";
        let body = serde_json::json!({ "text": words }).to_string();
        let (code, said) = phone.said_post(&format!("/r/{id}/say"), &body);
        assert_eq!(code, 200);
        assert!(said.contains("true"), "本文が送られていない: {said}");
        match ui.rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(RemoteCmd::Reply { tab_id, tab, name, dest, text }) => {
                assert_eq!(text, words, "本文が化けた");
                assert_eq!(tab_id.as_deref(), Some("coder"));
                assert_eq!(tab, 1);
                assert_eq!(name, "レビュワー");
                // The destination rides along so the confirmation goes back to
                // the same chat the notification came from, not the default one
                assert_eq!(dest, "");
            }
            other => panic!("返信が届いていない: {other:?}"),
        }

        // ...and the disconnect that ends every phone ends this too
        ui.cut_sessions();
        assert_eq!(phone.status(&format!("/r/{id}")), 404, "切断で無効になる");
    }

    /// A browser fetches a manifest on its own account, and a launcher fetches
    /// an icon days later -- neither carrying anything the page was given. So
    /// both answer without the token, and the test is that nothing else moved
    /// with them.
    #[test]
    fn the_home_screen_gets_what_it_needs_and_nothing_more() {
        let ui = RemoteUi::start(
            "127.0.0.1".parse().unwrap(),
            0,
            "board-token-0000".into(),
            String::new(),
        )
        .unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let phone = Phone::new(&base);

        let (code, body) = phone.said(crate::pwa::MANIFEST_PATH);
        assert_eq!(code, 200, "マニフェストはトークン無しで読めないと install できない");
        assert!(!body.contains("board-token-0000"), "鍵は載らない");
        let m: serde_json::Value = serde_json::from_str(&body).unwrap();
        for i in m["icons"].as_array().unwrap() {
            let src = i["src"].as_str().unwrap();
            assert_eq!(phone.status(src), 200, "{src} が 404 だと install ごと拒否される");
        }
        assert_eq!(phone.status("/apple-touch-icon.png"), 200);

        // Everything that can read or move something is where it was.
        assert_eq!(phone.status("/api/state"), 403);
        assert_eq!(phone.status("/cfg"), 403);
        assert_eq!(phone.status("/pwa/nothing.png"), 403);
        ui.shutdown();
    }

    /// Holds one reply page open, on this machine, for a person to look at.
    ///
    /// Ignored by default and run by hand (`cargo test -- --ignored
    /// hold_a_reply_page --nocapture`). The automated test above proves the
    /// door is the right size; this one is for the other question, which no
    /// assertion answers: whether the page reads like something a person would
    /// answer from a phone. It prints the link and then reports whatever is
    /// said back through it.
    #[test]
    #[ignore]
    fn hold_a_reply_page_open() {
        let ui = RemoteUi::start(
            "127.0.0.1".parse().unwrap(),
            0,
            "board-token-0000".into(),
            // SHIKISHA_HOLD_PW=... to look at the password gate as well
            std::env::var("SHIKISHA_HOLD_PW").unwrap_or_default(),
        )
        .unwrap();
        let link = ui.reply_link(crate::reply::Ticket::new(
            Some("coder".into()),
            2,
            "レビュワー".into(),
            "テストは全部通りました。次はどうしますか？

1. このままコミットする
2. 先にドキュメントを直す
3. いったん止める".into(),
            "telegram".into(),
        ));
        println!("REPLY LINK: {link}");
        let until = std::time::Instant::now() + std::time::Duration::from_secs(180);
        while std::time::Instant::now() < until {
            if let Ok(cmd) = ui.rx.recv_timeout(std::time::Duration::from_millis(500)) {
                println!("GOT: {cmd:?}");
            }
        }
        ui.shutdown();
    }

    /// Holds a board open on the loopback, for a browser to subscribe against.
    ///
    /// Ignored by default and run by hand (`cargo test -- --ignored
    /// hold_a_board_for_push --nocapture`). `127.0.0.1` is a secure context as
    /// far as a browser is concerned, which is what makes it possible to test
    /// the whole push path on one machine, with no certificate and no phone:
    /// open the printed address, subscribe, and the subscription is a real one
    /// with the browser vendor's own push service.
    #[test]
    #[ignore]
    fn hold_a_board_for_push() {
        let ui = RemoteUi::start(
            "127.0.0.1".parse().unwrap(),
            0,
            "board-token-0000".into(),
            String::new(),
        )
        .unwrap();
        println!("ORIGIN: {}", ui.url.split("/?").next().unwrap());
        println!("VAPID: {:?}", crate::push::public_key());
        std::thread::sleep(std::time::Duration::from_secs(240));
        ui.shutdown();
    }

    /// A board that listens on a network address answers on the loopback too.
    ///
    /// This is what lets `tailscale serve` put HTTPS in front of it. That
    /// proxy will not route to this machine's own tailnet address -- pointed
    /// at one, every request came back 502 -- so the only way in for it is
    /// 127.0.0.1. If this ever stops being true, HTTPS stops working and the
    /// only symptom is a phone that cannot install the page.
    ///
    /// Skipped where the machine has no address but the loopback, which is the
    /// one case where a second door would be the same door.
    #[test]
    fn a_board_on_the_network_answers_on_the_loopback_too() {
        let Some(ip) = crate::netaddr::tailscale_ip().or_else(crate::netaddr::lan_ip) else {
            println!("no network address on this machine; nothing to test");
            return;
        };
        let ui = RemoteUi::start(ip, 0, "board-token-0000".into(), String::new()).unwrap();
        let port = ui.port();
        let phone = Phone::new(&format!("http://127.0.0.1:{port}"));
        assert_eq!(
            phone.status(crate::pwa::MANIFEST_PATH),
            200,
            "127.0.0.1:{port} が答えない = HTTPS を前に立てられない"
        );
        // The same board, so the same rules: the loopback is a way in for a
        // proxy on this machine, not a way past the token.
        assert_eq!(phone.status("/api/state"), 403, "ループバックが鍵を迂回している");

        // ...and both doors are let go of together, or the next start fails.
        ui.shutdown();
        assert!(
            RemoteUi::start(ip, port, "board-token-0000".into(), String::new()).is_ok(),
            "閉じたのに片方のポートが残っている"
        );
    }

    /// A password does not close the reply link. It asks, once per device.
    ///
    /// This used to answer 403 and be done with it, which protected nothing
    /// that asking does not and cost the feature to everybody who set a second
    /// factor. What it must still not do is let the answer through before the
    /// password, tell an unproven visitor anything about the tab, or turn into
    /// a way onto the board.
    #[test]
    fn a_password_is_asked_once_and_then_remembered() {
        let ui = RemoteUi::start(
            "127.0.0.1".parse().unwrap(),
            0,
            "board-token-0000".into(),
            "aikotoba".into(),
        )
        .unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let link = ui.reply_link(crate::reply::Ticket::new(
            Some("coder".into()),
            1,
            "レビュワー".into(),
            "終わりました。進めますか".into(),
            String::new(),
        ));
        let id = link.rsplit('/').next().unwrap().to_string();
        let mut phone = Phone::new(&base);

        // Locked: a page that asks, and says nothing about the tab.
        let body = phone.text(&format!("/r/{id}"));
        assert!(body.contains("password") || body.contains("パスワード"), "訊いていない");
        assert!(!body.contains("レビュワー"), "名乗る前にタブ名を見せている");
        assert!(!body.contains("終わりました"), "名乗る前に答えを見せている");

        // ...and nothing can be said through it yet. The proof is not in what
        // the answer looks like -- the asking page comes back either way --
        // but in whether anything reached the board behind it.
        let (code, said) = phone.said_post(
            &format!("/r/{id}/say"),
            "{\"text\":\"勝手に送る\"}",
        );
        assert_eq!(code, 200);
        assert!(said.contains("<!doctype"), "送信が受け付けられている: {said}");
        assert!(
            ui.rx.try_recv().is_err(),
            "パスワードを言う前の書き込みがタブに届いてしまう"
        );

        // A wrong password is refused, and hands out nothing to keep.
        let mut wrong = phone.post(&format!("/r/{id}/unlock"), "{\"password\":\"chigau\"}");
        assert_eq!(wrong.status().as_u16(), 200);
        assert!(!wrong.body_mut().read_to_string().unwrap_or_default().contains("true"));
        assert!(wrong.headers().get("set-cookie").is_none(), "誤答に鍵を渡している");

        // The right one is remembered in a cookie of the reply pages' own.
        let right = phone.post(&format!("/r/{id}/unlock"), "{\"password\":\"aikotoba\"}");
        let set = right
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert!(set.starts_with("rq="), "返信ページ専用のクッキーではない: {set}");
        // Lax, and it has to be: a reply link is followed from a chat app,
        // which is another site, and a Strict cookie is withheld on exactly
        // that navigation -- so the password would be asked every single time.
        assert!(set.contains("SameSite=Lax"), "Strict では chat から来たとき送られない: {set}");
        assert!(set.contains("Path=/r/"), "返信ページの外まで届いてしまう: {set}");
        assert!(set.contains("HttpOnly"), "ページから読み出せてしまう: {set}");

        phone.also(set.split(';').next().unwrap_or(""));

        // Now the real page, and a reply that lands.
        let body = phone.text(&format!("/r/{id}"));
        assert!(body.contains("レビュワー"), "名乗ったのに開かない");
        let (code, said) = phone.said_post(&format!("/r/{id}/say"), "{\"text\":\"進めて\"}");
        assert_eq!(code, 200);
        assert!(said.contains("true"), "送れない: {said}");

        // What it is not: a way onto the board. That still wants the token,
        // the session, and the password on its own cookie.
        assert_eq!(phone.status("/api/state"), 403);
        assert_eq!(
            phone.said("/api/state?t=board-token-0000"),
            (403, "cut".to_string()),
            "返信の鍵で盤面が開いてしまう"
        );
        ui.shutdown();
    }

    #[test]
    fn rotating_the_token_cuts_the_old_session() {
        let mut ui =
            RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "old-token-000000".into(), String::new()).unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let mut phone = Phone::new(&base);
        phone.pair("old-token-000000");

        assert_eq!(phone.state("old-token-000000"), 200, "old token should work before the cut");
        ui.rotate_token("new-token-111111".into());
        assert_eq!(phone.state("old-token-000000"), 403, "old token must be dead after the cut");
        assert_eq!(
            phone.state("new-token-111111"),
            403,
            "learning the new token is not enough — the link has to be opened again"
        );
        phone.pair("new-token-111111");
        assert_eq!(phone.state("new-token-111111"), 200, "re-pairing must let it back in");
        assert!(ui.url.ends_with("t=new-token-111111"), "url not rebuilt: {}", ui.url);
    }

    /// The disconnect has to hold even when the token cannot change — a fixed
    /// token (remote.sticky_token) is a string the person wrote into settings,
    /// so there is nothing to rotate. Dropping the sockets alone was not enough:
    /// the page reconnected by itself a second later and went on watching and
    /// driving. What is revoked is the session, and only opening the link again
    /// brings one back.
    #[test]
    fn a_fixed_token_disconnect_locks_the_phone_out_too() {
        let ui = RemoteUi::start(
            "127.0.0.1".parse().unwrap(),
            0,
            "fixed-token-22222".into(),
            String::new(),
        )
        .unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let mut phone = Phone::new(&base);
        phone.pair("fixed-token-22222");
        assert_eq!(phone.state("fixed-token-22222"), 200);

        ui.cut_sessions();
        assert_eq!(
            phone.said("/api/state?t=fixed-token-22222"),
            (403, "cut".to_string()),
            "the phone still holds the token — the session is what must be gone, \
             and the body is what puts up its ⛔ screen"
        );

        phone.pair("fixed-token-22222");
        assert_eq!(
            phone.state("fixed-token-22222"),
            200,
            "the link still pairs: a fixed token is a lasting pairing by choice"
        );
    }

    /// Re-opening the link on a phone that is already paired keeps the session
    /// it has, so a habit of reloading can't push the other devices out of a
    /// bounded list.
    #[test]
    fn reloading_the_link_keeps_the_session_it_already_has() {
        let ids = Ids::new();
        let first = ids.keep("");
        assert_eq!(ids.keep(&first), first, "a valid session must be kept as it is");
        assert!(!ids.keep("not-one-we-minted").is_empty());
        for _ in 0..MAX_SESSIONS {
            ids.keep("");
        }
        assert!(!ids.has(&first), "the oldest falls off rather than growing forever");
        let last = ids.keep("");
        ids.clear();
        assert!(!ids.has(&last), "a cut leaves nothing behind");
    }

    /// Off→on must rebind at once: shutdown() may only return after the
    /// accept thread is gone and the socket is closed. A stop flag alone left
    /// the port held until the next request happened to arrive, so re-enabling
    /// failed with AddrInUse (the settings toggle then reported "stopped" no
    /// matter which way it was flipped) until the app was restarted
    #[test]
    fn shutdown_releases_the_port_for_an_immediate_restart() {
        let ui =
            RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into(), String::new()).unwrap();
        let port: u16 = ui
            .origin
            .rsplit(':')
            .next()
            .unwrap()
            .parse()
            .expect("originにポートが無い");
        ui.shutdown();
        let again = RemoteUi::start("127.0.0.1".parse().unwrap(), port, "tok123456789012".into(), String::new())
            .expect("ポートが解放されていない");
        again.shutdown();
    }

    /// The cost of wrong passwords: two slips are free, the third shuts the
    /// door for a second, the wait doubles to a minute and no further, a right
    /// password clears it, and ten quiet minutes forget it. Told with a clock
    /// that is handed in, so nothing here waits
    #[test]
    fn wrong_passwords_shut_the_door_for_longer_each_time() {
        let t0 = Instant::now();
        let mut m = Misses::new(t0);
        assert_eq!(m.wait_at(t0), None, "最初から閉まっている");
        m.missed_at(t0);
        assert_eq!(m.wait_at(t0), None, "一度の打ち間違いで待たされる");
        m.missed_at(t0);
        assert_eq!(m.wait_at(t0), None, "二度の打ち間違いで待たされる");
        m.missed_at(t0);
        assert_eq!(m.wait_at(t0), Some(Duration::from_secs(1)), "三度目で閉まらない");
        assert_eq!(
            m.wait_at(t0 + Duration::from_millis(400)),
            Some(Duration::from_millis(600)),
            "残り時間が減らない"
        );
        assert_eq!(m.wait_at(t0 + Duration::from_secs(1)), None, "時間が来ても開かない");
        // Doubling: 1, 2, 4, 8, 16, 32, 60, 60...
        for (n, secs) in [(4, 2), (5, 4), (6, 8), (7, 16), (8, 32), (9, 60), (10, 60), (40, 60)] {
            assert_eq!(
                Misses::shut_for(n),
                Duration::from_secs(secs),
                "{n}回目の待ち時間が違う"
            );
        }
        // A right password clears the score
        m.forgive();
        assert_eq!(m.wait_at(t0 + Duration::from_secs(2)), None, "正解のあとも閉まっている");
        // Ten quiet minutes forget it: a person who slipped last week starts afresh
        for _ in 0..9 {
            m.missed_at(t0);
        }
        assert_eq!(m.wait_at(t0), Some(Duration::from_secs(60)));
        let later = t0 + Duration::from_secs(601);
        assert_eq!(m.wait_at(later), None, "十分経っても忘れない");
        m.missed_at(later);
        assert_eq!(m.wait_at(later), None, "忘れたあとの一度目で待たされる");
    }

    /// Over the wire: after wrong passwords in a row the door answers 429 with
    /// a Retry-After, to a right password as much as to a wrong one, and the
    /// phone that is already in keeps being served the while. The reply page's
    /// unlock shares the score, so a guesser turned away at one door cannot
    /// walk round to the other
    #[test]
    fn guessing_the_password_is_turned_away_at_both_doors() {
        let ui = RemoteUi::start(
            "127.0.0.1".parse().unwrap(),
            0,
            "tok123456789012".into(),
            "aikotoba".into(),
        )
        .unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        // One phone that is already in
        let mut inside = Phone::new(&base);
        inside.pair("tok123456789012");
        let resp = inside.post("/auth?t=tok123456789012", r#"{"password":"aikotoba"}"#);
        assert_eq!(resp.status().as_u16(), 200);
        let cookie = resp
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        inside.also(&cookie);
        assert_eq!(inside.state("tok123456789012"), 200);

        // And one guessing
        let mut guesser = Phone::new(&base);
        guesser.pair("tok123456789012");
        for _ in 0..3 {
            assert_eq!(
                guesser.said_post("/auth?t=tok123456789012", r#"{"password":"chigau"}"#).0,
                403
            );
        }
        // The door is shut now — to the right password as well
        let shut = guesser.post("/auth?t=tok123456789012", r#"{"password":"aikotoba"}"#);
        assert_eq!(shut.status().as_u16(), 429, "続けて間違えても閉まらない");
        let after = shut
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .expect("Retry-After が無い");
        assert!((1..=60).contains(&after), "待ち時間の案内がおかしい: {after}");
        // ...and so is the reply page's, on the same score
        let link = ui.reply_link(crate::reply::Ticket::new(
            Some("coder".into()),
            1,
            "reviewer".into(),
            "done".into(),
            String::new(),
        ));
        let ticket = link.rsplit('/').next().unwrap().to_string();
        let (status, body) =
            guesser.said_post(&format!("/r/{ticket}/unlock"), r#"{"password":"aikotoba"}"#);
        assert_eq!(status, 429, "返信ページの扉が別勘定になっている");
        assert!(body.contains("\"wait\""), "待ち時間を返していない: {body}");
        // The phone that is in was never made to wait
        assert_eq!(inside.state("tok123456789012"), 200, "中の人まで止まった");
        ui.shutdown();
    }

    /// Actually starts the server and confirms auth and command delivery
    #[test]
    fn password_gate_requires_the_second_factor() {
        // With remote.password set, the URL token alone opens nothing:
        // data routes say "password" until /auth trades it for a cookie
        let ui = RemoteUi::start(
            "127.0.0.1".parse().unwrap(),
            0,
            "tok123456789012".into(),
            "aikotoba".into(),
        )
        .unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let mut phone = Phone::new(&base);
        phone.pair("tok123456789012");

        // Token and session only → refused, with the body that tells the shell
        // to prompt (and not the one that tells it the line was cut)
        assert_eq!(
            phone.said("/api/state?t=tok123456789012"),
            (403, "password".to_string()),
            "パスワード未提示で通ってしまう"
        );

        // Wrong password → refused
        assert_eq!(
            phone.said_post("/auth?t=tok123456789012", r#"{"password":"chigau"}"#),
            (403, "forbidden".to_string()),
            "誤パスワードで通る"
        );
        // The password rides in the body only. In the address — where the old
        // route took it, and where proxies and histories keep it — it is not a
        // password at all, and this is a phone that has not given one
        assert_eq!(
            phone.said("/auth?t=tok123456789012&p=aikotoba"),
            (403, "password".to_string()),
            "アドレスに書いたパスワードが通る"
        );

        // Right password → a session cookie, and data routes open with it
        let resp = phone.post("/auth?t=tok123456789012", r#"{"password":"aikotoba"}"#);
        assert_eq!(resp.status().as_u16(), 200, "正しいパスワードが通らない");
        let cookie = resp
            .headers()
            .get("set-cookie")
            .expect("セッションクッキーが出ない")
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        phone.also(&cookie);
        assert_eq!(phone.state("tok123456789012"), 200, "クッキー提示でも開かない");

        // No token at all stays refused even with the cookies
        assert_eq!(phone.status("/api/state"), 403, "トークン無しは常に拒否");

        // The disconnect takes the password with it: the device unlocks again
        // only after the person there says so
        ui.cut_sessions();
        phone.pair("tok123456789012");
        assert_eq!(
            phone.said("/api/state?t=tok123456789012"),
            (403, "password".to_string()),
            "切断後もパスワードが効いたままになっている"
        );
        ui.shutdown();
    }

    /// A phone reduced to polling still counts as someone watching.
    ///
    /// What the terminals are cut to hangs on that answer (see `terminal_size`
    /// in the main loop): counted only by the state socket, a phone whose
    /// network won't hold one open would be handed a terminal shaped for the
    /// window and left reading it sideways -- the very thing the fitting is for.
    #[test]
    fn a_phone_that_can_only_poll_is_still_watching() {
        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into(), String::new()).unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let mut phone = Phone::new(&base);
        assert!(!ui.watched(), "誰も来ていないのに見られている扱い");
        phone.pair("tok123456789012");
        // Pairing alone is not watching: the page has to ask for the state
        assert!(!ui.watched(), "ページを開いただけで見られている扱い");
        // Nor does a request that is refused -- it never saw the state
        let stranger = Phone::new(&base);
        assert_eq!(stranger.status("/api/state?t=tok123456789012"), 403);
        assert!(!ui.watched(), "断った相手が見ている扱いになった");
        phone.get("/api/state?t=tok123456789012");
        assert!(ui.watched(), "状態を取りに来た相手が見ている扱いにならない");
        ui.shutdown();
    }

    #[test]
    fn serves_state_and_forwards_commands() {
        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into(), String::new()).unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let mut phone = Phone::new(&base);

        // Neither half alone opens anything: no token is refused, and the token
        // without a session (a phone that never opened the link, or one the PC
        // has since cut) is refused too
        assert_eq!(phone.status("/api/state"), 403);
        assert_eq!(phone.said("/api/state?t=tok123456789012"), (403, "cut".to_string()));
        phone.pair("tok123456789012");

        // Returns state
        ui.snapshot.lock().unwrap().tabs = vec![RemoteTab {
            index: 1,
            name: "実装".into(),
            state: "QUESTION".into(),
            ..Default::default()
        }];
        let body = phone.text("/api/state?t=tok123456789012");
        assert!(body.contains("実装") && body.contains("QUESTION"), "{body}");

        // The instruction reaches the main loop
        phone.post("/api/send?t=tok123456789012", r#"{"tab":1,"text":"続けて"}"#);
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Send { tab, text } => {
                assert_eq!((tab, text.as_str()), (1, "続けて"));
            }
            other => panic!("想定外: {other:?}"),
        }

        // The entry point serves the same shell as the window — and now WITHOUT a
        // token, so a paired phone can strip `?t=…` from its URL yet still reload.
        // The page is inert (no secrets, no state); every data route stays gated,
        // asserted above (/api/state with no token = 403). There used to be a
        // separate, old phone-only page that never once reached the phone side.
        for entry in ["/", "/shell"] {
            let page = phone.text(entry);
            assert!(
                page.contains("api/intent") && page.contains("window.__state"),
                "{entry} が（トークン無しで）窓と同じ外皮を配っていない"
            );
        }

        // Operations from the screen reach the main loop
        phone.post("/api/intent?t=tok123456789012", r#"{"kind":"select","tab":2}"#);
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Ui(crate::browser::Ev::Select { tab }) => assert_eq!(tab, 2),
            other => panic!("想定外: {other:?}"),
        }

        // The "back" button on the top bar also reaches the main loop (both
        // the allow-list and the path). It used to be blocked by the
        // allow-list, and after that fix, silently dropped by keys_for
        phone.post("/api/intent?t=tok123456789012", r#"{"kind":"go","what":"back"}"#);
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Ui(crate::browser::Ev::Go {
                go: crate::browser::Go::Back,
            }) => {}
            other => panic!("戻るが本体まで届かない: {other:?}"),
        }

        // Something only the window can answer is stopped as soon as it's
        // received. That it didn't get through is confirmed on the next
        // receive (select comes through first)
        phone.post("/api/intent?t=tok123456789012", r#"{"kind":"menu","key":"k"}"#);
        phone.post("/api/intent?t=tok123456789012", r#"{"kind":"select","tab":3}"#);
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Ui(crate::browser::Ev::Select { tab }) => {
                assert_eq!(tab, 3, "止めたはずの操作が先に届いた")
            }
            other => panic!("窓にしか答えられないものが通った: {other:?}"),
        }
        ui.shutdown();
    }

    /// `/api/*` routing: the remote UI owns exactly its own verbs; every other
    /// `/api/*`, plus the settings pages, is the config server's.
    #[test]
    fn settings_paths_are_told_apart_from_the_remotes_own() {
        for p in ["/cfg", "/help", "/result", "/api/config", "/api/secrets", "/api/secrets/set"] {
            assert!(is_settings_path(p), "{p} should proxy to settings");
        }
        for p in [
            "/api/state", "/api/send", "/api/auto", "/api/intent", "/api/attach", "/api/read",
            "/api/replay", "/", "/shell", "/ws-state",
        ] {
            assert!(!is_settings_path(p), "{p} is the remote's own route");
        }
    }

    /// Every route this file answers is claimed in `OWN_VERBS`.
    ///
    /// Written because the two lists drifted once and nothing said so: a route
    /// that is not claimed is handed to the settings proxy, and the handler
    /// sitting right here simply never runs. Nothing about that looks broken
    /// from the outside — it looks like the feature was never built.
    #[test]
    fn every_verb_this_file_serves_is_claimed() {
        for line in include_str!("remote.rs").lines() {
            let Some(at) = line.find("\"/api/") else { continue };
            if !line.contains("=> {") {
                continue;
            }
            let verb = line[at + 6..].split(['"', '/', '?']).next().unwrap_or("");
            assert!(
                OWN_VERBS.contains(&verb),
                "/api/{verb} を自分で処理しているのに OWN_VERBS に無い (設定プロキシに吸われて到達しない)"
            );
        }
    }

    /// The settings proxy: no token is refused; the first hop with `?t=` is traded
    /// for a SameSite=Strict cookie and a clean redirect; a cookie-authed hop with
    /// no backend wired yet answers 503 (never falls through to the shell).
    #[test]
    fn settings_proxy_gates_and_hands_off_a_cookie() {
        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into(), String::new()).unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        // Phone does not follow redirects, so the 302 can be inspected.
        let mut phone = Phone::new(&base);

        // No token → 403
        assert_eq!(phone.status("/cfg"), 403, "unauthenticated /cfg must be refused");
        // The token without a session is refused as well: the settings screen is
        // reached on the same admission as the board, so a cut takes it too
        assert_eq!(phone.said("/cfg?t=tok123456789012"), (403, "cut".to_string()));
        phone.pair("tok123456789012");

        // First hop with ?t= → 302 + Set-Cookie sst=<token>, Location /cfg
        let r = phone.get("/cfg?t=tok123456789012");
        assert_eq!(r.status().as_u16(), 302, "the pairing hop should redirect");
        let cookie = r.headers().get("set-cookie").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert!(cookie.contains("sst=tok123456789012"), "cookie not set: {cookie}");
        assert!(cookie.contains("SameSite=Strict") && cookie.contains("HttpOnly"), "weak cookie: {cookie}");
        let loc = r.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("");
        assert_eq!(loc, "/cfg", "should bounce to a clean /cfg");

        // Cookie-authed hop, but no backend wired → 503 (not the shell, not a 404)
        phone.also("sst=tok123456789012");
        assert_eq!(phone.status("/cfg"), 503, "no backend yet → 503");
        ui.shutdown();
    }

    /// End to end, phone → proxy → settings server: pressing what used to be the
    /// folder button must come back as a refusal. Before this, the settings server
    /// opened a native picker on the PC and the phone's request hung until it was
    /// dismissed — the phone looking frozen. The proxy is what marks the caller as
    /// remote, so this is the piece the settings-side test can't see.
    #[test]
    fn the_phone_never_opens_a_picker_on_the_pc() {
        let dir = std::env::temp_dir().join(format!("shikitest_{}", crate::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, "{}").unwrap();
        let settings = crate::webui::WebUi::start_with(
            cfg,
            Arc::new(Mutex::new(crate::webui::RemoteInfo::default())),
            Arc::new(Mutex::new(None)),
        )
        .unwrap();
        let (origin, sett_token) = settings.url.split_once("/?token=").unwrap();

        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into(), String::new()).unwrap();
        ui.set_settings_backend(origin.to_string(), sett_token.to_string());
        let base = ui.url.split("/?").next().unwrap().to_string();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            // Well under the proxy's own 120s: a hang must fail the test, not stall it
            .timeout_global(Some(std::time::Duration::from_secs(15)))
            .build()
            .new_agent();
        // What the phone's browser holds by now: the session from opening the
        // link, and the settings hop's own cookie
        let mut paired = Phone::new(&base);
        paired.pair("tok123456789012");
        let cookie = format!("sst=tok123456789012; {}", paired.cookie);

        let mut r = agent
            .post(&format!("{base}/api/pick"))
            .header("Cookie", &cookie)
            .header("Content-Type", "application/json")
            .send(r#"{"kind":"dir"}"#)
            .unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&r.body_mut().read_to_string().unwrap()).unwrap();
        assert_eq!(v["ok"], serde_json::json!(false), "the picker answered a phone: {v}");

        // ...and the page the phone reads knows to leave the button off
        let mut r = agent
            .get(&format!("{base}/cfg"))
            .header("Cookie", &cookie)
            .call()
            .unwrap();
        let html = r.body_mut().read_to_string().unwrap();
        assert!(html.contains("const REMOTE = true;"), "the phone's settings page must know it is remote");

        ui.shutdown();
        settings.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The hop that trades the token for a cookie must not eat the deep-link.
    ///
    /// The phone reaches the settings as a page, so "open it already adding a tab"
    /// travels as `?addtab=`. Bouncing to a bare `/cfg` threw that away and the
    /// tab bar's + looked broken. The token itself still has to be gone.
    #[test]
    fn the_pairing_hop_keeps_which_screen_to_open() {
        assert_eq!(carried_query("/cfg?t=secret123&addtab=2"), "?addtab=2");
        assert_eq!(carried_query("/cfg?t=secret123&section=actions&ret=1"), "?section=actions&ret=1");
        assert_eq!(carried_query("/cfg?t=secret123"), "", "トークンだけなら何も残さない");
        assert_eq!(carried_query("/cfg"), "");
        // The result goes into a Location header, so nothing that isn't plain
        // query text is carried over — dropped outright rather than escaped
        assert_eq!(carried_query("/cfg?t=a&x=1&bad=a b"), "?x=1");
        assert_eq!(
            carried_query("/cfg?t=a&evil=%0d%0aSet-Cookie:%20x"),
            "",
            "ヘッダを割りに来る細工は丸ごと落とす"
        );
        // Percent-encoded text survives (it is literal text in a header, not a
        // break), so ordinary encoded values still reach the page
        assert_eq!(carried_query("/cfg?t=a&section=a%2Db"), "?section=a%2Db");
    }

    /// Confirms /ws handshakes and that a JPEG pushed via push_frame arrives
    /// as a WS binary frame. Verified end to end with just a raw TCP
    /// connection and our own ws module (no phone or external tool needed)
    #[test]
    fn ws_upgrades_and_delivers_a_frame() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into(), String::new()).unwrap();
        let hostport = ui
            .url
            .trim_start_matches("http://")
            .split("/?")
            .next()
            .unwrap()
            .to_string();

        // The relay is a data route like any other: it opens for a phone that
        // has paired, and the handshake carries that session as a browser would
        let mut phone = Phone::new(&format!("http://{hostport}"));
        phone.pair("tok123456789012");

        let mut sock = TcpStream::connect(&hostport).unwrap();
        // Same key as the RFC 6455 example (accept becomes s3pP...)
        let req = format!(
            "GET /ws?t=tok123456789012 HTTP/1.1\r\n\
             Host: localhost\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Cookie: {}\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n",
            phone.cookie
        );
        sock.write_all(req.as_bytes()).unwrap();

        // Read the response headers up to \r\n\r\n
        let mut buf = Vec::new();
        let mut one = [0u8; 1];
        loop {
            sock.read_exact(&mut one).unwrap();
            buf.push(one[0]);
            if buf.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let head = String::from_utf8_lossy(&buf);
        assert!(head.contains("101"), "101 で格上げされていない: {head}");
        assert!(
            head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "Sec-WebSocket-Accept が違う: {head}"
        );

        // Wait out the gap until registration is done, then push a frame
        std::thread::sleep(std::time::Duration::from_millis(200));
        ui.push_frame(vec![0xDE, 0xAD, 0xBE, 0xEF]);

        // Server-to-client frames are unmasked. Unpack it directly here
        let mut hdr = [0u8; 2];
        sock.read_exact(&mut hdr).unwrap();
        assert_eq!(hdr[0] & 0x0F, 0x2, "バイナリフレームでない");
        let len = (hdr[1] & 0x7F) as usize;
        assert_eq!(hdr[1] & 0x80, 0, "サーバーフレームにマスクが付いている");
        let mut payload = vec![0u8; len];
        sock.read_exact(&mut payload).unwrap();
        assert_eq!(payload, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        ui.shutdown();
    }

    /// Confirms /ws-in handshakes and that a sent input intent (finger trail) reaches the main loop
    #[test]
    fn ws_in_forwards_injected_input() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into(), String::new()).unwrap();
        let hostport = ui
            .url
            .trim_start_matches("http://")
            .split("/?")
            .next()
            .unwrap()
            .to_string();

        let mut phone = Phone::new(&format!("http://{hostport}"));
        phone.pair("tok123456789012");

        let mut sock = TcpStream::connect(&hostport).unwrap();
        let req = format!(
            "GET /ws-in?t=tok123456789012 HTTP/1.1\r\n\
             Host: localhost\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Cookie: {}\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n",
            phone.cookie
        );
        sock.write_all(req.as_bytes()).unwrap();
        let mut buf = Vec::new();
        let mut one = [0u8; 1];
        loop {
            sock.read_exact(&mut one).unwrap();
            buf.push(one[0]);
            if buf.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        assert!(String::from_utf8_lossy(&buf).contains("101"));

        // Client-to-server frames must be masked. Send an intent as text
        let intent = r#"{"kind":"inject","what":"mouse","phase":"pressed","x":0.5,"y":0.25}"#;
        sock.write_all(&mask_text_frame(intent)).unwrap();

        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Ui(crate::browser::Ev::Inject {
                input: crate::browser::Input::Mouse { phase, x, y, .. },
                ..
            }) => {
                assert_eq!(phase, "pressed");
                assert!((x - 0.5).abs() < 1e-9 && (y - 0.25).abs() < 1e-9);
            }
            other => panic!("軌跡が届いていない: {other:?}"),
        }

        // ...and once the PC ends the session, the very same line reaches
        // nothing. This socket is still open at the phone's end — which is
        // exactly the state the bug lived in: the picture was gone, so the
        // person believed the link was down, while every touch still landed on
        // a real browser.
        ui.cut_sessions();
        sock.write_all(&mask_text_frame(intent)).unwrap();
        assert!(
            ui.rx
                .recv_timeout(std::time::Duration::from_millis(700))
                .is_err(),
            "切断したはずの端末の操作が本体まで届いている"
        );
        ui.shutdown();
    }

    /// Test helper: build a text frame the way a client must (masked)
    fn mask_text_frame(s: &str) -> Vec<u8> {
        let payload = s.as_bytes();
        let mut out = vec![0x81u8]; // FIN + text
        let mask = [0xA1u8, 0xB2, 0xC3, 0xD4];
        let len = payload.len();
        assert!(len < 126, "テストの本文は126バイト未満");
        out.push(0x80 | len as u8);
        out.extend_from_slice(&mask);
        out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i & 3]));
        out
    }
}

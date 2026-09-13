//! The words the runtime and its shell say to each other.
//!
//! A shell -- the window on this machine, a browser on a phone, whatever comes
//! next -- reports what a person did as an [`Ev`], and is handed a picture of
//! the world back. Nothing here knows how any of that is drawn or delivered,
//! which is the point: these types are what both sides can agree on without
//! either of them having to know the other exists.
//!
//! They lived in the window's own module until 2026-09-11, which meant the
//! runtime could not be built without the window it was supposed to be
//! independent of.

/// A single input event for the screencast view. Coordinates arrive as a
/// fraction (0.0-1.0) of the screencast frame and get converted to real
/// pixels. This lets the same spot be pointed at even when the sender's
/// screen size or DPR differs
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Input {
    /// Mouse down/move/up. A drag is expressed as a chain of moves
    Mouse {
        /// "pressed" / "released" / "moved"
        phase: String,
        x: f64,
        y: f64,
        /// true if this move happens while the button is held (needed to replay drags)
        down: bool,
    },
    /// Wheel. dx/dy are in pixels
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    /// Insert an already-committed string at the current focus (IME conversion is done on the sender's side)
    Text { text: String },
    /// A named control key (Enter / Backspace / Tab / F1-F12, etc).
    /// ctrl/alt can be composed from the fixed toggles in the auxiliary
    /// key row (e.g. Ctrl+C)
    Key { named: String, ctrl: bool, alt: bool },
    /// The viewer's screen shape in CSS pixels. The page's viewport gets
    /// re-shaped to the same aspect ratio (keeping the PC-side width) so a
    /// portrait phone sees a full screen instead of a letterboxed strip
    View { w: f64, h: f64 },
}

/// A navigation request sent to the browser.
///
/// We could have the page call `history.back()` instead, but then we
/// wouldn't know when there's nowhere left to go, and an unpressable
/// button would show up looking pressable. The window itself knows
/// whether it can go back, so we ask it
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Go {
    Back,
    Forward,
    Reload,
    /// Reload having thrown away everything already held for the page.
    ///
    /// The ordinary reload is allowed to serve a cached copy, which is
    /// exactly wrong for the case a person presses it in: a page that is
    /// wrong, from a build that has moved on. Every browser hides this behind
    /// a modifier on the same button, and so does this one
    Hard,
    To(String),
}

/// One pane as the page measured it.
///
/// Rows and columns are what the terminal in that pane must be resized to;
/// the rect is where a browser placed in that pane has to sit. Only the page
/// can work these out — it owns the font metrics and the dividers — so they
/// are reported, never guessed on this side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PaneGeom {
    pub id: u32,
    pub rows: u16,
    pub cols: u16,
    pub rect: (i32, i32, i32, i32),
}

/// A report from the browser to the conductor
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub enum Ev {
    /// A document finished loading (arrives on every navigation).
    /// `from` is the name of the page that loaded (`None` is the main view)
    Ready {
        from: Option<String>,
        url: String,
        /// Whether referenced resources finished loading too.
        /// `false` means "`load` never fired, so we announced at the DOM-only point"
        complete: bool,
    },
    /// The result of `Eval`. `value` is JSON
    Result { id: u64, ok: bool, value: String },
    /// The window's ✕ was pressed. Not acted on here: whether that puts the
    /// window away or ends the program is the conductor's call (a setting,
    /// and a question when an AI is at work)
    CloseRequested,
    /// The notification-area icon was pressed, or "Open" chosen on its menu
    TrayOpen,
    /// "Quit" chosen on the notification-area icon's menu
    TrayQuit,
    /// The bar's button was pressed = the human finished their turn.
    /// `from` is the name of the page it was pressed on (`None` is the
    /// main view). Since multiple pages can be placed at once, without
    /// tracking which one it was, a neighboring browser's turn could
    /// wrongly be marked as finished
    /// The bar asking the person something was pressed. `from` is the page it
    /// stands under, as automation addresses it. Sent by the board (and the
    /// phone), never by a page: the bar is the app's own so that only a person
    /// can press it
    Button { from: Option<String> },
    /// A page placed in the window has the keyboard: whoever is at the machine
    /// is working there, so that is the pane in focus
    Touched { from: Option<String> },
    /// The pen a placed page draws for itself was pressed: open the composer
    Compose { from: Option<String> },
    /// From the window's own page: whether a placed page should be drawing
    /// that pen at all. Which page is the window's to work out -- it is the one
    /// in the focused pane -- so only the open/closed state travels here
    Pen { on: bool },
    /// The window's size changed (how many rows/columns fit)
    Resize {
        rows: u16,
        cols: u16,
        /// The content area (x, y, width, height). The browser is placed here
        area: (i32, i32, i32, i32),
        /// The whole content area, whatever the panes are doing. Where a screen
        /// that covers the window goes -- the settings form is one, and it is a
        /// page placed in the window, so it needs a rectangle like any other
        full: (i32, i32, i32, i32),
        /// Every pane's own measurements. One entry while the content area is
        /// undivided; one per pane once it is split
        panes: Vec<PaneGeom>,
    },
    /// A pane was clicked. Focus follows the click, the way it does in the tab bar
    FocusPane { id: u32 },
    /// A pane's ✕ was pressed. Closes the view, never the tab behind it
    ClosePane { id: u32 },
    /// A divider was dragged (or double-clicked, which asks for an even half).
    /// `divider` is its position in `Layout::dividers()` — the page is handed
    /// that list and hands the number back, so neither side has to work out
    /// which split a boundary belongs to from the way it looks.
    /// `ratio` is the first half's new share
    PaneRatio { divider: usize, ratio: f32 },
    /// A pane's ⊞ / ⊟ was pressed: divide that pane, the same as the keyboard
    SplitPane { id: u32, down: bool },
    /// The terminal was zoomed with Ctrl+wheel. The page has already redrawn
    /// itself at the new size; this is so it is still that size tomorrow
    FontSize { px: u8 },
    /// The tab bar's edge was dragged. Same story as the zoom: the page is
    /// already drawn that way, and this is so it opens that way next time.
    /// 0 means the bar has been put away
    TabWidth { px: u16 },
    /// The right-hand column's edge was dragged, or the column was put away or
    /// brought back. Same story as the tab bar: the page is already drawn that
    /// way, and this is so it opens that way next time. 0 means put away
    SideWidth { px: u16 },
    /// Wants to view this tab (0 = the operating board)
    Select { tab: usize },
    /// A tab has been asked for: the + on the tab bar, or the invitation in a
    /// pane with nothing in it. `pane` is that pane, when one asked -- the new
    /// tab goes there rather than wherever focus has wandered to by the time
    /// the form is done with
    AddTab {
        pane: Option<u32>,
        /// The folder it was asked for from, when it was asked for from one.
        /// Without it the form has to guess, and it guessed the first folder --
        /// so a tab added from the third one appeared in the first
        folder: Option<String>,
    },
    /// A folder's heading was asked about another branch of the same project.
    /// `from` is the folder it would be cut from, and `make` says whether this
    /// is the question or the answer -- the same message either way, so what is
    /// shown before it happens and what happens cannot describe two things
    /// Putting a working folder back on this machine: asked about, then asked
    /// for. The same event does both, so what is shown is what happens
    Repair {
        folder: String,
        /// The project it belongs to, in answer to the one question that has to
        /// be asked -- a remote URL, or "folder" for an ordinary folder. Empty
        /// while nothing has been chosen
        choose: String,
        /// The branch it holds, alongside that answer
        branch: String,
        /// Whether to go ahead with what was shown
        take: bool,
    },
    Branch {
        from: String,
        branch: String,
        /// What to grow it from. Empty asks for the sensible one
        base: String,
        make: bool,
        /// What to bring along, of what was offered
        carry: Vec<String>,
        /// What runs in the new folder: empty for the same tabs as the folder
        /// it is cut from, `none` for nothing, or the command of one AI
        start: String,
        /// One folder per AI named here, each branch named for its AI,
        /// instead of one folder. Empty means one
        ais: Vec<String>,
        /// Where to put it, when somebody would rather it went elsewhere.
        /// Empty is the place this app would choose
        at: String,
        /// The machine to make it on, by the name the settings gave it.
        /// Empty is this one
        host: String,
        /// Whether to run what the project says its environment needs. On
        /// unless somebody says otherwise: a folder that skipped it is a
        /// folder the first thing anybody does in is fail to build
        setup: bool,
    },
    /// Put the offered environment file in the project.
    ///
    /// Its own event and not a flag on the one above, because it is its own
    /// act: it changes the repository, it happens once, and it happens because
    /// somebody read what was offered and said yes. `from` is the folder whose
    /// project it is
    KeepEnv { from: String },
    /// A colour was chosen for the project a folder belongs to. Empty means
    /// "go back to the one you work out yourselves"
    FolderColor { folder: String, color: String },
    /// Looking for somewhere to work. An empty path asks for the drives;
    /// `open` says whether this is looking or choosing
    Browse { path: String, open: bool },
    /// A folder was renamed in the list, or taken out of it. An empty name
    /// hands it back to what the folder itself says
    FolderName { folder: String, name: String },
    /// A folder was closed: its tabs go, the files stay
    FolderClose { folder: String },
    /// A branch's folder was thrown away for good. Refused while there is
    /// anything in it that is not committed
    FolderDiscard { folder: String },
    /// "Close settings" on the settings page. Collapses the settings tab
    /// and returns to the operating board. This is a window-internal
    /// action, so it's not accepted from a phone (allowed_from_afar)
    CloseSettings,
    /// Open the settings page. A dedicated intent for the sidebar gear so it
    /// works from any tab (the menu "e" key only fires while INDEX is in view).
    /// Window-internal, so not accepted from a phone (allowed_from_afar).
    /// `section` deep-links to one settings card; `ret` asks the page to return
    /// to the board once it's saved (used by the sub-input bar's ⚙ shortcut).
    OpenSettings {
        section: Option<String>,
        ret: bool,
        /// A working folder to land on, when the ask came from its row
        folder: Option<String>,
        /// The tab in view when the gear was pressed, as its ordinal among the
        /// terminal tabs of `folder` (0-based). A place, not a name, so a tab
        /// that was never named lands the same. Paired with `folder`.
        tabpos: Option<u32>,
    },
    /// Save the newest run's replay.lua to the user's Downloads folder
    ReplaySave,
    /// ✨ natural language in, one suggested shell command out (assistant AI)
    Suggest { text: String },
    /// 🔍 run the environment survey in the active terminal (deterministic
    /// probe; its output becomes the tab's environment card)
    Survey,
    /// The operating board's menu was pressed
    Menu { key: String },
    /// Open the desk switcher. A dedicated intent (rather than reusing the
    /// plain 'w' keystroke of `Menu`) so the tab-bar button works from any tab:
    /// a bare 'w' would just be typed into whatever session is showing instead
    /// of opening the list. Converted to the Ctrl+B w prefix in `keys_for`.
    OpenDesk,
    /// Run a named key action -- what the command palette does. The name is a
    /// keys.rs action; the window turns it into the same keystroke pressing it
    /// would send, so the palette needs to know nothing about the keys
    RunKey { name: String },
    /// Search past conversations (the Vault). `query` is what to look for; a
    /// blank one lists the recent ones. The window answers by putting the hits
    /// into the next state
    VaultSearch { query: String },
    /// Reopen one past conversation as a tab, resuming it. Named by the values
    /// a hit carries, so the window can build the tab without holding the last
    /// search
    VaultOpen { program: String, id: String, cwd: Option<String>, title: String },
    /// Emergency stop
    Stop,
    /// Relaunch the tab being viewed. `keys_for` turns it into the Ctrl+B r that
    /// already does the job, so there is one restart in the app rather than two
    /// that can drift. Kept for the phone and the palette, which have a "this
    /// tab" and no pane to point at.
    Restart,
    /// Relaunch what a named pane holds. The ↻ pair in a pane's caption sends
    /// this.
    ///
    /// Named rather than implied, for the same reason the ⊞ in a caption is:
    /// with the screen divided, "the tab being viewed" is whichever pane has
    /// focus, and a button attached to a pane must mean THAT pane whether or
    /// not you were in it.
    ///
    /// `keep` is the choice between the two keys this stands for — Ctrl+B r
    /// carries the conversation over, Ctrl+B R starts a new one.
    RestartPane { id: u32, keep: bool },
    /// Cut every remote session from the window's side: rotate the access token
    /// and drop the open connections. Window-only — a phone can't disconnect
    /// itself (allowed_from_afar leaves it on the reject side).
    RemoteCut,
    /// The first-run pointer was closed, or the thing it pointed at was done.
    /// `step` is which of the two it was; a closed step never comes back
    Coach { step: u8 },
    /// The card that asks for a star (or a Store review) after the first
    /// answer was pressed. `open` says whether the page is to be opened;
    /// either way the card is put away for good. Window-only: the page it
    /// opens is this PC's
    Thanks { open: bool },
    /// The card that says a newer version is out was pressed. `open` says
    /// whether the settings' Update card is to be opened; either way the
    /// card is put away for that version. Nothing is installed from here
    Update { open: bool },
    /// The usage-limit notice on a tab was read. `tab` is the screen number
    LimitAck { tab: usize },
    /// The `?` beside the gear: the manual on the site, in the PC's browser.
    /// Window-only -- a phone reaches the same page through a plain link
    Help,
    /// A Lua quick-action fired from the bar. `index` is its position in
    /// config.actions; the code is looked up and run server-side (the page never
    /// holds Lua source). Allowed from afar — it runs the user's own action.
    RunAction { index: usize },
    /// Operate a target tab (🎯): attach the active AI as the operator of tab
    /// `target` (0 = detach) and, if `goal` is non-empty, hand it that goal. The
    /// AI then writes Lua to drive the target (reuses the browser-agent loop).
    Operate { target: usize, goal: String },
    /// 📼 record mode toggled in the composer. On arms the Lua recorder on the
    /// shown browser (the loop resolves which one that is); off silences it
    /// everywhere — there's only ever one recorder.
    Record { on: bool },
    /// ▶ run mode: Lua typed into the composer, to run against the shown
    /// browser in the same sandbox as the rally's AI-authored code (browser
    /// functions on that one tab, nothing else).
    RunLua { code: String },
    /// A file was pressed in the list: show it in an editor. `panel` names the
    /// place the same way the file list does (the tab whose folder this is),
    /// `path` is relative to that folder. Which editor it lands in is the
    /// loop's to decide -- the page does not know what is on screen elsewhere.
    /// An empty `path` closes that editor's file instead
    EditOpen {
        panel: String,
        path: String,
    },
    /// The window's own bar, which the page draws now that the frame is ours:
    /// "drag" (the bar was taken hold of), "minimize", "maximize" (toggles),
    /// "close". Answered where the window is, not in the loop -- the page is
    /// asking this window to do something to itself
    Window {
        act: String,
    },
    /// The file panel asking for a folder's contents, or for a search of the
    /// folder it stands in. `panel` names the place the same way the git panel
    /// does -- the tab whose working folder this is -- and `act` is one of a
    /// short list, for the same reason: a screen may ask for the things it
    /// draws, not reach the whole table through a message
    Files {
        panel: String,
        act: String,
        args: serde_json::Value,
    },
    /// The git panel asking for something. `panel` is the surface's own name,
    /// which is how the folder it reports on is found; `act` is one of a short
    /// list the loop turns into a primitive call. The panel does not name
    /// primitives itself -- a screen is allowed to ask for the things it draws,
    /// not to reach the whole table through a message
    Git {
        panel: String,
        act: String,
        /// Whatever this act needs, as it was written on the screen. A JSON
        /// object rather than a fixed set of fields: the panel grows buttons
        /// far faster than this enum should grow shapes
        args: serde_json::Value,
    },
    /// The file panel asking to see a folder, or to move a file between this
    /// machine and the server its tab is connected to. Shaped like `Git` and
    /// for the same reason: one message for a screen that grows buttons
    Sftp {
        panel: String,
        act: String,
        args: serde_json::Value,
    },
    /// One recorded step reported by a page being recorded. The pane's ipc
    /// handler stamps `from` with the page's name (same as `Button`).
    /// `act` is fill/click/press/secret; `value` is the committed text
    /// (fill), the key name (press), or empty. `xpath` says whether `sel` is
    /// an XPath (a text-anchored click) rather than CSS; `hint` is the
    /// element's visible text, carried into the Lua line as a comment so a
    /// broken selector can be repaired without re-recording.
    Recorded {
        from: Option<String>,
        act: String,
        sel: String,
        value: String,
        xpath: bool,
        hint: String,
    },
    /// A file attached in the desktop composer. `id` correlates the async reply
    /// (`window.__attachDone(id, …)`), `name` is the declared filename, `data` is
    /// the base64 bytes. Saved beside the active tab. Window-only — the phone
    /// attaches over the /api/attach HTTP route, so this never comes from afar.
    Attach { id: u64, name: String, data: String },
    /// The wheel was turned (positive = scroll back into the log, negative
    /// = return to the present). The number is a count of ticks.
    /// `row`/`col` is the cell it was over (needed to pass through to
    /// full-screen programs)
    Scroll { by: i32, row: u16, col: u16 },
    /// The result of a password entry (`None` = cancelled)
    Password { text: Option<String> },
    /// Something failed inside the page
    JsError { msg: String },
    /// The top bar was pressed. The destination is "whichever browser is
    /// currently being viewed", so the conductor decides which one it's
    /// for (only one bar is ever shown)
    Go { go: Go },
    /// The answer to `Cmd::Where`
    Where {
        from: Option<String>,
        url: String,
        can_back: bool,
        can_forward: bool,
    },
    /// One frame of the screencast. Base64 JPEG (usable as a data URL as-is).
    /// `from` is the source page. `w`/`h` are the frame's actual pixel dimensions
    Frame {
        from: Option<String>,
        data: String,
        w: u32,
        h: u32,
    },
    /// Page loading started/finished (shows "in progress" on the top bar).
    /// Only fires on main-frame document creation and `load`, so it won't
    /// light up for in-SPA navigation or background persistent connections
    /// (favoring honesty over false positives)
    Loading { from: Option<String>, busy: bool },
    /// The selected text (like PuTTY, copies as soon as it's selected)
    Copy { text: String },
    /// A paste request (right-click)
    Paste,
    /// An input request for the screencast view (arrives from a client; the conductor turns it into `Cmd::Inject`)
    Inject { to: Option<String>, input: Input },
    /// A keystroke in window mode. Either a committed character, a named
    /// control key, or Ctrl+character.
    ///
    /// `shift` and `alt` ride along with a named key. A character already
    /// carries its own shift -- the page sends `%`, not shift and `5` -- but
    /// Enter, Tab and the arrows have no such spelling, and a program that
    /// asked to tell Shift+Enter from Enter cannot be told without them
    Key {
        text: Option<String>,
        named: Option<String>,
        ctrl: Option<String>,
        shift: bool,
        alt: bool,
    },
    /// A person hands one tab a line, and that tab is named.
    ///
    /// Naming it is the whole point. This used to be delivered to "whichever
    /// tab is in front", which is a different tab from the one the sender meant
    /// whenever the two messages "look at N" and "here is a line" did not land
    /// in that order -- the discussion's topic box does exactly that pair, and
    /// its topic went to the wrong pane, or to nobody.
    Say { tab: usize, text: String },
    /// The window was closed
    Closed,
}

/// A browser data-storage spec. Represents both profile isolation (a login
/// box) and private (throwaway) mode in one type. When `private` is true,
/// `name` is ignored and a temporary area that's wiped on close is used instead.
///
/// wry's `WebContext` takes one "data folder". Same folder = same
/// cookies/login, different folder = different profile. Private mode just
/// hands it a unique temp folder (matches wry's own docs: keep a separate
/// context for normal tabs and one for private/incognito tabs).
#[derive(Clone, Debug)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct BrowserProfile {
    /// The profile name ("default", etc). Ignored when `private` is true
    pub name: String,
    /// Throwaway. If true, opens in a temp folder that keeps no history/cookies
    pub private: bool,
    /// What this page calls itself, when it is not to be what everything else
    /// calls itself. `None` takes the app-wide setting. It travels with the
    /// profile because it is the same question — who this page is to a site —
    /// and it is asked at every place a page is opened
    pub user_agent: Option<String>,
}

impl BrowserProfile {
    /// Build from a name and a private flag. An empty name falls back to "default"
    pub fn new(name: &str, private: bool) -> Self {
        let n = name.trim();
        Self {
            name: if n.is_empty() { "default".into() } else { n.to_string() },
            private,
            user_agent: None,
        }
    }

    /// The same profile, with a name of its own to give sites
    pub fn calling_itself(mut self, ua: Option<String>) -> Self {
        self.user_agent = ua.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        self
    }
    /// The default (shared "default" profile, persistent)
    pub fn shared_default() -> Self {
        Self { name: "default".into(), private: false, user_agent: None }
    }
}

/// A specifier for locating something on the page. CSS or XPath
#[derive(Debug, Clone)]
pub enum Sel {
    Css(String),
    Xpath(String),
    /// A number from the latest `browser_digest` of that page. Resolved to a
    /// CDP backendNodeId, and operated on with genuine (trusted) input —
    /// synthetic-event blind spots don't apply to it
    Ref(u32),
}

impl Sel {
    pub fn json(&self) -> serde_json::Value {
        match self {
            Sel::Css(s) => serde_json::json!({ "css": s }),
            Sel::Xpath(s) => serde_json::json!({ "xpath": s }),
            // Never sent to the page (ref operations go through CDP); kept
            // total so a stray call still serializes to something readable
            Sel::Ref(n) => serde_json::json!({ "ref": n }),
        }
    }
}

/// What a click/fill reports back. `state` keeps the three-state vocabulary;
/// the rest exists only on the `{ref=N}` path: `echo` is the human-readable
/// "what was really touched", and `anchor` is a durable, digest-free address
/// (id or text/attribute anchor) derived from the element itself — the raw
/// material for a portable replay script
#[derive(Debug)]
pub struct OpReport {
    pub state: Found,
    pub echo: Option<String>,
    /// ("css" | "xpath", value)
    pub anchor: Option<(String, String)>,
}

impl OpReport {
    pub fn bare(state: Found) -> Self {
        Self { state, echo: None, anchor: None }
    }
}

/// Where an element currently is. Click and fill return the same
/// vocabulary (if we touched it, it was reachable, hence `Visible`).
///
/// Distinguishing "not in the DOM" from "in the DOM but off-screen"
/// matters: the former means suspect the selector, the latter means
/// suspect the wait or the scroll position. Collapsing both into one
/// "failure" makes it impossible to know what to fix
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// Visible on screen
    Visible,
    /// In the DOM but off-screen
    OffScreen,
    /// Not in the DOM
    NotFound,
}

impl Found {
    pub fn as_str(self) -> &'static str {
        match self {
            Found::Visible => "visible",
            Found::OffScreen => "off_screen",
            Found::NotFound => "not_found",
        }
    }

    pub fn parse(json: &str) -> Self {
        match json.trim_matches('"') {
            "visible" => Found::Visible,
            "off_screen" => Found::OffScreen,
            _ => Found::NotFound,
        }
    }
}

/// One ask from the branch dialog, as the loop reads it off its queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchAsk {
    pub from: String,
    pub branch: String,
    pub base: String,
    pub make: bool,
    pub carry: Vec<String>,
    pub start: String,
    pub ais: Vec<String>,
    pub at: String,
    pub host: String,
    pub setup: bool,
}

impl BranchAsk {
    /// The ask carried by a branch event, or nothing for any other event.
    pub fn of(ev: Ev) -> Option<Self> {
        match ev {
            Ev::Branch { from, branch, base, make, carry, start, ais, at, host, setup } => {
                Some(BranchAsk { from, branch, base, make, carry, start, ais, at, host, setup })
            }
            _ => None,
        }
    }
}

/// What a runtime needs from whatever is showing pages, and nothing more.
///
/// The runtime asks for a click or a page's text; it does not know whether the
/// answer comes from a window on this machine, a browser somewhere else, or
/// nothing at all. A build with no window can leave every one of these
/// answering "there is nothing here to ask", and the automation that depends
/// on them fails honestly instead of pretending.
///
/// `to` names the page: `None` is the one in front.
/// Where a page sits in the window: left, top, width, height, in pixels.
///
/// Four numbers with no names is exactly as much as this has ever needed to
/// be, and every side of the app passes the same four -- so they are passed
/// under one name rather than spelled out at each end
pub type Rect = (i32, i32, i32, i32);

/// Somewhere to put a page, and the space it gets.
///
/// `None` where there is nowhere: a build with no window has no seat to offer,
/// and saying so is better than offering one that paints nothing
pub type Seat = (std::rc::Rc<dyn BrowserHost>, Rect);

pub trait BrowserHost {
    fn go(&self, to: Option<&str>, go: Go) -> anyhow::Result<()>;
    fn focus(&self, to: Option<&str>) -> anyhow::Result<()>;
    fn ask_where(&self, to: Option<&str>) -> anyhow::Result<()>;
    fn basic_auth(&self, to: Option<&str>, user: &str, pass: &str) -> anyhow::Result<()>;
    fn eval_in(&self, to: Option<&str>, js: &str) -> anyhow::Result<u64>;
    fn inject(&self, to: Option<&str>, input: Input) -> anyhow::Result<()>;
    fn screencast(&self, to: Option<&str>, on: bool) -> anyhow::Result<()>;
    fn record(&self, to: Option<&str>, on: bool) -> anyhow::Result<()>;

    fn find(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> anyhow::Result<Found>;
    fn click(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> anyhow::Result<OpReport>;
    fn fill(&self, to: Option<&str>, sel: &Sel, value: &str, timeout_ms: u64) -> anyhow::Result<OpReport>;
    fn text(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> anyhow::Result<Option<String>>;
    fn href(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String>;
    fn html(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String>;
    fn digest(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String>;
    fn snapshot(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<Vec<u8>>;

    fn cookies_out(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<serde_json::Value>;
    fn cookies_in(&self, to: Option<&str>, cookies: &serde_json::Value, timeout_ms: u64) -> anyhow::Result<()>;
    fn storage_out(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<serde_json::Value>;
    fn storage_in(&self, to: Option<&str>, items: &serde_json::Value, timeout_ms: u64) -> anyhow::Result<()>;
    fn fetch(&self, to: Option<&str>, url: &str, opts: &serde_json::Value, timeout_ms: u64) -> anyhow::Result<String>;

    /// Opening, placing and closing the pages the runtime asked for. A page is
    /// named, because the runtime refers to it by name from then on
    fn open_child(&self, name: &str, url: &str, rect: (i32, i32, i32, i32), profile: BrowserProfile) -> anyhow::Result<()>;
    fn child_bounds(&self, name: &str, rect: (i32, i32, i32, i32)) -> anyhow::Result<()>;
    fn close_child(&self, name: &str) -> anyhow::Result<()>;
    /// Let this origin through the same gate a person's click would open
    fn trust(&self, url: &str) -> anyhow::Result<()>;
    /// Stop every recording at once (a tab ended, or the run did)
    fn record_all_off(&self);

    /// What the device drawing this page is called, when the page is not drawn
    /// on this machine at all.
    ///
    /// A host that paints its own pages says nothing, which is the default and
    /// the ordinary case. The one that can place a page on somebody else's
    /// machine answers with that machine's name, and the screen needs it: such
    /// a page has no picture anyone here can be shown, so the only honest thing
    /// to put in its place is where it actually is.
    fn drawn_on(&self, to: Option<&str>) -> Option<String> {
        let _ = to;
        None
    }
}

/// Read one intent from the screen.
///
/// Arrives in the same shape whether from the window (ipc) or a phone
/// (HTTP). If parsing lived in two places, the day would come when the
/// same click gets interpreted two different ways, so it lives only here.
/// An unknown `kind` is `None`. Silently discarding it is correct
pub fn parse_intent(v: &serde_json::Value) -> Option<Ev> {
    Some(match v.get("kind").and_then(|k| k.as_str()) {
        Some("ready") => Ev::Ready {
            from: None,
            complete: v
                .get("complete")
                .and_then(|x| x.as_bool())
                .unwrap_or(true),
            url: v
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("loading") => Ev::Loading {
            from: None,
            busy: v.get("busy").and_then(|x| x.as_bool()).unwrap_or(false),
        },
        // The bar's button. `name` is the page it stands under, as automation
        // addresses it; only the board sends this (a placed page is refused)
        Some("button") => Ev::Button {
            from: v
                .get("name")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        },
        Some("touched") => Ev::Touched { from: None },
        Some("compose") => Ev::Compose { from: None },
        Some("pen") => Ev::Pen {
            on: v.get("on").and_then(|x| x.as_bool()).unwrap_or(false),
        },
        Some("select") => Ev::Select {
            tab: v.get("tab").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        },
        Some("addtab") => Ev::AddTab {
            pane: v.get("pane").and_then(|x| x.as_u64()).map(|n| n as u32),
            folder: v
                .get("folder")
                .and_then(|x| x.as_str())
                .filter(|f| !f.is_empty())
                .map(str::to_string),
        },
        Some("foldername") => Ev::FolderName {
            folder: v.get("folder").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            name: v.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        },
        Some("folderclose") => Ev::FolderClose {
            folder: v.get("folder").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        },
        Some("folderdiscard") => Ev::FolderDiscard {
            folder: v.get("folder").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        },
        Some("browse") => Ev::Browse {
            path: v.get("path").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            open: v.get("open").and_then(|x| x.as_bool()).unwrap_or(false),
        },
        Some("foldercolor") => Ev::FolderColor {
            folder: v.get("folder").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            color: v.get("color").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        },
        Some("repair") => Ev::Repair {
            folder: v.get("folder").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            choose: v.get("choose").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            branch: v.get("branch").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            take: v.get("take").and_then(|x| x.as_bool()).unwrap_or(false),
        },
        Some("keepenv") => Ev::KeepEnv {
            from: v.get("from").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        },
        Some("branch") => Ev::Branch {
            from: v.get("from").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            branch: v.get("branch").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            base: v.get("base").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            make: v.get("make").and_then(|x| x.as_bool()).unwrap_or(false),
            carry: v
                .get("carry")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            start: v.get("start").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            at: v.get("at").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            host: v.get("host").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            // Absent means yes: an older shell that does not send it is not
            // asking for a folder nothing can be built in
            setup: v.get("setup").and_then(|x| x.as_bool()).unwrap_or(true),
            ais: v
                .get("ais")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        },
        Some("closesettings") => Ev::CloseSettings,
        Some("opensettings") => Ev::OpenSettings {
            folder: v
                .get("folder")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            // A deep-link may name a section to land on and ask to return to the
            // board once saved (the sub-input bar's ⚙ shortcut does both).
            section: v
                .get("section")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            ret: v.get("ret").and_then(|x| x.as_bool()).unwrap_or(false),
            tabpos: v.get("tabpos").and_then(|x| x.as_u64()).map(|n| n as u32),
        },
        Some("menu") => Ev::Menu {
            key: v
                .get("key")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("opendesk") => Ev::OpenDesk,
        Some("runkey") => Ev::RunKey {
            name: v.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        },
        Some("vaultsearch") => Ev::VaultSearch {
            query: v.get("query").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        },
        Some("vaultopen") => Ev::VaultOpen {
            program: v.get("program").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            id: v.get("id").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            cwd: v.get("cwd").and_then(|x| x.as_str()).map(str::to_string),
            title: v.get("title").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        },
        Some("stop") => Ev::Stop,
        Some("restart") => Ev::Restart,
        Some("restartpane") => Ev::RestartPane {
            id: v.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            keep: v.get("keep").and_then(|x| x.as_bool()).unwrap_or(true),
        },
        Some("remotecut") => Ev::RemoteCut,
        Some("coach") => Ev::Coach {
            step: v.get("step").and_then(|x| x.as_u64()).unwrap_or(0).min(255) as u8,
        },
        Some("thanks") => Ev::Thanks { open: v.get("open").and_then(|x| x.as_bool()).unwrap_or(false) },
        Some("update") => Ev::Update { open: v.get("open").and_then(|x| x.as_bool()).unwrap_or(false) },
        Some("help") => Ev::Help,
        Some("limit_ack") => Ev::LimitAck {
            tab: v.get("tab").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        },
        // A quick-action chip whose payload is Lua (the code stays server-side —
        // the page only knows the index). Runs it against the active tab.
        Some("runaction") => Ev::RunAction {
            index: v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        },
        // 📼 record mode toggled in the composer (see `Ev::Record`).
        Some("record") => Ev::Record {
            on: v.get("on").and_then(|x| x.as_bool()).unwrap_or(false),
        },
        // ▶ composer Lua to run sandboxed against the shown browser (see `Ev::RunLua`).
        Some("runlua") => Ev::RunLua {
            code: v
                .get("code")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        // The file panel asking for a listing or a transfer (see `Ev::Sftp`).
        Some("sftp") => Ev::Sftp {
            panel: v.get("panel").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            act: v.get("act").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            args: v.get("args").cloned().unwrap_or(serde_json::Value::Null),
        },
        // A file pressed in the list (see `Ev::EditOpen`).
        Some("editopen") => Ev::EditOpen {
            panel: v.get("panel").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            path: v.get("path").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        },
        // The window's own bar (see `Ev::Window`).
        Some("window") => Ev::Window {
            act: v.get("act").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        },
        // The file panel asking for a listing or a search (see `Ev::Files`).
        Some("files") => Ev::Files {
            panel: v.get("panel").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            act: v.get("act").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            args: v.get("args").cloned().unwrap_or(serde_json::Value::Null),
        },
        // The git panel asking for a list, a diff, or a change (see `Ev::Git`).
        Some("git") => Ev::Git {
            panel: v.get("panel").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            act: v.get("act").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
            args: v.get("args").cloned().unwrap_or(serde_json::Value::Null),
        },
        // A recorded step from a page (who it came from is stamped by the pane's
        // ipc handler, like "button").
        Some("recorded") => Ev::Recorded {
            from: None,
            act: v
                .get("act")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            sel: v
                .get("sel")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            value: v
                .get("value")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
            xpath: v.get("xpath").and_then(|x| x.as_bool()).unwrap_or(false),
            hint: v
                .get("hint")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        // "Operate a target tab" (🎯): make the active AI drive tab `target`.
        // target 0 detaches. An optional `goal` (natural language) is handed to
        // the AI, which then writes Lua to operate the target.
        Some("operate") => Ev::Operate {
            target: v.get("target").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            goal: v
                .get("goal")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
        },
        // A file pasted/attached in the desktop composer. Saved beside the active
        // tab; the result is handed back by eval-ing window.__attachDone(id, …).
        // (The phone uses the /api/attach HTTP route instead, so this window-only
        // intent is never accepted from afar.)
        Some("attach") => Ev::Attach {
            id: v.get("id").and_then(|x| x.as_u64()).unwrap_or(0),
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("file")
                .to_string(),
            data: v
                .get("data")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("scroll") => Ev::Scroll {
            // A wheel tick or two from the window; up to a tall phone's whole
            // screen (≈ one row per tick) when the pager turns a page.
            by: v.get("by").and_then(|x| x.as_i64()).unwrap_or(0).clamp(-250, 250) as i32,
            row: v.get("row").and_then(|x| x.as_u64()).unwrap_or(0).min(9999) as u16,
            col: v.get("col").and_then(|x| x.as_u64()).unwrap_or(0).min(9999) as u16,
        },
        // The top bar. The destination is text the human typed, so narrow its type here
        Some("go") => Ev::Go {
            go: match v.get("what").and_then(|x| x.as_str()) {
                Some("back") => Go::Back,
                Some("forward") => Go::Forward,
                Some("reload") => Go::Reload,
                Some("hardreload") => Go::Hard,
                Some("to") => Go::To(
                    v.get("url")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string(),
                ),
                _ => return None,
            },
        },
        Some("jserror") => Ev::JsError {
            msg: v
                .get("msg")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("password") => Ev::Password {
            text: v.get("text").and_then(|x| x.as_str()).map(str::to_string),
        },
        Some("resize") => {
            let a = v.get("area").and_then(|x| x.as_array());
            let num = |i: usize| {
                a.and_then(|a| a.get(i))
                    .and_then(|x| x.as_i64())
                    .unwrap_or(0) as i32
            };
            // Every pane's own measurements ride along with the focused one's.
            // They arrive together because they are measured together: one
            // reflow of the page decides all of them, and splitting them into
            // two messages would let a pane act on a size the others no longer
            // agree with.
            let panes = v
                .get("panes")
                .and_then(|x| x.as_array())
                .map(|list| {
                    list.iter()
                        .filter_map(|p| {
                            let r = p.get("rect").and_then(|x| x.as_array());
                            let n = |i: usize| {
                                r.and_then(|r| r.get(i)).and_then(|x| x.as_i64()).unwrap_or(0) as i32
                            };
                            Some(PaneGeom {
                                id: p.get("id").and_then(|x| x.as_u64())? as u32,
                                rows: p.get("rows").and_then(|x| x.as_u64()).unwrap_or(24) as u16,
                                cols: p.get("cols").and_then(|x| x.as_u64()).unwrap_or(80) as u16,
                                rect: (n(0), n(1), n(2), n(3)),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let f = v.get("full").and_then(|x| x.as_array());
            let fnum = |i: usize| {
                f.and_then(|f| f.get(i)).and_then(|x| x.as_i64()).unwrap_or(0) as i32
            };
            Ev::Resize {
                rows: v.get("rows").and_then(|x| x.as_u64()).unwrap_or(24) as u16,
                cols: v.get("cols").and_then(|x| x.as_u64()).unwrap_or(80) as u16,
                area: (num(0), num(1), num(2), num(3)),
                full: (fnum(0), fnum(1), fnum(2), fnum(3)),
                panes,
            }
        }
        Some("focuspane") => Ev::FocusPane {
            id: v.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        },
        Some("closepane") => Ev::ClosePane {
            id: v.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
        },
        Some("paneratio") => Ev::PaneRatio {
            divider: v.get("divider").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            ratio: v.get("ratio").and_then(|x| x.as_f64()).unwrap_or(0.5) as f32,
        },
        Some("fontsize") => Ev::FontSize {
            px: v.get("px").and_then(|x| x.as_u64()).unwrap_or(14).clamp(8, 32) as u8,
        },
        Some("tabwidth") => Ev::TabWidth {
            px: v.get("px").and_then(|x| x.as_u64()).unwrap_or(0).min(u16::MAX as u64) as u16,
        },
        Some("sidewidth") => Ev::SideWidth {
            px: v.get("px").and_then(|x| x.as_u64()).unwrap_or(0).min(u16::MAX as u64) as u16,
        },
        Some("splitpane") => Ev::SplitPane {
            id: v.get("id").and_then(|x| x.as_u64()).unwrap_or(0) as u32,
            down: v.get("down").and_then(|x| x.as_bool()).unwrap_or(false),
        },
        Some("copy") => Ev::Copy {
            text: v
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("paste") => Ev::Paste,
        // Touch/mouse on the screencast view. Coordinates arrive as a fraction (0..1)
        Some("inject") => {
            let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
            let what = v.get("what").and_then(|x| x.as_str()).unwrap_or("");
            let input = match what {
                "mouse" => Input::Mouse {
                    phase: v.get("phase").and_then(|x| x.as_str()).unwrap_or("moved").to_string(),
                    x: f("x").clamp(0.0, 1.0),
                    y: f("y").clamp(0.0, 1.0),
                    down: v.get("down").and_then(|x| x.as_bool()).unwrap_or(false),
                },
                "wheel" => Input::Wheel {
                    x: f("x").clamp(0.0, 1.0),
                    y: f("y").clamp(0.0, 1.0),
                    dx: f("dx"),
                    dy: f("dy"),
                },
                "text" => Input::Text {
                    text: v.get("text").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                },
                "key" => Input::Key {
                    named: v.get("named").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
                    ctrl: v.get("ctrl").and_then(|x| x.as_bool()).unwrap_or(false),
                    alt: v.get("alt").and_then(|x| x.as_bool()).unwrap_or(false),
                },
                "view" => Input::View {
                    w: f("w").max(1.0),
                    h: f("h").max(1.0),
                },
                _ => return None,
            };
            Ev::Inject { to: None, input }
        }
        Some("key") => Ev::Key {
            text: v.get("text").and_then(|x| x.as_str()).map(str::to_string),
            named: v.get("named").and_then(|x| x.as_str()).map(str::to_string),
            ctrl: v.get("ctrl").and_then(|x| x.as_str()).map(str::to_string),
            shift: v.get("shift").and_then(|x| x.as_bool()).unwrap_or(false),
            alt: v.get("alt").and_then(|x| x.as_bool()).unwrap_or(false),
        },
        Some("say") => Ev::Say {
            tab: v.get("tab").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
            text: v
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        // Save the latest run's replay.lua where the user can grab it
        // (the window board can't download over HTTP, so it asks the app)
        Some("replaysave") => Ev::ReplaySave,
        // ✨ ask the assistant AI to turn natural language into one shell
        // command for the active terminal tab
        Some("suggest") => Ev::Suggest {
            text: v
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("survey") => Ev::Survey,
        Some("result") => Ev::Result {
            id: v.get("id").and_then(|i| i.as_u64()).unwrap_or(0),
            ok: v.get("ok").and_then(|o| o.as_bool()).unwrap_or(false),
            value: v
                .get("value")
                .map(|x| x.to_string())
                .unwrap_or_else(|| "null".into()),
        },
    _ => return None,
    })
}

/// Is this a URL we're allowed to open? Only http/https pass.
///
/// When wry receives IPC from a page, it builds that page's URL as an
/// `http::Uri` and `unwrap`s it (webview2/mod.rs). Both `file:///` and
/// `data:` fail to parse there and **take down the whole process**
/// (confirmed by testing). Since the initialization script we inject
/// always sends IPC, opening one of these guarantees a crash. So we
/// stop it at the door.
///
/// To show a local file, serve it over this app's own local HTTP server
/// instead — it achieves the same thing
pub fn is_openable(url: &str) -> bool {
    let u = url.trim();
    let scheme_ok = u.starts_with("https://") || u.starts_with("http://");
    let has_host = u.split("//").nth(1).is_some_and(|rest| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        !host.is_empty()
    });
    scheme_ok && has_host && !u.contains(['\n', '\r', ' '])
}

/// Whether a page the runtime placed -- somebody else's page -- may say this.
///
/// A placed page runs whatever script its site serves, and that script can
/// report through the same channel ours do. So a page is let to *report* -- a
/// step it recorded, that it is loading or has loaded, that it took the focus
/// or its pen was pressed, the answer to a question we put to it -- and never
/// to *ask*: nothing here types into a tab, runs Lua, touches git, or opens
/// the settings. Before this list existed, `{kind:"say"}` from any web page
/// went into the terminal as if the person had typed it.
///
/// Not on the list: the press of the bar that asks the person something
/// (`Button`). A page cannot be believed about that -- the whole point of the
/// bar is that a person, not the page, said "done" -- which is why the bar is
/// drawn outside the page, and only the board reports the press.
///
/// Written from the side that enumerates what gets through, like
/// `remote::allowed_from_afar` is for the phone. Add to it only after writing
/// down why a stranger's page needs it
pub fn allowed_from_page(ev: &Ev) -> bool {
    matches!(
        ev,
        Ev::Ready { .. }
            | Ev::Loading { .. }
            | Ev::Touched { .. }
            | Ev::Compose { .. }
            | Ev::Recorded { .. }
            | Ev::Result { .. }
    )
}

/// The control keys an [`Input::Key`] may name.
///
/// Kept here because it is the agreement itself: a script writes "pageup" and
/// something else has to know that is a key rather than a typo. What each host
/// turns the name into -- a Windows virtual-key code, a CDP key event -- is the
/// host's own business, and differs between them.
pub const NAMED_KEYS: [&str; 27] = [
    "enter", "backspace", "tab", "escape", "esc", "delete", "up", "down", "left", "right", "space",
    "home", "end", "pageup", "pagedown", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9",
    "f10", "f11", "f12",
];

/// Whether `named` is one of them, so a typo is refused where it was written
/// rather than quietly doing nothing on a page.
pub fn key_known(named: &str) -> bool {
    NAMED_KEYS.contains(&named)
}

/// Showing a banner on the machine a person is sitting at.
///
/// Slack, Discord, Telegram and a phone's push are all posted by the runtime
/// itself: they are somewhere else, reached over the network. A banner on this
/// desktop is the one destination that needs whatever is running the desktop,
/// which is why it arrives as a trait and not a function call. A runtime with
/// no shell has none, and says so rather than pretending it sent one.
pub trait Toasts: Send + Sync {
    fn show(&self, title: &str, body: &str, tab: Option<usize>) -> Result<(), String>;
    /// The tab a person pressed a banner for, if one was pressed since last asked
    fn clicked_tab(&self) -> Option<usize>;
    /// Bring whatever is showing this to the front, because they asked for it
    fn raise(&self);
}

/// Asking the person at the desktop to point at a file or a folder.
///
/// The settings screen is a web page either way, but "choose a folder" is the
/// one thing a page cannot do for itself: it needs the desktop's own dialog.
/// A runtime with no desktop has no picker, and the page that asked is told so
/// rather than left waiting on a dialog nobody can see.
pub trait FilePicker: Send + Sync {
    /// A folder. `title` is shown on the dialog; `start` is where it opens
    fn folder(&self, title: &str, start: Option<&std::path::Path>) -> Option<std::path::PathBuf>;
    /// A file to read, filtered to one extension (e.g. `json`)
    fn open(&self, title: &str, start: Option<&std::path::Path>, ext: (&str, &str)) -> Option<std::path::PathBuf>;
    /// Where to write a file, with a name already filled in
    fn save(&self, title: &str, start: Option<&std::path::Path>, name: &str, ext: (&str, &str)) -> Option<std::path::PathBuf>;
}

/// Putting text on the clipboard of the machine a person is sitting at.
///
/// A program in a tab can ask for this (OSC 52), and on a runtime with no
/// desktop there is no clipboard to put it on -- the ask is dropped, which is
/// also what happens today when the desktop refuses. Reading the clipboard is
/// deliberately not here: what someone copied last, from any application, is
/// not something the far end of an ssh session gets to see.
pub trait Clipboard: Send + Sync {
    fn set_text(&self, text: String);
}


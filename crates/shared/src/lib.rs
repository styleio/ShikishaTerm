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

/// The questions the app has put to pages, by id, and whom each was asked.
///

/// One pane as the page measured it.
///
/// Rows and columns are what the terminal in that pane must be resized to;
/// the rect is where a browser placed in that pane has to sit. Only the page
/// can work these out — it owns the font metrics and the dividers — so they
/// are reported, never guessed on this side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneGeom {
    pub id: u32,
    pub rows: u16,
    pub cols: u16,
    pub rect: (i32, i32, i32, i32),
}

/// A report from the browser to the conductor
#[derive(Debug, Clone)]
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
    },
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
    /// Open the workspace switcher. A dedicated intent (rather than reusing the
    /// plain 'w' keystroke of `Menu`) so the tab-bar button works from any tab:
    /// a bare 'w' would just be typed into whatever session is showing instead
    /// of opening the list. Converted to the Ctrl+B w prefix in `keys_for`.
    OpenWs,
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


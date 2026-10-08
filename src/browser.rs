//! Put one browser under command.
//!
//! Windows 11 ships with a Chromium engine (WebView2) built in,
//! and Microsoft keeps it updated. So we don't bundle our own. We borrow it.
//! That keeps the "no-install single exe" promise intact.
//!
//! The window's loop has the program's first thread, and the work that
//! drives it runs beside it (`Browser::host`): a Mac lets only the first
//! thread draw a window, and the two loops want to run on their own terms.
//!
//! What draws the window is the engine (`browser/webview2.rs` on Windows).
//! Where there is none yet, most of what is here has nothing to drive, and
//! is built all the same so it stays the same on every system.
//!
//! Use `run_return`, not `run`. `run` is `-> !` and calls
//! `process::exit` internally. Just closing the browser window would
//! take down the whole app.

#![cfg_attr(not(windows), allow(dead_code))]

use shikisha_core::pageops::{self, Speaks};
use shikisha_shared::{BrowserHost, BrowserProfile, allowed_from_page, is_openable, Ev, Found, Go, Input, OpReport, Sel, parse_intent};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Result, anyhow};

// The window's engine, reached through the few names `window` and `cdp`
// take from it: WebView2 on Windows, the Chromium carried in the .app (CEF) on
// a Mac. Elsewhere there is none yet, and the window says so
#[cfg(windows)]
mod webview2;
#[cfg(windows)]
use webview2 as engine;
#[cfg(windows)]
pub use webview2::runtime_version;
#[cfg(target_os = "macos")]
mod cef_engine;
#[cfg(target_os = "macos")]
use cef_engine as engine;
#[cfg(target_os = "macos")]
pub use cef_engine::runtime_version;
#[cfg(not(any(windows, target_os = "macos")))]
mod unready;
#[cfg(not(any(windows, target_os = "macos")))]
use unready as engine;
#[cfg(not(any(windows, target_os = "macos")))]
pub use unready::runtime_version;
// What is done through a page's DevTools protocol, the window's loop, and the
// window as the system dresses it: the same on every system
pub(crate) mod cdp;
mod frame;
mod window;
pub use frame::main_hwnd;
use window::run_window;

/// Added on top of `INIT_JS` for pages placed inside the window, and only for
/// those: the shell's own page already knows which pane was clicked.
///
/// A placed page is a native layer with its own window handle, so a press that
/// lands on it never reaches the pane underneath -- which is why a browser
/// pane could only be focused by its caption, and why the pen that summons the
/// composer never appeared over one.
///
/// What is reported is the page taking the keyboard, not a click. Automation
/// drives these pages by dispatching input at them, and dispatched input never
/// moves the window handle's focus -- so a rally clicking through a form
/// cannot pull the keyboard out from under someone typing in another pane,
/// which is exactly what reporting the click itself would have done.
const PLACED_JS: &str = r##"
(function () {
  const tell = () => window.ipc.postMessage(JSON.stringify({ kind: "touched" }));
  addEventListener("focus", tell);
  // The press that brought the focus here arrives before the focus event on
  // some paths and after it on others; asking whether we hold it is the same
  // question either way, and asking twice costs a message nobody reads
  addEventListener("pointerdown", () => { if (document.hasFocus()) tell(); }, true);

  // The pen that summons the composer, drawn HERE.
  //
  // The window cannot draw it over this page. A z-index orders things inside
  // one document; this page is a window of its own, and no element of the
  // window's page can be stacked above another window. The room for it used to
  // be taken out of the page instead -- the page was held up by the height of
  // a button, leaving a band of nothing under it. Drawn from inside, it floats
  // over the page exactly as it does over a terminal, in the same corner, and
  // costs no room at all.
  //
  // In a shadow root so the page's own CSS cannot reach it, and the other way
  // about -- same as the banner above
  let host = null;
  // The app's messages, drawn by the page for the same reason the pen is: a
  // window of its own cannot be drawn over, so a toast raised while this page
  // fills the focused pane would be hidden behind it -- or, in a split, cut in
  // half at the pane's edge. It seats itself at the bottom of THIS page, which
  // is the pane it is about.
  var toastEl = null, toastGo = 0;
  window.__shikisha_toast = function (text, warn) {
    if (!text) { if (toastEl) toastEl.style.display = "none"; return; }
    if (!toastEl) {
      toastEl = document.createElement("div");
      toastEl.id = "__shikisha_toast";
      toastEl.style.cssText =
        "position:fixed;left:50%;bottom:20px;transform:translateX(-50%);" +
        "z-index:2147483646;max-width:min(86%,560px)";
      (document.body || document.documentElement).appendChild(toastEl);
      toastEl.attachShadow({ mode: "open" });
      toastEl.shadowRoot.innerHTML =
        '<div style="padding:10px 14px;border-radius:9px;font-size:13.5px;' +
        'line-height:1.5;font-weight:600;text-align:left;overflow-wrap:anywhere;' +
        'max-height:9em;overflow:hidden;box-shadow:0 10px 30px rgba(0,0,0,.5);' +
        'font-family:system-ui,sans-serif"></div>';
    }
    var box = toastEl.shadowRoot.firstChild;
    box.style.background = warn ? "#c8382f" : "#7fd7ff";
    box.style.color = warn ? "#fff" : "#04121c";
    box.textContent = text;
    toastEl.style.display = "";
    // Long enough to read, and the same message arriving again restarts it
    clearTimeout(toastGo);
    toastGo = setTimeout(function () { if (toastEl) toastEl.style.display = "none"; },
      Math.min(12000, 3500 + text.length * 45));
  };
  window.__shikisha_pen = function (on) {
    if (!on) { if (host) host.style.display = "none"; return; }
    if (!host) {
      host = document.createElement("div");
      host.id = "__shikisha_pen";
      host.style.cssText =
        "position:fixed;right:16px;bottom:16px;z-index:2147483646";
      (document.body || document.documentElement).appendChild(host);
      host.attachShadow({ mode: "open" });
      host.shadowRoot.innerHTML =
        '<button style="width:44px;height:44px;border-radius:50%;font-size:19px;' +
        'line-height:1;display:flex;align-items:center;justify-content:center;' +
        'cursor:pointer;border:1px solid #00aaff;background:#0a0c0e;' +
        'box-shadow:0 4px 16px rgba(0,0,0,.45);opacity:.85">&#9999;&#65039;</button>';
      host.shadowRoot.querySelector("button").onclick = () =>
        window.ipc.postMessage(JSON.stringify({ kind: "compose" }));
    }
    host.style.display = "";
  };
})();
"##;

/// Added on top of the placed-page scripts for a window a page asked to open.
///
/// An adopted window has no title bar of its own — it sits in the seat its
/// opener was using — so the way out of it has to be drawn. `window.close` is
/// taken over for the same reason: a popup that finishes by closing itself
/// (which is how every sign-in popup ends) has to reach us, or the runtime
/// tears the page down from under the pane and leaves a hole.
const POPUP_JS: &str = r#"
(function () {
  // Both roads out. The message reaches the app from a window a link opened;
  // from one a script opened, only the browser's own close request does (the
  // app listens for it), so the real close is asked for as well
  const closeForReal = window.close.bind(window);
  const shut = () => {
    try { window.ipc.postMessage(JSON.stringify({kind:"popupclose"})); } catch (e) {}
    try { closeForReal(); } catch (e) {}
  };
  window.close = shut;
  const draw = () => {
    if (document.getElementById("__shikisha_popbar") || !document.documentElement) return;
    const bar = document.createElement("div");
    bar.id = "__shikisha_popbar";
    bar.style.cssText = "position:fixed;top:0;left:0;right:0;height:24px;z-index:2147483647;" +
      "display:flex;align-items:center;gap:8px;padding:0 8px;box-sizing:border-box;" +
      "background:#1b1d22;color:#c9ced8;font:11px/1 system-ui,sans-serif;";
    const who = document.createElement("span");
    who.textContent = location.origin;
    who.style.cssText = "flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap";
    const x = document.createElement("span");
    x.textContent = "\u2715";
    x.style.cssText = "cursor:pointer;padding:0 6px;font-size:13px";
    x.addEventListener("click", shut);
    bar.append(who, x);
    document.documentElement.appendChild(bar);
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", draw);
  else draw();
})();
"#;

/// Where a report goes, for a page inside the window: the channel wry gives
/// every document. Defined before the shared script, which calls it.
const WINDOW_POST: &str = r#"
(function () { window.__shikisha_post = (s) => window.ipc.postMessage(s); })();
"#;

/// Always injected into every document first.
///
/// It runs on every navigation, so the helpers automation calls into are
/// there however many times a login redirects. Nothing in here asks the
/// person anything: the bar that does is the app's own, drawn under the page
/// by the board (shell.rs), where a page cannot press it.
static INIT_JS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!("{WINDOW_POST}{}{}", frame::FRAME_JS, shikisha_core::pagejs::AUTOMATION)
});

/// An instruction from the conductor to the browser
#[derive(Debug, Clone)]
pub enum Cmd {
    /// The engine asked for a turn of the loop of its own (CEF on a Mac is
    /// pumped by the loop it lives in, at times it names)
    #[cfg_attr(windows, allow(dead_code))]
    EngineTurn,
    /// Register the keys that work from any program, on this thread: a Mac
    /// takes them only here (`hotkeys::register_here`)
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    RegisterKeys,
    /// Another address the app's own pages come from. The settings and the
    /// result view are served by a second local server whose port is only
    /// known once it starts, so it is told here rather than at the window's birth
    Trust { origin: String },
    /// Evaluate JS and return the result (matched up by `id`).
    /// `to` is the destination page name. `None` means the main view
    Eval {
        id: u64,
        to: Option<String>,
        js: String,
    },
    /// Call one CDP method and send its result back (matched up by `id`).
    /// The JS path (`Eval`) can't see the DevTools protocol, and CDP is
    /// where the accessibility tree, layout snapshot, and genuine input live
    Cdp {
        id: u64,
        to: Option<String>,
        method: String,
        params: String,
    },
    /// Place a named page inside the same window
    AddChild {
        name: String,
        url: String,
        rect: (i32, i32, i32, i32),
        /// This page's data storage (profile / private)
        profile: BrowserProfile,
        /// The port of a proxy on this machine that everything this page
        /// fetches goes through, when its network belongs to another machine
        through: Option<u16>,
    },
    /// Set the placed page's position and size. Width or height of 0 hides it
    ChildBounds {
        name: String,
        rect: (i32, i32, i32, i32),
    },
    /// Put a placed page above the others. A page built later already sits
    /// above one built earlier; this is for when that is the wrong way round
    RaiseChild { name: String },
    /// Remove a placed page
    RemoveChild { name: String },
    /// Take in the windows pages asked for, and let go of the ones that asked
    /// to close. Sent by the handlers that answer those requests: they run on
    /// the message loop and cannot reach into the loop's own bookkeeping
    Adopt,
    /// Keep a hidden page's compositor running (`on=true`) for the duration
    /// of genuine-input operations, and release it again (`on=false`).
    ///
    /// A page that isn't on screen (bounds 0×0) stops compositing, and mouse
    /// input is the one kind that needs the compositor — its ack never comes.
    /// A tiny screencast forces frame production (the same mechanism the
    /// phone relay rides), so input lands and acks deterministically; the
    /// synchronization is the CDP completion itself, never a timer
    Wake { to: Option<String>, on: bool },
    /// Move keyboard focus to this page. `None` for `to` means the main view.
    ///
    /// Focus inside the page (activeElement) and the focus the OS sees are
    /// separate things. Showing/hiding a stacked page can leave the OS-level
    /// focus stranded elsewhere, which shows up as keystrokes arriving fine
    /// while the Japanese IME candidate window pops up in the wrong corner
    Focus { to: Option<String> },
    /// Move a placed page (the human pressed the bar above it)
    Move { to: Option<String>, go: Go },
    /// Ask where we currently are and whether we can go back/forward.
    /// The answer comes back as `Ev::Where`
    Where { to: Option<String> },
    /// Search a page for words, or move through what was found. Where the
    /// search stands comes back as `Ev::Seek`
    Seek { to: Option<String>, text: String, step: shikisha_shared::Seek },
    /// Stop a download still being saved (the id its `Ev::Download` carried)
    CancelDownload { id: String },
    /// Whether Ctrl+F and F3 in this page open the board's search row (on) or
    /// are left to the browser's own box (off)
    FindKeys { to: Option<String>, on: bool },
    /// Start/stop screencasting (VNC-equivalent).
    /// Once started, `Ev::Frame` arrives on every change. `to` is the target
    /// page (`None` is the main view)
    Screencast {
        to: Option<String>,
        on: bool,
    },
    /// Inject real input into the screencast target (via CDP — treated as
    /// genuine input, not synthetic). Both a human's finger trace and a
    /// CAPTCHA swipe are replayed exactly as the points arrive
    Inject {
        to: Option<String>,
        input: Input,
    },
    /// Arm basic auth. From then on, this page's 401 challenges get
    /// credentials returned via CDP (Fetch.authRequired -> continueWithAuth).
    /// user/pass are already resolved from secrets and are never handed to AI/Lua
    BasicAuth {
        to: Option<String>,
        user: String,
        pass: String,
    },
    /// Start or stop hearing a page's console. While on, every line it says
    /// comes back as `Ev::ConsoleLine`
    Console { to: Option<String>, on: bool },
    /// A DevTools screen connected for a page (`devtools.rs`): what the page
    /// says to it goes to `out`
    DevtoolsOpen { conn: u64, to: Option<String>, out: Sender<String> },
    /// One message the screen said, for its page
    DevtoolsSay { conn: u64, text: String },
    /// The screen went away
    DevtoolsClose { conn: u64 },
    /// Put the window away. The program, its tabs and the phone's connection
    /// go on; only the picture is gone, and the icon in the notification area
    /// is how it comes back
    Hide,
    /// Bring the window back in front of the person, from put away or minimised
    Show,
    /// A notice from the notification-area icon (a banner Windows draws)
    TrayNotice { title: String, text: String },
    /// The screen's own browser died. `memory` says the machine had none
    /// left, which is the one reason not to rebuild it straight away --
    /// rebuilding costs more memory than the failure freed
    DisplayDied { memory: bool },
    /// Bring the screen back. `asked` means a person pressed for it, which
    /// also forgets what was tried before -- somebody pressing twice is not
    /// the same thing as a program retrying in a loop, and must not be
    /// counted as one. An attempt the program decided on keeps the count
    DisplayWanted { asked: bool },
    /// Close the window (when the conductor is gone)
    Close,
    /// Open a tool over a picture of the screen, after waiting `delay` seconds
    /// (see `Ev::Snip`). The window the tool is drawn in is this loop's own
    Snip { tool: String, delay: u8 },
    /// A second of the wait has gone. `press` is the press it belongs to, so a
    /// count from a press that was replaced by a newer one does nothing
    SnipTick { press: u64, left: u8 },
    /// The tool is done with: its window goes, and so does the picture
    SnipClose,
    /// The answer to a question the tool page asked (see `Ev::SnipAsk`),
    /// as JSON, handed to the page
    SnipAnswer { json: String },
    /// A key from any program asked for something on the board. Passed on to
    /// the conductor, which knows whether the window is put away (see
    /// `Ev::Summon`)
    Summon { what: String },
    /// The tool's window out of the way of a save dialog, keeping what is on it
    SnipAside,
    /// The save dialog is done: the tool's window back, told how it ended
    SnipBack { how: &'static str },
}




/// The part of an address that says which site a page belongs to: scheme,
/// host and port (the scheme's usual port when none is written). `None` for
/// anything that is not an address with a host
fn origin_of(addr: &str) -> Option<(String, String, u16)> {
    let uri: http::Uri = addr.trim().parse().ok()?;
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let host = uri.host()?.to_ascii_lowercase();
    let port = uri.port_u16().or(match scheme.as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    })?;
    Some((scheme, host, port))
}

/// Whether two addresses belong to the same site. Judged by parsing both, not
/// by the front of the string: `http://127.0.0.1:8787.evil.example/` starts
/// with our address and is not ours
pub fn same_origin(a: &str, b: &str) -> bool {
    match (origin_of(a), origin_of(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Whether `at` is one of the app's own addresses (the board's, the settings
/// server's). Two servers, because the settings and the result view are
/// served by one that starts on its own port when first needed
fn from_ours(own: &[String], at: &str) -> bool {
    own.iter().any(|o| same_origin(at, o))
}


/// A result is believed only from the page it was asked of. Without this, any
/// placed page could answer for another — a stranger's tab filling in what
/// `browser_text` read from the tab beside it — by posting `{kind:"result"}`
/// with a guessed id (ids count up from one). Bounded, because a page that
/// navigates away mid-question never answers, and its entry would otherwise
/// stay for the life of the window
#[derive(Default)]
pub struct Asked {
    of: std::collections::HashMap<u64, Option<String>>,
    order: std::collections::VecDeque<u64>,
}

impl Asked {
    /// The most questions kept waiting for an answer at once
    const KEPT: usize = 4096;

    /// Remember that question `id` went to `to` (`None` is the board itself)
    pub fn ask(&mut self, id: u64, to: Option<String>) {
        if self.of.insert(id, to).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > Self::KEPT {
            if let Some(old) = self.order.pop_front() {
                self.of.remove(&old);
            }
        }
    }

    /// Whether `by` is who question `id` was put to. True once: the question
    /// is closed by its answer, so a second answer — from anyone — is refused
    pub fn answered(&mut self, id: u64, by: Option<&str>) -> bool {
        match self.of.get(&id) {
            Some(to) if to.as_deref() == by => {
                self.of.remove(&id);
                self.order.retain(|x| *x != id);
                true
            }
            _ => false,
        }
    }
}

/// Read what a page in the window said, and keep only what it may say.
///
/// `who` is the page's name (`None` for the board itself). `ours` is whether the
/// page is one of the app's own — judged by the address it spoke from, so the
/// settings page keeps its full voice while the same pane, navigated to another
/// site, loses it. A stranger is held to `allowed_from_page`; everyone's
/// answers are checked against `asked`; and the report's sender is stamped
/// here, never taken from the message, so no page can speak as another
pub fn heard(
    body: &str,
    who: Option<&str>,
    ours: bool,
    asked: &mut Asked,
) -> Option<Ev> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let ev = parse_intent(&v)?;
    if !ours && !allowed_from_page(&ev) {
        return None;
    }
    let from = who.map(str::to_string);
    Some(match ev {
        Ev::Result { id, ok, value } => {
            if !asked.answered(id, who) {
                return None;
            }
            Ev::Result { id, ok, value }
        }
        // Reports that say who sent them. A placed page's carry its name,
        // whatever the message claimed; the board's own carry none -- except
        // the bar's press, which names the page the bar stands under, and
        // only the board is heard on that (see `allowed_from_page`)
        Ev::Button { from: named } if who.is_none() => Ev::Button { from: named },
        Ev::Button { .. } => Ev::Button { from },
        Ev::Touched { .. } => Ev::Touched { from },
        Ev::Compose { .. } => Ev::Compose { from },
        Ev::Recorded { act, sel, value, xpath, hint, .. } => {
            Ev::Recorded { from, act, sel, value, xpath, hint }
        }
        Ev::Picked { item, .. } => Ev::Picked { from, item },
        Ev::Ready { url, complete, .. } => Ev::Ready { from, url, complete },
        Ev::Loading { busy, .. } => Ev::Loading { from, busy },
        other => other,
    })
}

/// A line in the log for a message the window refused, from whom and why.
/// Said a handful of times per page and then no more: a page that keeps
/// trying must not be able to fill the log
fn note_refused(said: &std::cell::Cell<u8>, who: Option<&str>, at: &str, body: &str) {
    if said.get() >= 5 {
        return;
    }
    said.set(said.get() + 1);
    let kind = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_string))
        .unwrap_or_else(|| "?".into());
    // The site, not the whole address: a page's query string is its business
    let site = origin_of(at)
        .map(|(s, h, p)| format!("{s}://{h}:{p}"))
        .unwrap_or_else(|| "?".into());
    shikisha_core::append_hook_log(&format!(
        "[browser] refused '{kind}' from page '{}' at {site} (a placed page may report, not ask)",
        who.unwrap_or("(board)")
    ));
}



/// What the password prompt was answered (`Browser::poll_password`)
pub enum Typed {
    /// The prompt's own answer: the words, or none (its cancel)
    Answer(Option<String>),
    /// The window is going: its ✕, or Quit on the tray
    Quit,
    /// Nothing within the time asked
    Nothing,
}

/// A handle to one running browser
pub struct Browser {
    proxy: tao::event_loop::EventLoopProxy<Cmd>,
    /// Behind a lock because two threads now wait here: the loop that runs
    /// the window, and -- when this window is drawing a server's pages -- the
    /// one answering that server. Only ever one at a time, and each wait is
    /// bounded by the deadline it was given
    events: std::sync::Mutex<Receiver<Ev>>,
    next_id: AtomicU64,
    /// The window is put away and the board's page dropped with it (see `hide`)
    away: std::sync::atomic::AtomicBool,
    /// Pages whose Lua recorder is armed (📼). The same navigation problem as
    /// the bar: the JS world (and its recOn flag) dies on every navigation, so
    /// membership here is what's true, re-issued per new document.
    pending_rec: std::sync::Mutex<std::collections::HashSet<Option<String>>>,
    /// A different signal that arrived while we were waiting on something.
    ///
    /// Skipping and discarding it means anything sent before the wait
    /// began vanishes forever. That's exactly how the window's column
    /// count once never arrived
    spare: std::sync::Mutex<Vec<Ev>>,
    /// Where a placed page's traffic goes, when this window is drawing the
    /// pages of a server somewhere else. Everything the page fetches then
    /// leaves from *that* machine -- which is the only reason drawing it here
    /// is worth anything, since `localhost` has to mean the same thing on
    /// both sides (see `shikisha_core::tunnel`)
    through: std::sync::Mutex<Option<u16>>,
    /// The latest digest per page: position N-1 holds the backendNodeId
    /// behind `{ref=N}`. Cleared when that page navigates (backend ids die
    /// with the document, and a stale ref must say so, not click thin air)
    digests: std::sync::Mutex<std::collections::HashMap<Option<String>, Vec<i64>>>,
    /// Which process plays the sound of the page now being cast, or 0.
    ///
    /// Filled in by the window's own loop, because only it holds the page: a
    /// page is drawn by a browser of its own and played by a child of that,
    /// and the relay -- which knows nothing about browsers -- has to be told
    /// a number. Nothing is recorded because of this; it is the answer to
    /// "whose sound?" for if somebody taps the speaker on their phone
    sound_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,
    /// Where a DevTools screen connects to its page (`devtools.rs`), started
    /// the first time one is opened
    devtools: std::sync::Mutex<Option<crate::devtools::Bridge>>,
}




/// Where browser data lives — every last byte of it, under the one folder the
/// config names (`browser_data`). WebView2's store is heavy SQLite and cache, so
/// the default keeps it out of a Drive-synced folder.
///
/// One root matters more than it looks: each page is handed its own folder under
/// here, and that folder is the ONLY thing that separates one page's cookies from
/// another's. Point two pages at the same folder and they are the same visitor.
fn profiles_root() -> std::path::PathBuf {
    shikisha_core::config::browser_data_dir()
}

/// The window's own shell page (tab bar, board, terminal). It is our HTML, not
/// the web, so it shares nothing with the pages placed inside it — and it still
/// needs a folder of its own, or WebView2 drops one beside the exe
pub fn shell_data_dir() -> std::path::PathBuf {
    let dir = profiles_root().join("shell");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Turn a profile name into a safe folder name (strips path separators and `..`). Empty becomes "default"
fn sanitize_profile(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    let s = s.trim_matches('.').to_string();
    if s.is_empty() { "default".into() } else { s }
}

/// A running counter for private temp folder names (avoids collisions even within the same millisecond)
static PRIVATE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Return the data folder for a profile spec (creating it too).
/// For private mode, a unique temp folder (a different one on every call)
fn profile_dir(p: &BrowserProfile) -> std::path::PathBuf {
    let dir = if p.private {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let n = PRIVATE_SEQ.fetch_add(1, Ordering::Relaxed);
        profiles_root().join("_private").join(format!("{ms:013}-{n:04}"))
    } else {
        profiles_root().join("profiles").join(sanitize_profile(&p.name))
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Delete a closed private page's folder as soon as WebView2 lets go of it.
///
/// "Vanishes on close" is the whole promise of private mode, and the folder now
/// holds real cookies — so it can't be left lying about until the next launch.
/// WebView2 keeps its files locked for a moment after the view is gone, and the
/// window's own thread must not sit and wait (it pumps every message the window
/// gets), so the waiting happens off to the side. Startup's sweep is still the
/// backstop for anything this misses — a kill, a crash, a stubborn lock.
fn erase_when_released(dir: std::path::PathBuf) {
    std::thread::spawn(move || {
        for _ in 0..20 {
            if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });
}

/// At startup, sweep away any private areas left behind by a previous
/// abnormal exit. Private mode is supposed to "vanish on close", so
/// anything still there is garbage
pub fn sweep_private() {
    let _ = std::fs::remove_dir_all(profiles_root().join("_private"));
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    /// Two pages are the same visitor exactly when they are handed the same
    /// folder. Nothing else separates them, so this is worth pinning down.
    #[test]
    fn every_profile_gets_its_own_folder() {
        let a = profile_dir(&BrowserProfile::new("work", false));
        let b = profile_dir(&BrowserProfile::new("home", false));
        let same = profile_dir(&BrowserProfile::new("work", false));
        assert_ne!(a, b, "different profiles use the same container");
        assert_eq!(a, same, "the same name means the same container (the sign-in is kept)");
        // Private is a fresh area EVERY time, which is what makes reopening one a
        // reset rather than a reload — the site meets someone it has never seen
        let p1 = profile_dir(&BrowserProfile::new("", true));
        let p2 = profile_dir(&BrowserProfile::new("", true));
        assert_ne!(p1, p2, "private windows reuse the same container");
        assert!(p1.starts_with(profiles_root().join("_private")), "it is left out of the sweep: {p1:?}");
        // The shell is not one of the profiles, and never collides with a named one
        assert_ne!(shell_data_dir(), a);
        assert_ne!(shell_data_dir(), profile_dir(&BrowserProfile::shared_default()));
    }
}

impl Browser {
    /// Open the window on this thread, and run `work` beside it on a thread of
    /// its own, handed the window once its first page is ready.
    ///
    /// The window takes the thread it is called on because a Mac lets only the
    /// program's first thread draw windows: the loop that runs the window has
    /// to be the one `main` started on, and everything else -- the tabs, the
    /// automation, the board's server -- runs beside it. Windows allows either
    /// way round, and is given the same way, so there is one shape to reason
    /// about rather than one per system.
    ///
    /// `resident`: whether the window wears the program's icon in the
    /// notification area. The one window whose process outlives it does; a
    /// client, a probe and a self-check do not -- a second icon would only be
    /// a second thing to press that answers for the wrong copy.
    ///
    /// Returns what `work` returned, once both have finished: the window is
    /// closed when `work` is done with it, and `work` hears the window close
    /// the way it always has (`Ev::Closed`)
    pub fn host<R: Send + 'static>(
        url: &str,
        title: &str,
        resident: bool,
        work: impl FnOnce(Browser) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        if !is_openable(url) {
            return Err(anyhow!(shikisha_core::i18n::tp("err.browser.bad_url", &[("url", url)])));
        }
        let (proxy_tx, proxy_rx) = channel();
        let (ev_tx, ev_rx) = channel();
        let sound_pid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let its_sound_pid = std::sync::Arc::clone(&sound_pid);
        let beside = std::thread::Builder::new().name("shikisha-work".into()).spawn(move || -> Result<R> {
            let me = Self::once_open(proxy_rx, ev_rx, sound_pid)?;
            let closer = me.proxy.clone();
            let out = work(me);
            // Whatever `work` still holds of the window, the window is done
            let _ = closer.send_event(Cmd::Close);
            out
        })?;
        Self::run_here(url, title, resident, proxy_tx, ev_tx, its_sound_pid);
        beside.join().map_err(|_| anyhow!("the work beside the window stopped unexpectedly"))?
    }

    /// The same window on a thread of its own, held from the caller's thread.
    /// For the probes, which hold a window from the test's own thread: a test
    /// is not the program's first thread, and only Windows allows a window there
    #[cfg(all(windows, test))]
    pub fn spawn(url: &str, title: &str) -> Result<Self> {
        if !is_openable(url) {
            return Err(anyhow!(shikisha_core::i18n::tp("err.browser.bad_url", &[("url", url)])));
        }
        let (proxy_tx, proxy_rx) = channel();
        let (ev_tx, ev_rx) = channel();
        let (url, title) = (url.to_string(), title.to_string());
        let sound_pid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let its_sound_pid = std::sync::Arc::clone(&sound_pid);
        std::thread::Builder::new()
            .name("shikisha-browser".into())
            .spawn(move || Self::run_here(&url, &title, false, proxy_tx, ev_tx, its_sound_pid))?;
        Self::once_open(proxy_rx, ev_rx, sound_pid)
    }

    /// The window's loop, on the calling thread, until the window closes. A
    /// window that could not be made says so in the log, and its user hears
    /// it closed
    fn run_here(
        url: &str,
        title: &str,
        resident: bool,
        proxy_tx: Sender<tao::event_loop::EventLoopProxy<Cmd>>,
        ev_tx: Sender<Ev>,
        sound_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,
    ) {
        if let Err(e) = run_window(url, title, resident, proxy_tx, ev_tx.clone(), sound_pid) {
            shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                "err.browser.log_open_failed",
                &[("e", &format!("{e}"))],
            ));
            let _ = ev_tx.send(Ev::Closed);
        }
    }

    /// The handle on a window whose loop has been started, once its first
    /// page is ready
    fn once_open(
        proxy_rx: Receiver<tao::event_loop::EventLoopProxy<Cmd>>,
        ev_rx: Receiver<Ev>,
        sound_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,
    ) -> Result<Self> {
        // Wait until the window exists (if it can't be created, the proxy never arrives)
        let proxy = proxy_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .map_err(|_| anyhow!(shikisha_core::i18n::t("err.browser.startup_timeout")))?;

        let me = Self {
            proxy,
            events: std::sync::Mutex::new(ev_rx),
            next_id: AtomicU64::new(1),
            away: std::sync::atomic::AtomicBool::new(false),
            pending_rec: std::sync::Mutex::new(std::collections::HashSet::new()),
            devtools: std::sync::Mutex::new(None),
            spare: std::sync::Mutex::new(Vec::new()),
            through: std::sync::Mutex::new(None),
            digests: std::sync::Mutex::new(std::collections::HashMap::new()),
            sound_pid,
        };
        // Don't return until the document is ready. Returning as soon as
        // the window exists would leave the caller touching an empty
        // document, unable to tell "the selector is wrong" from "it just
        // hasn't arrived yet"
        me.wait_ready(std::time::Duration::from_secs(30))?;
        Ok(me)
    }

    /// Wait until the next document is ready and return its URL.
    /// Fires once per navigation, so this is also used after `open`
    pub fn wait_ready(&self, timeout: std::time::Duration) -> Result<String> {
        let until = std::time::Instant::now() + timeout;
        loop {
            let left = until
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow!(shikisha_core::i18n::t("err.browser.page_not_ready")))?;
            match self.events.lock().unwrap_or_else(|e| e.into_inner()).recv_timeout(left) {
                Ok(Ev::Ready { from, url, .. }) => {
                    self.reask(from.as_deref());
                    return Ok(url);
                }
                Ok(Ev::Closed) => return Err(anyhow!(shikisha_core::i18n::t("err.browser.closed"))),
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(_) => return Err(anyhow!(shikisha_core::i18n::t("err.browser.page_not_ready"))),
            }
        }
    }

    fn send(&self, cmd: Cmd) -> Result<()> {
        self.proxy
            .send_event(cmd)
            .map_err(|_| anyhow!(shikisha_core::i18n::t("err.browser.not_connected")))
    }

    /// Put the window away (see `Cmd::Hide`). From here until `show`, JS for
    /// the main view is refused at this end: the page is gone, and sending
    /// every screen change to it would be a message per keystroke into nothing
    pub fn hide(&self) -> Result<()> {
        self.away.store(true, Ordering::Relaxed);
        self.send(Cmd::Hide)
    }

    /// Open a tool over a picture of the screen (see `Cmd::Snip`)
    pub fn snip(&self, tool: &str, delay: u8) -> Result<()> {
        self.send(Cmd::Snip { tool: tool.to_string(), delay })
    }

    /// A way to open a tool from another thread: the keys that work from any
    /// program are heard on a thread of their own
    pub fn snip_opener(&self) -> SnipOpener {
        SnipOpener(self.proxy.clone())
    }

    /// A way to have those keys registered where the system takes them
    pub fn keys_registrar(&self) -> KeysRegistrar {
        KeysRegistrar(self.proxy.clone())
    }

    /// A way to hand the tool page its answer from another thread: the AI it
    /// asked is waited for away from the loop that draws everything
    pub fn snip_replier(&self) -> SnipReplier {
        SnipReplier(self.proxy.clone())
    }

    /// Bring the window back in front of the person
    pub fn show(&self) -> Result<()> {
        self.away.store(false, Ordering::Relaxed);
        self.send(Cmd::Show)
    }

    /// Say that pages from this address are the app's own and are heard in
    /// full (see `heard`). The board's own address is trusted from the start;
    /// the settings server's is told here once it has started
    pub fn trust(&self, url: &str) -> Result<()> {
        self.send(Cmd::Trust { origin: url.to_string() })
    }

    /// A banner from the notification-area icon
    pub fn tray_notice(&self, title: &str, text: &str) -> Result<()> {
        self.send(Cmd::TrayNotice { title: title.to_string(), text: text.to_string() })
    }

    /// Evaluate JS. The result arrives later as `Ev::Result`
    pub fn eval(&self, js: &str) -> Result<u64> {
        self.eval_in(None, js)
    }

    /// Evaluate JS against a target. `None` is the main view
    pub fn eval_in(&self, to: Option<&str>, js: &str) -> Result<u64> {
        if to.is_none() && self.away.load(Ordering::Relaxed) {
            return Err(anyhow!(shikisha_core::i18n::t("err.browser.page_not_placed")));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(Cmd::Eval {
            id,
            to: to.map(str::to_string),
            js: js.to_string(),
        })?;
        Ok(id)
    }

    /// Navigate a placed page
    pub fn go(&self, to: Option<&str>, go: Go) -> Result<()> {
        self.send(Cmd::Move {
            to: to.map(str::to_string),
            go,
        })
    }

    /// Start/stop screencasting (VNC-equivalent). Once started, `Ev::Frame` arrives
    pub fn screencast(&self, to: Option<&str>, on: bool) -> Result<()> {
        self.send(Cmd::Screencast {
            to: to.map(str::to_string),
            on,
        })
    }

    /// Where the DevTools of page `to` open from, starting the bridge the
    /// first time
    pub fn devtools_url(&self, to: Option<&str>) -> Result<String> {
        let mut bridge = self.devtools.lock().unwrap_or_else(|e| e.into_inner());
        if bridge.is_none() {
            *bridge = Some(crate::devtools::Bridge::start(self.proxy.clone())?);
        }
        Ok(bridge.as_ref().map(|b| b.screen_url(to)).unwrap_or_default())
    }

    /// Start or stop hearing a page's console (`Ev::ConsoleLine` per line)
    pub fn console(&self, to: Option<&str>, on: bool) -> Result<()> {
        self.send(Cmd::Console { to: to.map(str::to_string), on })
    }

    /// Inject input into the screencast view (finger traces, swipes, text)
    pub fn inject(&self, to: Option<&str>, input: Input) -> Result<()> {
        self.send(Cmd::Inject {
            to: to.map(str::to_string),
            input,
        })
    }

    /// Arm basic auth. From then on, returns credentials for this page's
    /// 401s. user/pass are already resolved from secrets (only the caller touches them)
    pub fn basic_auth(&self, to: Option<&str>, user: &str, pass: &str) -> Result<()> {
        self.send(Cmd::BasicAuth {
            to: to.map(str::to_string),
            user: user.to_string(),
            pass: pass.to_string(),
        })
    }

    /// Move keyboard focus (`None` = main view)
    pub fn focus(&self, to: Option<&str>) -> Result<()> {
        self.send(Cmd::Focus {
            to: to.map(str::to_string),
        })
    }

    /// Ask where we currently are (the answer arrives as a report)
    pub fn ask_where(&self, to: Option<&str>) -> Result<()> {
        self.send(Cmd::Where {
            to: to.map(str::to_string),
        })
    }

    /// Search a page for words (the answer arrives as a report)
    pub fn seek(&self, to: Option<&str>, text: &str, step: shikisha_shared::Seek) -> Result<()> {
        self.send(Cmd::Seek { to: to.map(str::to_string), text: text.to_string(), step })
    }

    /// Stop a download that is still being saved
    pub fn cancel_download(&self, id: &str) -> Result<()> {
        self.send(Cmd::CancelDownload { id: id.to_string() })
    }

    /// Give Ctrl+F in a page to the board's search row, or back to the browser
    pub fn find_keys(&self, to: Option<&str>, on: bool) -> Result<()> {
        self.send(Cmd::FindKeys { to: to.map(str::to_string), on })
    }

    /// Send everything placed pages fetch through this proxy from now on.
    ///
    /// Set once, when this window starts drawing another machine's pages. It
    /// takes effect for pages opened afterwards, because the setting belongs
    /// to the browser environment a page is born into and a page cannot be
    /// moved between two of them
    pub fn browse_through(&self, proxy_port: u16) {
        *self.through.lock().unwrap_or_else(|e| e.into_inner()) = Some(proxy_port);
    }

    /// Place a page inside the same window.
    ///
    /// Using a separate window would make ownership, position tracking,
    /// and even exposure during Windows Terminal tab switching all our
    /// own problem to manage. Placing it in the same window sidesteps all of it
    pub fn open_child(
        &self,
        name: &str,
        url: &str,
        rect: (i32, i32, i32, i32),
        profile: BrowserProfile,
    ) -> Result<()> {
        // The web, or a DevTools screen this window's own bridge made: its
        // address is the only way anything reaches that screen, so no other
        // `devtools://` address is let through
        let our_screen = self
            .devtools
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|b| b.made(url));
        if !is_openable(url) && !our_screen {
            return Err(anyhow!(shikisha_core::i18n::tp("err.browser.bad_url", &[("url", url)])));
        }
        self.send(Cmd::AddChild {
            name: name.to_string(),
            url: url.to_string(),
            rect,
            profile,
            through: *self.through.lock().unwrap_or_else(|e| e.into_inner()),
        })
    }

    /// The placed page's position and size. Setting width or height to 0 hides it
    pub fn child_bounds(&self, name: &str, rect: (i32, i32, i32, i32)) -> Result<()> {
        self.send(Cmd::ChildBounds {
            name: name.to_string(),
            rect,
        })
    }

    /// Put the placed page above the others.
    pub fn raise_child(&self, name: &str) -> Result<()> {
        self.send(Cmd::RaiseChild {
            name: name.to_string(),
        })
    }

    pub fn close_child(&self, name: &str) -> Result<()> {
        self.send(Cmd::RemoveChild {
            name: name.to_string(),
        })
    }















    /// Drain the reports accumulated so far (doesn't block).
    /// If we moved to a new document, re-show the bar that should be showing
    pub fn drain(&self) -> Vec<Ev> {
        // Return anything that arrived while we were waiting first (preserves arrival order)
        let mut evs: Vec<Ev> = std::mem::take(&mut *self.spare.lock().unwrap());
        evs.extend(self.events.lock().unwrap_or_else(|e| e.into_inner()).try_iter());
        // An answer belongs to whoever asked for it, not to whoever reads the
        // queue first. Put those back rather than hand them out: the asker is
        // blocked waiting, and a stolen answer is an operation that times out
        // for no reason anybody can see
        let (answers, rest): (Vec<Ev>, Vec<Ev>) =
            evs.into_iter().partition(|e| matches!(e, Ev::Result { .. }));
        if !answers.is_empty() {
            self.spare.lock().unwrap().extend(answers);
        }
        let evs = rest;
        for e in &evs {
            if let Ev::Ready { from, .. } = e {
                self.reask(from.as_deref());
            }
        }
        evs
    }

    /// Arm/disarm the Lua recorder (📼) on a page. Like the bar, "should it be
    /// recording" lives here and is re-issued on every new document.
    pub fn record(&self, to: Option<&str>, on: bool) -> Result<()> {
        let key = to.map(str::to_string);
        if on {
            self.pending_rec.lock().unwrap().insert(key);
        } else {
            self.pending_rec.lock().unwrap().remove(&key);
        }
        self.eval_in(to, &format!("window.__shikisha_rec && window.__shikisha_rec({on});"))
            .map(|_| ())
    }

    /// Silence the recorder everywhere (there's only ever one recorder — arming
    /// a page goes through this first, so two pages never record at once)
    pub fn record_all_off(&self) {
        let keys: Vec<Option<String>> =
            self.pending_rec.lock().unwrap().drain().collect();
        for k in keys {
            let _ = self.eval_in(
                k.as_deref(),
                "window.__shikisha_rec && window.__shikisha_rec(false);",
            );
        }
    }

    /// Re-dress a document that navigation just wiped: the recorder arming is
    /// Rust-remembered state, re-issued per new document. Only for the page
    /// that navigated. (The bar asking the person something is not in the
    /// page any more, so it needs no re-dressing: it stays up on the board
    /// until the script takes it down)
    fn reask(&self, to: Option<&str>) {
        let key = to.map(str::to_string);
        // The digest died with the document (backendNodeIds are per-document).
        // Dropping it here turns a later `{ref=N}` into a clear "take a new
        // digest" instead of a click on a node that no longer exists
        self.digests.lock().unwrap().remove(&key);
        if self.pending_rec.lock().unwrap().contains(&key) {
            let _ = self.eval_in(to, "window.__shikisha_rec && window.__shikisha_rec(true);");
        }
    }

    /// Wait until a password is entered.
    /// Any other signal that arrives while waiting is kept aside (discarding it loses it forever)
    pub fn wait_password(&self, timeout: std::time::Duration) -> Result<Option<String>> {
        match self.poll_password(timeout)? {
            Typed::Answer(text) => Ok(text),
            Typed::Quit => Err(anyhow!(shikisha_core::i18n::t("err.browser.window_closed"))),
            Typed::Nothing => Err(anyhow!(shikisha_core::i18n::t("err.browser.no_input"))),
        }
    }

    /// What the password prompt was answered within `timeout`, without making
    /// a missing answer an error: the lock asks this over and over while it
    /// also listens to the phone's door. The window's ✕ and the tray's Quit
    /// are an answer too -- the one way past the lock that is not a password.
    /// Any other signal is kept aside, as above
    pub fn poll_password(&self, timeout: std::time::Duration) -> Result<Typed> {
        let until = std::time::Instant::now() + timeout;
        loop {
            let Some(left) = until.checked_duration_since(std::time::Instant::now()) else {
                return Ok(Typed::Nothing);
            };
            match self.events.lock().unwrap_or_else(|e| e.into_inner()).recv_timeout(left) {
                Ok(Ev::Password { text }) => return Ok(Typed::Answer(text)),
                Ok(Ev::Closed | Ev::CloseRequested | Ev::TrayQuit) => return Ok(Typed::Quit),
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(Typed::Nothing),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(Typed::Quit),
            }
        }
    }

    /// Wait until a specific evaluation's result arrives
    pub fn wait_result(&self, id: u64, timeout: std::time::Duration) -> Result<String> {
        let (ok, value) = self.wait_ev(id, timeout)?;
        if ok {
            Ok(value)
        } else {
            Err(anyhow!(shikisha_core::i18n::tp(
                "err.browser.js_eval_failed",
                &[("value", &value)]
            )))
        }
    }

    /// The shared wait behind `Eval` and `Cdp`: the raw (ok, payload) pair for
    /// one id, so each caller can word its own failure
    fn wait_ev(&self, id: u64, timeout: std::time::Duration) -> Result<(bool, String)> {
        let until = std::time::Instant::now() + timeout;
        // It may already have arrived and been set aside -- by another wait, or
        // by a drain that knew it was not its to take
        {
            let mut spare = self.spare.lock().unwrap();
            if let Some(at) = spare
                .iter()
                .position(|e| matches!(e, Ev::Result { id: got, .. } if *got == id))
                && let Ev::Result { ok, value, .. } = spare.remove(at) {
                    return Ok((ok, value));
                }
        }
        loop {
            let left = until
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow!(shikisha_core::i18n::t("err.browser.no_result")))?;
            match self.events.lock().unwrap_or_else(|e| e.into_inner()).recv_timeout(left) {
                Ok(Ev::Result { id: got, ok, value }) if got == id => return Ok((ok, value)),
                Ok(Ev::Ready { from, .. }) => {
                    self.reask(from.as_deref());
                    continue;
                }
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(_) => return Err(anyhow!(shikisha_core::i18n::t("err.browser.no_result"))),
            }
        }
    }

    // ── CDP-backed operations (digest and {ref=N}) ──────────────────────
    //
    // The JS world can only see what a page chooses to expose; the DevTools
    // protocol sees what the browser itself knows (accessibility tree, layout,
    // and genuine input injection). The digest and every ref operation live on
    // this side so that names come from the browser's accname computation and
    // clicks/keys are real input events, indistinguishable from a human's.

    /// The board's page as paper: what `@media print` draws of it, as the
    /// bytes of a PDF. The page sets the paper's size and margins itself
    /// (`@page`), so what is drawn for paper is decided in one place
    pub fn print_pdf(&self) -> Result<Vec<u8>> {
        pageops::pdf(self, None, pageops::PRINT_WAIT_MS).map(|(bytes, _)| bytes)
    }

    /// Call one CDP method on a page and wait for its result (parsed JSON)
    fn cdp_call(
        &self,
        to: Option<&str>,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(Cmd::Cdp {
            id,
            to: to.map(str::to_string),
            method: method.to_string(),
            params: params.to_string(),
        })?;
        let (ok, value) = self.wait_ev(id, std::time::Duration::from_millis(timeout_ms))?;
        if !ok {
            return Err(anyhow!(shikisha_core::i18n::tp(
                "err.browser.cdp_failed",
                &[("method", method), ("e", &value)]
            )));
        }
        Ok(serde_json::from_str(&value).unwrap_or(serde_json::Value::Null))
    }
















}

impl Drop for Browser {
    /// Don't leave behind a window whose conductor is gone.
    /// It's fine if closing fails (that just means the other side already died first)
    fn drop(&mut self) {
        let _ = self.proxy.send_event(Cmd::Close);
    }
}

/// Wrap an expression so its result comes back over IPC.
///
/// Wrapped in an async function and awaited, so an async value like the
/// result of `fetch` also gets resolved before returning. Awaiting a
/// synchronous value just passes it through, so existing DOM calls still work as-is
/// What `Runtime.evaluate` said, in the shape a page's own answer has: whether
/// it worked, and the value (or the error) as JSON text.
fn evaluated(json: &str) -> (bool, String) {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    if let Some(ex) = v.get("exceptionDetails") {
        let said = ex
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| ex.get("text").and_then(|t| t.as_str()))
            .unwrap_or("error");
        return (false, serde_json::Value::String(said.to_string()).to_string());
    }
    let value = v
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    (true, value.to_string())
}

fn wrap_eval(id: u64, js: &str) -> String {
    format!(
        r#"(async function(){{
  try {{
    var v = await (async function(){{ {js} }})();
    window.ipc.postMessage(JSON.stringify({{kind:"result",id:{id},ok:true,
      value: v === undefined ? null : v}}));
  }} catch (e) {{
    window.ipc.postMessage(JSON.stringify({{kind:"result",id:{id},ok:false,
      value: String(e && e.message || e)}}));
  }}
}})();"#
    )
}







/// Everything a browser has to say about who it is, once a name is chosen.
///
/// The name is not the only place the claim is made. It is made again, in
/// `Sec-CH-UA` and in `navigator.userAgentData`, and those are not written
/// from the name — a browser calling itself Chrome while announcing
/// "Microsoft Edge WebView2" alongside has told a site more than it would
/// have by saying nothing at all. So the brands are built out of the name
/// itself, and the two always agree.
///
/// A name with no Chromium version in it (someone naming a browser that is
/// not one) is given no brands: that is what a browser which does not speak
/// client hints sends.
fn ua_override(ua: &str) -> String {
    let after = |mark: &str| -> Option<String> {
        let at = ua.find(mark)? + mark.len();
        let ver: String = ua[at..].chars().take_while(|c| c.is_ascii_digit()).collect();
        (!ver.is_empty()).then_some(ver)
    };
    let mut brands = Vec::new();
    if let Some(major) = after("Chrome/") {
        // The greased entry is part of the format, not decoration: a site
        // that hard-codes the list is meant to trip over it
        brands.push(serde_json::json!({ "brand": "Chromium", "version": major }));
        brands.push(serde_json::json!({ "brand": "Not=A?Brand", "version": "99" }));
        match after("Edg/") {
            Some(edge) => {
                brands.push(serde_json::json!({ "brand": "Microsoft Edge", "version": edge }))
            }
            None => brands
                .push(serde_json::json!({ "brand": "Google Chrome", "version": major })),
        }
    }
    serde_json::json!({
        "userAgent": ua,
        "userAgentMetadata": {
            "brands": brands,
            "platform": "Windows",
            "platformVersion": "15.0.0",
            "architecture": "x86",
            "bitness": "64",
            "model": "",
            "mobile": false,
        },
    })
    .to_string()
}

/// Has the keys that work from any program registered on the window's thread,
/// where a Mac takes them (see `hotkeys`)
#[derive(Clone)]
pub struct KeysRegistrar(tao::event_loop::EventLoopProxy<Cmd>);

impl KeysRegistrar {
    /// Register what the settings ask for now, letting go of what was held
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn register(&self) {
        let _ = self.0.send_event(Cmd::RegisterKeys);
    }
}

/// Opens a tool from another thread (see `Browser::snip_opener`).
pub struct SnipOpener(tao::event_loop::EventLoopProxy<Cmd>);

impl SnipOpener {
    /// Take the screen now and open `tool` over it, or with no tool, frame the
    /// picture first and choose after. No wait: a key is pressed with the
    /// program to be pictured already in front
    pub fn open(&self, tool: &str) {
        let _ = self.0.send_event(Cmd::Snip { tool: tool.to_string(), delay: 0 });
    }

    /// Bring the window to the front and open `what` on the board (one of
    /// `hotkeys::ON_THE_BOARD`)
    pub fn summon(&self, what: &str) {
        let _ = self.0.send_event(Cmd::Summon { what: what.to_string() });
    }
}

/// Hands the tool page an answer (see `Browser::snip_replier`).
pub struct SnipReplier(tao::event_loop::EventLoopProxy<Cmd>);

impl SnipReplier {
    pub fn answer(&self, json: String) {
        let _ = self.0.send_event(Cmd::SnipAnswer { json });
    }
}

#[cfg(test)]
mod nav_tests {
    use super::*;
    use shikisha_core::view::openable;

    /// Fill in a missing scheme, and never allow anything but http/https.
    ///
    /// The URL bar is a "gateway to anywhere", so opening `file:` would
    /// expose local files and `javascript:` could hijack the current
    /// page — and from there, automation would be exposed to it.
    /// The destination is narrowed down right here
    #[test]
    fn the_address_box_only_opens_web_pages() {
        assert_eq!(openable("example.com").as_deref(), Some("https://example.com"));
        assert_eq!(
            openable("  https://a.example/x?y=1  ").as_deref(),
            Some("https://a.example/x?y=1"),
            "surrounding spaces are dropped"
        );
        assert_eq!(
            openable("http://127.0.0.1:8080/").as_deref(),
            Some("http://127.0.0.1:8080/")
        );
        assert_eq!(
            openable("HTTPS://Example.com/A").as_deref(),
            Some("https://Example.com/A"),
            "a pasted upper-case scheme gets through too (the later checks assume lower case)"
        );
        for empty in ["", "   "] {
            assert!(openable(empty).is_none(), "it opens: {empty}");
        }
        // Dangerous schemes never reach the page — they become an inert search
        // instead. A file on this PC is taken before this and served over
        // HTTP (crate::localpage); a `\` path is never read as a host
        for bad in ["file:///C:/secret.txt", "ftp://x/y", "javascript:alert(1)", "D:\\site\\index.html"] {
            let got = openable(bad).unwrap_or_default();
            assert!(
                got.starts_with("https://www.google.com/search?q="),
                "it did not fall back to a search: {bad} -> {got}"
            );
        }
    }

    /// Text that doesn't read as an address searches Google instead — same
    /// habit as Chrome's box. Japanese (multibyte) must survive as UTF-8
    /// percent-encoding, and spaces split words with `+`
    #[test]
    fn the_address_box_searches_words() {
        assert_eq!(
            openable("エラー処理").as_deref(),
            Some("https://www.google.com/search?q=%E3%82%A8%E3%83%A9%E3%83%BC%E5%87%A6%E7%90%86")
        );
        assert_eq!(
            openable("rust async 使い方").as_deref(),
            Some("https://www.google.com/search?q=rust+async+%E4%BD%BF%E3%81%84%E6%96%B9")
        );
        // A dot inside a phrase with spaces is still a search, not an address
        assert_eq!(
            openable("tokio.rs とは").as_deref(),
            Some("https://www.google.com/search?q=tokio.rs+%E3%81%A8%E3%81%AF")
        );
        // A lone word with no dot searches; localhost is the address exception
        let one = openable("rust").unwrap_or_default();
        assert!(one.starts_with("https://www.google.com/search?q=rust"), "{one}");
        assert_eq!(
            openable("localhost:8080/x").as_deref(),
            Some("https://localhost:8080/x")
        );
        // Query characters that would break the search URL are encoded
        assert_eq!(
            openable("a&b=c").as_deref(),
            Some("https://www.google.com/search?q=a%26b%3Dc")
        );
    }

    /// The wheel's signal reads as an amount to scroll back through the log.
    /// Turning it up goes to the past (positive), turning it down goes to now (negative)
    #[test]
    fn the_wheel_asks_to_go_back_through_the_log() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v)
        };
        assert!(matches!(
            read(r#"{"kind":"scroll","by":3,"row":4,"col":9}"#),
            Some(Ev::Scroll { by: 3, row: 4, col: 9 })
        ));
        assert!(matches!(
            read(r#"{"kind":"scroll","by":-3,"row":0,"col":0}"#),
            Some(Ev::Scroll { by: -3, .. })
        ));
        // With no amount it doesn't move (0 means "do nothing", not "discard")
        assert!(matches!(read(r#"{"kind":"scroll"}"#), Some(Ev::Scroll { by: 0, .. })));
        // Clamp amounts beyond a tall phone's page turn (≈ one tick per row)
        assert!(matches!(
            read(r#"{"kind":"scroll","by":999999}"#),
            Some(Ev::Scroll { by: 250, .. })
        ));
    }

    /// A signal from the screen becomes a navigation instruction as-is
    #[test]
    fn the_bar_speaks_the_same_words_as_the_loop() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v)
        };
        assert!(matches!(
            read(r#"{"kind":"go","what":"back"}"#),
            Some(Ev::Go { go: Go::Back })
        ));
        assert!(matches!(
            read(r#"{"kind":"go","what":"reload"}"#),
            Some(Ev::Go { go: Go::Reload })
        ));
        // The same button, held down. A page that is wrong from a build that
        // has moved on is exactly when someone reaches for it
        assert!(matches!(
            read(r#"{"kind":"go","what":"hardreload"}"#),
            Some(Ev::Go { go: Go::Hard })
        ));
        match read(r#"{"kind":"go","what":"to","url":"example.com"}"#) {
            Some(Ev::Go { go: Go::To(u) }) => assert_eq!(u, "example.com"),
            other => panic!("the destination was not read: {other:?}"),
        }
        // Discard unknown instructions. Doing nothing is better than silently doing something else
        assert!(read(r#"{"kind":"go","what":"quit"}"#).is_none());
        assert!(read(r#"{"kind":"go"}"#).is_none());
    }

    /// A chosen name has to be told the same way twice, or the second telling
    /// gives away the first.
    #[test]
    fn what_the_browser_says_it_is_agrees_with_itself() {
        let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36";
        let v: serde_json::Value = serde_json::from_str(&ua_override(chrome)).unwrap();
        let brands = v["userAgentMetadata"]["brands"].as_array().unwrap();
        let names: Vec<&str> = brands.iter().filter_map(|b| b["brand"].as_str()).collect();
        assert!(names.contains(&"Google Chrome"), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("WebView")), "it disagrees with what it calls itself: {names:?}");
        assert_eq!(brands[0]["version"], "151");

        // Edge names itself twice; both have to be there or the pair is odd
        let edge = format!("{chrome} Edg/151.0.0.0");
        let v: serde_json::Value = serde_json::from_str(&ua_override(&edge)).unwrap();
        let names: Vec<&str> = v["userAgentMetadata"]["brands"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|b| b["brand"].as_str())
            .collect();
        assert!(names.contains(&"Microsoft Edge") && names.contains(&"Chromium"), "{names:?}");

        // Something that is not a Chromium at all sends no brands, which is
        // exactly what such a browser does
        let v: serde_json::Value =
            serde_json::from_str(&ua_override("Mozilla/5.0 … Firefox/999.0")).unwrap();
        assert!(v["userAgentMetadata"]["brands"].as_array().unwrap().is_empty());
    }

    /// A phone reporting its screen shape parses into a View input, and a
    /// nonsense size can never divide by zero downstream (floors at 1)
    #[test]
    fn a_viewer_screen_shape_parses() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"kind":"inject","what":"view","w":390,"h":780}"#).unwrap();
        match parse_intent(&v) {
            Some(Ev::Inject { input: Input::View { w, h, dpr }, .. }) => {
                assert_eq!((w, h), (390.0, 780.0));
                assert_eq!(dpr, 1.0, "a viewer that does not say its density is not taken at one to one");
            }
            other => panic!("the shape of the screen was not read: {other:?}"),
        }
        let z: serde_json::Value =
            serde_json::from_str(r#"{"kind":"inject","what":"view","w":0,"h":-5}"#).unwrap();
        match parse_intent(&z) {
            Some(Ev::Inject { input: Input::View { w, h, dpr }, .. }) => {
                assert!(w >= 1.0 && h >= 1.0, "a divide by zero waiting to happen: {w}x{h}");
                assert_eq!(dpr, 1.0);
            }
            other => panic!("the shape of the screen was not read: {other:?}"),
        }
        // A phone's density is believed up to what screens have, not past it
        for (said, taken) in [("3", 3.0), ("2.625", 2.625), ("40", shikisha_shared::MAX_VIEWER_DPR)] {
            let d: serde_json::Value = serde_json::from_str(&format!(
                r#"{{"kind":"inject","what":"view","w":390,"h":780,"dpr":{said}}}"#
            ))
            .unwrap();
            match parse_intent(&d) {
                Some(Ev::Inject { input: Input::View { dpr, .. }, .. }) => assert_eq!(dpr, taken, "said {said}"),
                other => panic!("the shape of the screen was not read: {other:?}"),
            }
        }
    }

    /// The desk button and the model-chat box parse into their own intents,
    /// not into a keystroke that would leak into the visible session.
    #[test]
    fn desk_and_chat_intents_parse() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v)
        };
        assert!(matches!(read(r#"{"kind":"opendesk"}"#), Some(Ev::OpenDesk)));
        // The gear names the tab in view by its place in the folder
        match read(r#"{"kind":"opensettings","tabpos":1,"folder":"D:/work","tabname":"PowerShell"}"#) {
            Some(Ev::OpenSettings { tabpos, folder, section, ret, tabname, tabkey: _, sheet: _ }) => {
                assert_eq!(tabpos, Some(1), "the position of the tab being looked at was dropped");
                assert_eq!(tabname.as_deref(), Some("PowerShell"), "the tab's name was dropped");
                assert_eq!(folder.as_deref(), Some("D:/work"));
                assert!(section.is_none() && !ret);
            }
            other => panic!("opensettings was not read: {other:?}"),
        }
        match read(r#"{"kind":"opensettings"}"#) {
            Some(Ev::OpenSettings { tabpos, .. }) => assert!(tabpos.is_none(), "it came in though there was no position"),
            other => panic!("opensettings was not read: {other:?}"),
        }
        match read(r#"{"kind":"say","tab":3,"uid":"tab-uid","text":"hello"}"#) {
            Some(Ev::Say { tab, text, .. }) => {
                assert_eq!((tab, text.as_str()), (3, "hello"), "the addressee and the text do not match");
            }
            other => panic!("say was not read: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// An answer read off the DevTools protocol has the shape a page's own
    /// answer has, so whoever asked cannot tell which road it came by.
    #[test]
    fn an_answer_by_the_devtools_road_reads_like_a_page_answer() {
        assert_eq!(
            evaluated(r#"{"result":{"type":"string","value":"PAGE-C"}}"#),
            (true, "\"PAGE-C\"".to_string())
        );
        assert_eq!(evaluated(r#"{"result":{"type":"undefined"}}"#), (true, "null".to_string()));
        assert_eq!(
            evaluated(r#"{"result":{"type":"object","value":{"a":1}}}"#),
            (true, r#"{"a":1}"#.to_string())
        );
        let (ok, said) = evaluated(
            r#"{"result":{"type":"object"},"exceptionDetails":{"text":"Uncaught","exception":{"description":"ReferenceError: x is not defined"}}}"#,
        );
        assert!(!ok);
        assert_eq!(said, "\"ReferenceError: x is not defined\"");
    }

    /// A window put away from minimized gets its board back.
    ///
    /// The board was built again before the window was restored, into a
    /// window with no size, and WebView2 refused it (0x80070057): what came
    /// back from the notification area was an empty frame. The window has to
    /// be shown and restored before the page is built
    #[test]
    fn the_board_is_built_into_a_window_that_has_a_size() {
        let src = include_str!("browser/window.rs");
        let show = src.find("Cmd::Show => {").expect("there is no Cmd::Show");
        let body = &src[show..show + 2500];
        let restored = body.find("window.set_minimized(false)").expect("it does not bring the window back from minimized");
        let built = body.find("pages.fill(&window, shell_spec())").expect("it does not rebuild the board");
        assert!(restored < built, "it builds the board while still minimized");
    }

    /// A window that is put away does not re-shape the page inside it.
    ///
    /// Windows gives a minimized window a small size of its own (128x220 on
    /// this machine) rather than none, so a guard against zero lets it
    /// through. The board laid itself out to it, passed it down to the page
    /// it was relaying, and a phone watching from another room got that page
    /// 128 pixels wide and blew it up to fill its screen
    #[test]
    fn a_put_away_window_does_not_resize_what_it_holds() {
        let src = include_str!("browser/window.rs");
        let resized =
            src.find("event: WindowEvent::Resized(size),").expect("nothing follows the size");
        // Between the size arriving and the board being given one, rather than
        // a fixed number of characters after it: a line added above the guard
        // used to carry it out of the window and fail a test about the guard
        let after = &src[resized..];
        let used = after.find("v.fill(w, h)").expect("the size is never used");
        let body = &after[..used];
        assert!(
            body.contains("window.is_minimized()") && body.contains("last_size"),
            "the board follows the window into being minimized"
        );
        let child =
            src.find("Cmd::ChildBounds { name, rect } => {").expect("no pane sizing");
        let body = &src[child..child + 900];
        assert!(
            body.contains("window.is_minimized()"),
            "a pane is re-sized by the layout of a window nobody can see"
        );
    }

    /// A page placed in the window may report, and may not ask. Every intent
    /// that types, runs, changes or opens something is refused from a
    /// stranger's page; the handful of reports our own scripts post from inside
    /// a page still come through. This is the list itself, pinned: a variant
    /// added to `Ev` is refused from a page until somebody writes down why not
    #[test]
    fn a_stranger_page_may_report_but_never_ask() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v).unwrap_or_else(|| panic!("parse_intent cannot read: {s}"))
        };
        for s in [
            r#"{"kind":"say","tab":1,"uid":"tab-uid","text":"rm -rf ~"}"#,
            r#"{"kind":"key","text":"x"}"#,
            r#"{"kind":"runlua","code":"print(1)"}"#,
            r#"{"kind":"runaction","index":0}"#,
            r#"{"kind":"runkey","name":"x"}"#,
            r#"{"kind":"git","panel":"a","act":"commit"}"#,
            r#"{"kind":"opensettings"}"#,
            r#"{"kind":"closesettings"}"#,
            r#"{"kind":"select","tab":2}"#,
            r#"{"kind":"menu","key":"q"}"#,
            r#"{"kind":"attach","id":1,"name":"a","data":"b"}"#,
            r#"{"kind":"paste"}"#,
            r#"{"kind":"copy","text":"x"}"#,
            r#"{"kind":"stop"}"#,
            r#"{"kind":"restart"}"#,
            r#"{"kind":"go","what":"to","url":"https://example.com"}"#,
            r#"{"kind":"inject","what":"text","text":"x"}"#,
            r#"{"kind":"suggest","text":"x"}"#,
            r#"{"kind":"password","text":"x"}"#,
            r#"{"kind":"addtab"}"#,
            r#"{"kind":"branch","from":"a","branch":"b"}"#,
            // The bar's press: a page saying "the person pressed it" is the
            // one report that must never be believed
            r#"{"kind":"button","name":"web"}"#,
        ] {
            assert!(!allowed_from_page(&read(s)), "it gets through from a page that is not ours: {s}");
        }
        for s in [
            r#"{"kind":"touched"}"#,
            r#"{"kind":"compose"}"#,
            r##"{"kind":"recorded","act":"click","sel":"#a"}"##,
            r#"{"kind":"ready","url":"https://example.com"}"#,
            r#"{"kind":"loading","busy":false}"#,
            r#"{"kind":"result","id":1,"ok":true,"value":"x"}"#,
        ] {
            assert!(allowed_from_page(&read(s)), "a page's report is dropped: {s}");
        }
    }

    /// What a stranger's page says is sifted: its asks vanish, its reports
    /// arrive stamped with the pane's name (whatever the message claimed), and
    /// the app's own page — judged by the address it spoke from — keeps its
    /// full voice. Broken JSON and unknown kinds are dropped without a word
    #[test]
    fn a_placed_page_is_heard_only_as_a_report() {
        let mut asked = Asked::default();
        let stranger = |body: &str, asked: &mut Asked| heard(body, Some("web"), false, asked);
        // The very message the review found going into the terminal
        assert!(
            stranger(r#"{"kind":"say","tab":1,"uid":"tab-uid","text":"SECURITY_REVIEW_MARKER"}"#, &mut asked)
                .is_none(),
            "a say from a page that is not ours reaches the terminal"
        );
        assert!(stranger(r#"{"kind":"key","text":"x"}"#, &mut asked).is_none());
        assert!(stranger(r#"{"kind":"runlua","code":"1"}"#, &mut asked).is_none());
        assert!(stranger("not json", &mut asked).is_none());
        assert!(stranger(r#"{"kind":"nosuchthing"}"#, &mut asked).is_none());
        // A page cannot press the bar for the person
        assert!(
            stranger(r#"{"kind":"button","name":"web"}"#, &mut asked).is_none(),
            "a page can press the banner's button"
        );
        // A report gets the pane's name, not the one it wrote
        match stranger(r#"{"kind":"touched","from":"settings"}"#, &mut asked) {
            Some(Ev::Touched { from }) => assert_eq!(from.as_deref(), Some("web")),
            other => panic!("the focus report does not arrive: {other:?}"),
        }
        match stranger(r#"{"kind":"ready","url":"https://a.example/"}"#, &mut asked) {
            Some(Ev::Ready { from, url, .. }) => {
                assert_eq!(from.as_deref(), Some("web"));
                assert_eq!(url, "https://a.example/");
            }
            other => panic!("the load-finished report does not arrive: {other:?}"),
        }
        // Our own page, speaking from our address, still asks
        assert!(matches!(
            heard(r#"{"kind":"closesettings"}"#, Some("settings"), true, &mut asked),
            Some(Ev::CloseSettings)
        ));
        assert!(matches!(
            heard(r#"{"kind":"select","tab":0}"#, Some("settings"), true, &mut asked),
            Some(Ev::Select { tab: 0 })
        ));
        // The board's own reports carry no name -- except the bar's press,
        // which names the page the bar stands under
        assert!(matches!(
            heard(r#"{"kind":"say","tab":1,"uid":"tab-uid","text":"ls"}"#, None, true, &mut asked),
            Some(Ev::Say { tab: 1, .. })
        ));
        match heard(r#"{"kind":"button","name":"br"}"#, None, true, &mut asked) {
            Some(Ev::Button { from }) => assert_eq!(from.as_deref(), Some("br")),
            other => panic!("a press on the board's banner does not arrive: {other:?}"),
        }
    }

    /// An answer is believed only from the page that was asked, and only once.
    /// A neighbour's page guessing the id cannot answer for it; a second
    /// answer, even from the right page, is refused; and the bookkeeping does
    /// not grow without end
    #[test]
    fn an_answer_is_believed_only_from_the_page_that_was_asked() {
        let mut asked = Asked::default();
        asked.ask(7, Some("bank".into()));
        asked.ask(8, None);
        let answer = |id: u64| format!(r#"{{"kind":"result","id":{id},"ok":true,"value":"x"}}"#);
        // The neighbour answers first, with the right id
        assert!(
            heard(&answer(7), Some("evil"), false, &mut asked).is_none(),
            "a neighboring page can replace the answer"
        );
        // Then the page that was asked
        assert!(matches!(
            heard(&answer(7), Some("bank"), false, &mut asked),
            Some(Ev::Result { id: 7, ok: true, .. })
        ));
        // ...and nobody answers the same question twice
        assert!(heard(&answer(7), Some("bank"), false, &mut asked).is_none());
        // The board's question is not a page's to answer, nor a page's the board's
        assert!(heard(&answer(8), Some("bank"), true, &mut asked).is_none());
        assert!(matches!(
            heard(&answer(8), None, true, &mut asked),
            Some(Ev::Result { id: 8, .. })
        ));
        // A question nobody asked has no answer
        assert!(heard(&answer(9), None, true, &mut asked).is_none());
        // Bounded: the oldest question is forgotten past the cap
        let mut many = Asked::default();
        for id in 0..(Asked::KEPT as u64 + 10) {
            many.ask(id, None);
        }
        assert_eq!(many.of.len(), Asked::KEPT);
        assert!(!many.answered(0, None), "old questions stay even past the limit");
        assert!(many.answered(Asked::KEPT as u64 + 9, None));
    }

    /// The app's pages come from two local servers: the board's, and the one
    /// serving the settings and the result view on a port of its own. A page
    /// from either is ours; a page from any other port on the same machine is
    /// not (a dev server in a browser tab is somebody else's code)
    #[test]
    fn our_pages_come_from_two_servers() {
        let own = vec!["http://127.0.0.1:8787/".to_string(), "http://127.0.0.1:51604/?token=x".to_string()];
        assert!(from_ours(&own, "http://127.0.0.1:8787/?token=abc"));
        assert!(from_ours(&own, "http://127.0.0.1:51604/?token=abc&desk=2"), "the settings page is treated as a stranger");
        assert!(from_ours(&own, "http://127.0.0.1:51604/result?token=abc&run=1"));
        assert!(!from_ours(&own, "http://127.0.0.1:3000/"), "another server on the same machine is treated as ours");
        assert!(!from_ours(&own, "https://example.com/"));
        assert!(!from_ours(&[], "http://127.0.0.1:8787/"));
    }

    /// "The same site" is scheme, host and port, read by parsing — a prefix
    /// match would let `http://127.0.0.1:8787.evil.example/` pass as ours
    #[test]
    fn same_site_means_scheme_host_and_port() {
        let own = "http://127.0.0.1:8787/";
        assert!(same_origin("http://127.0.0.1:8787/?token=abc", own));
        assert!(same_origin("http://127.0.0.1:8787/settings?x=1#y", own));
        assert!(same_origin("HTTP://127.0.0.1:8787/", own));
        assert!(!same_origin("http://127.0.0.1:8788/", own), "a different port is treated as the same");
        assert!(!same_origin("https://127.0.0.1:8787/", own), "a different scheme is treated as the same");
        assert!(!same_origin("http://localhost:8787/", own), "a different host name is treated as the same");
        assert!(!same_origin("http://127.0.0.1:8787.evil.example/", own), "a prefix match gets through");
        assert!(!same_origin("http://evil.example/http://127.0.0.1:8787/", own));
        assert!(!same_origin("about:blank", own));
        assert!(!same_origin("", own));
        assert!(!same_origin("garbage", own));
        // The usual port is the written port
        assert!(same_origin("https://a.example/", "https://a.example:443/x"));
        assert!(same_origin("http://a.example/", "http://a.example:80/x"));
        assert!(!same_origin("http://a.example/", "https://a.example/"));
    }

    /// A placed page can draw the app's own two pieces of chrome, because
    /// nothing of the window's can be drawn over it: the pen that summons the
    /// composer, and a message raised while that page is in front. Both live in
    /// the script every placed page is created with, and both hang off a name
    /// the app calls — rename one here and the window would go on calling into
    /// a page that no longer answers, silently.
    #[test]
    fn a_placed_page_can_draw_the_pen_and_a_message() {
        for name in ["__shikisha_pen", "__shikisha_toast"] {
            assert!(
                PLACED_JS.contains(&format!("window.{name} = function")),
                "{name} is not placed"
            );
        }
        // In a shadow root, or the page's own CSS reaches it (and ours reaches
        // the page). The host is findable by id so it can be checked for.
        assert!(
            PLACED_JS.contains("toastEl.id = \"__shikisha_toast\"")
                && PLACED_JS.matches("attachShadow").count() >= 2,
            "it is not placed in the shadow root / it has no name"
        );
    }


    /// A placed page reaches the network through the proxy it was given.
    ///
    /// This is the one link in "draw the page here, fetch through the server"
    /// that nothing else can prove: whether WebView2 honours the proxy it is
    /// handed when the page is placed. The address asked for cannot be
    /// resolved by anybody -- so if the proxy is not used, nothing arrives and
    /// no page loads.
    ///
    ///   cargo test --bin SHIKISHA-TERM through_the_proxy -- --ignored --nocapture
    // A real window, which only Windows can open yet
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn a_placed_page_reaches_the_network_through_the_proxy() {
        use std::io::{Read, Write};

        // Something standing where the tunnel's proxy stands
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let asked: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let heard = std::sync::Arc::clone(&asked);
        std::thread::spawn(move || {
            for sock in listener.incoming().flatten() {
                let heard = std::sync::Arc::clone(&heard);
                std::thread::spawn(move || {
                    let mut sock = sock;
                    let mut head = Vec::new();
                    let mut one = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        if sock.read(&mut one).unwrap_or(0) == 0 {
                            return;
                        }
                        head.push(one[0]);
                    }
                    let said = String::from_utf8_lossy(&head).to_string();
                    heard.lock().unwrap().push(said.lines().next().unwrap_or("").to_string());
                    let body = "<!doctype html><meta charset=utf-8><div id=via>プロキシ経由</div>";
                    let _ = sock.write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    );
                });
            }
        });

        let b = Browser::spawn(&serve("<!doctype html><meta charset=utf-8><body>盤面"), "proxy test")
            .unwrap();
        // `spawn` has already waited for the board to be ready
        b.browse_through(port);
        // A loopback address, which is the case this exists for and the one a
        // browser gets wrong by default: `--proxy-server` alone leaves
        // loopback out, so this would be fetched from *this* machine -- where
        // nothing is listening on that port -- instead of from the far one.
        // A high port on purpose: a browser refuses a list of low ones
        // (tcpmux, discard, and their neighbours) before a proxy is even
        // consulted, which looks exactly like the bug this guards
        b.open_child(
            "p",
            "http://127.0.0.1:39997/page",
            (0, 0, 600, 400),
            BrowserProfile::new("through-test", true),
        )
        .unwrap();

        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut text = String::new();
        while std::time::Instant::now() < until {
            if let Ok(html) = b.html(Some("p"), 3_000)
                && html.contains("プロキシ経由") {
                    text = html;
                    break;
                }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let asked = asked.lock().unwrap().clone();
        assert!(
            asked.iter().any(|line| line.contains("127.0.0.1:39997")),
            "loopback bypasses the proxy (the whole point of this feature): {asked:?}"
        );
        assert!(!text.is_empty(), "the page the proxy returned is not shown");
        // Everything the browser does goes this way, not only what a page
        // asked for: this run also caught WebView2 reaching for a Microsoft
        // service of its own accord. A proxy is the whole browser environment
        println!("proxy saw: {asked:?}");
    }

    /// Serve a test page on 127.0.0.1.
    /// `file:///` crashes on wry's IPC, so use http, same as production
    #[cfg(windows)]
    fn serve(body: &'static str) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        format!("http://127.0.0.1:{port}/")
    }

    const PAGE: &str = r#"<!doctype html><meta charset=utf-8><body>
<div id=here>ここにいる</div>
<input id=q value="">
<textarea id=multi></textarea>
<button id=go onclick="document.getElementById('log').textContent='pushed'">押す</button>
<div id=log></div>
<table><tr><td>氏名</td><td id=name>山田</td></tr></table>
<div style="height:4000px"></div>
<div id=far>ずっと下</div>
<script>
  var fired = 0;
  document.getElementById('q').addEventListener('input', function(){ fired++; });
</script>"#;

    /// Find it, click it, fill it, read it.
    ///
    ///   cargo test browser_page_ops -- --ignored --nocapture
    // A real window, which only Windows can open yet
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn browser_page_ops() {
        let b = Browser::spawn(&serve(PAGE), "SHIKISHA-TERM ops probe").expect("the window does not open");
        let t = 20_000;

        // Distinguish "not in the DOM" from "in the DOM but off-screen".
        // Collapsing them into one failure makes it impossible to tell whether to suspect the selector or the wait
        assert_eq!(b.find(None, &Sel::Css("#here".into()), t).unwrap(), Found::Visible);
        assert_eq!(b.find(None, &Sel::Css("#far".into()), t).unwrap(), Found::OffScreen);
        assert_eq!(b.find(None, &Sel::Css("#nope".into()), t).unwrap(), Found::NotFound);

        // XPath: a lookup CSS can't express (the cell next to a label)
        let name = b
            .text(None, &Sel::Xpath("//td[text()='氏名']/following-sibling::td".into()), t)
            .unwrap();
        assert_eq!(name.as_deref(), Some("山田"), "XPath cannot get the neighboring cell");

        // Click it
        assert_eq!(b.click(None, &Sel::Css("#go".into()), t).unwrap().state, Found::Visible);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(
            b.text(None, &Sel::Css("#log".into()), t).unwrap().as_deref(),
            Some("pushed"),
            "the result of the press does not show on the page"
        );

        // Fill it. Not just writing the value — the `input` event must fire too
        // (frameworks like React won't update state otherwise)
        assert_eq!(
            b.fill(None, &Sel::Css("#q".into()), "ふつうの値", t).unwrap().state,
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some("ふつうの値")
        );
        let id = b.eval("return fired;").unwrap();
        assert_eq!(
            b.wait_result(id, std::time::Duration::from_millis(t)).unwrap(),
            "1",
            "the input event was not fired"
        );

        // This is the crux: the value must never become code.
        // Even AI output or text read straight off a page arrives as a plain value
        let nasty = "'; window.__pwned = 1; //\"</script><img src=x onerror=alert(1)>\\";
        assert_eq!(
            b.fill(None, &Sel::Css("#q".into()), nasty, t).unwrap().state,
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some(nasty),
            "the value did not go in exactly as given"
        );

        // A value containing newlines. A single-line `input` drops
        // newlines (per the HTML spec), so multi-line values must go
        // through a `textarea`. The value isn't corrupted — the container just can't hold it
        let multi = format!("1行目\n2行目\t{nasty}");
        assert_eq!(
            b.fill(None, &Sel::Css("#multi".into()), &multi, t).unwrap().state,
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#multi".into()), t).unwrap().as_deref(),
            Some(multi.as_str()),
            "a value with newlines or tabs is mangled"
        );
        let id = b.eval("return typeof window.__pwned;").unwrap();
        assert_eq!(
            b.wait_result(id, std::time::Duration::from_millis(t)).unwrap(),
            "\"undefined\"",
            "the value passed in was run as code"
        );

        // The full parsed HTML
        let html = b.html(None, t).unwrap();
        assert!(html.contains("ここにいる"), "the HTML was not retrieved");
        assert!(html.len() > 200, "the HTML is too short: {}", html.len());
        println!("HTML {} chars / all passed", html.chars().count());

        drop(b);
    }


    /// Pages can be placed inside the same window.
    ///
    ///   cargo test child_view -- --ignored --nocapture
    ///
    /// With a separate window, ownership, position tracking, and even
    /// exposure during Windows Terminal tab switching all became our own problem
    // A real window, which only Windows can open yet
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn a_page_can_sit_inside_the_window() {
        let b = Browser::spawn(&serve(PAGE), "child probe").expect("the window does not open");
        b.open_child("side", "https://example.com/", (400, 0, 400, 500), BrowserProfile::shared_default())
            .expect("cannot place it");
        std::thread::sleep(std::time::Duration::from_secs(3));
        // Its position can be changed
        b.child_bounds("side", (200, 0, 600, 500)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(600));
        // Hidden with width 0
        b.child_bounds("side", (0, 0, 0, 0)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        b.close_child("side").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        // The shell itself stays alive
        let id = b.eval("return 1+1;").unwrap();
        assert_eq!(
            b.wait_result(id, std::time::Duration::from_secs(10)).unwrap(),
            "2",
            "placing a child stopped the outer window working"
        );
        println!("opening and closing child pages: passed");
        drop(b);
    }

    /// URLs we can't open are stopped at the door.
    ///
    /// wry turns a page's URL into an `http::Uri` and unwraps it on IPC,
    /// so opening `file:///` or `data:` takes down the whole process the
    /// moment the initialization script sends its first message. Confirmed by testing
    #[test]
    fn only_http_pages_are_opened() {
        assert!(is_openable("https://example.com/a"));
        assert!(is_openable("http://127.0.0.1:8080/"));

        assert!(!is_openable("file:///C:/tmp/a.html"), "file: is rejected");
        assert!(!is_openable("data:text/html,<b>x"), "data: is rejected");
        assert!(!is_openable("about:blank"));
        assert!(!is_openable("https://"), "there is no host");
        assert!(!is_openable(""));
        assert!(!is_openable("https://example.com/a\nhttps://evil"), "a newline slipped in");
    }

    /// The window opens, JS runs, results come back, and closing it
    /// doesn't kill the app.
    ///
    ///   cargo test browser_round_trip -- --ignored --nocapture
    ///
    /// That last point is the crux. tao's `run` calls `process::exit`
    /// internally, so a naive implementation would take down the whole TUI just by closing the window
    // A real window, which only Windows can open yet
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn browser_round_trip() {
        // Test through the same path as production. `file:///` crashes on wry's IPC, so it can't be used
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = "<title>t</title><body><div id=aaa>hello</div>";
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/");

        let b = Browser::spawn(&url, "SHIKISHA-TERM browser probe").expect("the window does not open");

        let id = b.eval("return 40 + 2;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).expect("no result");
        println!("eval(40+2) = {v}");
        assert_eq!(v, "42");

        let id = b.eval("return document.querySelector('#aaa').textContent;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).expect("no result");
        println!("querySelector = {v}");
        assert_eq!(v, "\"hello\"");

        let id = b.eval("return document.documentElement.outerHTML.length;").unwrap();
        println!("HTML length = {}", b.wait_result(id, Duration::from_secs(20)).unwrap());

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
        println!("got this far after closing (the process is alive)");
    }

    /// The CDP lane end-to-end: digest lists the operable elements (AX lane
    /// and JS-clickable lane both), `{ref=N}` clicks with a genuine mouse
    /// event that fires the page's own onclick, and ref-fill types multibyte
    /// text as real key events.
    ///
    ///   cargo test digest_round_trip -- --ignored --nocapture
    // A real window, which only Windows can open yet
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn digest_round_trip() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = r#"<title>t</title><body>
                  <button id="b" onclick="document.getElementById('log').textContent='clicked'">押す</button>
                  <a href="https://example.com/x">リンク</a>
                  <input id="i" placeholder="名前">
                  <div id="d" style="cursor:pointer" onclick="void 0">丸いやつ</div>
                  <a href="https://example.com/dup" onclick="return false">重複</a>
                  <a href="https://example.com/dup" onclick="return false">重複</a>
                  <div id="log"></div>"#;
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let b = Browser::spawn(&url, "SHIKISHA-TERM digest probe").expect("the window does not open");

        let text = b.digest(None, 20_000).expect("the digest could not be taken");
        println!("{text}");
        assert!(text.contains("button \"押す\""), "the button comes in from the AX lane:\n{text}");
        assert!(text.contains("リンク") && text.contains("https://example.com/x"), "{text}");
        assert!(text.contains("名前"), "the input box's name (from its placeholder) comes in:\n{text}");
        assert!(text.contains("div*") && text.contains("丸いやつ"), "JS clickables are filled in:\n{text}");

        // A line reads `[N] role "name" …` — pull N for the line matching `needle`
        let ref_of = |needle: &str| -> u32 {
            text.lines()
                .find(|l| l.contains(needle))
                .and_then(|l| l.strip_prefix('['))
                .and_then(|l| l.split(']').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("no ref found: {needle}"))
        };

        // A genuine click fires the page's own onclick, and the echo names
        // what was clicked (a wrong ref number would answer for itself)
        let rb = ref_of("押す");
        let rep = b.click(None, &Sel::Ref(rb), 10_000).unwrap();
        assert_eq!(rep.state, Found::Visible);
        let echo = rep.echo.expect("a ref click returns an echo");
        assert!(
            echo.contains("button") && echo.contains("押す"),
            "it names what it pressed: {echo}"
        );
        // The durable anchor for the replay journal: the button has a
        // human-made id, so the anchor is its css form
        assert_eq!(
            rep.anchor,
            Some(("css".to_string(), "#b".to_string())),
            "the anchor for an element with an id is #id"
        );
        std::thread::sleep(Duration::from_millis(400));
        let id = b.eval("return document.getElementById('log').textContent;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(10)).unwrap();
        assert_eq!(v, "\"clicked\"", "a real mouse event fires onclick");

        // Ref-fill types multibyte as char key events; ref-text reads it back
        let ri = ref_of("名前");
        assert_eq!(b.fill(None, &Sel::Ref(ri), "俳句テスト", 10_000).unwrap().state, Found::Visible);
        std::thread::sleep(Duration::from_millis(400));
        let id = b.eval("return document.getElementById('i').value;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(10)).unwrap();
        assert_eq!(v, "\"俳句テスト\"", "char key events put in multibyte text");
        assert_eq!(
            b.text(None, &Sel::Ref(ri), 10_000).unwrap().as_deref(),
            Some("俳句テスト")
        );

        // A stale/unknown ref refuses with guidance instead of clicking air
        let err = b.click(None, &Sel::Ref(999), 10_000).unwrap_err().to_string();
        println!("999 -> {err}");
        assert!(err.contains("999"), "it says which ref is bad: {err}");

        // A duplicated element (same text, same href — the Google-btnK shape)
        // still gets an anchor: the candidate pinned to its own position
        let dup2 = text
            .lines()
            .filter(|l| l.contains("重複"))
            .nth(1)
            .and_then(|l| l.strip_prefix('['))
            .and_then(|l| l.split(']').next())
            .and_then(|n| n.parse::<u32>().ok())
            .expect("the ref of the second duplicate link");
        let rep = b.click(None, &Sel::Ref(dup2), 10_000).unwrap();
        let (kind, v) = rep.anchor.expect("an anchor comes out even for a duplicate");
        println!("dup anchor = {kind} {v}");
        assert_eq!(kind, "xpath");
        assert!(
            v.starts_with('(') && v.ends_with(")[2]"),
            "the second element is pinned by position: {v}"
        );

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
    }

    /// The three verbs a page needs beyond click and type, against a real
    /// page: choosing in a native list, scrolling a box that holds more than
    /// it shows, and waiting for the page to stop reacting instead of
    /// sleeping. Also that the list's own choices reach the digest, so
    /// choosing costs no extra look.
    ///
    ///   cargo test choose_scroll_settle -- --ignored --nocapture
    // A real window, which only Windows can open yet
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn choose_scroll_and_settle_on_a_real_page() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = r#"<title>t</title><body>
                  <label for="c">クラス</label>
                  <select id="c" onchange="document.getElementById('log').textContent='chose ' + this.value">
                    <option value="e">エコノミー</option>
                    <option value="b">ビジネス</option>
                    <option value="f" disabled>ファースト</option>
                  </select>
                  <div id="panel" style="height:120px;overflow:auto;border:1px solid #000">
                    <div style="height:900px">なかみ</div>
                  </div>
                  <button id="slow" onclick="setTimeout(() => { document.getElementById('late').textContent='arrived'; }, 250)">おそい</button>
                  <div id="late"></div>
                  <div id="log"></div>
                  <div style="height:3000px">たけ</div>"#;
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let b = Browser::spawn(&url, "SHIKISHA-TERM page verbs probe").expect("the window does not open");

        let text = b.digest(None, 20_000).expect("the digest could not be taken");
        println!("{text}");
        // The list says what it offers, so choosing takes no extra look
        assert!(text.contains("choices=2"), "a disabled option is not on offer:\n{text}");
        assert!(text.contains("エコノミー"), "{text}");
        // The panel holds more than it shows, and has a number to scroll by
        assert!(text.contains("scrolls="), "the scrolling box is listed:\n{text}");
        // And the page itself says how far down it goes
        assert!(text.contains("screens tall"), "{text}");

        let ref_of = |needle: &str| -> u32 {
            text.lines()
                .find(|l| l.contains(needle))
                .and_then(|l| l.trim_start().strip_prefix('['))
                .and_then(|l| l.split(']').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("no ref found: {needle}\n{text}"))
        };

        // Choosing by the text a person reads, not by the value attribute
        let rc = ref_of("クラス");
        let rep = b.select(None, &Sel::Ref(rc), "ビジネス", 10_000).unwrap();
        assert_eq!(rep.state, Found::Visible);
        assert_eq!(rep.echo.as_deref(), Some("ビジネス"), "it echoes what it chose");
        std::thread::sleep(Duration::from_millis(300));
        let id = b.eval("return document.getElementById('log').textContent;").unwrap();
        assert_eq!(
            b.wait_result(id, Duration::from_secs(10)).unwrap(),
            "\"chose b\"",
            "the page's own change handler ran"
        );

        // A choice the list does not hold is refused, and the refusal says
        // what it does hold -- one more move instead of a loop of guesses
        let err = b.select(None, &Sel::Ref(rc), "ファースト", 10_000).unwrap_err().to_string();
        println!("disabled -> {err}");
        assert!(err.contains("エコノミー"), "it says what is on offer: {err}");

        // Scrolling the box moves the box, not the window
        let rp = ref_of("scrollbox");
        let (state, how_far) = b
            .scroll(None, Some(&Sel::Ref(rp)), &serde_json::json!(1), 10_000)
            .unwrap();
        println!("panel -> {state:?} {how_far}");
        assert_eq!(state, Found::Visible);
        let id = b.eval("return document.getElementById('panel').scrollTop;").unwrap();
        let moved: f64 = b
            .wait_result(id, Duration::from_secs(10))
            .unwrap()
            .parse()
            .unwrap_or(0.0);
        assert!(moved > 50.0, "the panel scrolled: {moved}");
        let id = b.eval("return window.scrollY;").unwrap();
        assert_eq!(b.wait_result(id, Duration::from_secs(10)).unwrap(), "0", "the window did not");

        // ...and the window scrolls when nothing is named
        let (_, how_far) = b.scroll(None, None, &serde_json::json!(1), 10_000).unwrap();
        println!("window -> {how_far}");
        let id = b.eval("return window.scrollY;").unwrap();
        let y: f64 = b.wait_result(id, Duration::from_secs(10)).unwrap().parse().unwrap_or(0.0);
        assert!(y > 100.0, "the page moved: {y}");
        let (_, at_end) = b.scroll(None, None, &serde_json::json!("bottom"), 10_000).unwrap();
        println!("bottom -> {at_end}");
        let (_, no_more) = b.scroll(None, None, &serde_json::json!(1), 10_000).unwrap();
        assert!(no_more.contains('0'), "at the end it says it did not move: {no_more}");

        // Settling returns as soon as the page is still, not when a clock says
        // so: the quiet page is the fast case, and the one still working is
        // waited out
        let quiet = std::time::Instant::now();
        let (ms, why) = b.settle(None, "quiet", 2_000, 300, 10_000).unwrap();
        println!("quiet -> {ms}ms {why} (wall {}ms)", quiet.elapsed().as_millis());
        assert_eq!(why, "nothing_happened", "a page nothing was done to says exactly that");
        assert!(
            quiet.elapsed() < Duration::from_millis(1_000),
            "and it costs the short grace, not the whole budget"
        );

        b.click(None, &Sel::Ref(ref_of("おそい")), 10_000).unwrap();
        let (ms, why) = b.settle(None, "quiet", 3_000, 300, 12_000).unwrap();
        println!("after the slow one -> {ms}ms {why}");
        assert_eq!(why, "still", "it waited for the reaction, then for it to finish");
        assert!(ms >= 250, "which cannot have happened in less than the page took: {ms}ms");
        let id = b.eval("return document.getElementById('late').textContent;").unwrap();
        assert_eq!(
            b.wait_result(id, Duration::from_secs(10)).unwrap(),
            "\"arrived\"",
            "settling waited for what the click set off, without being told how long"
        );

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
    }

    /// A hidden page (bounds 0×0, as during an operate rally showing the AI
    /// tab) has no compositor, so genuine mouse acks never come — the click
    /// must fall back to the synthetic path and still land. Key events and
    /// digest must work hidden as-is.
    ///
    ///   cargo test hidden_page_ref_click -- --ignored --nocapture
    // A real window, which only Windows can open yet
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn hidden_page_ref_click_falls_back() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                // mousedown fires only for genuine mouse input (a synthetic
                // el.click() skips it); beforeinput fires only for genuine
                // typing (the native-setter fallback dispatches `input` only).
                // Counting them tells WHICH path the operation really took
                let body = r#"<title>t</title><body>
                  <button id="b" onclick="document.getElementById('log').textContent='clicked'">押す</button>
                  <input id="i" placeholder="名前">
                  <div id="log"></div>
                  <script>
                    window.__ev = { md: 0, bi: 0 };
                    document.getElementById('b').addEventListener('mousedown', () => __ev.md++);
                    document.getElementById('i').addEventListener('beforeinput', () => __ev.bi++);
                  </script>"#;
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let b = Browser::spawn(&url, "SHIKISHA-TERM hidden probe").expect("the window does not open");
        // A page placed at zero size = hidden (how the app hides pages)
        b.open_child("c", &url, (0, 0, 0, 0), BrowserProfile::new("", true)).unwrap();
        std::thread::sleep(Duration::from_millis(2500));

        let text = b.digest(Some("c"), 20_000).expect("the digest of a hidden page could not be taken");
        println!("{text}");
        let ref_of = |needle: &str| -> u32 {
            text.lines()
                .find(|l| l.contains(needle))
                .and_then(|l| l.strip_prefix('['))
                .and_then(|l| l.split(']').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("no ref found: {needle}"))
        };

        let t0 = std::time::Instant::now();
        assert_eq!(
            b.click(Some("c"), &Sel::Ref(ref_of("押す")), 10_000).unwrap().state,
            Found::Visible,
            "a click works even when hidden"
        );
        println!("click took {}ms", t0.elapsed().as_millis());
        std::thread::sleep(Duration::from_millis(400));
        let id = b.eval_in(Some("c"), "return document.getElementById('log').textContent;").unwrap();
        assert_eq!(
            b.wait_result(id, Duration::from_secs(10)).unwrap(),
            "\"clicked\"",
            "onclick fires"
        );

        assert_eq!(
            b.fill(Some("c"), &Sel::Ref(ref_of("名前")), "俳句", 10_000).unwrap().state,
            Found::Visible
        );
        std::thread::sleep(Duration::from_millis(400));
        let id = b.eval_in(Some("c"), "return document.getElementById('i').value;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(10)).unwrap();
        assert_eq!(v, "\"俳句\"", "the value always goes in even when hidden");

        // The wake must have made GENUINE input land — not the fallbacks
        let id = b.eval_in(Some("c"), "return JSON.stringify(window.__ev);").unwrap();
        let ev = b.wait_result(id, Duration::from_secs(10)).unwrap();
        println!("genuine-input evidence = {ev}");
        assert!(
            ev.contains("\\\"md\\\":1") || ev.contains("md\":1"),
            "a real mouse (mousedown) should reach a hidden page: {ev}"
        );
        assert!(
            !ev.contains("bi\":0"),
            "real keystrokes (beforeinput) should reach a hidden page: {ev}"
        );

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
    }

    /// Auto-wait (the actionability engine) end-to-end:
    /// a click waits for an element the page hasn't built yet, waits out an
    /// animation, and — the replay.lua case — ops fired back-to-back with no
    /// pauses survive a navigation in between, because the outer retry
    /// re-enters the new document.
    ///
    ///   cargo test auto_wait_round_trip -- --ignored --nocapture
    // A real window, which only Windows can open yet
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn auto_wait_round_trip() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = if req.url().starts_with("/two") {
                    // The input arrives 600ms late — a client-rendered page
                    r#"<title>two</title><body>
                      <div id="slot"></div>
                      <button id="ok" onclick="document.getElementById('out').textContent =
                        document.getElementById('name').value">OK</button>
                      <div id="out"></div>
                      <script>
                        setTimeout(() => {
                          document.getElementById('slot').innerHTML =
                            '<input id="name" placeholder="なまえ">';
                        }, 600);
                      </script>"#
                } else {
                    // #late appears after 700ms; #move slides for ~500ms first
                    r#"<title>one</title><body>
                      <button id="move" style="transition:margin-left .5s" onclick="this.dataset.hit='1'">動く</button>
                      <div id="slot"></div>
                      <a id="go" href="/two">つぎへ</a>
                      <script>
                        requestAnimationFrame(() => { document.getElementById('move').style.marginLeft = '120px'; });
                        setTimeout(() => {
                          document.getElementById('slot').innerHTML =
                            '<button id="late" onclick="this.dataset.hit=1">遅れて出る</button>';
                        }, 700);
                      </script>"#
                };
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let b = Browser::spawn(&url, "SHIKISHA-TERM auto-wait probe").expect("the window does not open");

        // Back-to-back, replay-style: no pauses between any of these
        let t0 = std::time::Instant::now();
        let r = b.click(None, &Sel::Css("#late".into()), 10_000).unwrap();
        let waited = t0.elapsed().as_millis();
        assert_eq!(r.state, Found::Visible, "it can wait for an element that is not there yet and click it");
        println!("late click waited {waited}ms");
        assert!(waited >= 500, "it should have waited for an element that appears after 700ms: {waited}ms");

        assert_eq!(
            b.click(None, &Sel::Css("#move".into()), 10_000).unwrap().state,
            Found::Visible,
            "an element being animated is clicked once it holds still"
        );

        // Navigate, then immediately act on the next page's late element
        assert_eq!(b.click(None, &Sel::Css("#go".into()), 10_000).unwrap().state, Found::Visible);
        assert_eq!(
            b.fill(None, &Sel::Css("#name".into()), "俳句", 10_000).unwrap().state,
            Found::Visible,
            "an input box made late, right after navigating, can be written to by rapid moves with no waits"
        );
        assert_eq!(b.click(None, &Sel::Css("#ok".into()), 10_000).unwrap().state, Found::Visible);
        std::thread::sleep(Duration::from_millis(300));
        let id = b.eval("return document.getElementById('out').textContent + ' @ ' + location.pathname;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(10)).unwrap();
        println!("final: {v}");
        assert_eq!(v, "\"俳句 @ /two\"", "the rapid replay gets all the way through");

        // A truly absent element still says not_found — after the full wait
        let t0 = std::time::Instant::now();
        let r = b.click(None, &Sel::Css("#never".into()), 2_500).unwrap();
        assert_eq!(r.state, Found::NotFound);
        println!("absent verdict after {}ms", t0.elapsed().as_millis());

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
    }

    /// The full task an operator AI is asked to do, walked with the primitives
    /// alone against the live Google: digest → fill the search box by ref →
    /// Enter → digest the results → click the Wikipedia link by ref → land on
    /// ja.wikipedia.org. If this passes, every mechanical link in the chain
    /// (digest quality included) is sound and only the AI's judgment remains.
    ///
    ///   cargo test haiku_task_probe -- --ignored --nocapture
    // A real window, which only Windows can open yet
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn haiku_task_probe() {
        let b = Browser::spawn("https://www.google.com/", "SHIKISHA-TERM task probe")
            .expect("the window does not open");
        std::thread::sleep(Duration::from_millis(1500));

        let ref_of = |text: &str, needle: &str| -> Option<u32> {
            text.lines()
                .find(|l| l.contains(needle))
                .and_then(|l| l.strip_prefix('['))
                .and_then(|l| l.split(']').next())
                .and_then(|n| n.parse().ok())
        };

        // 1. Find and fill the search box
        let d1 = b.digest(None, 20_000).expect("digest 1");
        let q = ref_of(&d1, "combobox").or_else(|| ref_of(&d1, "textbox")).expect("the search box");
        let rep = b.fill(None, &Sel::Ref(q), "俳句", 10_000).expect("fill");
        println!("fill -> {:?} {:?}", rep.state, rep.echo);
        assert_eq!(rep.state, Found::Visible);

        // 2. Submit with Enter (the key goes to the focused element = the box)
        b.inject(None, Input::Key { named: "enter".into(), ctrl: false, alt: false }).unwrap();
        let url = b.wait_ready(Duration::from_secs(20)).expect("search results do not arrive");
        println!("results: {url}");
        assert!(url.contains("/search"), "it moves to the search results page: {url}");
        std::thread::sleep(Duration::from_millis(1200));

        // 3. Digest the results and click the Wikipedia link by number
        let d2 = b.digest(None, 20_000).expect("digest 2");
        println!("---- results digest ----\n{d2}\n----");
        // Read like a careful operator: the real result link lives under the
        // results-section heading; the same URL quoted inside an AI summary
        // (§AI…) opens a citation panel instead of navigating
        let wiki_links: Vec<&str> = d2
            .lines()
            .filter(|l| {
                l.starts_with('[') && l.contains("link") && l.contains("ja.wikipedia.org/wiki")
            })
            .collect();
        let wiki = wiki_links
            .iter()
            .find(|l| l.contains("§ウェブ検索結果") || l.contains("§検索結果"))
            .or_else(|| wiki_links.iter().find(|l| !l.contains("§AI")))
            .copied()
            .expect("the Wikipedia link in the results section comes into the digest");
        println!("wiki line: {wiki}");
        let r: u32 = wiki
            .strip_prefix('[')
            .and_then(|l| l.split(']').next())
            .and_then(|n| n.parse().ok())
            .unwrap();
        let rep = b.click(None, &Sel::Ref(r), 10_000).expect("click");
        println!("click -> {:?} {:?} anchor={:?}", rep.state, rep.echo, rep.anchor);
        let echo = rep.echo.clone().unwrap_or_default();
        assert!(
            echo.contains("俳句") || echo.to_lowercase().contains("wikipedia"),
            "the echo names the Wikipedia link: {echo}"
        );
        let url = b.wait_ready(Duration::from_secs(20)).expect("it does not move to Wikipedia");
        println!("landed: {url}");
        assert!(url.contains("ja.wikipedia.org/wiki"), "landed on Wikipedia: {url}");

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
    }

    /// Field probe against the real Google homepage: where exactly does a
    /// ref-click stall? Prints per-step timings instead of asserting, so the
    /// failing CDP call names itself.
    ///
    ///   cargo test google_probe -- --ignored --nocapture
    // A real window, which only Windows can open yet
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn google_probe() {
        let b = Browser::spawn("https://www.google.com/", "SHIKISHA-TERM google probe")
            .expect("the window does not open");
        std::thread::sleep(Duration::from_millis(1500));

        let t0 = std::time::Instant::now();
        let text = b.digest(None, 20_000).expect("the digest could not be taken");
        println!("digest: {}ms, {} lines\n{text}", t0.elapsed().as_millis(), text.lines().count());

        let ref_of = |needle: &str| -> Option<u32> {
            text.lines()
                .find(|l| l.contains(needle))
                .and_then(|l| l.strip_prefix('['))
                .and_then(|l| l.split(']').next())
                .and_then(|n| n.parse().ok())
        };
        let box_ref = ref_of("combobox")
            .or_else(|| ref_of("textbox"))
            .expect("the search box was not found");
        println!("search box = ref {box_ref}");

        let t0 = std::time::Instant::now();
        let r = b.fill(None, &Sel::Ref(box_ref), "俳句", 10_000);
        println!("fill: {:?} in {}ms", r, t0.elapsed().as_millis());

        std::thread::sleep(Duration::from_millis(800));
        let text2 = b.digest(None, 20_000).expect("the second digest could not be taken");
        let btn = text2
            .lines()
            .find(|l| l.contains("button") && l.contains("検索") && !l.contains("画像"))
            .map(str::to_string)
            .expect("the search button was not found");
        println!("button line: {btn}");
        let btn_ref: u32 = btn
            .strip_prefix('[')
            .and_then(|l| l.split(']').next())
            .and_then(|n| n.parse().ok())
            .unwrap();

        // What would the replay journal record for this button?
        let oid = pageops::ref_object(&b, None, btn_ref, 8_000).unwrap();
        println!("button anchor = {:?}", pageops::element_anchor(&b, None, &oid, 8_000));

        // The same steps click_ref takes, timed one by one
        let backend = pageops::ref_backend(&b, None, btn_ref).unwrap();
        for (what, method, params) in [
            ("scroll", "DOM.scrollIntoViewIfNeeded", serde_json::json!({"backendNodeId": backend})),
            ("quads", "DOM.getContentQuads", serde_json::json!({"backendNodeId": backend})),
        ] {
            let t0 = std::time::Instant::now();
            let r = b.cdp(None, method, params, 8_000);
            println!("{what}: {}ms ok={}", t0.elapsed().as_millis(), r.is_ok());
        }
        let q = b
            .cdp(None, "DOM.getContentQuads", serde_json::json!({"backendNodeId": backend}), 8_000)
            .unwrap();
        let (x, y) = shikisha_core::cdp::quad_center(&q).unwrap();
        for (kind, buttons, clicks) in
            [("mouseMoved", 0, 0), ("mousePressed", 1, 1), ("mouseReleased", 0, 1)]
        {
            let t0 = std::time::Instant::now();
            let r = b.cdp(
                None,
                "Input.dispatchMouseEvent",
                serde_json::json!({"type": kind, "x": x, "y": y,
                                   "button": "left", "buttons": buttons, "clickCount": clicks}),
                8_000,
            );
            println!(
                "{kind}: {}ms ok={} {:?}",
                t0.elapsed().as_millis(),
                r.is_ok(),
                r.err().map(|e| e.to_string())
            );
        }
        std::thread::sleep(Duration::from_millis(2500));
        let id = b.eval("return location.href;").unwrap();
        println!("after click url = {:?}", b.wait_result(id, Duration::from_secs(10)));
        drop(b);
    }
}

/// The window, as something that speaks the DevTools protocol.
///
/// Two moves, and everything a page can be asked to do is built out of them
/// up in `pageops` -- the same code the server's browser answers.
impl Speaks for Browser {
    fn cdp(
        &self,
        to: Option<&str>,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value> {
        Browser::cdp_call(self, to, method, params, timeout_ms)
    }

    fn eval(&self, to: Option<&str>, js: &str, timeout_ms: u64) -> Result<String> {
        let id = self.eval_in(to, js)?;
        self.wait_result(id, std::time::Duration::from_millis(timeout_ms))
    }

    fn refs(&self) -> &std::sync::Mutex<std::collections::HashMap<Option<String>, Vec<i64>>> {
        &self.digests
    }

    /// A page the window has hidden (bounds 0x0) stops compositing, and mouse
    /// input is the one kind that waits for a frame -- its ack never arrives.
    /// A tiny throwaway screencast forces frames back on for the duration.
    /// Fire-and-forget: the command channel preserves order, and the input
    /// call that follows synchronizes on its own completion
    fn wake(&self, to: Option<&str>, on: bool) {
        let _ = self.send(Cmd::Wake {
            to: to.map(str::to_string),
            on,
        });
    }
}

/// The window, seen as "something that shows pages".
///
/// Placing, moving and closing a page are the window's own; everything done
/// *to* a page is `pageops`, which is where the server's browser gets the
/// identical behaviour from the identical code.
impl BrowserHost for Browser {
    fn go(&self, to: Option<&str>, go: Go) -> Result<()> { Browser::go(self, to, go) }
    fn focus(&self, to: Option<&str>) -> Result<()> { Browser::focus(self, to) }
    fn ask_where(&self, to: Option<&str>) -> Result<()> { Browser::ask_where(self, to) }
    /// Searched on the window's own thread, which is the only one that may
    /// touch the page: where it stands is reported when it is known
    fn seek(&self, to: Option<&str>, text: &str, step: shikisha_shared::Seek) -> Result<Option<(u32, u32)>> {
        Browser::seek(self, to, text, step).map(|()| None)
    }
    fn cancel_download(&self, id: &str) -> Result<()> { Browser::cancel_download(self, id) }
    fn find_keys(&self, to: Option<&str>, on: bool) -> Result<()> { Browser::find_keys(self, to, on) }
    fn basic_auth(&self, to: Option<&str>, user: &str, pass: &str) -> Result<()> {
        Browser::basic_auth(self, to, user, pass)
    }
    fn eval_in(&self, to: Option<&str>, js: &str) -> Result<u64> { Browser::eval_in(self, to, js) }
    fn inject(&self, to: Option<&str>, input: Input) -> Result<()> { Browser::inject(self, to, input) }
    fn screencast(&self, to: Option<&str>, on: bool) -> Result<()> { Browser::screencast(self, to, on) }
    /// The page being cast is the only one anybody can ask to hear, so this
    /// is the one the window wrote down when the cast started
    fn sound_from(&self, _to: Option<&str>) -> Result<u32> {
        match self.sound_pid.load(std::sync::atomic::Ordering::SeqCst) {
            0 => anyhow::bail!("nothing is being watched, so nothing is playing"),
            pid => Ok(pid),
        }
    }
    fn record(&self, to: Option<&str>, on: bool) -> Result<()> { Browser::record(self, to, on) }
    fn console(&self, to: Option<&str>, on: bool) -> Result<()> { Browser::console(self, to, on) }
    fn devtools_url(&self, to: Option<&str>) -> Result<String> { Browser::devtools_url(self, to) }

    fn find(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Found> {
        pageops::find(self, to, sel, timeout_ms)
    }
    fn click(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<OpReport> {
        pageops::click(self, to, sel, timeout_ms)
    }
    fn fill(&self, to: Option<&str>, sel: &Sel, value: &str, timeout_ms: u64) -> Result<OpReport> {
        pageops::fill(self, to, sel, value, timeout_ms)
    }
    fn text(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Option<String>> {
        pageops::text(self, to, sel, timeout_ms)
    }
    fn select(&self, to: Option<&str>, sel: &Sel, value: &str, timeout_ms: u64) -> Result<OpReport> {
        pageops::select(self, to, sel, value, timeout_ms)
    }
    fn scroll(
        &self,
        to: Option<&str>,
        sel: Option<&Sel>,
        amount: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<(Found, String)> {
        pageops::scroll(self, to, sel, amount, timeout_ms)
    }
    fn settle(&self, to: Option<&str>, expect: &str, cap_ms: u64, first_ms: u64, timeout_ms: u64) -> Result<(u64, String)> {
        pageops::settle(self, to, expect, cap_ms, first_ms, timeout_ms)
    }
    fn href(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> {
        pageops::href(self, to, timeout_ms)
    }
    fn html(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> {
        pageops::html(self, to, timeout_ms)
    }
    fn source(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> {
        pageops::source(self, to, timeout_ms)
    }
    fn digest(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> {
        pageops::digest(self, to, timeout_ms)
    }
    fn elements(&self, to: Option<&str>, timeout_ms: u64) -> Result<serde_json::Value> {
        pageops::elements(self, to, timeout_ms)
    }
    fn snapshot(&self, to: Option<&str>, timeout_ms: u64) -> Result<Vec<u8>> {
        pageops::snapshot(self, to, timeout_ms)
    }
    fn pdf(&self, to: Option<&str>, timeout_ms: u64) -> Result<(Vec<u8>, String)> {
        pageops::pdf(self, to, timeout_ms)
    }

    fn cookies_out(&self, to: Option<&str>, timeout_ms: u64) -> Result<serde_json::Value> {
        pageops::cookies_out(self, to, timeout_ms)
    }
    fn cookies_in(&self, to: Option<&str>, cookies: &serde_json::Value, timeout_ms: u64) -> Result<()> {
        pageops::cookies_in(self, to, cookies, timeout_ms)
    }
    fn storage_out(&self, to: Option<&str>, timeout_ms: u64) -> Result<serde_json::Value> {
        pageops::storage_out(self, to, timeout_ms)
    }
    fn storage_in(&self, to: Option<&str>, items: &serde_json::Value, timeout_ms: u64) -> Result<()> {
        pageops::storage_in(self, to, items, timeout_ms)
    }
    fn fetch(&self, to: Option<&str>, url: &str, opts: &serde_json::Value, timeout_ms: u64) -> Result<String> {
        pageops::fetch(self, to, url, opts, timeout_ms)
    }

    fn open_child(&self, name: &str, url: &str, rect: (i32, i32, i32, i32), profile: BrowserProfile) -> Result<()> {
        Browser::open_child(self, name, url, rect, profile)
    }
    fn child_bounds(&self, name: &str, rect: (i32, i32, i32, i32)) -> Result<()> {
        Browser::child_bounds(self, name, rect)
    }
    fn raise_child(&self, name: &str) -> Result<()> { Browser::raise_child(self, name) }
    fn close_child(&self, name: &str) -> Result<()> { Browser::close_child(self, name) }
    fn trust(&self, url: &str) -> Result<()> { Browser::trust(self, url) }
    fn record_all_off(&self) { Browser::record_all_off(self) }
}

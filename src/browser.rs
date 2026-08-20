//! Put one browser under command.
//!
//! Windows 11 ships with a Chromium engine (WebView2) built in,
//! and Microsoft keeps it updated. So we don't bundle our own. We borrow it.
//! That keeps the "no-install single exe" promise intact.
//!
//! The window runs on its own thread. The TUI's render loop and the
//! message loop both want to run on their own terms, so they must not mix.
//!
//! Use `run_return`, not `run`. `run` is `-> !` and calls
//! `process::exit` internally. Just closing the browser window would
//! take down the whole app.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Result, anyhow};

/// Always injected into every document first.
///
/// It runs on every navigation, so no matter how many times a login
/// redirects, there's always a way to show the bar. But whether it
/// *should* show right now is something the Rust side remembers and
/// re-issues on every navigation (the JS world disappears on navigation).
const INIT_JS: &str = r#"
(function () {
  if (window.__shikisha) return;
  const send = (o) => window.ipc.postMessage(JSON.stringify(o));

  // Calls out to the human. Enclosed in a shadow root so it doesn't clash with the page's CSS
  window.__shikisha_ask = function (text, label) {
    let host = document.getElementById("__shikisha_bar");
    if (!host) {
      host = document.createElement("div");
      host.id = "__shikisha_bar";
      host.style.cssText =
        "position:fixed;left:0;right:0;bottom:0;z-index:2147483647";
      (document.body || document.documentElement).appendChild(host);
      host.attachShadow({ mode: "open" });
    }
    host.shadowRoot.innerHTML =
      '<div style="font:14px/1.5 system-ui,sans-serif;background:#0a0c0e;' +
      'color:#e8eef4;border-top:3px solid #00aaff;padding:12px 16px;' +
      'display:flex;align-items:center;gap:16px">' +
      '<span style="flex:1"></span>' +
      '<button style="font:600 14px system-ui;background:#00aaff;color:#04121c;' +
      'border:0;border-radius:6px;padding:8px 18px;cursor:pointer"></button></div>';
    host.shadowRoot.querySelector("span").textContent = text;
    const b = host.shadowRoot.querySelector("button");
    b.textContent = label;
    // Give immediate feedback that the click registered. Without it,
    // there's no way to tell whether the click landed, didn't land,
    // or just triggered work that produces nothing visible.
    // Also guards against double-clicks (the receiving side expects exactly one)
    b.onclick = () => {
      if (b.disabled) return;
      b.disabled = true;
      b.style.opacity = ".45";
      b.style.cursor = "default";
      send({ kind: "button" });
    };
  };

  window.__shikisha_unask = function () {
    const host = document.getElementById("__shikisha_bar");
    if (host) host.remove();
  };

  // A selector is either {css:"..."} or {xpath:"..."}.
  // XPath lets us express lookups CSS can't, like "the cell just to the
  // right of the cell labeled 'Name'", so we support both
  window.__shikisha_q = function (sel) {
    if (sel && sel.xpath) {
      return document.evaluate(sel.xpath, document, null, 9, null).singleNodeValue;
    }
    return document.querySelector(sel.css);
  };

  // Distinguish "not in the DOM" from "in the DOM but off-screen".
  // Collapsing them into one failure makes it impossible to tell whether
  // to suspect the selector or the wait
  window.__shikisha_state = function (sel) {
    const el = window.__shikisha_q(sel);
    if (!el) return "not_found";
    const r = el.getBoundingClientRect();
    const on =
      r.width > 0 && r.height > 0 &&
      r.bottom > 0 && r.right > 0 &&
      r.top < innerHeight && r.left < innerWidth;
    return on ? "visible" : "off_screen";
  };

  window.__shikisha_text = function (sel) {
    const el = window.__shikisha_q(sel);
    return el ? (el.value !== undefined ? el.value : el.innerText) : null;
  };

  window.__shikisha_click = function (sel) {
    const el = window.__shikisha_q(sel);
    if (!el) return "not_found";
    el.scrollIntoView({ block: "center" });
    el.click();
    // If we touched it, it was reachable. Keep the same vocabulary as find
    return "visible";
  };

  window.__shikisha_fill = function (sel, value) {
    const el = window.__shikisha_q(sel);
    if (!el) return "not_found";
    el.scrollIntoView({ block: "center" });
    el.focus();
    if (el.isContentEditable) {
      el.textContent = value;
    } else {
      // Frameworks like React don't notice a direct write to value.
      // Going through the original setter before dispatching input
      // also updates the framework's own state
      const proto =
        el instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : HTMLInputElement.prototype;
      const setter = Object.getOwnPropertyDescriptor(proto, "value");
      if (setter && setter.set) setter.set.call(el, value);
      else el.value = value;
    }
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return "visible";
  };

  window.__shikisha_html = function () {
    return document.documentElement.outerHTML;
  };

  // Make the request from inside the page so we can read the status/body/
  // headers (the WebView doesn't expose raw HTTP directly, so we have the
  // page itself make the call and hand back the result). credentials:"include"
  // so logged-in cookies are used. Failures are returned as a value, not thrown
  window.__shikisha_fetch = async function (url, opts) {
    const o = opts || {};
    try {
      const r = await fetch(url, {
        method: o.method || "GET",
        headers: o.headers || undefined,
        body: o.body,
        credentials: "include",
        redirect: "follow",
      });
      let body = "";
      try { body = await r.text(); } catch (e) {}
      const MAX = 200000;
      let truncated = false;
      if (body.length > MAX) { body = body.slice(0, MAX); truncated = true; }
      const headers = {};
      r.headers.forEach(function (v, k) { headers[k] = v; });
      return { ok: r.ok, status: r.status, url: r.url, redirected: r.redirected,
               truncated: truncated, headers: headers, body: body };
    } catch (e) {
      return { ok: false, status: 0, error: String(e && e.message || e) };
    }
  };

  window.__shikisha = true;

  // "Loading finished" waits for `load`. At DOMContentLoaded, images and
  // CSS haven't arrived yet, and content JS builds afterward isn't in place.
  //
  // But ad-laden pages wait on external tracking tags, so `load` can lag
  // several seconds, or never fire. If we can't wait that long, announce
  // at the DOM-only point instead and record which case it was in
  // `complete`. Better to be honest than to guess and be wrong
  let told = false;
  const announce = complete => {
    if (told) return;
    told = true;
    send({ kind: "loading", busy: false });   // Loading finished = clear the "busy" indicator
    send({ kind: "ready", url: location.href, complete: !!complete });
  };
  const SETTLE_MS = 8000;
  if (document.readyState === "complete") {
    announce(true);
  } else {
    addEventListener("load", () => announce(true), { once: true });
    const armFallback = () => setTimeout(() => announce(false), SETTLE_MS);
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", armFallback, { once: true });
    } else {
      armFallback();
    }
  }
})();
"#;

/// An instruction from the conductor to the browser
#[derive(Debug, Clone)]
pub enum Cmd {
    /// Evaluate JS and return the result (matched up by `id`).
    /// `to` is the destination page name. `None` means the main view
    Eval {
        id: u64,
        to: Option<String>,
        js: String,
    },
    /// Show a bar calling out to the human
    Ask {
        to: Option<String>,
        text: String,
        label: String,
    },
    /// Hide the bar
    Unask { to: Option<String> },
    /// Place a named page inside the same window
    AddChild {
        name: String,
        url: String,
        rect: (i32, i32, i32, i32),
        /// This page's data storage (profile / private)
        profile: BrowserProfile,
    },
    /// Set the placed page's position and size. Width or height of 0 hides it
    ChildBounds {
        name: String,
        rect: (i32, i32, i32, i32),
    },
    /// Remove a placed page
    RemoveChild { name: String },
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
    /// Close the window (when the conductor is gone)
    Close,
}

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
    To(String),
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
        Some("button") => Ev::Button { from: None },
        Some("select") => Ev::Select {
            tab: v.get("tab").and_then(|x| x.as_u64()).unwrap_or(0) as usize,
        },
        Some("addtab") => Ev::AddTab,
        Some("closesettings") => Ev::CloseSettings,
        Some("opensettings") => Ev::OpenSettings,
        Some("menu") => Ev::Menu {
            key: v
                .get("key")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        Some("openws") => Ev::OpenWs,
        Some("stop") => Ev::Stop,
        Some("remotecut") => Ev::RemoteCut,
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
            Ev::Resize {
                rows: v.get("rows").and_then(|x| x.as_u64()).unwrap_or(24) as u16,
                cols: v.get("cols").and_then(|x| x.as_u64()).unwrap_or(80) as u16,
                area: (num(0), num(1), num(2), num(3)),
            }
        }
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
                _ => return None,
            };
            Ev::Inject { to: None, input }
        }
        Some("key") => Ev::Key {
            text: v.get("text").and_then(|x| x.as_str()).map(str::to_string),
            named: v.get("named").and_then(|x| x.as_str()).map(str::to_string),
            ctrl: v.get("ctrl").and_then(|x| x.as_str()).map(str::to_string),
        },
        Some("chat") => Ev::Chat {
            text: v
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        },
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
    /// The bar's button was pressed = the human finished their turn.
    /// `from` is the name of the page it was pressed on (`None` is the
    /// main view). Since multiple pages can be placed at once, without
    /// tracking which one it was, a neighboring browser's turn could
    /// wrongly be marked as finished
    Button { from: Option<String> },
    /// The window's size changed (how many rows/columns fit)
    Resize {
        rows: u16,
        cols: u16,
        /// The content area (x, y, width, height). The browser is placed here
        area: (i32, i32, i32, i32),
    },
    /// Wants to view this tab (0 = the operating board)
    Select { tab: usize },
    /// The + on the tab bar was pressed (opens the settings screen in add-tab mode)
    AddTab,
    /// "Close settings" on the settings page. Collapses the settings tab
    /// and returns to the operating board. This is a window-internal
    /// action, so it's not accepted from a phone (allowed_from_afar)
    CloseSettings,
    /// Open the settings page. A dedicated intent for the sidebar gear so it
    /// works from any tab (the menu "e" key only fires while INDEX is in view).
    /// Window-internal, so not accepted from a phone (allowed_from_afar).
    OpenSettings,
    /// The operating board's menu was pressed
    Menu { key: String },
    /// Open the workspace switcher. A dedicated intent (rather than reusing the
    /// plain 'w' keystroke of `Menu`) so the tab-bar button works from any tab:
    /// a bare 'w' would just be typed into whatever session is showing instead
    /// of opening the list. Converted to the Ctrl+B w prefix in `keys_for`.
    OpenWs,
    /// Emergency stop
    Stop,
    /// Cut every remote session from the window's side: rotate the access token
    /// and drop the open connections. Window-only — a phone can't disconnect
    /// itself (allowed_from_afar leaves it on the reject side).
    RemoteCut,
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
    /// A keystroke in window mode. Either a committed character, a named control key, or Ctrl+character
    Key {
        text: Option<String>,
        named: Option<String>,
        ctrl: Option<String>,
    },
    /// A line typed into a model tab's chat box. Delivered to whichever model
    /// tab is currently in view (the bridge answers it directly).
    Chat { text: String },
    /// The window was closed
    Closed,
}

/// A handle to one running browser
pub struct Browser {
    proxy: tao::event_loop::EventLoopProxy<Cmd>,
    events: Receiver<Ev>,
    next_id: AtomicU64,
    /// The bar that should be showing. Navigation wipes out the whole JS
    /// world, so it gets re-shown every time a new document is ready.
    /// Logins commonly bounce through SSO two or three times, and without
    /// re-issuing it, it would "show only at the start and disappear partway".
    /// The bar we keep showing. One per page.
    /// A `None` key means the main view
    pending_ask: std::sync::Mutex<std::collections::HashMap<Option<String>, (String, String)>>,
    /// A different signal that arrived while we were waiting on something.
    ///
    /// Skipping and discarding it means anything sent before the wait
    /// began vanishes forever. That's exactly how the window's column
    /// count once never arrived
    spare: std::sync::Mutex<Vec<Ev>>,
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

/// A browser data-storage spec. Represents both profile isolation (a login
/// box) and private (throwaway) mode in one type. When `private` is true,
/// `name` is ignored and a temporary area that's wiped on close is used instead.
///
/// wry's `WebContext` takes one "data folder". Same folder = same
/// cookies/login, different folder = different profile. Private mode just
/// hands it a unique temp folder (matches wry's own docs: keep a separate
/// context for normal tabs and one for private/incognito tabs).
#[derive(Clone, Debug)]
pub struct BrowserProfile {
    /// The profile name ("default", etc). Ignored when `private` is true
    pub name: String,
    /// Throwaway. If true, opens in a temp folder that keeps no history/cookies
    pub private: bool,
}

impl BrowserProfile {
    /// Build from a name and a private flag. An empty name falls back to "default"
    pub fn new(name: &str, private: bool) -> Self {
        let n = name.trim();
        Self {
            name: if n.is_empty() { "default".into() } else { n.to_string() },
            private,
        }
    }
    /// The default (shared "default" profile, persistent)
    pub fn shared_default() -> Self {
        Self { name: "default".into(), private: false }
    }
}

/// Where browser profiles live. WebView2's data (SQLite, cache) is heavy
/// and clashes with Drive sync, so it goes under %LOCALAPPDATA%, same as
/// exchange (not next to the app binary)
fn profiles_root() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("ShikishaTerm").join("browser-profiles")
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
        profiles_root().join(sanitize_profile(&p.name))
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// At startup, sweep away any private areas left behind by a previous
/// abnormal exit. Private mode is supposed to "vanish on close", so
/// anything still there is garbage
pub fn sweep_private() {
    let _ = std::fs::remove_dir_all(profiles_root().join("_private"));
}

impl Browser {
    /// Open the window and get it ready to accept instructions
    pub fn spawn(url: &str, title: &str) -> Result<Self> {
        if !is_openable(url) {
            return Err(anyhow!(crate::i18n::tp("err.browser.bad_url", &[("url", url)])));
        }
        Self::start(url, title)
    }

    fn start(url: &str, title: &str) -> Result<Self> {
        let (proxy_tx, proxy_rx) = channel();
        let (ev_tx, ev_rx) = channel();
        let url = url.to_string();
        let title = title.to_string();

        std::thread::Builder::new()
            .name("shikisha-browser".into())
            .spawn(move || {
                if let Err(e) = run_window(&url, &title, proxy_tx, ev_tx.clone()) {
                    crate::append_hook_log(&crate::i18n::tp(
                        "err.browser.log_open_failed",
                        &[("e", &format!("{e}"))],
                    ));
                    let _ = ev_tx.send(Ev::Closed);
                }
            })?;

        // Wait until the window exists (if it can't be created, the proxy never arrives)
        let proxy = proxy_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .map_err(|_| anyhow!(crate::i18n::t("err.browser.startup_timeout")))?;

        let me = Self {
            proxy,
            events: ev_rx,
            next_id: AtomicU64::new(1),
            pending_ask: std::sync::Mutex::new(std::collections::HashMap::new()),
            spare: std::sync::Mutex::new(Vec::new()),
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
                .ok_or_else(|| anyhow!(crate::i18n::t("err.browser.page_not_ready")))?;
            match self.events.recv_timeout(left) {
                Ok(Ev::Ready { from, url, .. }) => {
                    self.reask(from.as_deref());
                    return Ok(url);
                }
                Ok(Ev::Closed) => return Err(anyhow!(crate::i18n::t("err.browser.closed"))),
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(_) => return Err(anyhow!(crate::i18n::t("err.browser.page_not_ready"))),
            }
        }
    }

    fn send(&self, cmd: Cmd) -> Result<()> {
        self.proxy
            .send_event(cmd)
            .map_err(|_| anyhow!(crate::i18n::t("err.browser.not_connected")))
    }

    /// Evaluate JS. The result arrives later as `Ev::Result`
    pub fn eval(&self, js: &str) -> Result<u64> {
        self.eval_in(None, js)
    }

    /// Evaluate JS against a target. `None` is the main view
    pub fn eval_in(&self, to: Option<&str>, js: &str) -> Result<u64> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(Cmd::Eval {
            id,
            to: to.map(str::to_string),
            js: js.to_string(),
        })?;
        Ok(id)
    }

    pub fn ask(&self, to: Option<&str>, text: &str, label: &str) -> Result<()> {
        self.pending_ask.lock().unwrap().insert(
            to.map(str::to_string),
            (text.to_string(), label.to_string()),
        );
        self.send(Cmd::Ask {
            to: to.map(str::to_string),
            text: text.to_string(),
            label: label.to_string(),
        })
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

    pub fn unask(&self, to: Option<&str>) -> Result<()> {
        self.pending_ask
            .lock()
            .unwrap()
            .remove(&to.map(str::to_string));
        self.send(Cmd::Unask {
            to: to.map(str::to_string),
        })
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
        if !is_openable(url) {
            return Err(anyhow!(crate::i18n::tp("err.browser.bad_url", &[("url", url)])));
        }
        self.send(Cmd::AddChild {
            name: name.to_string(),
            url: url.to_string(),
            rect,
            profile,
        })
    }

    /// The placed page's position and size. Setting width or height to 0 hides it
    pub fn child_bounds(&self, name: &str, rect: (i32, i32, i32, i32)) -> Result<()> {
        self.send(Cmd::ChildBounds {
            name: name.to_string(),
            rect,
        })
    }

    pub fn close_child(&self, name: &str) -> Result<()> {
        self.send(Cmd::RemoveChild {
            name: name.to_string(),
        })
    }

    /// Call JS once and wait for the result
    fn call(
        &self,
        to: Option<&str>,
        func: &str,
        args: &[serde_json::Value],
        timeout_ms: u64,
    ) -> Result<String> {
        let id = self.eval_in(to, &call_js(func, args))?;
        self.wait_result(id, std::time::Duration::from_millis(timeout_ms))
    }

    /// Where that element currently is
    pub fn find(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Found> {
        Ok(Found::parse(&self.call(
            to,
            "__shikisha_state",
            &[sel.json()],
            timeout_ms,
        )?))
    }

    /// Read text (an input field's contents, or the displayed string otherwise)
    pub fn text(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Option<String>> {
        let v = self.call(to, "__shikisha_text", &[sel.json()], timeout_ms)?;
        Ok(serde_json::from_str::<Option<String>>(&v).unwrap_or(None))
    }

    /// Click it
    pub fn click(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Found> {
        Ok(Found::parse(&self.call(
            to,
            "__shikisha_click",
            &[sel.json()],
            timeout_ms,
        )?))
    }

    /// Put a value into an input field
    pub fn fill(
        &self,
        to: Option<&str>,
        sel: &Sel,
        value: &str,
        timeout_ms: u64,
    ) -> Result<Found> {
        Ok(Found::parse(&self.call(
            to,
            "__shikisha_fill",
            &[sel.json(), serde_json::Value::String(value.to_string())],
            timeout_ms,
        )?))
    }

    /// The full parsed HTML
    pub fn html(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> {
        let v = self.call(to, "__shikisha_html", &[], timeout_ms)?;
        Ok(serde_json::from_str::<String>(&v).unwrap_or(v))
    }

    /// Make a request from inside the page. Returns a JSON string
    /// `{status,ok,url,headers,body,...}`.
    /// `opts` is `{method,headers,body}` (optional)
    pub fn fetch(
        &self,
        to: Option<&str>,
        url: &str,
        opts: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<String> {
        self.call(
            to,
            "__shikisha_fetch",
            &[serde_json::Value::String(url.to_string()), opts.clone()],
            timeout_ms,
        )
    }

    /// Drain the reports accumulated so far (doesn't block).
    /// If we moved to a new document, re-show the bar that should be showing
    pub fn drain(&self) -> Vec<Ev> {
        // Return anything that arrived while we were waiting first (preserves arrival order)
        let mut evs: Vec<Ev> = std::mem::take(&mut *self.spare.lock().unwrap());
        evs.extend(self.events.try_iter());
        for e in &evs {
            if let Ev::Ready { from, .. } = e {
                self.reask(from.as_deref());
            }
        }
        evs
    }

    /// Re-show a bar that navigation wiped out. Only re-shown for the page that navigated
    fn reask(&self, to: Option<&str>) {
        let key = to.map(str::to_string);
        let want = self.pending_ask.lock().unwrap().get(&key).cloned();
        if let Some((t, l)) = want {
            let _ = self.send(Cmd::Ask {
                to: key,
                text: t,
                label: l,
            });
        }
    }

    /// Wait until a password is entered.
    /// Any other signal that arrives while waiting is kept aside (discarding it loses it forever)
    pub fn wait_password(&self, timeout: std::time::Duration) -> Result<Option<String>> {
        let until = std::time::Instant::now() + timeout;
        loop {
            let left = until
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow!(crate::i18n::t("err.browser.no_input")))?;
            match self.events.recv_timeout(left) {
                Ok(Ev::Password { text }) => return Ok(text),
                Ok(Ev::Closed) => return Err(anyhow!(crate::i18n::t("err.browser.window_closed"))),
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(_) => return Err(anyhow!(crate::i18n::t("err.browser.no_input"))),
            }
        }
    }

    /// Wait until a specific evaluation's result arrives
    pub fn wait_result(&self, id: u64, timeout: std::time::Duration) -> Result<String> {
        let until = std::time::Instant::now() + timeout;
        loop {
            let left = until
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow!(crate::i18n::t("err.browser.no_result")))?;
            match self.events.recv_timeout(left) {
                Ok(Ev::Result { id: got, ok, value }) if got == id => {
                    return if ok {
                        Ok(value)
                    } else {
                        Err(anyhow!(crate::i18n::tp(
                            "err.browser.js_eval_failed",
                            &[("value", &value)]
                        )))
                    };
                }
                Ok(Ev::Ready { from, .. }) => {
                    self.reask(from.as_deref());
                    continue;
                }
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(_) => return Err(anyhow!(crate::i18n::t("err.browser.no_result"))),
            }
        }
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

/// A specifier for locating something on the page. CSS or XPath
#[derive(Debug, Clone)]
pub enum Sel {
    Css(String),
    Xpath(String),
}

impl Sel {
    fn json(&self) -> serde_json::Value {
        match self {
            Sel::Css(s) => serde_json::json!({ "css": s }),
            Sel::Xpath(s) => serde_json::json!({ "xpath": s }),
        }
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

    fn parse(json: &str) -> Self {
        match json.trim_matches('"') {
            "visible" => Found::Visible,
            "off_screen" => Found::OffScreen,
            _ => Found::NotFound,
        }
    }
}

/// Resolve an instruction's destination. `None` is the main view; a name is that page.
/// If a name is given but not found, returns `None`.
/// Falling back to the main view would run site-facing JS against our own screen
/// Convert a control key name for the screencast view into what CDP needs (key name, Windows virtual key code)
fn named_vk(named: &str) -> Option<(&'static str, u32)> {
    Some(match named {
        "enter" => ("Enter", 13),
        "backspace" => ("Backspace", 8),
        "tab" => ("Tab", 9),
        "escape" | "esc" => ("Escape", 27),
        "delete" => ("Delete", 46),
        "up" => ("ArrowUp", 38),
        "down" => ("ArrowDown", 40),
        "left" => ("ArrowLeft", 37),
        "right" => ("ArrowRight", 39),
        "space" => (" ", 32),
        "home" => ("Home", 36),
        "end" => ("End", 35),
        "pageup" => ("PageUp", 33),
        "pagedown" => ("PageDown", 34),
        "f1" => ("F1", 112),
        "f2" => ("F2", 113),
        "f3" => ("F3", 114),
        "f4" => ("F4", 115),
        "f5" => ("F5", 116),
        "f6" => ("F6", 117),
        "f7" => ("F7", 118),
        "f8" => ("F8", 119),
        "f9" => ("F9", 120),
        "f10" => ("F10", 121),
        "f11" => ("F11", 122),
        "f12" => ("F12", 123),
        _ => return None,
    })
}

/// Whether `named` is a control key we can dispatch (enter/tab/escape/arrows/
/// f-keys/…). Lets `browser_press` reject a typo instead of silently no-op-ing.
pub fn key_known(named: &str) -> bool {
    named_vk(named).is_some()
}

fn target<'a>(
    main: &'a wry::WebView,
    children: &'a std::collections::HashMap<String, wry::WebView>,
    to: &Option<String>,
) -> Option<&'a wry::WebView> {
    match to {
        None => Some(main),
        Some(name) => children.get(name),
    }
}

/// Build a JS function call.
///
/// **Arguments must always go through here.** Everything is serialized
/// with `serde_json`, so quotes and newlines survive intact and the value
/// passed in is never interpreted as code. Even AI output or text read
/// straight off a page arrives as a plain value
fn call_js(func: &str, args: &[serde_json::Value]) -> String {
    let list: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    format!("return window.{func}({});", list.join(","))
}

/// Convert a position and size into wry's shape
fn to_rect((x, y, w, h): (i32, i32, i32, i32)) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(x, y).into(),
        size: wry::dpi::LogicalSize::new(w.max(0), h.max(0)).into(),
    }
}

/// Turn text a human typed into a destination we're allowed to open.
///
/// Works like a browser's combined address/search box: text that reads as a
/// web address goes there (`example.com` -> `https://example.com`), and
/// anything else — words with spaces, Japanese text, a lone word — becomes a
/// Google search. `file:` can read local files and `javascript:` can hijack
/// the current page, so neither passes through an address bar — a "gateway
/// to anywhere"; they too fall through to search, which is inert.
pub fn openable(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((scheme, rest)) = s.split_once("://") {
        // An explicit scheme means the writer wanted a URL, not a search.
        // Normalize its case so a pasted HTTPS:// still opens.
        if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
            return Some(format!("{}://{rest}", scheme.to_ascii_lowercase()));
        }
        // file:// and friends never open here — hand them to search instead
        return Some(search_url(s));
    }
    // Scheme-less: a single token whose host part has a dot (example.com,
    // 127.0.0.1) or is localhost reads as an address; everything else —
    // including `javascript:alert(1)`, which has no dot — reads as words
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    let address_like = !s.chars().any(char::is_whitespace)
        && (host.contains('.') || host == "localhost" || host.starts_with("localhost:"));
    if address_like {
        Some(format!("https://{s}"))
    } else {
        Some(search_url(s))
    }
}

/// A Google search for the given words, with every byte outside the URL-safe
/// set percent-encoded (UTF-8), so Japanese and symbols survive the trip
fn search_url(words: &str) -> String {
    use std::fmt::Write as _;
    let mut u = String::from("https://www.google.com/search?q=");
    for b in words.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                u.push(*b as char)
            }
            b' ' => u.push('+'),
            _ => {
                let _ = write!(u, "%{b:02X}");
            }
        }
    }
    u
}

fn ask_js(text: &str, label: &str) -> String {
    format!(
        "window.__shikisha_ask({}, {});",
        serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into()),
        serde_json::to_string(label).unwrap_or_else(|_| "\"OK\"".into())
    )
}

fn run_window(
    url: &str,
    title: &str,
    proxy_tx: Sender<tao::event_loop::EventLoopProxy<Cmd>>,
    ev_tx: Sender<Ev>,
) -> Result<()> {
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::platform::windows::EventLoopBuilderExtWindows;
    use tao::window::WindowBuilder;
    use wry::{WebContext, WebViewBuilder};

    // Runs on a separate thread from the TUI's render loop, so lift the main-thread restriction
    let mut ev_loop = EventLoopBuilder::<Cmd>::with_user_event()
        .with_any_thread(true)
        .build();
    proxy_tx
        .send(ev_loop.create_proxy())
        .map_err(|_| anyhow!(crate::i18n::t("err.browser.proxy_connect_failed")))?;

    let window = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 900.0))
        .build(&ev_loop)?;

    let ipc = ev_tx.clone();
    let webview = WebViewBuilder::new()
        .with_url(url)
        .with_initialization_script(INIT_JS)
        .with_ipc_handler(move |req| {
            let body: &str = req.body();
            let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
                return;
            };
            let Some(ev) = parse_intent(&v) else {
                return;
            };
            let _ = ipc.send(ev);
        })
        .build(&window)?;

    // Pages placed inside the same window. Looked up by name
    let mut children: std::collections::HashMap<String, wry::WebView> =
        std::collections::HashMap::new();
    // WebContext per profile. One per data folder (tabs with the same name share it).
    // Not needed after creation on Windows, but keeping it around is harmless, so hold it keyed by folder
    let mut web_ctxs: std::collections::HashMap<std::path::PathBuf, WebContext> =
        std::collections::HashMap::new();
    // Temp folders for children placed in private mode (child name -> folder). Removed on close
    let mut ephemeral_dirs: std::collections::HashMap<String, std::path::PathBuf> =
        std::collections::HashMap::new();

    // Screencasts. One per target. Frames only arrive while this is held
    let mut casts: std::collections::HashMap<Option<String>, cdp::Cast> =
        std::collections::HashMap::new();
    // Basic-auth arming. One per target. Only answers 401s while this is held
    let mut auths: std::collections::HashMap<Option<String>, cdp::AuthArm> =
        std::collections::HashMap::new();
    // Automatic handling of JS dialogs. One per child. Without this, automation freezes on things like "leave this page?" confirmations
    let mut dialogs: std::collections::HashMap<Option<String>, cdp::DialogArm> =
        std::collections::HashMap::new();
    // The most recent frame's CSS pixel dimensions (used to convert
    // coordinates for input injection).
    // Frame notification and input injection run on the same thread, so `Rc<Cell>` is enough
    let cast_dims = std::rc::Rc::new(std::cell::Cell::new((0.0f64, 0.0f64)));
    // For drag detection: is the button currently held down
    let mut mouse_down = false;

    // Reports are sent from inside the loop too, so grab a sender for "closed" ahead of time
    let closed_tx = ev_tx.clone();
    // The channel that answers "where are we now". Only known from inside the window, so it answers from here
    let where_tx = ev_tx.clone();
    ev_loop.run_return(move |event, _, control| {
        *control = ControlFlow::Wait;
        match event {
            Event::UserEvent(cmd) => match cmd {
                Cmd::Eval { id, to, js } => {
                    // When the destination can't be found, don't fall back
                    // to the main view. That would run site-facing JS against our own screen
                    if let Some(v) = target(&webview, &children, &to) {
                        let _ = v.evaluate_script(&wrap_eval(id, &js));
                    } else {
                        let _ = ev_tx.send(Ev::Result {
                            id,
                            ok: false,
                            value: serde_json::Value::String(crate::i18n::tp(
                                "err.browser.page_not_placed",
                                &[("to", &to.unwrap_or_default())],
                            ))
                            .to_string(),
                        });
                    }
                }
                Cmd::BasicAuth { to, user, pass } => {
                    // If credentials are already armed, swap them; otherwise enable Fetch and arm them
                    if let Some(arm) = auths.get(&to) {
                        *arm.creds.borrow_mut() = (user, pass);
                    } else if let Some(v) = target(&webview, &children, &to) {
                        let wv = cdp::webview_of(v);
                        match cdp::arm_basic_auth(&wv, &user, &pass) {
                            Some(arm) => {
                                auths.insert(to.clone(), arm);
                            }
                            None => crate::append_hook_log(&crate::i18n::t(
                                "err.browser.log_basic_auth_failed",
                            )),
                        }
                    }
                }
                Cmd::Ask { to, text, label } => {
                    if let Some(v) = target(&webview, &children, &to) {
                        let _ = v.evaluate_script(&ask_js(&text, &label));
                    }
                }
                Cmd::Unask { to } => {
                    if let Some(v) = target(&webview, &children, &to) {
                        let _ = v.evaluate_script(
                            "window.__shikisha_unask&&window.__shikisha_unask();",
                        );
                    }
                }
                Cmd::AddChild { name, url, rect, profile } => {
                    let bounds = to_rect(rect);
                    // Decide this page's data storage (profile/private).
                    // Same folder = same cookies/login, different folder = different profile.
                    // All tabs, including "default", are isolated under
                    // browser-profiles/<name> (like Chrome's "person").
                    // Private mode gets a unique temp folder on every call
                    // and is removed on close.
                    let data_dir = profile_dir(&profile);
                    if profile.private {
                        ephemeral_dirs.insert(name.clone(), data_dir.clone());
                    }
                    let ctx = web_ctxs
                        .entry(data_dir.clone())
                        .or_insert_with(|| WebContext::new(Some(data_dir.clone())));
                    // Equip the child with the same tools as the main view.
                    // Without them, a placed page would just be something displayed, nothing more
                    let ipc = ev_tx.clone();
                    let who = name.clone();
                    // Signaling "in progress" from the in-page script (at
                    // document creation) is too late. If the server is slow,
                    // the document isn't created until the response comes
                    // back, so the indicator would stay off the whole time
                    // we're waiting. Instead, turn it on when navigation
                    // starts (the moment it's pressed, before any response)
                    // and off when loading finishes. This keeps it lit for the entire wait
                    let nav_tx = ev_tx.clone();
                    let nav_who = name.clone();
                    let fin_tx = ev_tx.clone();
                    let fin_who = name.clone();
                    match WebViewBuilder::new_with_web_context(ctx)
                        .with_url(&url)
                        .with_bounds(bounds)
                        .with_initialization_script(INIT_JS)
                        .with_navigation_handler(move |_url| {
                            let _ = nav_tx.send(Ev::Loading { from: Some(nav_who.clone()), busy: true });
                            true // Don't block the navigation. This is only here to emit a signal
                        })
                        .with_on_page_load_handler(move |e, _url| {
                            if matches!(e, wry::PageLoadEvent::Finished) {
                                let _ = fin_tx.send(Ev::Loading { from: Some(fin_who.clone()), busy: false });
                            }
                        })
                        .with_ipc_handler(move |req| {
                            let body: &str = req.body();
                            let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
                                return;
                            };
                            let Some(ev) = parse_intent(&v) else {
                                return;
                            };
                            // There's no way to know who pressed it except here
                            let ev = match ev {
                                Ev::Button { .. } => Ev::Button {
                                    from: Some(who.clone()),
                                },
                                Ev::Ready { url, complete, .. } => Ev::Ready {
                                    from: Some(who.clone()),
                                    url,
                                    complete,
                                },
                                Ev::Loading { busy, .. } => Ev::Loading {
                                    from: Some(who.clone()),
                                    busy,
                                },
                                other => other,
                            };
                            let _ = ipc.send(ev);
                        })
                        .build_as_child(&window)
                    {
                        Ok(v) => {
                            // Arm automatic dialog handling right after placing it (don't let "leave page?" freeze it)
                            let wvh = cdp::webview_of(&v);
                            if let Some(arm) = cdp::arm_dialogs(&wvh) {
                                dialogs.insert(Some(name.clone()), arm);
                            }
                            children.insert(name, v);
                        }
                        Err(e) => crate::append_hook_log(&crate::i18n::tp(
                            "err.browser.log_place_failed",
                            &[("name", &name), ("e", &format!("{e}"))],
                        )),
                    }
                }
                Cmd::ChildBounds { name, rect } => {
                    if let Some(v) = children.get(&name) {
                        let _ = v.set_bounds(to_rect(rect));
                    }
                }
                Cmd::RemoveChild { name } => {
                    children.remove(&name);
                    dialogs.remove(&Some(name.clone()));
                    auths.remove(&Some(name.clone()));
                    // If this child was placed in private mode, clean up
                    // its throwaway folder. WebView2 can take a moment to
                    // release the lock, so this is best-effort
                    // (anything missed gets swept up by sweep_private at startup)
                    if let Some(dir) = ephemeral_dirs.remove(&name) {
                        web_ctxs.remove(&dir);
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                }
                Cmd::Focus { to } => {
                    if let Some(v) = target(&webview, &children, &to) {
                        if let Err(e) = v.focus() {
                            crate::append_hook_log(&crate::i18n::tp(
                                "err.browser.log_focus_failed",
                                &[("to", &format!("{to:?}")), ("e", &format!("{e}"))],
                            ));
                        }
                    }
                }
                Cmd::Move { to, go } => match target(&webview, &children, &to) {
                    Some(v) => {
                        let r = match &go {
                            Go::Back => v.go_back(),
                            Go::Forward => v.go_forward(),
                            Go::Reload => v.reload(),
                            Go::To(u) => v.load_url(u),
                        };
                        if let Err(e) = r {
                            crate::append_hook_log(&crate::i18n::tp(
                                "err.browser.log_move_failed",
                                &[("go", &format!("{go:?}")), ("e", &format!("{e}"))],
                            ));
                        }
                    }
                    None => crate::append_hook_log(&crate::i18n::tp(
                        "err.browser.log_no_target",
                        &[("to", &format!("{to:?}"))],
                    )),
                },
                Cmd::Where { to } => {
                    if let Some(v) = target(&webview, &children, &to) {
                        let _ = where_tx.send(Ev::Where {
                            from: to,
                            url: v.url().unwrap_or_default(),
                            can_back: v.can_go_back().unwrap_or(false),
                            can_forward: v.can_go_forward().unwrap_or(false),
                        });
                    }
                }
                Cmd::Screencast { to, on } => {
                    if on {
                        if casts.contains_key(&to) {
                            // Already streaming. We won't register twice,
                            // but re-issue startScreencast to push out one
                            // fresh frame (otherwise a new viewer joining
                            // while the page is static would see nothing indefinitely)
                            if let Some(view) = target(&webview, &children, &to) {
                                cdp::kick(&cdp::webview_of(view));
                            }
                        } else if let Some(view) = target(&webview, &children, &to) {
                            let wv = cdp::webview_of(view);
                            let tx = ev_tx.clone();
                            let from = to.clone();
                            let dims = cast_dims.clone();
                            if let Some(cast) = cdp::start(&wv, move |data, w, h| {
                                dims.set((w, h));
                                let _ = tx.send(Ev::Frame {
                                    from: from.clone(),
                                    data,
                                    w: w as u32,
                                    h: h as u32,
                                });
                            }) {
                                casts.insert(to.clone(), cast);
                            } else {
                                crate::append_hook_log(&crate::i18n::t(
                                    "err.browser.log_screencast_failed",
                                ));
                            }
                        }
                    } else if let Some(cast) = casts.remove(&to) {
                        cdp::stop(cast);
                    }
                }
                Cmd::Inject { to, input } => {
                    if let Some(view) = target(&webview, &children, &to) {
                        let wv = cdp::webview_of(view);
                        let (cw, ch) = cast_dims.get();
                        match input {
                            Input::Mouse { phase, x, y, down } => {
                                let (px, py) = (x * cw, y * ch);
                                let (kind, buttons) = match phase.as_str() {
                                    "pressed" => {
                                        mouse_down = true;
                                        ("mousePressed", 1)
                                    }
                                    "released" => {
                                        mouse_down = false;
                                        ("mouseReleased", 0)
                                    }
                                    _ => ("mouseMoved", if down || mouse_down { 1 } else { 0 }),
                                };
                                let params = serde_json::json!({
                                    "type": kind, "x": px, "y": py,
                                    "button": "left", "buttons": buttons, "clickCount": 1,
                                })
                                .to_string();
                                cdp::call(&wv, "Input.dispatchMouseEvent", &params);
                            }
                            Input::Wheel { x, y, dx, dy } => {
                                let params = serde_json::json!({
                                    "type": "mouseWheel", "x": x * cw, "y": y * ch,
                                    "deltaX": dx, "deltaY": dy,
                                })
                                .to_string();
                                cdp::call(&wv, "Input.dispatchMouseEvent", &params);
                            }
                            Input::Text { text } => {
                                // insertText doesn't land in the input fields
                                // of some sites, e.g. Google (they ignore the
                                // input event). Sending one char key event
                                // per character gets treated as a real
                                // keystroke and works much more broadly.
                                // IME conversion is already done on the sender's side, so just send the committed characters through
                                for ch in text.chars() {
                                    let mut buf = [0u8; 4];
                                    let s: &str = ch.encode_utf8(&mut buf);
                                    let params =
                                        serde_json::json!({ "type": "char", "text": s }).to_string();
                                    cdp::call(&wv, "Input.dispatchKeyEvent", &params);
                                }
                            }
                            Input::Key { named, ctrl, alt } => {
                                if let Some((key, vk)) = named_vk(&named) {
                                    // CDP modifier bits: Alt=1, Ctrl=2, Meta=4, Shift=8
                                    let mods = (if alt { 1 } else { 0 }) | (if ctrl { 2 } else { 0 });
                                    for kind in ["keyDown", "keyUp"] {
                                        let mut ev = serde_json::json!({
                                            "type": kind, "key": key,
                                            "windowsVirtualKeyCode": vk,
                                            "nativeVirtualKeyCode": vk,
                                            "modifiers": mods,
                                        });
                                        // Space needs a `text` field attached, or it won't land in
                                        // the input field. When combined with a modifier (e.g. Ctrl+Space), treat it as a shortcut instead
                                        if kind == "keyDown" && named == "space" && mods == 0 {
                                            ev["text"] = serde_json::Value::from(" ");
                                        }
                                        cdp::call(&wv, "Input.dispatchKeyEvent", &ev.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                Cmd::Close => {
                    *control = ControlFlow::Exit;
                }
            },
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control = ControlFlow::Exit;
            }
            _ => {}
        }
    });

    let _ = closed_tx.send(Ev::Closed);
    Ok(())
}

/// Screencasting and input injection over CDP (Chrome DevTools Protocol).
///
/// WebView2 is Chromium under the hood, so it speaks the developer-tools
/// protocol. Using it lets us receive "only what changed" as JPEG frames
/// (lighter than VNC), and inject mouse, wheel, and text input as
/// **genuine input** (not synthetic events).
///
/// COM objects are thread-bound, so calls must always be made from the
/// window's event-loop thread (inside `run_window`). Frame notifications also arrive on that same thread.
mod cdp {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceivedEventArgs,
        ICoreWebView2DevToolsProtocolEventReceiver,
    };
    use webview2_com::{
        CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
    };
    use windows::core::{HSTRING, PCWSTR};

    /// What's needed to tear down a screencast (notifications only arrive while this is held)
    pub struct Cast {
        pub receiver: ICoreWebView2DevToolsProtocolEventReceiver,
        pub token: i64,
        pub webview: ICoreWebView2,
    }

    /// Call one CDP method (the result is discarded). `params_json` can just be "{}"
    pub fn call(webview: &ICoreWebView2, method: &str, params_json: &str) {
        let method = HSTRING::from(method);
        let params = HSTRING::from(params_json);
        let handler =
            CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_hr, _json| Ok(())));
        unsafe {
            let _ = webview.CallDevToolsProtocolMethod(
                PCWSTR(method.as_ptr()),
                PCWSTR(params.as_ptr()),
                &handler,
            );
        }
    }

    /// Pull the underlying `ICoreWebView2` out of a wry `WebView`
    pub fn webview_of(view: &wry::WebView) -> ICoreWebView2 {
        use wry::WebViewExtWindows;
        view.webview()
    }

    /// Start screencasting. Calls `on_frame(base64_jpeg, css_w, css_h)`
    /// every time a frame arrives.
    /// This also sends the frame ack automatically (without it, the next frame never comes)
    pub fn start<F>(webview: &ICoreWebView2, on_frame: F) -> Option<Cast>
    where
        F: FnMut(String, f64, f64) + 'static,
    {
        let cb = std::rc::Rc::new(std::cell::RefCell::new(on_frame));
        let wv = webview.clone();
        let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(
            move |_sender, args: Option<ICoreWebView2DevToolsProtocolEventReceivedEventArgs>| {
                if let Some(args) = args {
                    let mut raw = windows::core::PWSTR::null();
                    unsafe {
                        if args.ParameterObjectAsJson(&mut raw).is_ok() {
                            let json = webview2_com::take_pwstr(raw);
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                                let data = v
                                    .get("data")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or_default()
                                    .to_string();
                                let meta = v.get("metadata");
                                let w = meta
                                    .and_then(|m| m.get("deviceWidth"))
                                    .and_then(|x| x.as_f64())
                                    .unwrap_or(0.0);
                                let h = meta
                                    .and_then(|m| m.get("deviceHeight"))
                                    .and_then(|x| x.as_f64())
                                    .unwrap_or(0.0);
                                let sid = v.get("sessionId").and_then(|x| x.as_i64()).unwrap_or(0);
                                // Send the ack first, then deliver the frame (avoids stalling the pipe)
                                call(
                                    &wv,
                                    "Page.screencastFrameAck",
                                    &format!("{{\"sessionId\":{sid}}}"),
                                );
                                if !data.is_empty() {
                                    (cb.borrow_mut())(data, w, h);
                                }
                            }
                        }
                    }
                }
                Ok(())
            },
        ));

        let name = HSTRING::from("Page.screencastFrame");
        let mut token = 0i64;
        unsafe {
            let receiver = webview
                .GetDevToolsProtocolEventReceiver(PCWSTR(name.as_ptr()))
                .ok()?;
            receiver
                .add_DevToolsProtocolEventReceived(&handler, &mut token)
                .ok()?;
            call(webview, "Page.enable", "{}");
            call(
                webview,
                "Page.startScreencast",
                "{\"format\":\"jpeg\",\"quality\":60,\"maxWidth\":1600,\"maxHeight\":1200,\"everyNthFrame\":1}",
            );
            Some(Cast {
                receiver,
                token,
                webview: webview.clone(),
            })
        }
    }

    /// Basic-auth arming (401s only get answered while this is held).
    ///
    /// Receiving auth challenges (authRequired) requires intercepting
    /// requests, so we catch every request via Fetch. A caught, ordinary
    /// request is passed straight through with continueRequest (holding
    /// it forever would stall the page); only auth challenges get
    /// credentials back via continueWithAuth
    pub struct AuthArm {
        pub receivers: Vec<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
        pub webview: ICoreWebView2,
        /// The credentials to return (user, pass). Held shared so it can be swapped out
        pub creds: std::rc::Rc<std::cell::RefCell<(String, String)>>,
    }

    /// Subscribe to one CDP event. Calls `on` with the received JSON
    fn subscribe<F>(
        webview: &ICoreWebView2,
        event: &str,
        on: F,
    ) -> Option<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>
    where
        F: Fn(&serde_json::Value) + 'static,
    {
        let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(
            move |_sender, args: Option<ICoreWebView2DevToolsProtocolEventReceivedEventArgs>| {
                if let Some(args) = args {
                    let mut raw = windows::core::PWSTR::null();
                    unsafe {
                        if args.ParameterObjectAsJson(&mut raw).is_ok() {
                            let json = webview2_com::take_pwstr(raw);
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                                on(&v);
                            }
                        }
                    }
                }
                Ok(())
            },
        ));
        let name = HSTRING::from(event);
        let mut token = 0i64;
        unsafe {
            let receiver = webview
                .GetDevToolsProtocolEventReceiver(PCWSTR(name.as_ptr()))
                .ok()?;
            receiver
                .add_DevToolsProtocolEventReceived(&handler, &mut token)
                .ok()?;
            Some((receiver, token))
        }
    }

    pub fn arm_basic_auth(webview: &ICoreWebView2, user: &str, pass: &str) -> Option<AuthArm> {
        let creds = std::rc::Rc::new(std::cell::RefCell::new((user.to_string(), pass.to_string())));

        // Pass a caught, ordinary request straight through (not continuing it would stall the page)
        let wv_req = webview.clone();
        let paused = subscribe(webview, "Fetch.requestPaused", move |v| {
            if let Some(id) = v.get("requestId").and_then(|x| x.as_str()) {
                call(&wv_req, "Fetch.continueRequest", &format!("{{\"requestId\":\"{id}\"}}"));
            }
        })?;

        // Return credentials for auth challenges
        let wv_auth = webview.clone();
        let creds_h = std::rc::Rc::clone(&creds);
        let required = subscribe(webview, "Fetch.authRequired", move |v| {
            let id = v.get("requestId").and_then(|x| x.as_str()).unwrap_or_default();
            let (u, p) = {
                let c = creds_h.borrow();
                (c.0.clone(), c.1.clone())
            };
            let params = serde_json::json!({
                "requestId": id,
                "authChallengeResponse": {
                    "response": "ProvideCredentials",
                    "username": u,
                    "password": p,
                }
            })
            .to_string();
            call(&wv_auth, "Fetch.continueWithAuth", &params);
        })?;

        // Catch every request, and auth too
        call(
            webview,
            "Fetch.enable",
            r#"{"patterns":[{"urlPattern":"*"}],"handleAuthRequests":true}"#,
        );
        Some(AuthArm {
            receivers: vec![paused, required],
            webview: webview.clone(),
            creds,
        })
    }

    /// Automatic handling of JS dialogs (alert / confirm / prompt / beforeunload).
    ///
    /// Without this, things like a page's "leave this page?" confirmation
    /// open as a native dialog, the CDP response channel stalls, and
    /// `browser_*` hangs with "no result returned" (automation freezes
    /// entirely). Since this is for automation, the default is
    /// accept=true = proceed: beforeunload means "navigate away", confirm
    /// means OK, alert/prompt means dismiss. Once `Page` is enabled and
    /// this is subscribed, no more native dialogs appear — we close them
    /// immediately instead. Only active while this is held (unsubscribes on drop).
    pub struct DialogArm {
        pub receivers: Vec<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
        pub webview: ICoreWebView2,
    }

    pub fn arm_dialogs(webview: &ICoreWebView2) -> Option<DialogArm> {
        let wv = webview.clone();
        let opening = subscribe(webview, "Page.javascriptDialogOpening", move |_v| {
            call(&wv, "Page.handleJavaScriptDialog", r#"{"accept":true}"#);
        })?;
        // Enable Page so the subscription actually fires (idempotent even if screencast already enabled it)
        call(webview, "Page.enable", "{}");
        Some(DialogArm {
            receivers: vec![opening],
            webview: webview.clone(),
        })
    }

    /// Force the current screen out as one frame (re-issues startScreencast).
    /// Used when a new viewer joins but the page is static and no new change is coming
    pub fn kick(webview: &ICoreWebView2) {
        call(
            webview,
            "Page.startScreencast",
            "{\"format\":\"jpeg\",\"quality\":60,\"maxWidth\":1600,\"maxHeight\":1200,\"everyNthFrame\":1}",
        );
    }

    /// Stop the screencast and unsubscribe its notifications too
    pub fn stop(cast: Cast) {
        unsafe {
            call(&cast.webview, "Page.stopScreencast", "{}");
            let _ = cast
                .receiver
                .remove_DevToolsProtocolEventReceived(cast.token);
        }
    }
}

#[cfg(test)]
mod nav_tests {
    use super::*;

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
            "前後の空白は落とす"
        );
        assert_eq!(
            openable("http://127.0.0.1:8080/").as_deref(),
            Some("http://127.0.0.1:8080/")
        );
        assert_eq!(
            openable("HTTPS://Example.com/A").as_deref(),
            Some("https://Example.com/A"),
            "貼り付けた大文字スキームも通す（後段の検査は小文字前提）"
        );
        for empty in ["", "   "] {
            assert!(openable(empty).is_none(), "開けてしまう: {empty}");
        }
        // Dangerous schemes never reach the page — they become an inert search instead
        for bad in ["file:///C:/secret.txt", "ftp://x/y", "javascript:alert(1)"] {
            let got = openable(bad).unwrap_or_default();
            assert!(
                got.starts_with("https://www.google.com/search?q="),
                "検索に落ちていない: {bad} -> {got}"
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
        match read(r#"{"kind":"go","what":"to","url":"example.com"}"#) {
            Some(Ev::Go { go: Go::To(u) }) => assert_eq!(u, "example.com"),
            other => panic!("行き先が読めていない: {other:?}"),
        }
        // Discard unknown instructions. Doing nothing is better than silently doing something else
        assert!(read(r#"{"kind":"go","what":"quit"}"#).is_none());
        assert!(read(r#"{"kind":"go"}"#).is_none());
    }

    /// The workspace button and the model-chat box parse into their own intents,
    /// not into a keystroke that would leak into the visible session.
    #[test]
    fn workspace_and_chat_intents_parse() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v)
        };
        assert!(matches!(read(r#"{"kind":"openws"}"#), Some(Ev::OpenWs)));
        match read(r#"{"kind":"chat","text":"hello"}"#) {
            Some(Ev::Chat { text }) => assert_eq!(text, "hello"),
            other => panic!("chat が読めていない: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;


    /// Serve a test page on 127.0.0.1.
    /// `file:///` crashes on wry's IPC, so use http, same as production
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
    #[test]
    #[ignore]
    fn browser_page_ops() {
        let b = Browser::spawn(&serve(PAGE), "SHIKISHA-TERM ops probe").expect("窓が開かない");
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
        assert_eq!(name.as_deref(), Some("山田"), "XPathで隣のセルが取れない");

        // Click it
        assert_eq!(b.click(None, &Sel::Css("#go".into()), t).unwrap(), Found::Visible);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(
            b.text(None, &Sel::Css("#log".into()), t).unwrap().as_deref(),
            Some("pushed"),
            "押した結果がページに出ていない"
        );

        // Fill it. Not just writing the value — the `input` event must fire too
        // (frameworks like React won't update state otherwise)
        assert_eq!(
            b.fill(None, &Sel::Css("#q".into()), "ふつうの値", t).unwrap(),
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
            "input イベントが飛んでいない"
        );

        // This is the crux: the value must never become code.
        // Even AI output or text read straight off a page arrives as a plain value
        let nasty = "'; window.__pwned = 1; //\"</script><img src=x onerror=alert(1)>\\";
        assert_eq!(
            b.fill(None, &Sel::Css("#q".into()), nasty, t).unwrap(),
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some(nasty),
            "値が一字一句そのまま入っていない"
        );

        // A value containing newlines. A single-line `input` drops
        // newlines (per the HTML spec), so multi-line values must go
        // through a `textarea`. The value isn't corrupted — the container just can't hold it
        let multi = format!("1行目\n2行目\t{nasty}");
        assert_eq!(
            b.fill(None, &Sel::Css("#multi".into()), &multi, t).unwrap(),
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#multi".into()), t).unwrap().as_deref(),
            Some(multi.as_str()),
            "改行やタブを含む値が崩れている"
        );
        let id = b.eval("return typeof window.__pwned;").unwrap();
        assert_eq!(
            b.wait_result(id, std::time::Duration::from_millis(t)).unwrap(),
            "\"undefined\"",
            "渡した値がコードとして実行された"
        );

        // The full parsed HTML
        let html = b.html(None, t).unwrap();
        assert!(html.contains("ここにいる"), "HTMLが取れていない");
        assert!(html.len() > 200, "HTMLが短すぎる: {}", html.len());
        println!("HTML {} 文字 / すべて通過", html.chars().count());

        drop(b);
    }


    /// Pages can be placed inside the same window.
    ///
    ///   cargo test child_view -- --ignored --nocapture
    ///
    /// With a separate window, ownership, position tracking, and even
    /// exposure during Windows Terminal tab switching all became our own problem
    #[test]
    #[ignore]
    fn a_page_can_sit_inside_the_window() {
        let b = Browser::spawn(&serve(PAGE), "child probe").expect("窓が開かない");
        b.open_child("side", "https://example.com/", (400, 0, 400, 500), BrowserProfile::shared_default())
            .expect("置けない");
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
            "子を置いたら外皮が動かなくなった"
        );
        println!("子ページの出し入れ: 通過");
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

        assert!(!is_openable("file:///C:/tmp/a.html"), "file: は落ちる");
        assert!(!is_openable("data:text/html,<b>x"), "data: は落ちる");
        assert!(!is_openable("about:blank"));
        assert!(!is_openable("https://"), "ホストが無い");
        assert!(!is_openable(""));
        assert!(!is_openable("https://example.com/a\nhttps://evil"), "改行の混入");
    }

    /// The window opens, JS runs, results come back, and closing it
    /// doesn't kill the app.
    ///
    ///   cargo test browser_round_trip -- --ignored --nocapture
    ///
    /// That last point is the crux. tao's `run` calls `process::exit`
    /// internally, so a naive implementation would take down the whole TUI just by closing the window
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

        let b = Browser::spawn(&url, "SHIKISHA-TERM browser probe").expect("窓が開かない");

        let id = b.eval("return 40 + 2;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).expect("結果なし");
        println!("eval(40+2) = {v}");
        assert_eq!(v, "42");

        let id = b.eval("return document.querySelector('#aaa').textContent;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).expect("結果なし");
        println!("querySelector = {v}");
        assert_eq!(v, "\"hello\"");

        let id = b.eval("return document.documentElement.outerHTML.length;").unwrap();
        println!("HTML長 = {}", b.wait_result(id, Duration::from_secs(20)).unwrap());

        b.ask(None, "ログインしてください", "できました").unwrap();
        std::thread::sleep(Duration::from_millis(800));
        let id = b.eval("return !!document.getElementById('__shikisha_bar');").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).unwrap();
        println!("帯が出ているか = {v}");
        assert_eq!(v, "true", "呼びかけの帯が出ていない");

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
        println!("閉じてもここまで来た (プロセスは生きている)");
    }
}

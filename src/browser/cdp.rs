//! What is done through a page's DevTools protocol (CDP), whatever engine
//! draws the page.
//!
//! Both engines this window is drawn with are Chromium, and both let the
//! protocol be spoken to a page: WebView2 through its COM interfaces, CEF
//! through its own calls. The engine hands over a [`Port`] -- one page's line
//! to the protocol, with a call, a call answered, and an event heard -- and
//! everything here is built on those three. A screencast, the answers to a
//! site's sign-in box, the dialogs a page opens and its console are the same
//! protocol on every system, so they are written once.
//!
//! Everything here runs on the window's own thread, which is where an engine
//! takes its calls and gives its answers.

pub(crate) use super::engine::{Heard, Port};
use shikisha_core::cdp::CAST_PARAMS;

/// Wake parameters: the cheapest cast that still forces the compositor
/// to produce frames. The frames themselves are thrown away — the point
/// is that a hidden page becomes able to process (and ack) mouse input
pub const WAKE_PARAMS: &str =
    "{\"format\":\"jpeg\",\"quality\":10,\"maxWidth\":32,\"maxHeight\":32,\"everyNthFrame\":10}";

/// A screencast. Frames arrive only while this is held
pub struct Cast {
    _frames: Heard,
    port: Port,
}

/// Start screencasting. Calls `on_frame(base64_jpeg, css_w, css_h)`
/// every time a frame arrives.
/// This also sends the frame ack automatically (without it, the next frame never comes)
pub fn start<F>(port: &Port, on_frame: F) -> Option<Cast>
where
    F: FnMut(String, f64, f64) + 'static,
{
    start_with(port, CAST_PARAMS, on_frame)
}

/// `start` with explicit cast parameters (the wake path wants tiny frames)
pub fn start_with<F>(port: &Port, params: &str, on_frame: F) -> Option<Cast>
where
    F: FnMut(String, f64, f64) + 'static,
{
    let cb = std::cell::RefCell::new(on_frame);
    let acker = port.clone();
    let frames = port.listen("Page.screencastFrame", move |v| {
        let data = v.get("data").and_then(|x| x.as_str()).unwrap_or_default().to_string();
        let meta = v.get("metadata");
        let w = meta.and_then(|m| m.get("deviceWidth")).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let h = meta.and_then(|m| m.get("deviceHeight")).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let sid = v.get("sessionId").and_then(|x| x.as_i64()).unwrap_or(0);
        // Send the ack first, then deliver the frame (avoids stalling the pipe)
        acker.call("Page.screencastFrameAck", &format!("{{\"sessionId\":{sid}}}"));
        if !data.is_empty() {
            (cb.borrow_mut())(data, w, h);
        }
    })?;
    port.call("Page.enable", "{}");
    port.call("Page.startScreencast", params);
    Some(Cast { _frames: frames, port: port.clone() })
}

/// Force a fresh first frame, even if the page has not changed.
/// Used when a new viewer joins but the page is static and no new change is coming
pub fn kick(port: &Port) {
    port.call("Page.stopScreencast", "{}");
    port.call("Page.startScreencast", CAST_PARAMS);
}

/// Stop the screencast and unsubscribe its notifications too
pub fn stop(cast: Cast) {
    cast.port.call("Page.stopScreencast", "{}");
}

/// Basic-auth arming (401s only get answered while this is held).
///
/// Receiving auth challenges (authRequired) requires intercepting
/// requests, so we catch every request via Fetch. A caught, ordinary
/// request is passed straight through with continueRequest (holding
/// it forever would stall the page); only auth challenges get
/// credentials back via continueWithAuth
pub struct AuthArm {
    _heard: Vec<Heard>,
    port: Port,
    /// The credentials to return (user, pass). Held shared so it can be swapped out
    pub creds: std::rc::Rc<std::cell::RefCell<(String, String)>>,
}

impl Drop for AuthArm {
    /// Catching every request is only safe while somebody is answering
    /// them: `Fetch.enable` left on with nothing subscribed is a page
    /// whose requests are held forever and never continued
    fn drop(&mut self) {
        self.port.call("Fetch.disable", "{}");
    }
}

pub fn arm_basic_auth(port: &Port, user: &str, pass: &str) -> Option<AuthArm> {
    let creds = std::rc::Rc::new(std::cell::RefCell::new((user.to_string(), pass.to_string())));

    // Pass a caught, ordinary request straight through (not continuing it would stall the page)
    let through = port.clone();
    let paused = port.listen("Fetch.requestPaused", move |v| {
        if let Some(id) = v.get("requestId").and_then(|x| x.as_str()) {
            through.call("Fetch.continueRequest", &format!("{{\"requestId\":\"{id}\"}}"));
        }
    })?;

    // Return credentials for auth challenges
    let answer = port.clone();
    let creds_h = std::rc::Rc::clone(&creds);
    let required = port.listen("Fetch.authRequired", move |v| {
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
        answer.call("Fetch.continueWithAuth", &params);
    })?;

    // Catch every request, and auth too
    port.call("Fetch.enable", r#"{"patterns":[{"urlPattern":"*"}],"handleAuthRequests":true}"#);
    Some(AuthArm { _heard: vec![paused, required], port: port.clone(), creds })
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
    _heard: Heard,
}

pub fn arm_dialogs(port: &Port) -> Option<DialogArm> {
    let answer = port.clone();
    let opening = port.listen("Page.javascriptDialogOpening", move |_v| {
        answer.call("Page.handleJavaScriptDialog", r#"{"accept":true}"#);
    })?;
    // Enable Page so the subscription actually fires (idempotent even if screencast already enabled it)
    port.call("Page.enable", "{}");
    Some(DialogArm { _heard: opening })
}

/// Hearing a page's console. Only while this is held: dropping it lets go
/// of every subscription and turns the log reports back off
pub struct ConsoleArm {
    _heard: Vec<Heard>,
    port: Port,
}

impl Drop for ConsoleArm {
    /// `Runtime` stays on: the page's automation evaluates through it,
    /// and turning it off would pull it out from under a run. `Log` is
    /// only ever on for this
    fn drop(&mut self) {
        self.port.call("Log.disable", "{}");
    }
}

/// Start hearing a page's console: what its code logs, what it throws
/// and nobody catches, and what the browser says about it. Each becomes
/// one line (`console::entry_of`) handed to `on`
pub fn arm_console<F>(port: &Port, on: F) -> Option<ConsoleArm>
where
    F: Fn(serde_json::Value) + 'static,
{
    let on = std::rc::Rc::new(on);
    let mut heard = Vec::new();
    for event in shikisha_core::console::EVENTS {
        let tell = std::rc::Rc::clone(&on);
        heard.push(port.listen(event, move |v| {
            if let Some(entry) = shikisha_core::console::entry_of(event, v, shikisha_core::sqlite::now_ms()) {
                tell(entry);
            }
        })?);
    }
    for domain in shikisha_core::console::DOMAINS {
        port.call(&format!("{domain}.enable"), "{}");
    }
    Some(ConsoleArm { _heard: heard, port: port.clone() })
}

/// The browser's own search, by the protocol's name for it, for an engine
/// that lends none of its own -- and the script every page is given, for one
/// that cannot be searched that way either. One move; where the search
/// stands comes back as `Ev::Seek`
pub fn seek_by_script(
    port: &Port,
    text: &str,
    step: shikisha_shared::Seek,
    page: Option<String>,
    tell: std::sync::mpsc::Sender<shikisha_shared::Ev>,
) {
    let js = shikisha_core::pageops::seek_js(text, step);
    let params = serde_json::json!({
        "expression": format!("(function () {{ {js} }})()"),
        "returnByValue": true,
    })
    .to_string();
    port.call_result("Runtime.evaluate", &params, move |ok, json| {
        let said = serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|v| v.pointer("/result/value").cloned())
            .map(|v| v.to_string())
            .unwrap_or_default();
        let (at, of) = if ok { shikisha_core::pageops::seek_answer(&said) } else { (0, 0) };
        let _ = tell.send(shikisha_shared::Ev::Seek { from: page, at, of });
    });
}

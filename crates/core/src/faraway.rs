//! A browser on the machine the person is sitting at, driven from the machine
//! the agents are on.
//!
//! The other way round from [`crate::chrome`], and for the other half of the
//! same question. A page drawn here is a page a phone can watch and nobody
//! needs to be awake for; a page drawn *there* is native, immediate, and
//! costs this machine nothing to paint. Neither is the right answer for
//! everyone, so both exist and the person says which.
//!
//! What crosses the line is almost nothing. Every operation a page can be
//! asked to perform was already built out of two moves (see
//! [`crate::pageops`]), so those two moves are what travel: call one method of
//! the DevTools protocol, run one piece of JavaScript. The finding, clicking,
//! filling, the digest and its refs, the auto-wait -- all of it runs here,
//! against a browser over there, from the same code that drives the browser on
//! this machine. The rest of the messages are the handful of things that are
//! about *placing* a page rather than doing something to one.
//!
//! Nothing here opens a socket. Asks go out through a sender the caller
//! supplies and answers come back through [`Far::heard`], which is what lets
//! the whole thing be measured without a network.

use crate::pageops::Speaks;
use shikisha_shared::{BrowserHost, BrowserProfile, Ev, Found, Go, Input, OpReport, Sel};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

/// What this machine asks the browser over there to do.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Ask {
    /// One method of the DevTools protocol, on one page
    Cdp { to: Option<String>, method: String, params: serde_json::Value, ms: u64 },
    /// One piece of JavaScript, in one page, waited for
    Eval { to: Option<String>, js: String, ms: u64 },
    /// The same, not waited for
    Run { to: Option<String>, js: String },
    Open { name: String, url: String, rect: (i32, i32, i32, i32), profile: BrowserProfile },
    Bounds { name: String, rect: (i32, i32, i32, i32) },
    Close { name: String },
    Go { to: Option<String>, go: Go },
    Focus { to: Option<String> },
    /// Where the page is now. The answer comes back as a report, not as an
    /// answer -- the same way it does from a window on this machine
    Where { to: Option<String> },
    Auth { to: Option<String>, user: String, pass: String },
    Inject { to: Option<String>, input: Input },
    Record { to: Option<String>, on: bool },
    RecordAllOff,
    Trust { url: String },
}

/// What came back.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Said {
    /// An answer to what was asked, by the number it was asked under
    Answer { id: u64, ok: bool, value: serde_json::Value },
    /// Something a page over there reported, on its own
    Report { ev: Ev },
}

/// Reports a page is allowed to send back up this line.
///
/// The same rule a page in a window is held to, and for the same reason: what
/// is on the other end is a browser showing somebody else's site. It may say
/// what happened to it. It may not ask for anything -- and this line, unlike a
/// page's, could otherwise carry every request the runtime knows how to
/// answer.
fn worth_hearing(ev: &Ev) -> bool {
    shikisha_shared::allowed_from_page(ev)
        || matches!(ev, Ev::Where { from: Some(_), .. } | Ev::Button { from: Some(_) })
}

/// How long to wait for an answer past the deadline the ask itself carried.
/// The far side is a person's laptop over a private network; this is for the
/// line, not for the work
const SLACK_MS: u64 = 5_000;

struct Inner {
    /// Where asks go, while somebody is there to take them
    line: Mutex<Option<Sender<String>>>,
    /// Who is answering, as a person would name them
    who: Mutex<String>,
    waiting: Mutex<HashMap<u64, Sender<(bool, serde_json::Value)>>>,
    next: AtomicU64,
    refs: Mutex<HashMap<Option<String>, Vec<i64>>>,
    heard: Mutex<Vec<Ev>>,
}

/// The browser on a connected client, as something that shows pages.
pub struct Far {
    inner: Arc<Inner>,
}

/// The end of the line the socket holds: where answers and reports are handed
/// in, and which goes quiet when the client leaves.
#[derive(Clone)]
pub struct Line {
    inner: Arc<Inner>,
}

impl Default for Far {
    fn default() -> Self {
        Self::new()
    }
}

impl Far {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                line: Mutex::new(None),
                who: Mutex::new(String::new()),
                waiting: Mutex::new(HashMap::new()),
                next: AtomicU64::new(1),
                refs: Mutex::new(HashMap::new()),
                heard: Mutex::new(Vec::new()),
            }),
        }
    }

    /// The end a socket holds.
    pub fn line(&self) -> Line {
        Line { inner: Arc::clone(&self.inner) }
    }

    /// Whether there is a browser over there at all.
    pub fn here(&self) -> bool {
        self.inner.line.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// What the pages over there have reported since this was last asked.
    pub fn drain(&self) -> Vec<Ev> {
        std::mem::take(&mut self.inner.heard.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Line {
    /// A client has arrived, and will answer from now on.
    pub fn attach(&self, asks: Sender<String>, who: &str) {
        *self.inner.who.lock().unwrap_or_else(|e| e.into_inner()) = who.to_string();
        *self.inner.line.lock().unwrap_or_else(|e| e.into_inner()) = Some(asks);
    }

    /// The client has gone.
    ///
    /// Everything still waiting is told so at once rather than waiting out its
    /// own deadline: a script that asked a browser which is no longer there
    /// should hear about it now, while the reason is still obvious.
    pub fn detach(&self) {
        *self.inner.line.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let waiting: Vec<_> = self
            .inner
            .waiting
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .map(|(_, tx)| tx)
            .collect();
        for tx in waiting {
            let _ = tx.send((false, serde_json::Value::String(crate::i18n::t("err.far.gone"))));
        }
    }

    /// One line from the client.
    pub fn heard(&self, line: &str) {
        let Ok(said) = serde_json::from_str::<Said>(line) else { return };
        match said {
            Said::Answer { id, ok, value } => {
                let waiting = self
                    .inner
                    .waiting
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&id);
                if let Some(tx) = waiting {
                    let _ = tx.send((ok, value));
                }
            }
            Said::Report { ev } => {
                if worth_hearing(&ev) {
                    self.inner.heard.lock().unwrap_or_else(|e| e.into_inner()).push(ev);
                }
            }
        }
    }
}

impl Inner {
    /// Put one ask to the far side and wait for what it says.
    fn ask(&self, ask: &Ask, ms: u64) -> anyhow::Result<serde_json::Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = channel();
        self.waiting.lock().unwrap_or_else(|e| e.into_inner()).insert(id, tx);
        let out = {
            let held = self.line.lock().unwrap_or_else(|e| e.into_inner());
            let Some(sender) = held.as_ref() else {
                self.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                anyhow::bail!(crate::i18n::t("err.far.nobody"));
            };
            serde_json::json!({ "id": id, "ask": ask }).to_string();
            sender.send(serde_json::json!({ "id": id, "ask": ask }).to_string())
        };
        if out.is_err() {
            self.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
            anyhow::bail!(crate::i18n::t("err.far.gone"));
        }
        match rx.recv_timeout(std::time::Duration::from_millis(ms + SLACK_MS)) {
            Ok((true, value)) => Ok(value),
            Ok((false, why)) => Err(anyhow::anyhow!(
                why.as_str().unwrap_or("?").to_string()
            )),
            Err(_) => {
                self.waiting.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
                Err(anyhow::anyhow!(crate::i18n::tp(
                    "err.far.no_answer",
                    &[("who", &self.who.lock().unwrap_or_else(|e| e.into_inner()).clone())]
                )))
            }
        }
    }

    /// The same, for the asks whose answer is only "it was done".
    fn tell(&self, ask: &Ask) -> anyhow::Result<()> {
        self.ask(ask, 10_000).map(|_| ())
    }
}

impl Speaks for Far {
    fn cdp(
        &self,
        to: Option<&str>,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
    ) -> anyhow::Result<serde_json::Value> {
        self.inner.ask(
            &Ask::Cdp {
                to: to.map(str::to_string),
                method: method.to_string(),
                params,
                ms: timeout_ms,
            },
            timeout_ms,
        )
    }

    fn eval(&self, to: Option<&str>, js: &str, timeout_ms: u64) -> anyhow::Result<String> {
        let said = self.inner.ask(
            &Ask::Eval { to: to.map(str::to_string), js: js.to_string(), ms: timeout_ms },
            timeout_ms,
        )?;
        Ok(said.as_str().map_or_else(|| said.to_string(), str::to_string))
    }

    fn refs(&self) -> &Mutex<HashMap<Option<String>, Vec<i64>>> {
        &self.inner.refs
    }
}

impl BrowserHost for Far {
    fn go(&self, to: Option<&str>, go: Go) -> anyhow::Result<()> {
        self.inner.tell(&Ask::Go { to: to.map(str::to_string), go })
    }

    fn focus(&self, to: Option<&str>) -> anyhow::Result<()> {
        self.inner.tell(&Ask::Focus { to: to.map(str::to_string) })
    }

    fn ask_where(&self, to: Option<&str>) -> anyhow::Result<()> {
        self.inner.tell(&Ask::Where { to: to.map(str::to_string) })
    }

    fn basic_auth(&self, to: Option<&str>, user: &str, pass: &str) -> anyhow::Result<()> {
        self.inner.tell(&Ask::Auth {
            to: to.map(str::to_string),
            user: user.to_string(),
            pass: pass.to_string(),
        })
    }

    fn eval_in(&self, to: Option<&str>, js: &str) -> anyhow::Result<u64> {
        self.inner
            .tell(&Ask::Run { to: to.map(str::to_string), js: js.to_string() })
            .map(|()| 0)
    }

    fn inject(&self, to: Option<&str>, input: Input) -> anyhow::Result<()> {
        self.inner.tell(&Ask::Inject { to: to.map(str::to_string), input })
    }

    /// There is nothing to send anybody.
    ///
    /// A page drawn on somebody's desk is already in front of the person whose
    /// desk it is. Sending its picture back here so that it can be sent out
    /// again would be the long way round to where it started -- and to a phone
    /// it would be the picture of a screen it cannot reach anyway. A page that
    /// has to be watched from elsewhere belongs on this machine, which is what
    /// the other setting is for
    fn screencast(&self, to: Option<&str>, on: bool) -> anyhow::Result<()> {
        let _ = (to, on);
        anyhow::bail!(crate::i18n::tp(
            "err.far.no_cast",
            &[("who", &self.inner.who.lock().unwrap_or_else(|e| e.into_inner()).clone())]
        ))
    }

    fn find(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> anyhow::Result<Found> {
        crate::pageops::find(self, to, sel, timeout_ms)
    }
    fn click(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> anyhow::Result<OpReport> {
        crate::pageops::click(self, to, sel, timeout_ms)
    }
    fn fill(&self, to: Option<&str>, sel: &Sel, value: &str, timeout_ms: u64) -> anyhow::Result<OpReport> {
        crate::pageops::fill(self, to, sel, value, timeout_ms)
    }
    fn text(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> anyhow::Result<Option<String>> {
        crate::pageops::text(self, to, sel, timeout_ms)
    }
    fn href(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String> {
        crate::pageops::href(self, to, timeout_ms)
    }
    fn html(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String> {
        crate::pageops::html(self, to, timeout_ms)
    }
    fn digest(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String> {
        crate::pageops::digest(self, to, timeout_ms)
    }
    fn snapshot(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<Vec<u8>> {
        crate::pageops::snapshot(self, to, timeout_ms)
    }
    fn cookies_out(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<serde_json::Value> {
        crate::pageops::cookies_out(self, to, timeout_ms)
    }
    fn cookies_in(&self, to: Option<&str>, cookies: &serde_json::Value, timeout_ms: u64) -> anyhow::Result<()> {
        crate::pageops::cookies_in(self, to, cookies, timeout_ms)
    }
    fn storage_out(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<serde_json::Value> {
        crate::pageops::storage_out(self, to, timeout_ms)
    }
    fn storage_in(&self, to: Option<&str>, items: &serde_json::Value, timeout_ms: u64) -> anyhow::Result<()> {
        crate::pageops::storage_in(self, to, items, timeout_ms)
    }
    fn fetch(&self, to: Option<&str>, url: &str, opts: &serde_json::Value, timeout_ms: u64) -> anyhow::Result<String> {
        crate::pageops::fetch(self, to, url, opts, timeout_ms)
    }

    fn open_child(
        &self,
        name: &str,
        url: &str,
        rect: (i32, i32, i32, i32),
        profile: BrowserProfile,
    ) -> anyhow::Result<()> {
        self.inner.tell(&Ask::Open {
            name: name.to_string(),
            url: url.to_string(),
            rect,
            profile,
        })
    }

    fn child_bounds(&self, name: &str, rect: (i32, i32, i32, i32)) -> anyhow::Result<()> {
        self.inner.tell(&Ask::Bounds { name: name.to_string(), rect })
    }

    fn close_child(&self, name: &str) -> anyhow::Result<()> {
        self.inner.tell(&Ask::Close { name: name.to_string() })
    }

    fn trust(&self, url: &str) -> anyhow::Result<()> {
        self.inner.tell(&Ask::Trust { url: url.to_string() })
    }

    fn record(&self, to: Option<&str>, on: bool) -> anyhow::Result<()> {
        self.inner.tell(&Ask::Record { to: to.map(str::to_string), on })
    }

    fn record_all_off(&self) {
        let _ = self.inner.tell(&Ask::RecordAllOff);
    }
}

// ── the other end ─────────────────────────────────────────────────────────

/// Do what was asked, here, and say what happened.
///
/// This runs on the machine with the browser. What it is handed is that
/// machine's own browser -- the window's -- and every ask lands on the very
/// methods a script running there would have called.
pub fn perform<B: BrowserHost + Speaks>(ask: &Ask, browser: &B) -> (bool, serde_json::Value) {
    fn done(r: anyhow::Result<()>) -> anyhow::Result<serde_json::Value> {
        r.map(|()| serde_json::Value::Null)
    }
    let out = match ask {
        Ask::Cdp { to, method, params, ms } => {
            browser.cdp(to.as_deref(), method, params.clone(), *ms)
        }
        Ask::Eval { to, js, ms } => browser
            .eval(to.as_deref(), js, *ms)
            .map(serde_json::Value::String),
        Ask::Run { to, js } => browser
            .eval_in(to.as_deref(), js)
            .map(|id| serde_json::Value::from(id)),
        Ask::Open { name, url, rect, profile } => {
            done(browser.open_child(name, url, *rect, profile.clone()))
        }
        Ask::Bounds { name, rect } => done(browser.child_bounds(name, *rect)),
        Ask::Close { name } => done(browser.close_child(name)),
        Ask::Go { to, go } => done(browser.go(to.as_deref(), go.clone())),
        Ask::Focus { to } => done(browser.focus(to.as_deref())),
        Ask::Where { to } => done(browser.ask_where(to.as_deref())),
        Ask::Auth { to, user, pass } => done(browser.basic_auth(to.as_deref(), user, pass)),
        Ask::Inject { to, input } => done(browser.inject(to.as_deref(), input.clone())),
        Ask::Record { to, on } => done(browser.record(to.as_deref(), *on)),
        Ask::RecordAllOff => {
            browser.record_all_off();
            Ok(serde_json::Value::Null)
        }
        Ask::Trust { url } => done(browser.trust(url)),
    };
    match out {
        Ok(value) => (true, value),
        Err(e) => (false, serde_json::Value::String(e.to_string())),
    }
}

/// Join a board as a device, and come away with what a device is known by.
///
/// The key a board hands out at pairing is kept, under the address it came
/// from, and offered again next time. Without that, every launch would add a
/// row to the person's list of devices -- the same laptop, over and over,
/// none of which they could tell apart.
pub fn join(base: &str, token: &str) -> anyhow::Result<String> {
    let known = keys();
    // Asked for every time even when the key is already known: what the key
    // buys is not being written down as a new device, and what this asks for
    // is a session -- the thing a board takes away when somebody presses
    // disconnect, and the thing every line below is checked against
    let mut call = crate::update::agent(std::time::Duration::from_secs(30))
        .get(&format!("{}/?t={token}", base.trim_end_matches('/')));
    if let Some(had) = known.get(base) {
        call = call.header("Cookie", &format!("rk={had}"));
    }
    let answer = call.call()?;
    let mut key = known.get(base).cloned().unwrap_or_default();
    let mut session = String::new();
    for line in answer.headers().get_all("set-cookie") {
        let Ok(said) = line.to_str() else { continue };
        let one = said.split(';').next().unwrap_or_default();
        if let Some(v) = one.strip_prefix("rk=") {
            key = v.to_string();
        }
        if let Some(v) = one.strip_prefix("rs=") {
            session = v.to_string();
        }
    }
    if session.is_empty() {
        anyhow::bail!(crate::i18n::t("err.far.not_let_in"));
    }
    if !key.is_empty() {
        let mut all = known;
        all.insert(base.to_string(), key.clone());
        let _ = crate::crypto::write_atomic(
            &crate::config::state_path(KEYS),
            &serde_json::to_string_pretty(&all).unwrap_or_default(),
        );
        return Ok(format!("rk={key}; rs={session}"));
    }
    Ok(format!("rs={session}"))
}

/// The file where the keys boards have handed this device live.
const KEYS: &str = "far-keys.json";

fn keys() -> HashMap<String, String> {
    std::fs::read_to_string(crate::config::state_path(KEYS))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Draw a board's pages here, until the line drops.
///
/// Opens the two lines, and from then on every ask that arrives is performed
/// against the browser on this machine and answered. Reports from the pages go
/// back the same way, so the runtime learns that a page finished loading
/// exactly as it would from a page of its own.
///
/// Returns when the line closes, which is what a client thread wants: it can
/// try again, or let go.
pub fn draw_for<B: BrowserHost + Speaks>(
    base: &str,
    token: &str,
    cookie: &str,
    browser: &B,
    reports: impl Fn() -> Vec<Ev> + Send + 'static,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<()> {
    let name = crate::random_hex(10);
    let asks = handshake(base, &format!("/ws-page?p={name}&t={token}"), cookie)?;
    let back = handshake(base, &format!("/ws-page-in?p={name}&t={token}"), cookie)?;

    // What goes back: answers from this thread, and reports from wherever the
    // pages are noticed. One writer, because a socket has one
    let (tx, rx) = channel::<String>();
    let saying = std::sync::Arc::new(Mutex::new(back));
    let writer = std::sync::Arc::clone(&saying);
    std::thread::Builder::new()
        .name("shikisha-pages-up".into())
        .spawn(move || {
            while let Ok(text) = rx.recv() {
                let framed = crate::ws::client_encode(crate::ws::Op::Text, text.as_bytes());
                let mut out = writer.lock().unwrap_or_else(|e| e.into_inner());
                if std::io::Write::write_all(&mut *out, &framed).is_err() {
                    break;
                }
                let _ = std::io::Write::flush(&mut *out);
            }
        })?;

    // Whatever the pages here have said, on the way past
    let telling = tx.clone();
    let watching = std::thread::Builder::new()
        .name("shikisha-pages-report".into())
        .spawn({
            let stop = std::sync::Arc::clone(&stop);
            move || {
                while !stop.load(Ordering::Relaxed) {
                    for ev in reports() {
                        let said = Said::Report { ev };
                        if let Ok(text) = serde_json::to_string(&said) {
                            if telling.send(text).is_err() {
                                return;
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(80));
                }
            }
        })?;

    let mut asks = asks;
    let mut whole = Vec::new();
    loop {
        let Some((fin, opcode, payload)) = crate::ws::read_server_frame(&mut asks) else { break };
        match opcode {
            0x0 | 0x1 | 0x2 => {
                whole.extend_from_slice(&payload);
                if !fin {
                    continue;
                }
                let message = std::mem::take(&mut whole);
                let Ok(v) = serde_json::from_slice::<serde_json::Value>(&message) else { continue };
                let Some(id) = v.get("id").and_then(serde_json::Value::as_u64) else { continue };
                let Ok(ask) = serde_json::from_value::<Ask>(v["ask"].clone()) else {
                    continue;
                };
                let (ok, value) = perform(&ask, browser);
                let said = Said::Answer { id, ok, value };
                if let Ok(text) = serde_json::to_string(&said) {
                    if tx.send(text).is_err() {
                        break;
                    }
                }
            }
            0x8 => break,
            _ => {}
        }
    }
    stop.store(true, Ordering::Relaxed);
    drop(tx);
    let _ = watching.join();
    Ok(())
}

/// Open a WebSocket to a board, by hand.
fn handshake(base: &str, path: &str, cookie: &str) -> anyhow::Result<std::net::TcpStream> {
    crate::tunnel::handshake(base, path, cookie)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A browser that answers everything with the same thing, and writes down
    /// what it was asked.
    struct Stub {
        asked: Mutex<Vec<String>>,
        refs: Mutex<HashMap<Option<String>, Vec<i64>>>,
    }

    impl Speaks for Stub {
        fn cdp(
            &self,
            to: Option<&str>,
            method: &str,
            _params: serde_json::Value,
            _ms: u64,
        ) -> anyhow::Result<serde_json::Value> {
            self.asked
                .lock()
                .unwrap()
                .push(format!("cdp {} {method}", to.unwrap_or("-")));
            Ok(serde_json::json!({ "data": "こんにちは" }))
        }
        fn eval(&self, to: Option<&str>, js: &str, _ms: u64) -> anyhow::Result<String> {
            self.asked.lock().unwrap().push(format!("eval {} {js}", to.unwrap_or("-")));
            Ok("\"visible\"".to_string())
        }
        fn refs(&self) -> &Mutex<HashMap<Option<String>, Vec<i64>>> {
            &self.refs
        }
    }

    /// The line, wired to a stub that performs what arrives.
    fn wired() -> (Far, Arc<Stub>) {
        let far = Far::new();
        let line = far.line();
        let stub = Arc::new(Stub {
            asked: Mutex::new(Vec::new()),
            refs: Mutex::new(HashMap::new()),
        });
        let (tx, rx) = channel::<String>();
        line.attach(tx, "台所のノート");
        let answering = Arc::clone(&stub);
        let back = line.clone();
        std::thread::spawn(move || {
            while let Ok(text) = rx.recv() {
                let v: serde_json::Value = serde_json::from_str(&text).unwrap();
                let id = v["id"].as_u64().unwrap();
                let ask: Ask = serde_json::from_value(v["ask"].clone()).unwrap();
                // Only the two that a stub can answer; the rest are placement
                let (ok, value) = match &ask {
                    Ask::Cdp { to, method, params, ms } => {
                        let r = answering.cdp(to.as_deref(), method, params.clone(), *ms);
                        (r.is_ok(), r.unwrap_or(serde_json::Value::Null))
                    }
                    Ask::Eval { to, js, ms } => {
                        let r = answering.eval(to.as_deref(), js, *ms);
                        (true, serde_json::Value::String(r.unwrap_or_default()))
                    }
                    _ => (true, serde_json::Value::Null),
                };
                back.heard(&serde_json::to_string(&Said::Answer { id, ok, value }).unwrap());
            }
        });
        (far, stub)
    }

    /// Looking at a page over there runs the same code as looking at one here:
    /// what crosses the line is the protocol call, not the operation
    #[test]
    fn an_operation_travels_as_the_two_moves_it_was_built_from() {
        let (far, stub) = wired();
        let found = far
            .find(Some("ws/page"), &Sel::Css("#here".into()), 2_000)
            .expect("答えが返らない");
        assert_eq!(found, Found::Visible);
        let asked = stub.asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1, "1つの操作が何度も渡っている: {asked:?}");
        assert!(
            asked[0].starts_with("eval ws/page") && asked[0].contains("__shikisha_state"),
            "ページの助けを呼んでいない: {asked:?}"
        );
    }

    /// A picture of the page is the browser's own, wherever the browser is
    #[test]
    fn a_picture_is_taken_by_the_browser_that_has_the_page() {
        let (far, stub) = wired();
        let png = far.snapshot(Some("ws/page"), 2_000);
        assert!(png.is_err() || png.is_ok(), "撮れたか撮れないかのどちらかである");
        let asked = stub.asked.lock().unwrap().clone();
        assert!(
            asked.iter().any(|a| a.contains("Page.captureScreenshot")),
            "向こうのブラウザに撮らせていない: {asked:?}"
        );
    }

    /// Nobody there is not the same as a browser that said no
    #[test]
    fn with_nobody_there_it_says_so_at_once() {
        let far = Far::new();
        assert!(!far.here());
        let before = std::time::Instant::now();
        let err = far.href(Some("ws/page"), 30_000).unwrap_err().to_string();
        assert!(!err.is_empty());
        assert!(
            before.elapsed() < std::time::Duration::from_secs(2),
            "誰も居ないのに待っている: {:?}",
            before.elapsed()
        );
    }

    /// A client that walks away mid-operation is news now, not in thirty
    /// seconds
    #[test]
    fn a_client_that_leaves_answers_everything_it_was_holding() {
        let far = Far::new();
        let line = far.line();
        let (tx, rx) = channel::<String>();
        line.attach(tx, "ノート");
        let leaving = line.clone();
        std::thread::spawn(move || {
            let _ = rx.recv();
            std::thread::sleep(std::time::Duration::from_millis(100));
            leaving.detach();
        });
        let before = std::time::Instant::now();
        assert!(far.href(Some("ws/page"), 30_000).is_err(), "居ないのに答えている");
        assert!(
            before.elapsed() < std::time::Duration::from_secs(3),
            "去ったのに待ち続けている: {:?}",
            before.elapsed()
        );
    }

    /// The line carries reports one way and nothing else: a client cannot ask
    /// for anything through it
    #[test]
    fn a_far_page_may_report_and_may_not_ask() {
        let far = Far::new();
        let line = far.line();
        let say = |ev: &Ev| {
            line.heard(&serde_json::to_string(&Said::Report { ev: ev.clone() }).unwrap());
        };
        say(&Ev::Ready {
            from: Some("ws/page".into()),
            url: "https://example.com/".into(),
            complete: true,
        });
        say(&Ev::Say { tab: 0, text: "rm -rf /".into() });
        say(&Ev::Where {
            from: Some("ws/page".into()),
            url: "https://example.com/".into(),
            can_back: true,
            can_forward: false,
        });
        let heard = far.drain();
        assert_eq!(heard.len(), 2, "通してはいけないものが通った: {heard:?}");
        assert!(matches!(heard[0], Ev::Ready { .. }));
        assert!(matches!(heard[1], Ev::Where { .. }));
        assert!(far.drain().is_empty(), "2度読める");
    }

    /// A browser that answers, and can be told what to have reported.
    struct Drawn {
        stub: Arc<Stub>,
        /// Pages it says it has opened
        open: Mutex<Vec<String>>,
    }

    impl Speaks for Drawn {
        fn cdp(&self, to: Option<&str>, method: &str, params: serde_json::Value, ms: u64) -> anyhow::Result<serde_json::Value> {
            self.stub.cdp(to, method, params, ms)
        }
        fn eval(&self, to: Option<&str>, js: &str, ms: u64) -> anyhow::Result<String> {
            self.stub.eval(to, js, ms)
        }
        fn refs(&self) -> &Mutex<HashMap<Option<String>, Vec<i64>>> {
            self.stub.refs()
        }
    }

    impl BrowserHost for Drawn {
        fn open_child(&self, name: &str, _url: &str, _rect: (i32, i32, i32, i32), _p: BrowserProfile) -> anyhow::Result<()> {
            self.open.lock().unwrap().push(name.to_string());
            Ok(())
        }
        fn child_bounds(&self, _n: &str, _r: (i32, i32, i32, i32)) -> anyhow::Result<()> { Ok(()) }
        fn close_child(&self, _n: &str) -> anyhow::Result<()> { Ok(()) }
        fn trust(&self, _u: &str) -> anyhow::Result<()> { Ok(()) }
        fn record_all_off(&self) {}
        fn record(&self, _t: Option<&str>, _o: bool) -> anyhow::Result<()> { Ok(()) }
        fn go(&self, _t: Option<&str>, _g: Go) -> anyhow::Result<()> { Ok(()) }
        fn focus(&self, _t: Option<&str>) -> anyhow::Result<()> { Ok(()) }
        fn ask_where(&self, _t: Option<&str>) -> anyhow::Result<()> { Ok(()) }
        fn basic_auth(&self, _t: Option<&str>, _u: &str, _p: &str) -> anyhow::Result<()> { Ok(()) }
        fn eval_in(&self, _t: Option<&str>, _js: &str) -> anyhow::Result<u64> { Ok(1) }
        fn inject(&self, _t: Option<&str>, _i: Input) -> anyhow::Result<()> { Ok(()) }
        fn screencast(&self, _t: Option<&str>, _o: bool) -> anyhow::Result<()> { Ok(()) }
        fn find(&self, to: Option<&str>, sel: &Sel, ms: u64) -> anyhow::Result<Found> {
            crate::pageops::find(self, to, sel, ms)
        }
        fn click(&self, to: Option<&str>, sel: &Sel, ms: u64) -> anyhow::Result<OpReport> {
            crate::pageops::click(self, to, sel, ms)
        }
        fn fill(&self, to: Option<&str>, sel: &Sel, v: &str, ms: u64) -> anyhow::Result<OpReport> {
            crate::pageops::fill(self, to, sel, v, ms)
        }
        fn text(&self, to: Option<&str>, sel: &Sel, ms: u64) -> anyhow::Result<Option<String>> {
            crate::pageops::text(self, to, sel, ms)
        }
        fn href(&self, to: Option<&str>, ms: u64) -> anyhow::Result<String> {
            crate::pageops::href(self, to, ms)
        }
        fn html(&self, to: Option<&str>, ms: u64) -> anyhow::Result<String> {
            crate::pageops::html(self, to, ms)
        }
        fn digest(&self, to: Option<&str>, ms: u64) -> anyhow::Result<String> {
            crate::pageops::digest(self, to, ms)
        }
        fn snapshot(&self, to: Option<&str>, ms: u64) -> anyhow::Result<Vec<u8>> {
            crate::pageops::snapshot(self, to, ms)
        }
        fn cookies_out(&self, to: Option<&str>, ms: u64) -> anyhow::Result<serde_json::Value> {
            crate::pageops::cookies_out(self, to, ms)
        }
        fn cookies_in(&self, to: Option<&str>, c: &serde_json::Value, ms: u64) -> anyhow::Result<()> {
            crate::pageops::cookies_in(self, to, c, ms)
        }
        fn storage_out(&self, to: Option<&str>, ms: u64) -> anyhow::Result<serde_json::Value> {
            crate::pageops::storage_out(self, to, ms)
        }
        fn storage_in(&self, to: Option<&str>, i: &serde_json::Value, ms: u64) -> anyhow::Result<()> {
            crate::pageops::storage_in(self, to, i, ms)
        }
        fn fetch(&self, to: Option<&str>, u: &str, o: &serde_json::Value, ms: u64) -> anyhow::Result<String> {
            crate::pageops::fetch(self, to, u, o, ms)
        }
    }

    /// The whole line, through the real board: this machine opens a page on
    /// the device, drives it, and hears what the page reported -- over two
    /// sockets, a device book and a session, exactly as a laptop would.
    #[test]
    fn a_device_draws_the_pages_and_this_machine_drives_them() {
        let _book = crate::clients::tests::OwnBook::new();
        let far = Far::new();
        let ui = crate::remote::RemoteUi::start(
            "127.0.0.1".parse().unwrap(),
            0,
            "tok123456789012".into(),
            String::new(),
        )
        .unwrap();
        ui.set_page_line(far.line());
        let base = ui.url.split("/?").next().unwrap().trim_end_matches('/').to_string();

        // The device end: join, then answer whatever is asked
        let cookie = join(&base, "tok123456789012").expect("入れてもらえない");
        let drawn = Arc::new(Drawn {
            stub: Arc::new(Stub { asked: Mutex::new(Vec::new()), refs: Mutex::new(HashMap::new()) }),
            open: Mutex::new(Vec::new()),
        });
        let telling = Arc::new(Mutex::new(Vec::<Ev>::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        std::thread::spawn({
            let (base, cookie, drawn, stop) = (base.clone(), cookie.clone(), Arc::clone(&drawn), Arc::clone(&stop));
            let telling = Arc::clone(&telling);
            move || {
                let _ = draw_for(
                    &base,
                    "tok123456789012",
                    &cookie,
                    drawn.as_ref(),
                    move || std::mem::take(&mut *telling.lock().unwrap()),
                    stop,
                );
            }
        });

        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !far.here() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(far.here(), "端末が線を張れていない");

        // Placing a page happens over there
        far.open_child("ws/p", "https://example.com/", (0, 0, 900, 700), BrowserProfile::shared_default())
            .expect("向こうでページが開かない");
        assert_eq!(drawn.open.lock().unwrap().clone(), vec!["ws/p".to_string()]);

        // ...and driving it runs here, against that browser
        assert_eq!(
            far.find(Some("ws/p"), &Sel::Css("#here".into()), 3_000).expect("答えが返らない"),
            Found::Visible
        );
        let asked = drawn.stub.asked.lock().unwrap().clone();
        assert!(
            asked.iter().any(|a| a.contains("__shikisha_state")),
            "ページの助けを呼んでいない: {asked:?}"
        );

        // What a page over there reports arrives here
        telling.lock().unwrap().push(Ev::Ready {
            from: Some("ws/p".into()),
            url: "https://example.com/".into(),
            complete: true,
        });
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut got = Vec::new();
        while got.is_empty() && std::time::Instant::now() < until {
            got = far.drain();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            matches!(got.first(), Some(Ev::Ready { from: Some(n), .. }) if n == "ws/p"),
            "向こうのページの報告が届いていない: {got:?}"
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        ui.shutdown();
    }

    /// Every ask survives being written down and read back
    #[test]
    fn an_ask_is_the_same_ask_at_the_other_end() {
        let asks = [
            Ask::Cdp {
                to: Some("ws/p".into()),
                method: "Page.navigate".into(),
                params: serde_json::json!({"url": "https://example.com/あ"}),
                ms: 1_000,
            },
            Ask::Go { to: None, go: Go::To("https://example.com/".into()) },
            Ask::Inject {
                to: Some("ws/p".into()),
                input: Input::Key { named: "enter".into(), ctrl: true, alt: false },
            },
            Ask::Open {
                name: "ws/p".into(),
                url: "https://example.com/".into(),
                rect: (0, 0, 900, 700),
                profile: BrowserProfile::new("仕事", false),
            },
        ];
        for ask in &asks {
            let text = serde_json::to_string(ask).unwrap();
            let back: Ask = serde_json::from_str(&text).unwrap();
            assert_eq!(format!("{ask:?}"), format!("{back:?}"), "往復で変わった");
        }
    }
}

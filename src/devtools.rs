//! A page's DevTools, opened as a page of its own.
//!
//! WebView2 carries the browser's own DevTools screen
//! (`devtools://devtools/bundled/devtools_app.html`). Given `?ws=<address>` it
//! talks the DevTools protocol over that WebSocket -- so this is the other end:
//! a WebSocket on the loopback that carries each message to the inspected page
//! and each answer and event back.
//!
//! Not the browser's own remote-debugging port. That port lets any program on
//! this machine drive every page, the signed-in ones included, and it cannot be
//! opened for one page alone. This one needs a key made at each start, is only
//! answered for a page this window has, and only to the DevTools screen itself
//! (the `Origin` it connects from).
//!
//! How a message reaches the page: the page's own protocol session attaches a
//! second session to the page (`Target.attachToTarget`) and speaks through it
//! (`Target.sendMessageToTarget`); everything that session says comes back as
//! one event (`Target.receivedMessageFromTarget`). That keeps the DevTools
//! screen's session apart from the one automation uses, and needs no list of
//! the events a screen of some other version might want.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};

use shikisha_core::ws::{self, Op};

use crate::browser::Cmd;

/// Where the bridge answers, and the key a connection has to name
pub struct Bridge {
    pub port: u16,
    pub key: String,
}

/// Numbers the connections, so the window can tell whose answer is whose
static NEXT: AtomicU64 = AtomicU64::new(1);

/// The one origin a connection is answered from
const SCREEN_ORIGIN: &str = "devtools://devtools";

/// The most the DevTools screen may say in one message. What it sends are
/// commands -- a node to inspect, an expression to run -- and a command larger
/// than this is a script pasted into the console, which still fits
const ONE_COMMAND: usize = 8 * 1024 * 1024;

impl Bridge {
    /// Start answering on a port of the system's choosing. `tell` carries a
    /// connection's messages to the window's loop
    pub fn start(tell: tao::event_loop::EventLoopProxy<Cmd>) -> std::io::Result<Bridge> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        // 16 bytes: the key only has to outlast one run of the app, and
        // guessing it is a round trip to this machine per try
        let key = shikisha_core::random_hex(16);
        let want = key.clone();
        std::thread::Builder::new()
            .name("devtools-bridge".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let tell = tell.clone();
                    let want = want.clone();
                    let _ = std::thread::Builder::new()
                        .name("devtools-line".into())
                        .spawn(move || serve(stream, &want, tell));
                }
            })?;
        Ok(Bridge { port, key })
    }

    /// Whether an address is one of this bridge's screens
    pub fn made(&self, url: &str) -> bool {
        url.starts_with(&format!(
            "devtools://devtools/bundled/devtools_app.html?ws=127.0.0.1:{}/devtools/{}/",
            self.port, self.key
        ))
    }

    /// The address the DevTools screen is opened at for page `to`
    pub fn screen_url(&self, to: Option<&str>) -> String {
        format!(
            "devtools://devtools/bundled/devtools_app.html?ws=127.0.0.1:{}/devtools/{}/{}",
            self.port,
            self.key,
            hex(to.unwrap_or_default())
        )
    }
}

/// A page's in-window name as a path segment. Hex, because a name carries a
/// `/` and the screen passes the address on as it was given
fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<String> {
    if s.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect();
    String::from_utf8(bytes?).ok()
}

/// What a request asked for: the page, when the key, the path and the origin
/// are all right, and nothing otherwise
fn admitted(path: &str, origin: &str, key: &str) -> Option<String> {
    let rest = path.strip_prefix("/devtools/")?;
    let (given, page) = rest.split_once('/')?;
    if !shikisha_core::crypto::token_eq(given, key) || origin != SCREEN_ORIGIN {
        return None;
    }
    unhex(page).filter(|p| !p.is_empty())
}

fn serve(stream: TcpStream, key: &str, tell: tao::event_loop::EventLoopProxy<Cmd>) {
    let Ok(read_side) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read_side);
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let path = first.split_whitespace().nth(1).unwrap_or_default().to_string();
    let (mut origin, mut client_key) = (String::new(), String::new());
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "origin" => origin = v.trim().to_string(),
                "sec-websocket-key" => client_key = v.trim().to_string(),
                _ => {}
            }
        }
    }
    let mut out = stream;
    let Some(page) = admitted(&path, &origin, key).filter(|_| !client_key.is_empty()) else {
        // Nothing about why: a caller that has no business here learns nothing
        let _ = out.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        // The address carries the key, so only the page is written down
        shikisha_core::append_hook_log("[devtools] refused a connection");
        return;
    };
    let hello = format!(
        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
        ws::accept_key(&client_key)
    );
    if out.write_all(hello.as_bytes()).is_err() {
        return;
    }
    let conn = NEXT.fetch_add(1, Ordering::Relaxed);
    shikisha_core::append_hook_log(&format!("[devtools] screen {conn} opened on page '{page}'"));
    // What the page says goes out on a writer of its own, so a page talking
    // a great deal never waits on the screen typing
    let (said_tx, said_rx) = channel::<String>();
    let Ok(write_side) = out.try_clone() else { return };
    std::thread::spawn(move || {
        let mut w = ws::WsWriter::new(write_side);
        for text in said_rx {
            if w.send_text(&text).is_err() {
                break;
            }
        }
    });
    let _ = tell.send_event(Cmd::DevtoolsOpen { conn, to: Some(page.clone()), out: said_tx.clone() });
    let mut pong = ws::WsWriter::new(out);
    loop {
        match ws::read_frame_upto(&mut reader, ONE_COMMAND) {
            Ok((Op::Text, body)) => {
                let Ok(text) = String::from_utf8(body) else { continue };
                if tell.send_event(Cmd::DevtoolsSay { conn, text }).is_err() {
                    break;
                }
            }
            Ok((Op::Ping, body)) => {
                let _ = pong.send_pong(&body);
            }
            Ok((Op::Close, _)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = tell.send_event(Cmd::DevtoolsClose { conn });
    drop(said_tx);
    shikisha_core::append_hook_log(&format!("[devtools] screen {conn} closed"));
}

/// The window's half: which session each screen speaks through, and the
/// messages it said before that session existed. Spoken through the page's
/// own DevTools protocol, which is the engine's (WebView2 on Windows)
#[cfg(windows)]
#[derive(Default)]
pub struct Screens {
    open: std::collections::HashMap<u64, Screen>,
}

/// A screen's session, and whether the screen has gone. Both halves of the
/// attach answer later, on the window's thread, and the screen can be closed
/// in between: a session that arrives for a screen already gone must be let
/// go at once, or it stays attached to the page with nobody to detach it
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Default)]
struct Attach {
    session: Option<String>,
    closed: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl Attach {
    /// The page answered with a session. `Some` is a session to let go of
    /// straight away: its screen closed while it was on its way
    fn attached(&mut self, sid: String) -> Option<String> {
        if self.closed {
            return Some(sid);
        }
        self.session = Some(sid);
        None
    }

    /// The screen went away. `Some` is the session to let go of, when there
    /// already is one; one still on its way is let go of when it arrives
    fn close(&mut self) -> Option<String> {
        self.closed = true;
        self.session.take()
    }
}

#[cfg(windows)]
struct Screen {
    to: Option<String>,
    /// The session attached for this screen, once the page has said which
    session: std::rc::Rc<std::cell::RefCell<Attach>>,
    /// Said before the session was there, in order
    waiting: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    heard: Option<(webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
    webview: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
}

#[cfg(windows)]
impl Screens {
    /// A screen connected for a page: attach a session of its own, and send
    /// it everything that session says
    pub fn open(
        &mut self,
        conn: u64,
        to: Option<String>,
        webview: webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2,
        out: Sender<String>,
    ) {
        let session = std::rc::Rc::new(std::cell::RefCell::new(Attach::default()));
        let waiting = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let mine = std::rc::Rc::clone(&session);
        let heard = crate::browser::cdp::listen(&webview, "Target.receivedMessageFromTarget", move |v| {
            let from = v.get("sessionId").and_then(|x| x.as_str());
            if from.is_some() && from == mine.borrow().session.as_deref()
                && let Some(m) = v.get("message").and_then(|x| x.as_str())
            {
                let _ = out.send(m.to_string());
            }
        });
        let wv = webview.clone();
        let (slot, queue) = (std::rc::Rc::clone(&session), std::rc::Rc::clone(&waiting));
        crate::browser::cdp::call_result(&webview, "Target.getTargetInfo", "{}", move |ok, json| {
            let target = ok
                .then(|| serde_json::from_str::<serde_json::Value>(&json).ok())
                .flatten()
                .and_then(|v| v.pointer("/targetInfo/targetId").and_then(|x| x.as_str()).map(str::to_string));
            let Some(target) = target else {
                shikisha_core::append_hook_log(&format!("[devtools] screen {conn}: the page did not say which it is ({json})"));
                return;
            };
            // Closed while the page was being asked: nothing to attach for
            if slot.borrow().closed {
                return;
            }
            let params = serde_json::json!({"targetId": target, "flatten": false}).to_string();
            let wv2 = wv.clone();
            crate::browser::cdp::call_result(&wv, "Target.attachToTarget", &params, move |ok, json| {
                let sid = ok
                    .then(|| serde_json::from_str::<serde_json::Value>(&json).ok())
                    .flatten()
                    .and_then(|v| v.get("sessionId").and_then(|x| x.as_str()).map(str::to_string));
                let Some(sid) = sid else {
                    shikisha_core::append_hook_log(&format!("[devtools] screen {conn}: could not attach ({json})"));
                    return;
                };
                let late = slot.borrow_mut().attached(sid.clone());
                if let Some(late) = late {
                    detach(&wv2, &late);
                    queue.borrow_mut().clear();
                    shikisha_core::append_hook_log(&format!("[devtools] screen {conn}: attached after it closed, let go"));
                    return;
                }
                for text in queue.borrow_mut().drain(..) {
                    say(&wv2, &sid, &text);
                }
            });
        });
        self.open.insert(conn, Screen { to, session, waiting, heard, webview });
    }

    /// What the screen said, on to its session (or kept until it has one)
    pub fn say(&mut self, conn: u64, text: String) {
        let Some(s) = self.open.get(&conn) else { return };
        let sid = s.session.borrow().session.clone();
        match sid {
            Some(sid) => say(&s.webview, &sid, &text),
            None => s.waiting.borrow_mut().push(text),
        }
    }

    /// The screen went away: its session goes with it
    pub fn close(&mut self, conn: u64) {
        if let Some(s) = self.open.remove(&conn) {
            s.let_go();
        }
    }

    /// The page closed: every screen on it goes
    pub fn page_closed(&mut self, to: &Option<String>) {
        let gone: Vec<u64> = self.open.iter().filter(|(_, s)| &s.to == to).map(|(c, _)| *c).collect();
        for c in gone {
            self.close(c);
        }
    }
}

#[cfg(windows)]
impl Screen {
    fn let_go(self) {
        let sid = self.session.borrow_mut().close();
        if let Some(sid) = sid {
            detach(&self.webview, &sid);
        }
        if let Some(h) = &self.heard {
            crate::browser::cdp::unlisten(h);
        }
    }
}

#[cfg(windows)]
fn detach(webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2, sid: &str) {
    crate::browser::cdp::call(webview, "Target.detachFromTarget", &serde_json::json!({"sessionId": sid}).to_string());
}

#[cfg(windows)]
fn say(webview: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2, sid: &str, text: &str) {
    let params = serde_json::json!({"sessionId": sid, "message": text}).to_string();
    crate::browser::cdp::call(webview, "Target.sendMessageToTarget", &params);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Closed before the page answered: the session that arrives after is
    /// handed back to be let go of, not kept -- before, it was kept by a
    /// screen nobody had any more and stayed attached to the page
    #[test]
    fn a_session_that_arrives_after_its_screen_closed_is_let_go() {
        let mut early = Attach::default();
        assert_eq!(early.close(), None, "nothing attached yet, nothing to let go of");
        assert_eq!(early.attached("s1".into()), Some("s1".into()), "a late session was kept");
        assert_eq!(early.session, None);

        let mut usual = Attach::default();
        assert_eq!(usual.attached("s2".into()), None);
        assert_eq!(usual.session.as_deref(), Some("s2"));
        assert_eq!(usual.close(), Some("s2".into()), "closing did not let go of its session");
    }

    #[test]
    fn a_page_name_survives_the_address() {
        assert_eq!(unhex(&hex("0/page one")).as_deref(), Some("0/page one"));
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex("abc"), None);
    }

    #[test]
    fn only_the_screen_with_the_key_is_let_in() {
        let path = format!("/devtools/k3y/{}", hex("0/page"));
        assert_eq!(admitted(&path, SCREEN_ORIGIN, "k3y").as_deref(), Some("0/page"));
        assert_eq!(admitted(&path, SCREEN_ORIGIN, "other"), None, "a wrong key");
        assert_eq!(admitted(&path, "http://127.0.0.1:9", "k3y"), None, "a page of the web");
        assert_eq!(admitted(&path, "", "k3y"), None, "no origin at all");
        assert_eq!(admitted("/devtools/k3y/", SCREEN_ORIGIN, "k3y"), None, "no page");
        assert_eq!(admitted("/json/list", SCREEN_ORIGIN, "k3y"), None);
    }
}

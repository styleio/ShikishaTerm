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
//! opened for one page alone. This one needs a key made for the page the
//! screen was asked for, is only answered for a page this window has, only to
//! the DevTools screen itself (the `Origin` it connects from), and passes on
//! only what stays with that page (`shikisha_core::devscreen`).
//!
//! How a message reaches the page: the page's own protocol session attaches a
//! second session to the page (`Target.attachToTarget`) and speaks through it
//! (`Target.sendMessageToTarget`); everything that session says comes back as
//! one event (`Target.receivedMessageFromTarget`). That keeps the DevTools
//! screen's session apart from the one automation uses, and needs no list of
//! the events a screen of some other version might want.

use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};


use shikisha_core::ws::{self, Op};

use crate::browser::Cmd;

/// Where the bridge answers, and the key a connection has to name for each
/// page a screen has been asked for. A key opens its own page and no other,
/// so the address of one tab's screen is not a way into the next tab
pub struct Bridge {
    pub port: u16,
    keys: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
}

/// Numbers the connections, so the window can tell whose answer is whose
static NEXT: AtomicU64 = AtomicU64::new(1);

use shikisha_core::devscreen::{BEHIND, EARLY, ONE_COMMAND, ORIGIN as SCREEN_ORIGIN, STALLED};

impl Bridge {
    /// Start answering on a port of the system's choosing. `tell` carries a
    /// connection's messages to the window's loop
    pub fn start(tell: tao::event_loop::EventLoopProxy<Cmd>) -> std::io::Result<Bridge> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let keys: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>> = Default::default();
        let want = std::sync::Arc::clone(&keys);
        let serving = shikisha_core::devscreen::Serving::default();
        std::thread::Builder::new()
            .name("devtools-bridge".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    // Anybody on the machine can knock. One that knocks and
                    // says nothing is not waited on, and past so many at
                    // once the rest are not answered at all
                    let Some(served) = serving.enter() else { continue };
                    let _ = stream.set_read_timeout(Some(ws::OPENING_WITHIN));
                    let tell = tell.clone();
                    let want = std::sync::Arc::clone(&want);
                    let _ = std::thread::Builder::new().name("devtools-line".into()).spawn(move || {
                        let _served = served;
                        serve(stream, &want, tell);
                    });
                }
            })?;
        Ok(Bridge { port, keys })
    }

    /// What every screen's address begins with; the page's key and name follow
    fn prefix(&self) -> String {
        format!("devtools://devtools/bundled/devtools_app.html?ws=127.0.0.1:{}/devtools/", self.port)
    }

    /// Whether an address is one of this bridge's screens
    pub fn made(&self, url: &str) -> bool {
        let Some((key, page)) = url.strip_prefix(&self.prefix()).and_then(|rest| rest.split_once('/')) else { return false };
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).get(page).is_some_and(|k| shikisha_core::crypto::token_eq(k, key))
    }

    /// The address the DevTools screen is opened at for page `to`. Its key
    /// is made the first time it is asked for: 16 bytes, which only have to
    /// outlast one run of the app, and guessing them is a round trip to this
    /// machine per try
    pub fn screen_url(&self, to: Option<&str>) -> String {
        let page = hex(to.unwrap_or_default());
        let mut keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        let key = keys.entry(page.clone()).or_insert_with(|| shikisha_core::random_hex(16));
        format!("{}{key}/{page}", self.prefix())
    }
}

/// A page's in-window name as a path segment. Hex, because a name carries a
/// `/` and the screen passes the address on as it was given
fn hex(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<String> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect();
    String::from_utf8(bytes?).ok()
}

/// What a request asked for: the page, when its key, the path and the
/// origin are all right, and nothing otherwise. `keys` is each page's key,
/// by the page's name as the address spells it
fn admitted(path: &str, origin: &str, keys: &std::collections::HashMap<String, String>) -> Option<String> {
    let rest = path.strip_prefix("/devtools/")?;
    let (given, page) = rest.split_once('/')?;
    let keyed = keys.get(page).is_some_and(|key| shikisha_core::crypto::token_eq(given, key));
    if !keyed || origin != SCREEN_ORIGIN {
        return None;
    }
    unhex(page).filter(|p| !p.is_empty())
}

fn serve(stream: TcpStream, keys: &std::sync::Mutex<std::collections::HashMap<String, String>>, tell: tao::event_loop::EventLoopProxy<Cmd>) {
    let Ok(read_side) = stream.try_clone() else { return };
    let mut reader = BufReader::new(read_side);
    let Some(asked) = ws::read_opening(&mut reader) else { return };
    let mut out = stream;
    let page = admitted(&asked.path, &asked.origin, &keys.lock().unwrap_or_else(|e| e.into_inner()));
    let Some(page) = page.filter(|_| !asked.key.is_empty()) else {
        let _ = out.write_all(ws::TURNED_AWAY);
        // The address carries the key, so only the page is written down
        shikisha_core::append_hook_log("[devtools] refused a connection");
        return;
    };
    if out.write_all(ws::welcome(&asked.key).as_bytes()).is_err() {
        return;
    }
    // Let in: from here it is waited on for as long as it is open
    let _ = out.set_read_timeout(None);
    let conn = NEXT.fetch_add(1, Ordering::Relaxed);
    shikisha_core::append_hook_log(&format!("[devtools] screen {conn} opened on page '{page}'"));
    // What the page says goes out on a writer of its own, so a page talking
    // a great deal never waits on the screen typing -- and is let go of when
    // the screen has stopped reading and too much waits on it, which ends
    // the reading below
    let Ok(write_side) = out.try_clone() else { return };
    let _ = write_side.set_write_timeout(Some(STALLED));
    let Ok(whole) = out.try_clone() else { return };
    let said = ws::Lane::open(write_side, BEHIND, move || {
        let _ = whole.shutdown(std::net::Shutdown::Both);
    });
    let _ = tell.send_event(Cmd::DevtoolsOpen { conn, to: Some(page.clone()), out: said.clone() });
    // A long command comes in pieces, and is put together here
    let mut partial = ws::Partial::default();
    loop {
        match ws::read_message_upto(&mut reader, ONE_COMMAND, &mut partial) {
            Ok((Op::Text, body)) => {
                let Ok(text) = String::from_utf8(body) else { continue };
                // What reaches past the page is answered here and goes no
                // further. The sessions it names are the page's own: they
                // are inside the one session this screen speaks through
                match shikisha_core::devscreen::said(&text, None, &|_| true) {
                    shikisha_core::devscreen::Said::On => {
                        if tell.send_event(Cmd::DevtoolsSay { conn, text }).is_err() {
                            break;
                        }
                    }
                    shikisha_core::devscreen::Said::Instead(answer) => {
                        let _ = said.send_text(&answer);
                    }
                    shikisha_core::devscreen::Said::Dropped => {}
                }
            }
            Ok((Op::Ping, body)) => {
                let _ = said.send(ws::encode(Op::Pong, &body));
            }
            Ok((Op::Close, _)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    let _ = tell.send_event(Cmd::DevtoolsClose { conn });
    let why = if said.closed() { " (it had stopped reading)" } else { "" };
    drop(said);
    shikisha_core::append_hook_log(&format!("[devtools] screen {conn} closed{why}"));
}

/// The window's half: which session each screen speaks through, and the
/// messages it said before that session existed. Spoken through the page's
/// own DevTools protocol, which the engine carries (`browser::cdp::Port`)
#[derive(Default)]
pub struct Screens {
    open: std::collections::HashMap<u64, Screen>,
}

/// A screen's session, and whether the screen has gone. Both halves of the
/// attach answer later, on the window's thread, and the screen can be closed
/// in between: a session that arrives for a screen already gone must be let
/// go at once, or it stays attached to the page with nobody to detach it
#[derive(Debug, Default)]
struct Attach {
    session: Option<String>,
    closed: bool,
}

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

struct Screen {
    to: Option<String>,
    /// The session attached for this screen, once the page has said which
    session: std::rc::Rc<std::cell::RefCell<Attach>>,
    /// Said before the session was there, in order
    waiting: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    _heard: Option<crate::browser::cdp::Heard>,
    port: crate::browser::cdp::Port,
}

impl Screens {
    /// A screen connected for a page: attach a session of its own, and send
    /// it everything that session says
    pub fn open(
        &mut self,
        conn: u64,
        to: Option<String>,
        port: crate::browser::cdp::Port,
        out: ws::Lane,
    ) {
        let session = std::rc::Rc::new(std::cell::RefCell::new(Attach::default()));
        let waiting = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        let mine = std::rc::Rc::clone(&session);
        let heard = port.listen("Target.receivedMessageFromTarget", move |v| {
            let from = v.get("sessionId").and_then(|x| x.as_str());
            if from.is_some() && from == mine.borrow().session.as_deref()
                && let Some(m) = v.get("message").and_then(|x| x.as_str())
            {
                let _ = out.send_text(m);
            }
        });
        let wv = port.clone();
        let (slot, queue) = (std::rc::Rc::clone(&session), std::rc::Rc::clone(&waiting));
        port.call_result("Target.getTargetInfo", "{}", move |ok, json| {
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
            wv.call_result("Target.attachToTarget", &params, move |ok, json| {
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
        self.open.insert(conn, Screen { to, session, waiting, _heard: heard, port });
    }

    /// What the screen said, on to its session (or kept until it has one)
    pub fn say(&mut self, conn: u64, text: String) {
        let Some(s) = self.open.get(&conn) else { return };
        let sid = s.session.borrow().session.clone();
        match sid {
            Some(sid) => say(&s.port, &sid, &text),
            None => {
                let mut waiting = s.waiting.borrow_mut();
                let held: usize = waiting.iter().map(String::len).sum();
                match held + text.len() > EARLY {
                    true => shikisha_core::append_hook_log(&format!("[devtools] screen {conn}: said too much before its page answered; a message is dropped")),
                    false => waiting.push(text),
                }
            }
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

impl Screen {
    fn let_go(self) {
        let sid = self.session.borrow_mut().close();
        if let Some(sid) = sid {
            detach(&self.port, &sid);
        }
    }
}

fn detach(port: &crate::browser::cdp::Port, sid: &str) {
    port.call("Target.detachFromTarget", &serde_json::json!({"sessionId": sid}).to_string());
}

fn say(port: &crate::browser::cdp::Port, sid: &str, text: &str) {
    let params = serde_json::json!({"sessionId": sid, "message": text}).to_string();
    port.call("Target.sendMessageToTarget", &params);
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
        let keys: std::collections::HashMap<String, String> =
            [(hex("0/page"), "k3y".to_string()), (hex("0/beside"), "0th3r".to_string())].into_iter().collect();
        let path = format!("/devtools/k3y/{}", hex("0/page"));
        assert_eq!(admitted(&path, SCREEN_ORIGIN, &keys).as_deref(), Some("0/page"));
        assert_eq!(admitted(&format!("/devtools/other/{}", hex("0/page")), SCREEN_ORIGIN, &keys), None, "a wrong key");
        assert_eq!(admitted(&path, "http://127.0.0.1:9", &keys), None, "a page of the web");
        assert_eq!(admitted(&path, "", &keys), None, "no origin at all");
        assert_eq!(admitted("/devtools/k3y/", SCREEN_ORIGIN, &keys), None, "no page");
        assert_eq!(admitted("/json/list", SCREEN_ORIGIN, &keys), None);
        // One page's key is not another's, and a page no screen was asked
        // for has none
        assert_eq!(admitted(&format!("/devtools/k3y/{}", hex("0/beside")), SCREEN_ORIGIN, &keys), None, "one page's key opened another");
        assert_eq!(admitted(&format!("/devtools/k3y/{}", hex("0/unasked")), SCREEN_ORIGIN, &keys), None);
        assert_eq!(admitted(&format!("/devtools/0th3r/{}", hex("0/beside")), SCREEN_ORIGIN, &keys).as_deref(), Some("0/beside"));
    }
}

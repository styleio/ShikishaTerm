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
        // Input into the relay screen (finger trail / swipe / characters). The
        // heart of remote control, so let it through
        Ev::Inject { .. } => true,
        // Back/forward/reload/navigate. Since this remotely controls a browser,
        // the buttons on the top bar need to work too, or it's only half done.
        // It only changes the destination; it doesn't stop the window
        Ev::Go { .. } => true,
        // Copy the selected text. Keep the same manners as the window (same as PuTTY)
        Ev::Copy { .. } => true,
        Ev::Menu { key } => !matches!(
            key.as_str(),
            // Settings and the browser appear inside the window. Nothing
            // happens on the remote side
            "e" | "o"
            // The master password is asked inside the window.
            // Calling it from afar would block the app until the person
            // in front of the window answers
            | "k"
        ),
        // Opening the workspace switcher is allowed (it was allowed before as
        // Menu "w"). It only shows the list; picking a workspace is a separate
        // digit intent, so this alone doesn't disrupt the window.
        Ev::OpenWs => true,
        // Chatting with a model tab from the phone is just like typing into it.
        Ev::Chat { .. } => true,
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
    pub note: Option<String>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    pub rx: Receiver<RemoteCmd>,
    stop: Arc<AtomicBool>,
    /// Destinations for relay frames. JPEGs arriving from the browser flow here
    frame_clients: FrameClients,
    /// Destinations for state pushes (screen HTML / UI JSON) over /ws-state
    state_clients: StateClients,
    /// A new viewer joined. The main loop will emit one frame of the current
    /// screen at the next opportunity (so a static page doesn't stay blank
    /// waiting for the next change)
    keyframe_wanted: Arc<AtomicBool>,
}

impl RemoteUi {
    pub fn start(bind: std::net::Ipv4Addr, port: u16, token: String) -> Result<Self> {
        let server = Server::http((bind, port)).map_err(|e| {
            let addr = format!("{bind}:{port}");
            // "In use" is a long OS message that doesn't say what to do about
            // it. The culprit is usually your own previous instance still running
            let in_use = e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse);
            if in_use {
                anyhow::anyhow!(crate::i18n::tp("remote.err.in_use", &[("addr", &addr)]))
            } else {
                anyhow::anyhow!(crate::i18n::tp(
                    "remote.err.start",
                    &[("addr", &addr), ("error", &e.to_string())]
                ))
            }
        })?;
        let real_port = server
            .server_addr()
            .to_ip()
            .with_context(|| crate::i18n::t("err.remote.no_port"))?
            .port();
        let url = format!("http://{bind}:{real_port}/?t={token}");
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let (tx, rx) = channel::<RemoteCmd>();
        let stop = Arc::new(AtomicBool::new(false));
        let frame_clients: FrameClients = Arc::new(Mutex::new(Vec::new()));
        let state_clients: StateClients = Arc::new(Mutex::new(Vec::new()));
        let keyframe_wanted = Arc::new(AtomicBool::new(false));

        {
            let snapshot = Arc::clone(&snapshot);
            let stop = Arc::clone(&stop);
            let clients = Arc::clone(&frame_clients);
            let states = Arc::clone(&state_clients);
            let kf = Arc::clone(&keyframe_wanted);
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Err(e) = handle(req, &token, &snapshot, &tx, &clients, &states, &kf) {
                        crate::append_hook_log(&crate::i18n::tp(
                            "err.remote.hook_log",
                            &[("e", &e.to_string())],
                        ));
                    }
                }
            });
        }
        Ok(Self {
            url,
            note: None,
            snapshot,
            rx,
            stop,
            frame_clients,
            state_clients,
            keyframe_wanted,
        })
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

    /// Push one state message (a small JSON object with `ui` or `screen_html`)
    /// to every connected viewer. Drop lines whose peer has gone.
    pub fn push_state(&self, msg: String) {
        let mut clients = self.state_clients.lock().unwrap();
        clients.retain(|tx| tx.send(msg.clone()).is_ok());
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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

fn json_response(v: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(v.to_string())
        .with_header(
            Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                .unwrap(),
        )
        .with_header(Header::from_bytes(&b"Referrer-Policy"[..], &b"no-referrer"[..]).unwrap())
        .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap())
}

/// Maximum accepted request-body size (see webui::read_body).
const MAX_BODY: usize = 1 << 20; // 1 MiB

/// Read a request body, capped at `max` bytes; None if it would exceed the cap.
fn read_body(req: &mut tiny_http::Request, max: usize) -> std::io::Result<Option<String>> {
    use std::io::Read as _;
    let mut body = String::new();
    req.as_reader().take(max as u64 + 1).read_to_string(&mut body)?;
    Ok((body.len() <= max).then_some(body))
}

fn handle(
    req: tiny_http::Request,
    token: &str,
    snapshot: &Arc<Mutex<Snapshot>>,
    tx: &Sender<RemoteCmd>,
    frame_clients: &FrameClients,
    state_clients: &StateClients,
    keyframe_wanted: &Arc<AtomicBool>,
) -> Result<()> {
    let supplied = {
        let h = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("X-Token"))
            .map(|h| h.value.as_str().to_string())
            .unwrap_or_default();
        if h.is_empty() {
            query_value(req.url(), "t")
        } else {
            h
        }
    };
    if !token_eq(&supplied, token) {
        return req
            .respond(Response::from_string("forbidden").with_status_code(403))
            .map_err(Into::into);
    }

    let method = req.method().as_str().to_string();
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    match (method.as_str(), path.as_str()) {
        // Same shell as the window. One entry point so the appearance
        // isn't written twice
        ("GET", "/") | ("GET", "/shell") => {
            req.respond(
                Response::from_string(crate::shell::page(token))
                    .with_header(
                        Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    )
                    // Never show a stale page after an update
                    .with_header(
                        Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap(),
                    )
                    // Keep the URL token out of the Referer header
                    .with_header(
                        Header::from_bytes(&b"Referrer-Policy"[..], &b"no-referrer"[..]).unwrap(),
                    ),
            )?;
        }
        ("GET", "/api/state") => {
            let snap = snapshot.lock().unwrap().clone();
            req.respond(json_response(serde_json::to_value(snap)?))?;
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
            std::thread::spawn(move || {
                loop {
                    match crate::ws::read_frame(&mut stream) {
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
        _ => {
            req.respond(Response::from_string("not found").with_status_code(404))?;
        }
    }
    Ok(())
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
        // Back/forward/reload/navigate must work from remote, or the top bar is just decoration
        assert!(super::allowed_from_afar(&Ev::Go { go: crate::browser::Go::Back }));
        assert!(super::allowed_from_afar(&Ev::Go {
            go: crate::browser::Go::To("example.com".into())
        }));
        assert!(menu("a") && menu("?") && menu("w"), "普通の操作が通らない");
        // Scrolling back to review earlier output is core to monitoring from afar
        assert!(
            super::allowed_from_afar(&Ev::Scroll { by: 3, row: 0, col: 0 }),
            "遠くから履歴を遡れない"
        );

        assert!(!menu("k"), "マスターパスワードを遠くから呼べてしまう");
        assert!(!menu("e") && !menu("o"), "窓の中にしか出ないものを呼べる");
        // Sizing from afar is intentionally allowed: a phone needs a terminal that
        // fits it, and the two sides hand off (each re-reports only on its own size
        // change) rather than fight in a loop.
        assert!(
            super::allowed_from_afar(&Ev::Resize {
                rows: 10,
                cols: 20,
                area: (0, 0, 0, 0)
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
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "abcd"));
        assert_eq!(query_value("/?t=xyz", "t"), "xyz");
        assert_eq!(query_value("/api/state", "t"), "");
    }

    /// Actually starts the server and confirms auth and command delivery
    #[test]
    fn serves_state_and_forwards_commands() {
        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into()).unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let agent = ureq::Agent::new_with_defaults();
        let status = |r: Result<ureq::http::Response<ureq::Body>, ureq::Error>| match r {
            Ok(x) => x.status().as_u16(),
            Err(ureq::Error::StatusCode(c)) => c,
            Err(e) => panic!("unexpected: {e}"),
        };

        // No token is rejected
        assert_eq!(status(agent.get(&format!("{base}/api/state")).call()), 403);

        // Returns state
        ui.snapshot.lock().unwrap().tabs = vec![RemoteTab {
            index: 1,
            name: "実装".into(),
            state: "QUESTION".into(),
            ..Default::default()
        }];
        let body = agent
            .get(&format!("{base}/api/state?t=tok123456789012"))
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        assert!(body.contains("実装") && body.contains("QUESTION"), "{body}");

        // The instruction reaches the main loop
        agent
            .post(&format!("{base}/api/send?t=tok123456789012"))
            .send(r#"{"tab":1,"text":"続けて"}"#)
            .unwrap();
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Send { tab, text } => {
                assert_eq!((tab, text.as_str()), (1, "続けて"));
            }
            other => panic!("想定外: {other:?}"),
        }

        // The entry point serves the same shell as the window.
        // There used to be a separate, old page just for the phone that,
        // even after being fixed, never once reached the phone side
        for entry in ["/", "/shell"] {
            let page = agent
                .get(&format!("{base}{entry}?t=tok123456789012"))
                .call()
                .unwrap()
                .body_mut()
                .read_to_string()
                .unwrap();
            assert!(
                page.contains("api/intent") && page.contains("window.__state"),
                "{entry} が窓と同じ外皮を配っていない"
            );
        }

        // Operations from the screen reach the main loop
        agent
            .post(&format!("{base}/api/intent?t=tok123456789012"))
            .send(r#"{"kind":"select","tab":2}"#)
            .unwrap();
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Ui(crate::browser::Ev::Select { tab }) => assert_eq!(tab, 2),
            other => panic!("想定外: {other:?}"),
        }

        // The "back" button on the top bar also reaches the main loop (both
        // the allow-list and the path). It used to be blocked by the
        // allow-list, and after that fix, silently dropped by keys_for
        agent
            .post(&format!("{base}/api/intent?t=tok123456789012"))
            .send(r#"{"kind":"go","what":"back"}"#)
            .unwrap();
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Ui(crate::browser::Ev::Go {
                go: crate::browser::Go::Back,
            }) => {}
            other => panic!("戻るが本体まで届かない: {other:?}"),
        }

        // Something only the window can answer is stopped as soon as it's
        // received. That it didn't get through is confirmed on the next
        // receive (select comes through first)
        agent
            .post(&format!("{base}/api/intent?t=tok123456789012"))
            .send(r#"{"kind":"menu","key":"k"}"#)
            .unwrap();
        agent
            .post(&format!("{base}/api/intent?t=tok123456789012"))
            .send(r#"{"kind":"select","tab":3}"#)
            .unwrap();
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Ui(crate::browser::Ev::Select { tab }) => {
                assert_eq!(tab, 3, "止めたはずの操作が先に届いた")
            }
            other => panic!("窓にしか答えられないものが通った: {other:?}"),
        }
        ui.shutdown();
    }

    /// Confirms /ws handshakes and that a JPEG pushed via push_frame arrives
    /// as a WS binary frame. Verified end to end with just a raw TCP
    /// connection and our own ws module (no phone or external tool needed)
    #[test]
    fn ws_upgrades_and_delivers_a_frame() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into()).unwrap();
        let hostport = ui
            .url
            .trim_start_matches("http://")
            .split("/?")
            .next()
            .unwrap()
            .to_string();

        let mut sock = TcpStream::connect(&hostport).unwrap();
        // Same key as the RFC 6455 example (accept becomes s3pP...)
        let req = "GET /ws?t=tok123456789012 HTTP/1.1\r\n\
             Host: localhost\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n";
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

        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into()).unwrap();
        let hostport = ui
            .url
            .trim_start_matches("http://")
            .split("/?")
            .next()
            .unwrap()
            .to_string();

        let mut sock = TcpStream::connect(&hostport).unwrap();
        let req = "GET /ws-in?t=tok123456789012 HTTP/1.1\r\n\
             Host: localhost\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n";
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

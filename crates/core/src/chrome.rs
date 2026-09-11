//! A browser on the machine the agents run on.
//!
//! The window has one already: WebView2, embedded in a pane, driven over the
//! DevTools protocol. A runtime with no window has none, and that was the last
//! thing the server could not do -- no placed pages, no `browser_*`, nothing
//! for an agent that needs to look at what it just built.
//!
//! The obvious answer was to throw those commands back to whichever client is
//! connected and let its browser do the work. It is wrong twice. A phone cannot
//! do it at all -- the page would have to reach into another site's document,
//! which no browser allows -- and nobody is connected at the times this matters
//! most, since closing the laptop is the whole point of running on a server.
//!
//! So the browser lives here, beside the agents. One more thing it buys, which
//! the throwing-back design could never have: **one browser session for every
//! client**. Log in once on the server and the window, the phone and the laptop
//! are all looking at the same logged-in browser.
//!
//! Not bundled. A release carrying a browser is a release several hundred
//! megabytes larger, and the thing that makes a Chromium your responsibility is
//! shipping the binary, not using one. So: whatever the machine already has, and
//! a plain refusal when it has none.

use std::io::{Read as _, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

/// The names a Chromium goes by. Ordered so that a plain `chromium` -- the one
/// a distribution's package manager installs -- is found before a vendor build
/// somebody may have left behind
#[cfg(unix)]
const NAMES: &[&str] = &[
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
    "brave-browser",
    "microsoft-edge",
];

#[cfg(windows)]
const NAMES: &[&str] = &["chrome.exe", "msedge.exe"];

/// Where one is, if this machine has one.
///
/// Looked for rather than configured, because a person who installed a browser
/// with their package manager has already said where it goes. A setting would
/// be a second place for that answer to be wrong.
pub fn found() -> Option<std::path::PathBuf> {
    if let Some(told) = std::env::var_os("SHIKISHA_CHROME") {
        let p = std::path::PathBuf::from(told);
        return p.is_file().then_some(p);
    }
    for name in NAMES {
        if let Some(p) = on_path(name) {
            return Some(p);
        }
    }
    #[cfg(windows)]
    for base in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
        let Some(root) = std::env::var_os(base) else { continue };
        for rest in [
            r"Google\Chrome\Application\chrome.exe",
            r"Microsoft\Edge\Application\msedge.exe",
        ] {
            let p = std::path::Path::new(&root).join(rest);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn on_path(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// A browser this process started, and the connection to it.
pub struct Chrome {
    child: std::process::Child,
    /// Where its own profile lives. Taken away with it: a profile left behind
    /// is somebody's cookies sitting in a temporary folder
    profile: std::path::PathBuf,
    cdp: Cdp,
}

impl Chrome {
    /// Start one, with no window and its own profile.
    ///
    /// `--headless=new` rather than the old mode: the old one is a different
    /// browser wearing the same name, and the things it does differently are
    /// exactly the things a page notices.
    pub fn start() -> anyhow::Result<Self> {
        let exe = found().ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.none")))?;
        let profile = std::env::temp_dir().join(format!("shikisha-chrome-{}", crate::random_hex(8)));
        std::fs::create_dir_all(&profile)?;

        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--headless=new")
            .arg("--remote-debugging-port=0")
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            // A server has no sound card and no use for one
            .arg("--mute-audio")
            // Shared memory is small in a container, and a browser that runs
            // out of it crashes in a way that reads as "the page is broken"
            .arg("--disable-dev-shm-usage")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        let mut child = crate::detach_console(&mut cmd).spawn()?;

        // The port it chose is announced on its error stream, once, before
        // anything else. Asked for as 0 rather than picked by us: two runtimes
        // on one machine picking the same number is a collision nobody debugs
        let ws = match read_ws_endpoint(&mut child) {
            Ok(ws) => ws,
            Err(e) => {
                let _ = child.kill();
                let _ = std::fs::remove_dir_all(&profile);
                return Err(e);
            }
        };
        let cdp = match Cdp::connect(&ws) {
            Ok(c) => c,
            Err(e) => {
                let _ = child.kill();
                let _ = std::fs::remove_dir_all(&profile);
                return Err(e);
            }
        };
        Ok(Self { child, profile, cdp })
    }

    /// Say something to the browser itself, rather than to a page in it.
    pub fn call(&self, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        self.cdp.call(None, method, params)
    }

    /// Say something to one page.
    pub fn call_page(
        &self,
        session: &str,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.cdp.call(Some(session), method, params)
    }

    /// Open a page and attach to it, handing back the session to address it by.
    ///
    /// Flattened, so one connection carries every page: the alternative is a
    /// socket per tab, and a socket per tab is a set of threads per tab.
    pub fn open(&self, url: &str) -> anyhow::Result<String> {
        let made = self.call("Target.createTarget", serde_json::json!({ "url": url }))?;
        let id = made
            .get("targetId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("the browser opened no page"))?;
        let att = self.call(
            "Target.attachToTarget",
            serde_json::json!({ "targetId": id, "flatten": true }),
        )?;
        att.get("sessionId")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("the browser attached to nothing"))
    }
}

impl Drop for Chrome {
    fn drop(&mut self) {
        // Asked first, killed second. A browser told to close writes its
        // profile out; one that is killed leaves a lock file that the next
        // start of the same profile spends ten seconds deciding about
        let _ = self.call("Browser.close", serde_json::json!({}));
        let _ = self.child.wait_timeout(std::time::Duration::from_secs(3));
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

/// `Child::wait` with a deadline, which the standard library does not have.
trait WaitFor {
    fn wait_timeout(&mut self, how_long: std::time::Duration) -> std::io::Result<bool>;
}

impl WaitFor for std::process::Child {
    fn wait_timeout(&mut self, how_long: std::time::Duration) -> std::io::Result<bool> {
        let until = std::time::Instant::now() + how_long;
        loop {
            if self.try_wait()?.is_some() {
                return Ok(true);
            }
            if std::time::Instant::now() >= until {
                return Ok(false);
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// The address it printed, or what it said instead.
fn read_ws_endpoint(child: &mut std::process::Child) -> anyhow::Result<String> {
    use std::io::BufRead as _;
    let err = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("the browser has no error stream to read"))?;
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut said = Vec::new();
        for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
            if let Some(ws) = line.trim().strip_prefix("DevTools listening on ") {
                let _ = tx.send(Ok(ws.to_string()));
                return;
            }
            // Kept so that a browser which refuses to start can say why in its
            // own words, rather than as "it did not answer in time"
            if said.len() < 20 {
                said.push(line);
            }
        }
        let _ = tx.send(Err(said.join("\n")));
    });
    match rx.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(Ok(ws)) => Ok(ws),
        Ok(Err(said)) if said.is_empty() => {
            Err(anyhow::anyhow!(crate::i18n::t("err.chrome.silent")))
        }
        Ok(Err(said)) => Err(anyhow::anyhow!(crate::i18n::tp(
            "err.chrome.refused",
            &[("said", &said)]
        ))),
        Err(_) => Err(anyhow::anyhow!(crate::i18n::t("err.chrome.slow"))),
    }
}

// ── the protocol ──────────────────────────────────────────────────────────

/// One connection, carrying every page.
struct Cdp {
    out: Arc<Mutex<Box<dyn Write + Send>>>,
    next: AtomicU64,
    /// Who is waiting for which answer
    waiting: Arc<Mutex<std::collections::HashMap<u64, Sender<serde_json::Value>>>>,
    /// Whether the far side is still there. A call made after it has gone
    /// should say so rather than wait out its timeout
    alive: Arc<std::sync::atomic::AtomicBool>,
}

impl Cdp {
    fn connect(ws_url: &str) -> anyhow::Result<Self> {
        let (host, path) = split_ws(ws_url)?;
        let stream = std::net::TcpStream::connect(&host)?;
        stream.set_nodelay(true)?;
        let mut sock = stream.try_clone()?;

        // The key is random and the answer is not checked. What that check
        // proves is that the far side speaks WebSocket rather than being a
        // cache or a proxy that echoed the request -- and there is no cache
        // between here and a port this process started on this machine
        let key = {
            use base64::Engine as _;
            let bytes = crate::random_bytes(16).unwrap_or_else(|| vec![0; 16]);
            base64::engine::general_purpose::STANDARD.encode(bytes)
        };
        write!(
            sock,
            "GET {path} HTTP/1.1\r\n\
             Host: {host}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: {key}\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n"
        )?;
        sock.flush()?;
        read_until_headers_end(&stream)?;

        let waiting: Arc<Mutex<std::collections::HashMap<u64, Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));

        let reading = stream.try_clone()?;
        let post = Arc::clone(&waiting);
        let still = Arc::clone(&alive);
        std::thread::Builder::new()
            .name("shikisha-cdp".into())
            .spawn(move || {
                read_frames(reading, post);
                still.store(false, Ordering::Relaxed);
            })?;

        Ok(Self {
            out: Arc::new(Mutex::new(Box::new(sock))),
            next: AtomicU64::new(1),
            waiting,
            alive,
        })
    }

    fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        if !self.alive.load(Ordering::Relaxed) {
            anyhow::bail!(crate::i18n::t("err.chrome.gone"));
        }
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut msg = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = serde_json::Value::String(s.to_string());
        }

        let (tx, rx) = channel();
        self.waiting.lock().unwrap().insert(id, tx);
        let sent = {
            let mut out = self.out.lock().unwrap();
            out.write_all(&text_frame(&msg.to_string()))
                .and_then(|()| out.flush())
        };
        if let Err(e) = sent {
            self.waiting.lock().unwrap().remove(&id);
            return Err(e.into());
        }

        match rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(v) => match v.get("error") {
                Some(e) => Err(anyhow::anyhow!(crate::i18n::tp(
                    "err.chrome.said",
                    &[(
                        "said",
                        e.get("message").and_then(|m| m.as_str()).unwrap_or("?")
                    )]
                ))),
                None => Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null)),
            },
            Err(_) => {
                self.waiting.lock().unwrap().remove(&id);
                Err(anyhow::anyhow!(crate::i18n::tp(
                    "err.chrome.no_answer",
                    &[("method", method)]
                )))
            }
        }
    }
}

fn split_ws(url: &str) -> anyhow::Result<(String, String)> {
    let rest = url
        .strip_prefix("ws://")
        .ok_or_else(|| anyhow::anyhow!("the browser named an address this cannot open: {url}"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    Ok((host.to_string(), format!("/{path}")))
}

fn read_until_headers_end(stream: &std::net::TcpStream) -> anyhow::Result<()> {
    let mut s = stream.try_clone()?;
    let mut seen = Vec::new();
    let mut one = [0u8; 1];
    while seen.len() < 8192 {
        if s.read(&mut one)? == 0 {
            anyhow::bail!("the browser closed the connection while being greeted");
        }
        seen.push(one[0]);
        if seen.ends_with(b"\r\n\r\n") {
            return Ok(());
        }
    }
    anyhow::bail!("the browser's greeting never ended")
}

/// One text frame, masked as a client must.
fn text_frame(s: &str) -> Vec<u8> {
    let payload = s.as_bytes();
    let mut out = vec![0x81u8]; // FIN + text
    let mask: [u8; 4] = crate::random_bytes(4)
        .and_then(|b| b.try_into().ok())
        .unwrap_or([0xA1, 0xB2, 0xC3, 0xD4]);
    let len = payload.len();
    match len {
        0..=125 => out.push(0x80 | len as u8),
        126..=65535 => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        _ => {
            out.push(0x80 | 127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i & 3]));
    out
}

/// Read frames until the far side stops, handing each answer to whoever asked.
///
/// Events -- anything with no `id` -- are dropped here. Nothing needs them yet,
/// and a queue nobody drains is a memory leak with a schedule.
fn read_frames(
    stream: std::net::TcpStream,
    waiting: Arc<Mutex<std::collections::HashMap<u64, Sender<serde_json::Value>>>>,
) {
    let mut s = std::io::BufReader::new(stream);
    let mut whole = Vec::new();
    loop {
        let Some((fin, opcode, payload)) = read_one_frame(&mut s) else {
            return;
        };
        match opcode {
            // continuation, text, binary
            0x0 | 0x1 | 0x2 => {
                whole.extend_from_slice(&payload);
                if !fin {
                    continue;
                }
                let text = std::mem::take(&mut whole);
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&text) {
                    if let Some(id) = v.get("id").and_then(|i| i.as_u64()) {
                        if let Some(tx) = waiting.lock().unwrap().remove(&id) {
                            let _ = tx.send(v);
                        }
                    }
                }
            }
            0x8 => return, // close
            _ => {}        // ping/pong: the browser does not need ours
        }
    }
}

fn read_one_frame(s: &mut impl std::io::Read) -> Option<(bool, u8, Vec<u8>)> {
    let mut head = [0u8; 2];
    s.read_exact(&mut head).ok()?;
    let fin = head[0] & 0x80 != 0;
    let opcode = head[0] & 0x0f;
    let masked = head[1] & 0x80 != 0;
    let len = match head[1] & 0x7f {
        126 => {
            let mut n = [0u8; 2];
            s.read_exact(&mut n).ok()?;
            u16::from_be_bytes(n) as usize
        }
        127 => {
            let mut n = [0u8; 8];
            s.read_exact(&mut n).ok()?;
            u64::from_be_bytes(n) as usize
        }
        n => n as usize,
    };
    // A browser answering a screenshot sends megabytes; anything past this is
    // not an answer we asked for
    if len > 64 * 1024 * 1024 {
        return None;
    }
    let mask = masked
        .then(|| {
            let mut m = [0u8; 4];
            s.read_exact(&mut m).ok().map(|()| m)
        })
        .flatten();
    let mut payload = vec![0u8; len];
    s.read_exact(&mut payload).ok()?;
    if let Some(m) = mask {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= m[i & 3];
        }
    }
    Some((fin, opcode, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame this writes must be one the other side can read back. The two
    /// halves are written here, so a mistake in the length forms would agree
    /// with itself and prove nothing -- the lengths below are chosen to cross
    /// both boundaries where the form changes
    #[test]
    fn a_frame_survives_being_written_and_read() {
        // Lengths in bytes, so the body is made of one-byte characters. The
        // three forms change at 125/126 and 65535/65536, and each is crossed
        let mut bodies: Vec<String> = [0usize, 1, 125, 126, 127, 65535, 65536]
            .into_iter()
            .map(|n| "a".repeat(n))
            .collect();
        // ...and one that is not ASCII at all, because the length is counted
        // in bytes and a character is not a byte
        bodies.push("あいうえお かきくけこ".to_string());
        for body in &bodies {
            let framed = text_frame(body);
            let mut cursor = std::io::Cursor::new(framed);
            let (fin, opcode, payload) =
                read_one_frame(&mut cursor).expect("書いた枠が読めない");
            assert!(fin, "1枚で終わっていない");
            assert_eq!(opcode, 0x1, "文字の枠ではない");
            assert_eq!(&String::from_utf8(payload).unwrap(), body, "中身が変わった");
        }
    }

    /// The mask is what makes a client's frame legal, and a fresh one each
    /// time is what makes it worth having
    #[test]
    fn every_frame_is_masked_differently() {
        let a = text_frame("hello");
        let b = text_frame("hello");
        assert_eq!(a[1] & 0x80, 0x80, "マスク無しの枠を送っている");
        assert_ne!(a, b, "毎回同じマスクを使っている");
    }

    /// The whole spine, against a real browser: start one, open a page, and
    /// read back what is in it.
    ///
    /// Ignored by default because a machine may have no browser, and a test
    /// that fails for that reason is a test that teaches people to ignore red.
    /// Run it where one is installed:
    ///
    ///     cargo test -p shikisha-core --lib chrome:: -- --ignored
    #[test]
    #[ignore = "needs a browser on this machine"]
    fn a_real_browser_opens_a_page_and_says_what_is_in_it() {
        let Some(exe) = found() else {
            panic!("この機械にブラウザが無い（--ignored で走らせる前に入れる）");
        };
        println!("browser: {}", exe.display());

        let chrome = Chrome::start().expect("ブラウザが起動しない");
        let version = chrome
            .call("Browser.getVersion", serde_json::json!({}))
            .expect("版を答えない");
        println!("version: {}", version.get("product").and_then(|v| v.as_str()).unwrap_or("?"));

        // A page of our own making, so the test does not depend on the network
        // The charset is declared, because a `data:` URL without one is read
        // as windows-1252 and every character above ASCII comes back as
        // something else. Found by this test saying exactly that
        let page = "data:text/html;charset=utf-8,                    <title>shikisha</title><h1 id=t>ここに書いた</h1>";
        let session = chrome.open(page).expect("ページが開かない");

        // The page has to have finished before it can be read
        chrome
            .call_page(&session, "Page.enable", serde_json::json!({}))
            .expect("Page を有効にできない");
        std::thread::sleep(std::time::Duration::from_millis(700));

        let got = chrome
            .call_page(
                &session,
                "Runtime.evaluate",
                serde_json::json!({
                    "expression": "document.getElementById('t').textContent",
                    "returnByValue": true,
                }),
            )
            .expect("読み返せない");
        let text = got
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(text, "ここに書いた", "書いたものが読み返せない: {got}");

        // And a picture of it, which is what a phone is shown
        let shot = chrome
            .call_page(&session, "Page.captureScreenshot", serde_json::json!({"format": "png"}))
            .expect("撮れない");
        let png = shot.get("data").and_then(|v| v.as_str()).unwrap_or("");
        assert!(png.len() > 1000, "絵が小さすぎる: {} 文字", png.len());
        println!("screenshot: {} 文字の PNG", png.len());
    }

    /// The address a browser prints is turned into somewhere to connect
    #[test]
    fn the_browsers_address_is_split_into_host_and_path() {
        let (host, path) = split_ws("ws://127.0.0.1:9222/devtools/browser/abc-123").unwrap();
        assert_eq!(host, "127.0.0.1:9222");
        assert_eq!(path, "/devtools/browser/abc-123");
        assert!(split_ws("http://127.0.0.1:9222/").is_err(), "ws でない住所を受けた");
    }
}

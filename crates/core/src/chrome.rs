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
use std::rc::Rc;
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
///
/// The one this program fetched for itself comes before the machine's, when
/// there is one: its version is known, and a known version is the whole reason
/// it was fetched.
pub fn found() -> Option<std::path::PathBuf> {
    if let Some(told) = std::env::var_os("SHIKISHA_CHROME") {
        let p = std::path::PathBuf::from(told);
        return p.is_file().then_some(p);
    }
    if let Some(ours) = fetched() {
        return Some(ours);
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

// ── a browser of this program's own ───────────────────────────────────────
//
// A machine with no browser used to be the end of it: the commands refused
// and said `apt install chromium`. That is a fair thing to say to somebody
// who is already installing things on a server, and a poor thing to say to
// somebody whose agent just tried to look at the page it built.
//
// So: when there is none, fetch one. Not into the release -- a release
// carrying a browser is several hundred megabytes larger on every channel,
// and the thing that makes a Chromium your responsibility is shipping the
// binary. This is Google's own build, downloaded from Google, kept under this
// program's data folder. Nobody who does not use the browser pays for it.

/// The version fetched when the machine has none.
///
/// Pinned, rather than whatever is newest. The DevTools protocol is not a
/// stable interface, and the parts leaned on hardest here -- the accessibility
/// tree and the layout snapshot the digest is built from -- are exactly the
/// parts that differ between versions. A pinned version is the difference
/// between a script behaving the same everywhere and a script behaving
/// differently on every machine it is run on.
const PINNED: &str = "153.0.8010.36";

/// A build of that version, for one shape of machine.
struct Build {
    /// What the download calls this machine
    platform: &'static str,
    /// Where the browser is inside the zip
    exe: &'static str,
    /// What the zip weighed when it was measured here, as a check that it
    /// arrived whole. `None` for a platform nobody has measured yet, where
    /// the transport is all the assurance there is
    sha256: Option<&'static str>,
}

/// The build for this machine, or nothing where no build is published.
fn build_for_this_machine() -> Option<Build> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Some(Build {
        platform: "linux64",
        exe: "chrome-linux64/chrome",
        sha256: None,
    });
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Some(Build {
        platform: "linux-arm64",
        exe: "chrome-linux-arm64/chrome",
        sha256: None,
    });
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Some(Build {
        platform: "win64",
        exe: "chrome-win64/chrome.exe",
        sha256: None,
    });
    #[cfg(not(any(
        all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")),
        all(target_os = "windows", target_arch = "x86_64")
    )))]
    return None;
}

/// Where fetched browsers are kept. Under the version, so a later pin lands
/// beside the one in use rather than on top of a browser that is running
fn browsers_dir() -> std::path::PathBuf {
    crate::config::chromium_data_dir().join("browsers")
}

/// The one this program fetched, if it is already here.
pub fn fetched() -> Option<std::path::PathBuf> {
    let build = build_for_this_machine()?;
    let exe = browsers_dir().join(PINNED).join(build.platform).join(build.exe);
    exe.is_file().then_some(exe)
}

/// Fetch the pinned browser, unless it is already here.
///
/// Google publishes these builds for exactly this purpose (Chrome for
/// Testing): a version that can be named, at an address that does not move.
/// What arrives is unpacked beside the version it belongs to, and only put
/// under that name once it is whole -- so a download cut off halfway is never
/// mistaken for a browser.
pub fn fetch() -> anyhow::Result<std::path::PathBuf> {
    if let Some(here) = fetched() {
        return Ok(here);
    }
    let build = build_for_this_machine()
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.none")))?;
    let platform = build.platform;
    let url = format!(
        "https://storage.googleapis.com/chrome-for-testing-public/{PINNED}/{platform}/chrome-{platform}.zip"
    );
    let root = browsers_dir();
    std::fs::create_dir_all(&root)?;
    let part = root.join(format!("part-{}", crate::random_hex(6)));
    std::fs::create_dir_all(&part)?;

    let outcome = (|| -> anyhow::Result<()> {
        let zip = part.join("chrome.zip");
        stream_to(&url, &zip)?;
        if let Some(want) = build.sha256 {
            crate::update::verify_sha256(&std::fs::read(&zip)?, want)?;
        }
        crate::update::unpack(&zip, &part.join("out"))?;
        if !part.join("out").join(build.exe).is_file() {
            anyhow::bail!(crate::i18n::t("err.chrome.download_empty"));
        }
        // Somebody else may have finished the same download while this one
        // ran. Theirs is as good as ours, and theirs may already be running
        if fetched().is_some() {
            return Ok(());
        }
        let home = root.join(PINNED);
        std::fs::create_dir_all(&home)?;
        let _ = std::fs::remove_dir_all(home.join(platform));
        std::fs::rename(part.join("out"), home.join(platform))?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&part);
    outcome?;

    fetched().ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.download_empty")))
}

/// Pull a large file down, saying how far along it is.
///
/// A few hundred megabytes with nothing said about it is indistinguishable
/// from a program that has stopped, so it is said -- in the log, which is
/// where a runtime with nobody in front of it can say anything at all.
fn stream_to(url: &str, to: &std::path::Path) -> anyhow::Result<()> {
    let mut resp = crate::update::agent(std::time::Duration::from_secs(20 * 60))
        .get(url)
        .call()?;
    let total: Option<u64> = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok());
    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(to)?;
    let mut got: u64 = 0;
    let mut said = 0;
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = std::io::Read::read(&mut reader, &mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])?;
        got += n as u64;
        let part = total.map_or(0, |t| got * 100 / t.max(1));
        if part >= said + 20 {
            said = part;
            crate::append_hook_log(&format!("chrome: fetching {PINNED} -- {part}%"));
        }
    }
    Ok(())
}

/// Where a browser is, fetching this program's own if the machine has none.
fn ready() -> anyhow::Result<std::path::PathBuf> {
    if let Some(p) = found() {
        return Ok(p);
    }
    // Said before it starts, because it is a few hundred megabytes of
    // somebody's connection and they are entitled to know where it went
    crate::append_hook_log(&format!(
        "chrome: no browser on this machine, fetching {PINNED} from Google (install one with your package manager to use that instead)"
    ));
    // Both halves of it, when this fails: there is none here, and the one
    // that would have been fetched did not arrive. Either can be answered by
    // the person, and only if they are told which
    fetch().map_err(|e| {
        anyhow::anyhow!(crate::i18n::tp("err.chrome.no_fetch", &[("e", &e.to_string())]))
    })
}

/// A browser this process started, and the connection to it.
pub struct Chrome {
    child: std::process::Child,
    /// Where its profile lives -- its cookies, its logins, everything a person
    /// signed in once and expects to stay signed in
    profile: std::path::PathBuf,
    /// Whether that folder goes away with the browser. A throwaway profile
    /// left behind is somebody's cookies sitting in a temporary folder
    temporary: bool,
    cdp: Cdp,
}

impl Chrome {
    /// Start one with a profile that lasts no longer than it does.
    pub fn start() -> anyhow::Result<Self> {
        let profile = std::env::temp_dir().join(format!("shikisha-chrome-{}", crate::random_hex(8)));
        Self::start_in(profile, true)
    }

    /// Start one, with no window and the profile in `profile`.
    ///
    /// `--headless=new` rather than the old mode: the old one is a different
    /// browser wearing the same name, and the things it does differently are
    /// exactly the things a page notices.
    pub fn start_in(profile: std::path::PathBuf, temporary: bool) -> anyhow::Result<Self> {
        let exe = ready()?;
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
        // A server is often root, and a Chromium started by root refuses to
        // run at all rather than run with a sandbox it cannot build. There is
        // no third option to choose here -- the choice is this or no browser --
        // so it is said out loud in the log rather than hidden in a flag
        if as_root() {
            crate::append_hook_log(
                "chrome: running as root, so the browser's own sandbox is off (it refuses to start otherwise)",
            );
            cmd.arg("--no-sandbox");
        }
        let mut child = crate::detach_console(&mut cmd).spawn()?;

        // The port it chose is announced on its error stream, once, before
        // anything else. Asked for as 0 rather than picked by us: two runtimes
        // on one machine picking the same number is a collision nobody debugs
        let started = read_ws_endpoint(&mut child).and_then(|ws| Cdp::connect(&ws));
        let cdp = match started {
            Ok(c) => c,
            Err(e) => {
                let _ = child.kill();
                if temporary {
                    let _ = std::fs::remove_dir_all(&profile);
                }
                return Err(e);
            }
        };
        Ok(Self { child, profile, temporary, cdp })
    }

    /// Say something to the browser itself, rather than to a page in it.
    pub fn call(&self, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        self.cdp.call(None, method, params, CALL_MS)
    }

    /// Say something to one page.
    pub fn call_page(
        &self,
        session: &str,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.cdp.call(Some(session), method, params, CALL_MS)
    }

    /// The same, with the caller's own patience.
    pub fn call_within(
        &self,
        session: Option<&str>,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
    ) -> anyhow::Result<serde_json::Value> {
        self.cdp.call(session, method, params, timeout_ms)
    }

    /// Say something and not wait to hear about it.
    ///
    /// For the answers that are acknowledgements rather than answers -- a
    /// screencast frame's ack, chiefly, which has to be sent from the thread
    /// that is reading frames and would otherwise wait on itself.
    pub fn speaker(&self) -> Speaker {
        self.cdp.speaker()
    }

    /// Hand every event the browser reports to `sink`, on the thread that
    /// reads them.
    ///
    /// Whatever it does there must not wait for an answer from the browser --
    /// the answer would arrive on this same thread, behind it.
    pub fn on_event(&self, sink: impl Fn(Event) + Send + 'static) {
        *self.cdp.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(sink));
    }

    /// A storage of its own, shared with no other page in this browser.
    ///
    /// What "private" means: a page opened in one of these has nothing from
    /// the profile it sits beside, and takes nothing back to it
    pub fn new_context(&self) -> anyhow::Result<String> {
        let made = self.call("Target.createBrowserContext", serde_json::json!({}))?;
        made.get("browserContextId")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.no_page")))
    }

    pub fn close_context(&self, context: &str) {
        let _ = self.call(
            "Target.disposeBrowserContext",
            serde_json::json!({ "browserContextId": context }),
        );
    }

    /// Open an empty page and attach to it: its target, and the session to
    /// address it by.
    ///
    /// Empty, because everything that has to be in place before the first
    /// document -- the script every page is given, the binding it reports
    /// through -- can only be put in place once there is a page to put it in.
    ///
    /// Flattened, so one connection carries every page: the alternative is a
    /// socket per tab, and a socket per tab is a set of threads per tab.
    pub fn open_blank(&self, context: Option<&str>) -> anyhow::Result<(String, String)> {
        let mut params = serde_json::json!({ "url": "about:blank" });
        if let Some(c) = context {
            params["browserContextId"] = serde_json::Value::String(c.to_string());
        }
        let made = self.call("Target.createTarget", params)?;
        let target = made
            .get("targetId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.no_page")))?
            .to_string();
        let att = self.call(
            "Target.attachToTarget",
            serde_json::json!({ "targetId": target, "flatten": true }),
        )?;
        let session = att
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.no_page")))?
            .to_string();
        Ok((target, session))
    }

    /// Open a page at an address and attach to it, handing back its session.
    pub fn open(&self, url: &str) -> anyhow::Result<String> {
        let made = self.call("Target.createTarget", serde_json::json!({ "url": url }))?;
        let id = made
            .get("targetId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.no_page")))?;
        let att = self.call(
            "Target.attachToTarget",
            serde_json::json!({ "targetId": id, "flatten": true }),
        )?;
        att.get("sessionId")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.no_page")))
    }
}

/// Whether this process is root.
///
/// Only asked so a browser can be started at all: Chromium run by root will
/// not build its sandbox, and refuses to run rather than run without one
/// unless it is told to.
#[cfg(unix)]
fn as_root() -> bool {
    use std::os::unix::fs::MetadataExt as _;
    std::fs::metadata("/proc/self").map(|m| m.uid() == 0).unwrap_or(false)
}

#[cfg(not(unix))]
fn as_root() -> bool {
    false
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
        if self.temporary {
            let _ = std::fs::remove_dir_all(&self.profile);
        }
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

/// How long an answer is waited for when the caller names no time of its own.
const CALL_MS: u64 = 30_000;

/// Something the browser said that nobody asked about: a frame of a
/// screencast, a page reporting what a person did, a request wanting
/// credentials.
///
/// `session` is the page it came from, `None` the browser itself.
pub struct Event {
    pub session: Option<String>,
    pub method: String,
    pub params: serde_json::Value,
}

/// A way to say something to the browser without waiting to hear back.
#[derive(Clone)]
pub struct Speaker {
    out: Arc<Mutex<Box<dyn Write + Send>>>,
    next: Arc<AtomicU64>,
}

impl Speaker {
    pub fn tell(&self, session: Option<&str>, method: &str, params: serde_json::Value) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut msg = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = serde_json::Value::String(s.to_string());
        }
        if let Ok(mut out) = self.out.lock() {
            let _ = out.write_all(&text_frame(&msg.to_string())).and_then(|()| out.flush());
        }
    }
}

type Sink = Box<dyn Fn(Event) + Send>;

/// One connection, carrying every page.
struct Cdp {
    out: Arc<Mutex<Box<dyn Write + Send>>>,
    next: Arc<AtomicU64>,
    /// Who is waiting for which answer
    waiting: Arc<Mutex<std::collections::HashMap<u64, Sender<serde_json::Value>>>>,
    /// Where anything the browser says of its own accord goes
    sink: Arc<Mutex<Option<Sink>>>,
    /// Whether the far side is still there. A call made after it has gone
    /// should say so rather than wait out its timeout
    alive: Arc<std::sync::atomic::AtomicBool>,
}

impl Cdp {
    fn speaker(&self) -> Speaker {
        Speaker { out: Arc::clone(&self.out), next: Arc::clone(&self.next) }
    }

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
        let sink: Arc<Mutex<Option<Sink>>> = Arc::new(Mutex::new(None));

        let reading = stream.try_clone()?;
        let post = Arc::clone(&waiting);
        let still = Arc::clone(&alive);
        let heard = Arc::clone(&sink);
        std::thread::Builder::new()
            .name("shikisha-cdp".into())
            .spawn(move || {
                read_frames(reading, post, heard);
                still.store(false, Ordering::Relaxed);
            })?;

        Ok(Self {
            out: Arc::new(Mutex::new(Box::new(sock))),
            next: Arc::new(AtomicU64::new(1)),
            waiting,
            sink,
            alive,
        })
    }

    fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
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

        match rx.recv_timeout(std::time::Duration::from_millis(timeout_ms)) {
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

/// Read frames until the far side stops, handing each answer to whoever asked
/// and everything else to the sink.
///
/// With no sink installed, events are dropped where they arrive: a queue
/// nobody drains is a memory leak with a schedule.
fn read_frames(
    stream: std::net::TcpStream,
    waiting: Arc<Mutex<std::collections::HashMap<u64, Sender<serde_json::Value>>>>,
    sink: Arc<Mutex<Option<Sink>>>,
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
                    match v.get("id").and_then(|i| i.as_u64()) {
                        Some(id) => {
                            if let Some(tx) = waiting.lock().unwrap().remove(&id) {
                                let _ = tx.send(v);
                            }
                        }
                        None => {
                            let held = sink.lock().unwrap_or_else(|e| e.into_inner());
                            if let (Some(f), Some(method)) =
                                (held.as_ref(), v.get("method").and_then(|m| m.as_str()))
                            {
                                f(Event {
                                    session: v
                                        .get("sessionId")
                                        .and_then(|s| s.as_str())
                                        .map(str::to_string),
                                    method: method.to_string(),
                                    params: v.get("params").cloned().unwrap_or_default(),
                                });
                            }
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

// ── the pages the runtime has open ────────────────────────────────────────

/// The name a page reports through.
///
/// The window gives every document `window.ipc`; there is no such thing here,
/// so one is made: a binding of this name is a real function on the page's
/// global that arrives back as an event. Same reports, same shape, same
/// parsing -- see `pagejs`, which is written to call whatever the host put
/// there.
const BINDING: &str = "__shikisha_ipc";

/// The script every page in this browser is given.
fn page_script() -> String {
    format!(
        "(function () {{ window.__shikisha_post = (s) => {BINDING}(s); }})();\n{}",
        crate::pagejs::AUTOMATION
    )
}

/// One page, and how to reach it.
struct Page {
    browser: Rc<Chrome>,
    session: String,
    target: String,
    /// The storage this page was given, when it is not to share the profile's.
    /// Thrown away with the page, which is what private means
    context: Option<String>,
    /// Whether a relayed mouse button is being held down
    held: bool,
    /// The shape this page's frames had before a viewer asked for its own
    natural: Option<(f64, f64)>,
}

/// What the reading threads write down and the runtime reads back.
#[derive(Default)]
struct Seen {
    /// Reports on their way to the mailbox
    mail: Vec<shikisha_shared::Ev>,
    /// Which page each session belongs to. The reading thread is handed a
    /// session and has to say a name
    named: std::collections::HashMap<String, String>,
    /// The size of the last frame each page cast, which is what a relayed
    /// touch's coordinates are a fraction of
    cast: std::collections::HashMap<String, (f64, f64)>,
    /// Credentials armed for a page's 401s, by page
    auth: std::collections::HashMap<String, (String, String)>,
}

/// Every page the runtime has open in the browser on this machine.
///
/// This is the server's answer to "something that shows pages": no window, no
/// panes, no position -- a page is a name, and everything else about it is the
/// same as it is in the window, because it is the same code (`pageops`) asking
/// the same browser the same questions.
pub struct Pages {
    /// One browser per profile. A profile is a `--user-data-dir`, and a
    /// process has exactly one, so two profiles are two processes
    browsers: std::cell::RefCell<std::collections::HashMap<String, Rc<Chrome>>>,
    open: std::cell::RefCell<std::collections::HashMap<String, Page>>,
    seen: Arc<Mutex<Seen>>,
    refs: Mutex<std::collections::HashMap<Option<String>, Vec<i64>>>,
    /// Pages whose recorder is armed. Like the window, this is remembered
    /// here and re-issued on every new document
    recording: std::cell::RefCell<std::collections::HashSet<Option<String>>>,
    /// Where the profiles live, and whether they outlive the runtime
    root: std::path::PathBuf,
    temporary: bool,
    next_id: AtomicU64,
}

impl Default for Pages {
    fn default() -> Self {
        Self::new()
    }
}

impl Pages {
    /// Pages in profiles that last: a person who signs in on the server signs
    /// in once.
    pub fn new() -> Self {
        Self::under(crate::config::chromium_data_dir(), false)
    }

    /// Pages whose profiles live under a folder of your choosing, and can be
    /// told to go away with them. For a test that wants a real browser and
    /// nobody's real cookies.
    pub fn under(root: std::path::PathBuf, temporary: bool) -> Self {
        Self {
            browsers: std::cell::RefCell::new(std::collections::HashMap::new()),
            open: std::cell::RefCell::new(std::collections::HashMap::new()),
            seen: Arc::new(Mutex::new(Seen::default())),
            refs: Mutex::new(std::collections::HashMap::new()),
            recording: std::cell::RefCell::new(std::collections::HashSet::new()),
            root,
            temporary,
            next_id: AtomicU64::new(1),
        }
    }

    /// Whether anything has actually started a browser yet.
    ///
    /// Nothing starts one until a page is asked for, so a runtime that never
    /// touches the web never pays for a browser it did not use
    pub fn started(&self) -> bool {
        !self.browsers.borrow().is_empty()
    }

    /// Everything the pages have reported since this was last asked.
    ///
    /// A page that has just finished loading is re-dressed on the way past:
    /// navigation replaced the document, taking with it both the recorder's
    /// arming (remembered here, so it can be re-issued) and the digest's refs
    /// (which cannot be re-issued -- they named nodes in a document that is
    /// gone, so dropping them turns a later `{ref=N}` into "take a new digest"
    /// rather than a click on something that no longer exists).
    pub fn drain(&self) -> Vec<shikisha_shared::Ev> {
        use crate::pageops::Speaks as _;
        let mail = std::mem::take(&mut self.seen.lock().unwrap_or_else(|e| e.into_inner()).mail);
        for ev in &mail {
            let shikisha_shared::Ev::Ready { from, .. } = ev else { continue };
            self.refs.lock().unwrap_or_else(|e| e.into_inner()).remove(from);
            if self.recording.borrow().contains(from) {
                let _ = self.eval(
                    from.as_deref(),
                    "window.__shikisha_rec && window.__shikisha_rec(true);",
                    CALL_MS,
                );
            }
        }
        mail
    }

    /// The browser this profile's pages live in, started if this is the first.
    fn browser_for(&self, profile: &shikisha_shared::BrowserProfile) -> anyhow::Result<Rc<Chrome>> {
        // A private page is given its own storage inside the ordinary browser
        // rather than a browser of its own: the isolation is the storage, and
        // a second process would buy nothing but a second process
        let key = if profile.private { "default".to_string() } else { folder_name(&profile.name) };
        if let Some(c) = self.browsers.borrow().get(&key) {
            return Ok(Rc::clone(c));
        }
        let chrome = Rc::new(Chrome::start_in(self.root.join(&key), self.temporary)?);
        self.listen(&chrome);
        self.browsers.borrow_mut().insert(key, Rc::clone(&chrome));
        Ok(chrome)
    }

    /// Take in everything this browser says of its own accord.
    fn listen(&self, chrome: &Rc<Chrome>) {
        let speaker = chrome.speaker();
        let seen = Arc::clone(&self.seen);
        chrome.on_event(move |ev| {
            let Some(session) = ev.session else { return };
            let mut book = seen.lock().unwrap_or_else(|e| e.into_inner());
            let Some(page) = book.named.get(&session).cloned() else { return };
            match ev.method.as_str() {
                "Page.screencastFrame" => {
                    // Acknowledged first: the next frame does not come until
                    // this one is, and everything below is bookkeeping
                    if let Some(n) = ev.params.get("sessionId") {
                        speaker.tell(
                            Some(&session),
                            "Page.screencastFrameAck",
                            serde_json::json!({ "sessionId": n }),
                        );
                    }
                    let meta = ev.params.get("metadata");
                    let size = |k: &str| {
                        meta.and_then(|m| m.get(k)).and_then(serde_json::Value::as_f64).unwrap_or(0.0)
                    };
                    let (w, h) = (size("deviceWidth"), size("deviceHeight"));
                    let data = ev
                        .params
                        .get("data")
                        .and_then(|d| d.as_str())
                        .unwrap_or_default()
                        .to_string();
                    book.cast.insert(page.clone(), (w, h));
                    book.mail.push(shikisha_shared::Ev::Frame {
                        from: Some(page),
                        data,
                        w: w as u32,
                        h: h as u32,
                    });
                }
                // The page said something. It is a page, so it is heard on
                // the same terms a page is heard in the window: reports only
                "Runtime.bindingCalled" => {
                    if ev.params.get("name").and_then(|n| n.as_str()) != Some(BINDING) {
                        return;
                    }
                    let said = ev.params.get("payload").and_then(|p| p.as_str()).unwrap_or("");
                    let Some(intent) = serde_json::from_str(said)
                        .ok()
                        .and_then(|v: serde_json::Value| shikisha_shared::parse_intent(&v))
                    else {
                        return;
                    };
                    if let Some(ev) = from_page(intent, &page) {
                        book.mail.push(ev);
                    }
                }
                // Something wants a password. Answered only where a script
                // armed one for this page, and with "you deal with it"
                // everywhere else -- which is a 401 the page can show
                "Fetch.authRequired" => {
                    let answer = match book.auth.get(&page) {
                        Some((user, pass)) => serde_json::json!({
                            "response": "ProvideCredentials", "username": user, "password": pass,
                        }),
                        None => serde_json::json!({ "response": "Default" }),
                    };
                    speaker.tell(
                        Some(&session),
                        "Fetch.continueWithAuth",
                        serde_json::json!({
                            "requestId": ev.params.get("requestId"),
                            "authChallengeResponse": answer,
                        }),
                    );
                }
                // Arming the credentials above holds every request of this
                // page until it is let through. Nothing here inspects them
                "Fetch.requestPaused" => speaker.tell(
                    Some(&session),
                    "Fetch.continueRequest",
                    serde_json::json!({ "requestId": ev.params.get("requestId") }),
                ),
                _ => {}
            }
        });
    }

    /// The browser and session behind a name.
    fn at(&self, to: Option<&str>) -> anyhow::Result<(Rc<Chrome>, String)> {
        let name = to.ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.no_front")))?;
        let open = self.open.borrow();
        let page = open.get(name).ok_or_else(|| {
            anyhow::anyhow!(crate::i18n::tp("err.chrome.no_such_page", &[("name", name)]))
        })?;
        Ok((Rc::clone(&page.browser), page.session.clone()))
    }

    /// The size of the last frame this page cast, or the shape of the page
    /// itself when it is not being watched.
    ///
    /// A relayed touch arrives as a fraction of the screen it was made on, so
    /// it has to be multiplied by something. The frame is the right something
    /// while a viewer is watching; without one, the page's own viewport is,
    /// because a script driving a page nobody is looking at still means the
    /// middle when it says the middle
    fn scale(&self, name: &str, chrome: &Chrome, session: &str) -> (f64, f64) {
        let watched = self.seen.lock().unwrap_or_else(|e| e.into_inner()).cast.get(name).copied();
        if let Some((w, h)) = watched.filter(|&(w, h)| w >= 1.0 && h >= 1.0) {
            return (w, h);
        }
        let m = chrome.call_page(session, "Page.getLayoutMetrics", serde_json::json!({}));
        let read = |k: &str| {
            m.as_ref()
                .ok()
                .and_then(|v| v.get("cssVisualViewport"))
                .and_then(|v| v.get(k))
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.0)
        };
        (read("clientWidth"), read("clientHeight"))
    }
}

/// A report from a page, with the page's name put on it.
///
/// A page is heard on exactly the terms the window hears one on: it may say
/// what happened to it, and it may not ask for anything. What it claims to be
/// called is not read -- the name comes from the session it arrived on.
fn from_page(ev: shikisha_shared::Ev, page: &str) -> Option<shikisha_shared::Ev> {
    use shikisha_shared::Ev;
    if !shikisha_shared::allowed_from_page(&ev) {
        return None;
    }
    let from = Some(page.to_string());
    Some(match ev {
        Ev::Ready { url, complete, .. } => Ev::Ready { from, url, complete },
        Ev::Loading { busy, .. } => Ev::Loading { from, busy },
        Ev::Touched { .. } => Ev::Touched { from },
        Ev::Compose { .. } => Ev::Compose { from },
        Ev::Recorded { act, sel, value, xpath, hint, .. } => {
            Ev::Recorded { from, act, sel, value, xpath, hint }
        }
        // An answer to an evaluation. Nothing here asks through the binding
        // -- an evaluation is answered by the protocol call that made it --
        // so one arriving is a page volunteering an answer to a question that
        // was never put to it
        Ev::Result { .. } => return None,
        other => other,
    })
}

/// Where a page is in its own history: where it is now, the earliest place it
/// counts as having been, and the whole list.
///
/// The earliest is not always the first. A page here is made empty and
/// navigated afterwards -- that is the only moment there is to put the script
/// and the binding in place -- so the blank page the runtime made is the first
/// entry in every page's history. Nobody was ever on it, and "back" must not
/// go there.
fn history(log: &serde_json::Value) -> (i64, i64, &[serde_json::Value]) {
    static NONE: &[serde_json::Value] = &[];
    let here = log.get("currentIndex").and_then(serde_json::Value::as_i64).unwrap_or(0);
    let entries = log.get("entries").and_then(|e| e.as_array()).map_or(NONE, Vec::as_slice);
    let first = i64::from(
        entries
            .first()
            .and_then(|e| e.get("url"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|u| u == "about:blank"),
    );
    (here, first, entries)
}

/// A profile name, as a folder name: what is left after anything that could
/// name a different folder is taken out.
fn folder_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    let kept = kept.trim_matches('.').to_string();
    if kept.is_empty() { "default".into() } else { kept }
}

impl crate::pageops::Speaks for Pages {
    fn cdp(
        &self,
        to: Option<&str>,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
    ) -> anyhow::Result<serde_json::Value> {
        let (chrome, session) = self.at(to)?;
        chrome.call_within(Some(&session), method, params, timeout_ms)
    }

    /// Run a body of JavaScript in the page.
    ///
    /// Wrapped in an async function and awaited, so a value that is still a
    /// promise -- the result of `fetch`, or of the in-page auto-wait -- is
    /// resolved before it counts as returned. Awaiting a value that is not one
    /// passes it straight through, which is the ordinary case.
    fn eval(&self, to: Option<&str>, js: &str, timeout_ms: u64) -> anyhow::Result<String> {
        let (chrome, session) = self.at(to)?;
        let v = chrome.call_within(
            Some(&session),
            "Runtime.evaluate",
            serde_json::json!({
                "expression": format!("(async function () {{ {js} }})()"),
                "returnByValue": true,
                "awaitPromise": true,
                // A page is allowed to treat this as something a person did:
                // several things (playing sound, opening a window) are refused
                // to a script that nobody asked for
                "userGesture": true,
            }),
            timeout_ms,
        )?;
        if let Some(thrown) = v.get("exceptionDetails") {
            let said = thrown
                .get("exception")
                .and_then(|e| e.get("description"))
                .or_else(|| thrown.get("text"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            anyhow::bail!(crate::i18n::tp("err.browser.js_eval_failed", &[("value", said)]));
        }
        // `undefined` is a value a JS function can return, and the window's
        // side turns it into null. The same here, so both answer alike
        Ok(v.get("result")
            .and_then(|r| r.get("value"))
            .map_or_else(|| "null".to_string(), std::string::ToString::to_string))
    }

    fn refs(&self) -> &Mutex<std::collections::HashMap<Option<String>, Vec<i64>>> {
        &self.refs
    }
}

impl shikisha_shared::BrowserHost for Pages {
    fn go(&self, to: Option<&str>, go: shikisha_shared::Go) -> anyhow::Result<()> {
        use shikisha_shared::Go;
        let (chrome, session) = self.at(to)?;
        match go {
            Go::To(url) => {
                if !shikisha_shared::is_openable(&url) {
                    anyhow::bail!(crate::i18n::tp("err.browser.bad_url", &[("url", &url)]));
                }
                chrome.call_page(&session, "Page.navigate", serde_json::json!({ "url": url }))?;
            }
            Go::Reload => {
                chrome.call_page(&session, "Page.reload", serde_json::json!({ "ignoreCache": false }))?;
            }
            Go::Hard => {
                chrome.call_page(&session, "Page.reload", serde_json::json!({ "ignoreCache": true }))?;
            }
            Go::Back | Go::Forward => {
                let log = chrome.call_page(&session, "Page.getNavigationHistory", serde_json::json!({}))?;
                let (here, first, entries) = history(&log);
                let want = if go == Go::Back { here - 1 } else { here + 1 };
                // Nowhere to go is not a failure. The bar asked, and the
                // answer is that there is nothing behind this page
                if want < first {
                    return Ok(());
                }
                if let Some(entry) = usize::try_from(want).ok().and_then(|i| entries.get(i)) {
                    chrome.call_page(
                        &session,
                        "Page.navigateToHistoryEntry",
                        serde_json::json!({ "entryId": entry.get("id") }),
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Which page input goes to when the browser is asked without being told.
    fn focus(&self, to: Option<&str>) -> anyhow::Result<()> {
        let (chrome, session) = self.at(to)?;
        chrome.call_page(&session, "Page.bringToFront", serde_json::json!({}))?;
        Ok(())
    }

    /// Where this page is, and whether it has anywhere to go back to.
    ///
    /// Asked of the browser's own history rather than of the page: the page
    /// knows the address it is at and nothing about what came before it
    fn ask_where(&self, to: Option<&str>) -> anyhow::Result<()> {
        let name = to.unwrap_or_default().to_string();
        let (chrome, session) = self.at(to)?;
        let log = chrome.call_page(&session, "Page.getNavigationHistory", serde_json::json!({}))?;
        let (here, first, entries) = history(&log);
        let url = usize::try_from(here)
            .ok()
            .and_then(|i| entries.get(i))
            .and_then(|e| e.get("url"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .mail
            .push(shikisha_shared::Ev::Where {
                from: Some(name),
                url,
                can_back: here > first,
                can_forward: here + 1 < entries.len() as i64,
            });
        Ok(())
    }

    /// Answer this page's password prompts with these credentials.
    ///
    /// Armed rather than sent: a 401 is not something to wait for, and the
    /// page it belongs to may be several redirects away. The credentials are
    /// already resolved from a secret before they arrive here
    fn basic_auth(&self, to: Option<&str>, user: &str, pass: &str) -> anyhow::Result<()> {
        let name = to.unwrap_or_default().to_string();
        let (chrome, session) = self.at(to)?;
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .auth
            .insert(name, (user.to_string(), pass.to_string()));
        chrome.call_page(
            &session,
            "Fetch.enable",
            serde_json::json!({ "handleAuthRequests": true }),
        )?;
        Ok(())
    }

    /// Run some JavaScript and don't wait around for it.
    ///
    /// The number handed back is what the window would match an answer up by.
    /// Nothing here waits on one, so a failure would be invisible: it goes in
    /// the log instead of nowhere
    fn eval_in(&self, to: Option<&str>, js: &str) -> anyhow::Result<u64> {
        use crate::pageops::Speaks as _;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Err(e) = self.eval(to, js, CALL_MS) {
            crate::append_hook_log(&format!("chrome: {} からの応答なし: {e}", to.unwrap_or("?")));
        }
        Ok(id)
    }

    /// What a person did somewhere else, done to this page.
    fn inject(&self, to: Option<&str>, input: shikisha_shared::Input) -> anyhow::Result<()> {
        use shikisha_shared::Input;
        let name = to.unwrap_or_default().to_string();
        let (chrome, session) = self.at(to)?;
        let (cw, ch) = self.scale(&name, &chrome, &session);
        match input {
            Input::Mouse { phase, x, y, down } => {
                let held = self.open.borrow().get(&name).is_some_and(|p| p.held);
                let (ev, now) = crate::cdp::mouse_event(&phase, x * cw, y * ch, down, held);
                if let Some(p) = self.open.borrow_mut().get_mut(&name) {
                    p.held = now;
                }
                chrome.call_page(&session, "Input.dispatchMouseEvent", ev)?;
            }
            Input::Wheel { x, y, dx, dy } => {
                let ev = crate::cdp::wheel_event(x * cw, y * ch, dx, dy);
                chrome.call_page(&session, "Input.dispatchMouseEvent", ev)?;
            }
            Input::Text { text } => {
                for ev in crate::cdp::text_events(&text) {
                    chrome.call_page(&session, "Input.dispatchKeyEvent", ev)?;
                }
            }
            Input::Key { named, ctrl, alt } => {
                for ev in crate::cdp::key_events(&named, ctrl, alt) {
                    chrome.call_page(&session, "Input.dispatchKeyEvent", ev)?;
                }
            }
            // A viewer said what shape its screen is. The page is re-shaped to
            // match, so a phone sees a full screen rather than a strip
            Input::View { w, h } => {
                if cw >= 1.0 && ch >= 1.0 {
                    let natural = {
                        let mut open = self.open.borrow_mut();
                        let page = open.get_mut(&name);
                        page.map(|p| *p.natural.get_or_insert((cw, ch)))
                    };
                    if let Some(nat) = natural {
                        match crate::cdp::view_metrics(nat, w, h) {
                            Some(m) => {
                                chrome.call_page(&session, "Emulation.setDeviceMetricsOverride", m)?;
                            }
                            // e.g. turned sideways -- its own shape is right
                            None => {
                                chrome.call_page(
                                    &session,
                                    "Emulation.clearDeviceMetricsOverride",
                                    serde_json::json!({}),
                                )?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Send this page's picture, frame by frame, until told to stop.
    ///
    /// This is how a phone sees the browser on the server: the frames go into
    /// the mailbox and out to whoever is watching, and what comes back the
    /// other way is `inject`
    fn screencast(&self, to: Option<&str>, on: bool) -> anyhow::Result<()> {
        let name = to.unwrap_or_default().to_string();
        let (chrome, session) = self.at(to)?;
        if on {
            let params: serde_json::Value =
                serde_json::from_str(crate::cdp::CAST_PARAMS).unwrap_or_default();
            chrome.call_page(&session, "Page.startScreencast", params)?;
            return Ok(());
        }
        chrome.call_page(&session, "Page.stopScreencast", serde_json::json!({}))?;
        // Give the page its own shape back, if a viewer had reshaped it
        let reshaped = self
            .open
            .borrow_mut()
            .get_mut(&name)
            .and_then(|p| p.natural.take())
            .is_some();
        if reshaped {
            chrome.call_page(
                &session,
                "Emulation.clearDeviceMetricsOverride",
                serde_json::json!({}),
            )?;
        }
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).cast.remove(&name);
        Ok(())
    }

    fn find(&self, to: Option<&str>, sel: &shikisha_shared::Sel, timeout_ms: u64) -> anyhow::Result<shikisha_shared::Found> {
        crate::pageops::find(self, to, sel, timeout_ms)
    }
    fn click(&self, to: Option<&str>, sel: &shikisha_shared::Sel, timeout_ms: u64) -> anyhow::Result<shikisha_shared::OpReport> {
        crate::pageops::click(self, to, sel, timeout_ms)
    }
    fn fill(&self, to: Option<&str>, sel: &shikisha_shared::Sel, value: &str, timeout_ms: u64) -> anyhow::Result<shikisha_shared::OpReport> {
        crate::pageops::fill(self, to, sel, value, timeout_ms)
    }
    fn text(&self, to: Option<&str>, sel: &shikisha_shared::Sel, timeout_ms: u64) -> anyhow::Result<Option<String>> {
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

    /// Open a page under this name.
    ///
    /// The page is made empty and navigated afterwards, because everything
    /// that has to be there before the first document -- the script, the
    /// binding it reports through, who this browser says it is -- can only be
    /// put in place once there is a page to put it in.
    ///
    /// The rectangle is not a place here; there is no screen to have a place
    /// on. Its size is still the page's size, which is what a picture of it
    /// and every coordinate in it are measured in
    fn open_child(
        &self,
        name: &str,
        url: &str,
        rect: (i32, i32, i32, i32),
        profile: shikisha_shared::BrowserProfile,
    ) -> anyhow::Result<()> {
        if !shikisha_shared::is_openable(url) {
            anyhow::bail!(crate::i18n::tp("err.browser.bad_url", &[("url", url)]));
        }
        let chrome = self.browser_for(&profile)?;
        let context = match profile.private {
            true => Some(chrome.new_context()?),
            false => None,
        };
        let (target, session) = chrome.open_blank(context.as_deref())?;
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .named
            .insert(session.clone(), name.to_string());

        let dress = || -> anyhow::Result<()> {
            chrome.call_page(&session, "Page.enable", serde_json::json!({}))?;
            chrome.call_page(&session, "Runtime.enable", serde_json::json!({}))?;
            chrome.call_page(&session, "Runtime.addBinding", serde_json::json!({ "name": BINDING }))?;
            chrome.call_page(
                &session,
                "Page.addScriptToEvaluateOnNewDocument",
                serde_json::json!({ "source": page_script() }),
            )?;
            if let Some(ua) = profile.user_agent.as_deref() {
                chrome.call_page(
                    &session,
                    "Network.setUserAgentOverride",
                    serde_json::json!({ "userAgent": ua }),
                )?;
            }
            if rect.2 > 0 && rect.3 > 0 {
                chrome.call_page(
                    &session,
                    "Emulation.setDeviceMetricsOverride",
                    serde_json::json!({
                        "width": rect.2, "height": rect.3,
                        "deviceScaleFactor": 0, "mobile": false,
                    }),
                )?;
            }
            Ok(())
        };
        if let Err(e) = dress() {
            let _ = chrome.call("Target.closeTarget", serde_json::json!({ "targetId": target }));
            if let Some(c) = context.as_deref() {
                chrome.close_context(c);
            }
            self.seen.lock().unwrap_or_else(|x| x.into_inner()).named.remove(&session);
            return Err(e);
        }

        self.open.borrow_mut().insert(
            name.to_string(),
            Page {
                browser: Rc::clone(&chrome),
                session: session.clone(),
                target,
                context,
                held: false,
                natural: None,
            },
        );
        chrome.call_page(&session, "Page.navigate", serde_json::json!({ "url": url }))?;
        Ok(())
    }

    /// How big the page is. There is nowhere to put it, so where is ignored.
    ///
    /// A width or height of nothing means the window has hidden the page
    /// behind another one; a browser with no window has nothing to hide it
    /// behind, and the page keeps the size it had
    fn child_bounds(&self, name: &str, rect: (i32, i32, i32, i32)) -> anyhow::Result<()> {
        if rect.2 <= 0 || rect.3 <= 0 {
            return Ok(());
        }
        let (chrome, session) = self.at(Some(name))?;
        chrome.call_page(
            &session,
            "Emulation.setDeviceMetricsOverride",
            serde_json::json!({
                "width": rect.2, "height": rect.3, "deviceScaleFactor": 0, "mobile": false,
            }),
        )?;
        Ok(())
    }

    fn close_child(&self, name: &str) -> anyhow::Result<()> {
        let Some(page) = self.open.borrow_mut().remove(name) else {
            return Ok(());
        };
        let _ = page.browser.call(
            "Target.closeTarget",
            serde_json::json!({ "targetId": page.target }),
        );
        // A private page's storage goes with it. That is the whole promise
        if let Some(c) = page.context.as_deref() {
            page.browser.close_context(c);
        }
        let mut book = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        book.named.remove(&page.session);
        book.cast.remove(name);
        book.auth.remove(name);
        self.refs.lock().unwrap_or_else(|e| e.into_inner()).remove(&Some(name.to_string()));
        Ok(())
    }

    /// Nothing to trust.
    ///
    /// In the window this says "pages from here are the app's own, and may ask
    /// for things a site may not". There is no app page in this browser -- the
    /// board is not drawn here, it is served over the network -- so every page
    /// in it is a site, and every one of them is heard as one
    fn trust(&self, url: &str) -> anyhow::Result<()> {
        let _ = url;
        Ok(())
    }

    /// Arm or silence the recorder on a page.
    ///
    /// Like the window: whether it should be recording is remembered here, not
    /// in the page, because the page is replaced on every navigation
    fn record(&self, to: Option<&str>, on: bool) -> anyhow::Result<()> {
        use crate::pageops::Speaks as _;
        let key = to.map(str::to_string);
        if on {
            self.recording.borrow_mut().insert(key);
        } else {
            self.recording.borrow_mut().remove(&key);
        }
        self.eval(
            to,
            &format!("window.__shikisha_rec && window.__shikisha_rec({on});"),
            CALL_MS,
        )
        .map(|_| ())
    }

    /// There is only ever one recorder, so arming a page silences the rest.
    fn record_all_off(&self) {
        use crate::pageops::Speaks as _;
        let armed: Vec<Option<String>> = self.recording.borrow_mut().drain().collect();
        for page in armed {
            let _ = self.eval(
                page.as_deref(),
                "window.__shikisha_rec && window.__shikisha_rec(false);",
                CALL_MS,
            );
        }
    }
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

    /// The browser this program fetches for itself, on a machine with none.
    ///
    /// Slow the first time -- it is a few hundred megabytes -- and instant
    /// afterwards, which is the part a runtime depends on: the fetch is what
    /// gets called, and it has to cost nothing once the browser is there.
    ///
    ///     cargo test -p shikisha-core --lib gets_one -- --ignored --nocapture
    #[test]
    #[ignore = "fetches a browser (a few hundred megabytes, once)"]
    fn a_machine_with_no_browser_gets_one() {
        let started = std::time::Instant::now();
        let exe = fetch().expect("ブラウザを取得できない");
        println!("browser: {} ({:?})", exe.display(), started.elapsed());
        assert!(exe.is_file(), "取得したはずの場所に無い");

        // Asked again, it hands back the same one without fetching anything
        let again = std::time::Instant::now();
        assert_eq!(fetch().expect("2度目"), exe);
        assert!(
            again.elapsed() < std::time::Duration::from_secs(2),
            "2度目も落としに行っている: {:?}",
            again.elapsed()
        );

        // The finder prefers it over whatever else is on this machine --
        // a known version being the whole reason it was fetched
        assert_eq!(found().as_deref(), Some(exe.as_path()), "自前のものを選んでいない");

        // And it is a browser: it starts, and it is the version that was asked for
        let chrome = Chrome::start().expect("取得したブラウザが起動しない");
        let said = chrome.call("Browser.getVersion", serde_json::json!({})).expect("版を答えない");
        let product = said.get("product").and_then(|v| v.as_str()).unwrap_or("");
        assert!(product.contains(PINNED), "固定した版ではない: {product}");
        println!("version: {product}");
    }

    /// A profile name cannot become a different folder.
    #[test]
    fn a_profile_name_is_only_ever_one_folder() {
        assert_eq!(folder_name("work"), "work");
        assert_eq!(folder_name("../../etc"), "etc");
        assert_eq!(folder_name("a/b"), "ab");
        assert_eq!(folder_name(""), "default");
        assert_eq!(folder_name("..."), "default");
        assert_ne!(folder_name("work"), folder_name("home"), "別名が同じ場所になる");
    }

    /// A page may say what happened to it, and may not ask for anything --
    /// and what it says it is called is not read: the name comes from the
    /// session the report arrived on
    #[test]
    fn a_page_is_heard_only_as_a_report_and_never_about_its_own_name() {
        use shikisha_shared::Ev;
        let told = from_page(
            Ev::Ready { from: Some("嘘".into()), url: "https://example.com/".into(), complete: true },
            "p",
        );
        assert!(
            matches!(told, Some(Ev::Ready { from: Some(ref n), .. }) if n == "p"),
            "報告に別の名前を名乗らせている: {told:?}"
        );
        // Asking for something is not a report
        assert!(from_page(Ev::Say { tab: 0, text: "cat /etc/shadow".into() }, "p").is_none());
        // Nothing is asked through the binding, so nothing answers through it
        assert!(
            from_page(Ev::Result { id: 1, ok: true, value: "\"y\"".into() }, "p").is_none(),
            "誰も聞いていない答えを受け取っている"
        );
    }

    /// The whole host, against a real browser: open a page, look at it, act on
    /// it, and be answered the way the window answers.
    ///
    /// Ignored by default, and run where a browser is installed:
    ///
    ///     cargo test -p shikisha-core --lib chrome:: -- --ignored --nocapture
    #[test]
    #[ignore = "needs a browser on this machine"]
    fn a_page_on_this_machine_answers_everything_the_window_would() {
        use crate::pageops;
        use shikisha_shared::{BrowserHost, BrowserProfile, Found, Go, Input, Sel};

        assert!(found().is_some(), "この機械にブラウザが無い");
        let site = serve();
        let store = std::env::temp_dir().join(format!("shikisha-pages-{}", crate::random_hex(8)));
        let pages = Pages::under(store, true);
        let t = 10_000;

        pages
            .open_child("p", &site, (0, 0, 900, 700), BrowserProfile::shared_default())
            .expect("ページが開かない");

        // The page reports for itself, through a binding that stands where
        // the window's channel stands. Nothing below works if this does not
        let (url, complete) = wait_ready(&pages).expect("ページが読み込み完了を報告しない");
        assert_eq!(url, site, "違う住所を報告している");
        assert!(complete, "画像や CSS を待たずに完了と言っている");

        // ── looking ──────────────────────────────────────────────────────
        assert_eq!(pages.find(Some("p"), &Sel::Css("#here".into()), t).unwrap(), Found::Visible);
        assert_eq!(pages.find(Some("p"), &Sel::Css("#far".into()), t).unwrap(), Found::OffScreen);
        assert_eq!(pages.find(Some("p"), &Sel::Css("#nope".into()), t).unwrap(), Found::NotFound);
        assert_eq!(pages.href(Some("p"), t).unwrap(), site);
        assert!(pages.html(Some("p"), t).unwrap().contains("ここにいる"));
        assert_eq!(
            pages.text(Some("p"), &Sel::Css("#here".into()), t).unwrap().as_deref(),
            Some("ここにいる")
        );

        // ── acting ───────────────────────────────────────────────────────
        assert_eq!(
            pages.click(Some("p"), &Sel::Css("#go".into()), t).unwrap().state,
            Found::Visible
        );
        assert_eq!(
            pages.text(Some("p"), &Sel::Css("#log".into()), t).unwrap().as_deref(),
            Some("押された"),
            "押したことになっていない"
        );
        // The crux of it: a value must never become code. This is what AI
        // output, and text read straight off a page, look like at their worst
        let nasty = "'; window.__pwned = 1; //\"</script><img src=x onerror=alert(1)>\\";
        assert_eq!(
            pages.fill(Some("p"), &Sel::Css("#q".into()), nasty, t).unwrap().state,
            Found::Visible
        );
        assert_eq!(
            pages.text(Some("p"), &Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some(nasty),
            "入れた値が変わって返ってくる"
        );

        // ── the digest, and the refs it hands out ────────────────────────
        let digest = pages.digest(Some("p"), t).expect("digest が取れない");
        assert!(digest.contains("押す"), "盤面に見えるものが digest に無い: {digest}");
        let number = |label: &str| -> u32 {
            digest
                .lines()
                .find(|l| l.contains(label))
                .and_then(|l| l.split(']').next())
                .and_then(|l| l.trim_start_matches('[').trim().parse().ok())
                .unwrap_or_else(|| panic!("{label} の ref が digest に無い:\n{digest}"))
        };
        // A genuine mouse event, which is the whole reason refs exist
        let rep = pages.click(Some("p"), &Sel::Ref(number("触る")), t).expect("ref を押せない");
        assert_eq!(rep.state, Found::Visible);
        assert!(rep.echo.unwrap_or_default().contains("触る"), "何を押したか答えていない");
        assert_eq!(
            pages.text(Some("p"), &Sel::Css("#tapped".into()), t).unwrap().as_deref(),
            Some("触られた"),
            "本物の入力が届いていない"
        );
        // And a ref that no longer means anything says so
        let err = pages.click(Some("p"), &Sel::Ref(999), t).unwrap_err().to_string();
        assert!(!err.is_empty(), "無い ref を黙って押している");

        // ── carrying a login from one place to another ───────────────────
        let cookies = pages.cookies_out(Some("p"), t).expect("cookie が読めない");
        assert!(
            cookies.as_array().is_some_and(|c| {
                c.iter().any(|k| k.get("value").and_then(|n| n.as_str()) == Some("mark"))
            }),
            "ブラウザが持っている cookie が出てこない: {cookies}"
        );
        let store = pages.storage_out(Some("p"), t).expect("localStorage が読めない");
        assert!(store.to_string().contains("しるし"), "localStorage が出てこない: {store}");
        pages.cookies_in(Some("p"), &cookies, t).expect("cookie が戻せない");
        pages.storage_in(Some("p"), &store, t).expect("localStorage が戻せない");

        // ── a picture of it, which is what a phone is shown ──────────────
        let png = pages.snapshot(Some("p"), t).expect("撮れない");
        assert!(png.len() > 2_000, "絵が小さすぎる: {} バイト", png.len());
        assert_eq!(&png[..4], b"\x89PNG", "PNG ではない");

        // ── a request made from inside the page ──────────────────────────
        let got = pages
            .fetch(Some("p"), &format!("{site}next"), &serde_json::json!({}), t)
            .expect("fetch できない");
        assert!(got.contains("\"status\":200"), "fetch の答えが変: {got}");
        assert!(got.contains("次の画面"), "本文が入っていない: {got}");

        // ── being watched, and touched from wherever it is watched ───────
        pages.screencast(Some("p"), true).expect("配信が始まらない");
        let (w, h) = wait_frame(&pages).expect("絵が一枚も来ない");
        assert!(w >= 1 && h >= 1, "絵の寸法が無い");
        // Where something is, as a fraction of the screen -- which is how a
        // finger on a phone arrives, having touched a picture of the page
        let spot = |id: &str| -> Vec<f64> {
            let js = format!(
                "const r = document.getElementById('{id}').getBoundingClientRect();\
                 return [(r.x + r.width / 2) / innerWidth, (r.y + r.height / 2) / innerHeight];"
            );
            serde_json::from_str(&pageops::Speaks::eval(&pages, Some("p"), &js, t).unwrap()).unwrap()
        };
        let tap = |at: &[f64]| {
            for phase in ["pressed", "released"] {
                pages
                    .inject(
                        Some("p"),
                        Input::Mouse { phase: phase.into(), x: at[0], y: at[1], down: false },
                    )
                    .expect("触れない");
            }
        };
        tap(&spot("reach"));
        assert_eq!(
            pages.text(Some("p"), &Sel::Css("#reached".into()), t).unwrap().as_deref(),
            Some("届いた"),
            "遠くからの指が届いていない"
        );
        // Typing arrives the same way, and lands wherever the last touch put
        // the cursor -- which is what makes a phone able to fill in a form
        pages.fill(Some("p"), &Sel::Css("#q".into()), "", t).unwrap();
        tap(&spot("q"));
        pages.inject(Some("p"), Input::Text { text: "遠くから".into() }).expect("打てない");
        assert_eq!(
            pages.text(Some("p"), &Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some("遠くから"),
            "打った文字が入っていない"
        );
        pages.screencast(Some("p"), false).expect("配信が止まらない");

        // ── where it is, and where it has been ───────────────────────────
        pages.ask_where(Some("p")).unwrap();
        let (at, back) = wait_where(&pages).expect("今どこかを答えない");
        assert_eq!(at, site);
        assert!(!back, "最初のページなのに戻れると言っている");
        pages.go(Some("p"), Go::To(format!("{site}next"))).expect("移動できない");
        wait_ready(&pages).expect("移動先が読み込み完了を報告しない");
        assert!(pages.href(Some("p"), t).unwrap().ends_with("/next"));
        pages.ask_where(Some("p")).unwrap();
        let (_, back) = wait_where(&pages).expect("移動後にどこかを答えない");
        assert!(back, "1つ前があるのに戻れないと言っている");
        pages.go(Some("p"), Go::Back).expect("戻れない");
        wait_ready(&pages).expect("戻った先が読み込み完了を報告しない");
        assert_eq!(pages.href(Some("p"), t).unwrap(), site, "戻っていない");

        pages.close_child("p").expect("閉じられない");
        assert!(
            pages.find(Some("p"), &Sel::Css("#here".into()), t).is_err(),
            "閉じたページがまだ操作できる"
        );
        println!("すべて通過");
    }

    /// The seam: a runtime with no window, asked for something that shows
    /// pages, is handed this -- and what a page says lands in the mailbox the
    /// loop reads, without the loop knowing any of the above.
    #[test]
    #[ignore = "needs a browser on this machine"]
    fn a_runtime_with_no_window_is_handed_this_browser() {
        use crate::host::Shell as _;
        use shikisha_shared::{BrowserProfile, Sel};

        assert!(found().is_some(), "この機械にブラウザが無い");
        let site = serve();
        let store = std::env::temp_dir().join(format!("shikisha-seam-{}", crate::random_hex(8)));
        let mut shell = crate::host::Headless::browsing(24, 80, Rc::new(Pages::under(store, true)));

        let (host, rect) = shell.host().expect("窓の無いランタイムがページを断っている");
        assert!(rect.2 > 0 && rect.3 > 0, "ページの寸法が無い: {rect:?}");
        host.open_child("p", &site, rect, BrowserProfile::shared_default())
            .expect("ページが開かない");

        // The loop's own move: wait a moment, and see what arrived
        let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < until && shell.mail().loads.is_empty() {
            shell.poll(std::time::Duration::from_millis(200), None).unwrap();
        }
        let loaded = std::mem::take(&mut shell.mail().loads);
        assert!(
            loaded.iter().any(|(name, url, _)| name == "p" && url == &site),
            "読み込み完了が郵便受けに届いていない: {loaded:?}"
        );
        assert_eq!(
            host.text(Some("p"), &Sel::Css("#here".into()), 10_000).unwrap().as_deref(),
            Some("ここにいる")
        );
        host.close_child("p").unwrap();
    }

    /// Wait for a page to say it has finished loading.
    fn wait_ready(pages: &Pages) -> Option<(String, bool)> {
        waited(pages, |ev| match ev {
            shikisha_shared::Ev::Ready { from: Some(n), url, complete } if n == "p" => {
                Some((url.clone(), *complete))
            }
            _ => None,
        })
    }

    fn wait_frame(pages: &Pages) -> Option<(u32, u32)> {
        waited(pages, |ev| match ev {
            shikisha_shared::Ev::Frame { w, h, data, .. } if !data.is_empty() => Some((*w, *h)),
            _ => None,
        })
    }

    fn wait_where(pages: &Pages) -> Option<(String, bool)> {
        waited(pages, |ev| match ev {
            shikisha_shared::Ev::Where { url, can_back, .. } => Some((url.clone(), *can_back)),
            _ => None,
        })
    }

    /// Drain the reports until one of them is the one being waited for.
    fn waited<T>(pages: &Pages, want: impl Fn(&shikisha_shared::Ev) -> Option<T>) -> Option<T> {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < until {
            for ev in pages.drain() {
                if let Some(got) = want(&ev) {
                    return Some(got);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        None
    }

    /// A site of two pages on 127.0.0.1, which is what the browser is pointed
    /// at: `data:` and `file:` are not pages a runtime will open, and a test
    /// that reaches the real network is a test that fails on a train.
    fn serve() -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = if req.url().starts_with("/next") { NEXT } else { PAGE };
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

    const PAGE: &str = r#"<!doctype html><meta charset=utf-8><title>ためし</title><body>
<div id=here>ここにいる</div>
<input id=q value="">
<button id=go onclick="document.getElementById('log').textContent='押された'">押す</button>
<div id=log></div>
<button id=tap onclick="document.getElementById('tapped').textContent='触られた'">触る</button>
<div id=tapped></div>
<button id=reach onclick="document.getElementById('reached').textContent='届いた'">遠くから</button>
<div id=reached></div>
<div style="height:4000px"></div>
<div id=far>ずっと下</div>
<script>
  // Plain ASCII on purpose: a cookie value with anything else in it cannot
  // be sent as a header, and every request the page makes afterwards fails
  // with "Failed to fetch". The browser's rule, found by this test
  document.cookie = "who=mark; path=/";
  localStorage.setItem("mark", "しるし");
</script>
"#;

    const NEXT: &str = "<!doctype html><meta charset=utf-8><title>つぎ</title><body><div id=next>次の画面</div>";

    /// The address a browser prints is turned into somewhere to connect
    #[test]
    fn the_browsers_address_is_split_into_host_and_path() {
        let (host, path) = split_ws("ws://127.0.0.1:9222/devtools/browser/abc-123").unwrap();
        assert_eq!(host, "127.0.0.1:9222");
        assert_eq!(path, "/devtools/browser/abc-123");
        assert!(split_ws("http://127.0.0.1:9222/").is_err(), "ws でない住所を受けた");
    }
}

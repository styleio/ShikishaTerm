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

use std::io::Write;
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
    // A Mac's browsers are apps, in the Applications folder of the machine or
    // of the person, and never on PATH
    #[cfg(target_os = "macos")]
    for root in [Some(std::path::PathBuf::from("/Applications")), crate::home_dir().map(|h| h.join("Applications"))]
        .into_iter()
        .flatten()
    {
        for app in [
            "Google Chrome.app/Contents/MacOS/Google Chrome",
            "Chromium.app/Contents/MacOS/Chromium",
            "Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "Brave Browser.app/Contents/MacOS/Brave Browser",
        ] {
            let p = root.join(app);
            if p.is_file() {
                return Some(p);
            }
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

// The sums below were taken by streaming each published zip through sha256 on
// 2026-09-12. They belong to the version above and to no other, which is why
// they sit beside it: changing one without the other is how a check becomes a
// refusal to start.

/// A build of that version, for one shape of machine.
struct Build {
    /// What the download calls this machine
    platform: &'static str,
    /// Where the browser is inside the zip
    exe: &'static str,
    /// What the zip came to when it was measured here, as a check that what
    /// arrives is what was measured. `None` for a platform nobody has
    /// measured, where the transport is all the assurance there is
    sha256: Option<&'static str>,
}

/// The build for this machine, or nothing where no build is published.
fn build_for_this_machine() -> Option<Build> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Some(Build {
        platform: "linux64",
        exe: "chrome-linux64/chrome",
        sha256: Some("167a098c4fdec156b58a9f678c90a84f9072d789f9c6e7b35496a6987b8b7ef8"),
    });
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Some(Build {
        platform: "linux-arm64",
        exe: "chrome-linux-arm64/chrome",
        sha256: Some("dfc4955719c5d494c8507990506d2d5bed174c31bf89266aa2dc5593c6607e8b"),
    });
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Some(Build {
        platform: "win64",
        exe: "chrome-win64/chrome.exe",
        sha256: Some("8edfaa0923c11a30a9315a5e7e5794c5efb60146edea7e3f749f7fdc2aa026cb"),
    });
    // A Mac's is an app inside the zip, held together by links
    // (`update::unpack` makes them links again). Measured 2026-10-08
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Some(Build {
        platform: "mac-arm64",
        exe: "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        sha256: Some("1f701ef60757c63c6ccf98afaf28291dd0c8d1457d3d738e81fd62201c230ad0"),
    });
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Some(Build {
        platform: "mac-x64",
        exe: "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
        sha256: Some("cddfd83fadf88808fb036f44282488b1cf3f1b50efacce470225ae41f14b0302"),
    });
    #[cfg(not(any(
        all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "macos", any(target_arch = "x86_64", target_arch = "aarch64"))
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
    /// Where a DevTools screen for one of its pages connects, once one has
    /// been asked for (see [`ScreenDoor`]). `None` inside: it could not be
    /// opened
    door: std::sync::OnceLock<Option<ScreenDoor>>,
    /// Set when the browser is being let go, for the door to stop answering
    closing: Arc<std::sync::atomic::AtomicBool>,
    /// What its pages are told to call themselves when no tab says
    /// otherwise: its own name without the word that says it has no window
    /// (`useragent::with_a_window`). `None` when its name does not say so
    calls_itself: Option<String>,
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
        // A browser left behind on this profile by a run that was killed
        // would keep the profile to itself, and this one would never say it
        // had started
        if !temporary {
            reclaim(&profile);
        }

        // Spoken to over two pipes only this program and the browser hold,
        // not over a port. A port on the loopback answers every program of
        // every account on the machine, and whoever reaches it drives every
        // page in the browser -- the signed-in ones included -- with nothing
        // asked of them
        let wire = Wire::new()?;
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--headless=new")
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
        // A Mac's keychain is asked for the key a profile's cookies are kept
        // under, and asking it is a window of the system's own -- "Google
        // Chrome for Testing wants to use Chromium Safe Storage", password
        // field and all -- put up by a browser nobody can see, over whatever
        // the person was doing. This browser has no person: its profiles are
        // automation's, most of them gone when it stops, so its key is a fixed
        // one kept by the browser itself
        if cfg!(target_os = "macos") {
            cmd.arg("--use-mock-keychain");
        }
        if as_root() {
            crate::append_hook_log(
                "chrome: running as root, so the browser's own sandbox is off (it refuses to start otherwise)",
            );
            cmd.arg("--no-sandbox");
        }
        wire.give(&mut cmd);
        // Its ends are its own once it has started; held here as well, the
        // pipe would never read as closed when the browser goes
        let (mut child, to, from) = wire.spawn(crate::detach_console(&mut cmd))?;
        let said = first_words(&mut child);

        // There when it answers: nothing is announced on a pipe, so it is
        // asked something it can always answer
        let started = Cdp::over(to, from).and_then(|cdp| {
            match cdp.call(None, "Browser.getVersion", serde_json::json!({}), START_MS) {
                Ok(v) => Ok((cdp, v.get("userAgent").and_then(|u| u.as_str()).and_then(crate::useragent::with_a_window))),
                Err(_) => {
                    // What it said about why, in its own words, rather than
                    // "it did not answer in time" -- given a moment to arrive
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let said = said.lock().unwrap_or_else(|e| e.into_inner()).join("\n");
                    Err(match (said.is_empty(), cdp.alive.load(Ordering::Relaxed)) {
                        (false, _) => anyhow::anyhow!(crate::i18n::tp("err.chrome.refused", &[("said", &said)])),
                        (true, false) => anyhow::anyhow!(crate::i18n::t("err.chrome.silent")),
                        (true, true) => anyhow::anyhow!(crate::i18n::t("err.chrome.slow")),
                    })
                }
            }
        });
        let (cdp, calls_itself) = match started {
            Ok(c) => c,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                if temporary {
                    let _ = std::fs::remove_dir_all(&profile);
                }
                return Err(e);
            }
        };
        Ok(Self { child, profile, temporary, cdp, door: Default::default(), closing: Default::default(), calls_itself })
    }

    /// Whether the browser is still there to be spoken to. It can go on its
    /// own: killed for the memory it took, crashed, stopped by somebody
    pub fn alive(&self) -> bool {
        self.cdp.alive.load(Ordering::Relaxed)
    }

    /// Where a DevTools screen for the page `target` is opened, in this same
    /// browser: its own screen, pointed at this program's door for it (see
    /// [`ScreenDoor`]). The door is opened the first time a screen is asked
    /// for, so a browser nobody inspects answers on no port at all
    pub fn devtools_screen(&self, target: &str) -> anyhow::Result<String> {
        let door = self
            .door
            .get_or_init(|| match ScreenDoor::open(self.cdp.clone(), Arc::clone(&self.closing)) {
                Ok(door) => Some(door),
                Err(e) => {
                    crate::append_hook_log(&format!("chrome: the door for DevTools screens could not be opened: {e}"));
                    None
                }
            })
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.no_devtools")))?;
        Ok(door.address(target))
    }

    /// Whether an address is a DevTools screen this browser was asked for
    fn made_screen(&self, url: &str) -> bool {
        self.door.get().and_then(Option::as_ref).is_some_and(|d| d.made(url))
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
    // SAFETY: only reads this process's own user id
    unsafe { libc::geteuid() == 0 }
}

#[cfg(not(unix))]
fn as_root() -> bool {
    false
}

/// End a browser that an earlier run left on this profile.
///
/// A run that is killed outright (out of memory, `kill -9`, the machine's
/// own doing) never gets to close its browser, and the browser goes on
/// without it. The next browser started on the same profile finds the
/// profile held and hands its work to the one holding it -- which nobody is
/// connected to -- so the start waits its whole time and fails, every time,
/// until somebody finds the stray process.
///
/// Only one nobody owns is ended: a browser on this profile whose starter is
/// gone (its parent is no longer a program of this name). One that another
/// run of this program started and still holds is left to that run
#[cfg(unix)]
fn reclaim(profile: &std::path::Path) {
    let named = format!("--user-data-dir={}", profile.display());
    let ours = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
    let table = crate::repo::process_table(None, sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always));
    let strays: Vec<sysinfo::Pid> = table
        .processes()
        .iter()
        .filter(|(_, p)| is_the_browser_on(p.cmd(), &named))
        .filter(|(_, p)| {
            let owner = p.parent().and_then(|pp| table.process(pp)).map(|pp| pp.name().to_string_lossy().into_owned());
            owner.is_none() || owner != ours
        })
        .map(|(pid, _)| *pid)
        .collect();
    for pid in strays {
        crate::append_hook_log(&format!(
            "chrome: a browser left on {} by a run that is gone (pid {pid}) is ended, so that this one can start",
            profile.display()
        ));
        // SAFETY: a signal to a process id read from the table a moment ago
        unsafe {
            libc::kill(pid.as_u32() as libc::pid_t, libc::SIGTERM);
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let gone = || unsafe { libc::kill(pid.as_u32() as libc::pid_t, 0) != 0 };
        while !gone() && std::time::Instant::now() < until {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        if !gone() {
            // SAFETY: as above
            unsafe {
                libc::kill(pid.as_u32() as libc::pid_t, libc::SIGKILL);
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
    }
}

/// Whether a command line is that of the browser itself on a profile: it
/// names the profile, and is not one of the helpers the browser starts
/// (which say what kind they are and go when it does). Read as one line: a
/// browser rewrites its own command line, and what the system hands back
/// is sometimes the words and sometimes all of them run together
#[cfg(any(unix, test))]
fn is_the_browser_on(cmd: &[std::ffi::OsString], named: &str) -> bool {
    let line = cmd.iter().map(|w| w.to_string_lossy()).collect::<Vec<_>>().join(" ");
    let names_it = line.match_indices(named).any(|(at, _)| line[at + named.len()..].chars().next().is_none_or(|c| c == ' '));
    names_it && !line.contains("--type=")
}

/// Windows ends a browser with the run that started it (the job it is in),
/// so none is left to find
#[cfg(not(unix))]
fn reclaim(_profile: &std::path::Path) {}

impl Drop for Chrome {
    fn drop(&mut self) {
        // Asked first, killed second. A browser told to close writes its
        // profile out; one that is killed leaves a lock file that the next
        // start of the same profile spends ten seconds deciding about
        let _ = self.call("Browser.close", serde_json::json!({}));
        // The door stops answering: told, then woken to hear it
        self.closing.store(true, Ordering::Relaxed);
        if let Some(door) = self.door.get().and_then(Option::as_ref) {
            let _ = std::net::TcpStream::connect(("127.0.0.1", door.port));
        }
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

/// How long a browser just started is given to answer
const START_MS: u64 = 20_000;

/// The first things a browser says on its error stream, kept so that one
/// which refuses to start can say why in its own words. Read to the end and
/// otherwise thrown away: a stream nobody reads fills, and the browser
/// stops at its next word
fn first_words(child: &mut std::process::Child) -> Arc<Mutex<Vec<String>>> {
    use std::io::BufRead as _;
    let said: Arc<Mutex<Vec<String>>> = Default::default();
    if let Some(err) = child.stderr.take() {
        let kept = Arc::clone(&said);
        std::thread::spawn(move || {
            // By the piece and not by the line: what a browser says is not
            // always text, and one bad byte must not end the reading
            for line in std::io::BufReader::new(err).split(b'\n').map_while(Result::ok) {
                let mut kept = kept.lock().unwrap_or_else(|e| e.into_inner());
                if kept.len() < 20 {
                    kept.push(String::from_utf8_lossy(&line).trim_end().to_string());
                }
            }
        });
    }
    said
}

// ── the line to the browser ───────────────────────────────────────────────

/// The two pipes a browser is spoken to over: one it reads what is said to
/// it from, one it writes what it says to. Made before it starts, since it
/// has to start holding its ends
struct Wire {
    to: std::fs::File,
    from: std::fs::File,
    theirs: Theirs,
}

/// The browser's ends of the pipes: the one it reads, the one it writes
#[cfg(unix)]
struct Theirs(std::os::fd::OwnedFd, std::os::fd::OwnedFd);

/// The same, as the numbers Windows knows them by
#[cfg(windows)]
struct Theirs(End, End);

/// One end of a pipe, closed when let go of
#[cfg(windows)]
struct End(usize);

#[cfg(windows)]
impl Drop for End {
    fn drop(&mut self) {
        // SAFETY: made by `CreatePipe` below and closed nowhere else
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0 as _);
        }
    }
}

#[cfg(unix)]
impl Wire {
    fn new() -> std::io::Result<Wire> {
        use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};
        let pipe = || -> std::io::Result<(OwnedFd, OwnedFd)> {
            let mut ends = [0 as libc::c_int; 2];
            // SAFETY: two numbers written into the array given
            if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: both were just made and belong to nobody else
            let (r, w) = unsafe { (OwnedFd::from_raw_fd(ends[0]), OwnedFd::from_raw_fd(ends[1])) };
            // Not handed to whatever else this program starts: an end held
            // by a stranger is a pipe that never reads as closed
            for end in [&r, &w] {
                // SAFETY: a flag set on a descriptor this owns
                unsafe {
                    libc::fcntl(end.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
                }
            }
            Ok((r, w))
        };
        let (their_in, to) = pipe()?;
        let (from, their_out) = pipe()?;
        Ok(Wire { to: to.into(), from: from.into(), theirs: Theirs(their_in, their_out) })
    }

    /// Have the browser start holding its ends where it looks for them: the
    /// third and fourth of its descriptors
    fn give(&self, cmd: &mut std::process::Command) {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::process::CommandExt as _;
        cmd.arg("--remote-debugging-pipe");
        let (r, w) = (self.theirs.0.as_raw_fd(), self.theirs.1.as_raw_fd());
        // SAFETY: runs in the child between fork and exec, and calls only
        // what may be called there
        unsafe {
            cmd.pre_exec(move || {
                // Out of the way first: either may be sitting on 3 or 4
                let (r, w) = (libc::fcntl(r, libc::F_DUPFD, 5), libc::fcntl(w, libc::F_DUPFD, 5));
                if r < 0 || w < 0 || libc::dup2(r, 3) < 0 || libc::dup2(w, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(r);
                libc::close(w);
                Ok(())
            });
        }
    }
}

#[cfg(windows)]
impl Wire {
    fn new() -> std::io::Result<Wire> {
        use std::os::windows::io::FromRawHandle as _;
        use windows_sys::Win32::Foundation::HANDLE;
        // Neither end is one a program started by this one would be handed:
        // the browser's are made so only for the moment it starts (`spawn`)
        let pipe = |ours_reads: bool| -> std::io::Result<(std::fs::File, End)> {
            let (mut r, mut w): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
            // SAFETY: two handles written into the places given
            if unsafe { windows_sys::Win32::System::Pipes::CreatePipe(&mut r, &mut w, std::ptr::null(), 0) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let (ours, theirs) = if ours_reads { (r, w) } else { (w, r) };
            // SAFETY: just made, and owned by the file from here
            Ok((unsafe { std::fs::File::from_raw_handle(ours as _) }, End(theirs as usize)))
        };
        let (to, their_in) = pipe(false)?;
        let (from, their_out) = pipe(true)?;
        Ok(Wire { to, from, theirs: Theirs(their_in, their_out) })
    }

    /// Have the browser start holding its ends, and told which they are
    fn give(&self, cmd: &mut std::process::Command) {
        cmd.arg("--remote-debugging-pipe").arg(format!("--remote-debugging-io-pipes={},{}", self.theirs.0.0, self.theirs.1.0));
    }
}

impl Wire {
    /// Start the browser holding its ends, and hand back this side's: what
    /// is written to it, and what is read from it. The browser's are let go
    /// of here.
    ///
    /// On Windows a program is started holding every handle of this one
    /// that is marked to be handed on -- any program, started by any thread.
    /// The browser's ends are marked only for as long as its own start
    /// takes, so that a tab or a git started at the same moment is not
    /// handed the browser's line too. That moment cannot be closed
    /// altogether from here: it would take every start in this program
    /// naming the handles it hands on
    fn spawn(self, cmd: &mut std::process::Command) -> std::io::Result<(std::process::Child, std::fs::File, std::fs::File)> {
        #[cfg(windows)]
        let hand_on = |on: bool| {
            use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
            for end in [&self.theirs.0, &self.theirs.1] {
                // SAFETY: a flag on a handle this owns
                unsafe {
                    SetHandleInformation(end.0 as _, HANDLE_FLAG_INHERIT, if on { HANDLE_FLAG_INHERIT } else { 0 });
                }
            }
        };
        #[cfg(windows)]
        hand_on(true);
        let started = cmd.spawn();
        #[cfg(windows)]
        hand_on(false);
        Ok((started?, self.to, self.from))
    }
}

/// One thing said to the browser: on a pipe, each ends with a zero
fn say(out: &Mutex<Box<dyn Write + Send>>, msg: &serde_json::Value) -> std::io::Result<()> {
    let mut bytes = msg.to_string().into_bytes();
    bytes.push(0);
    let mut out = out.lock().unwrap_or_else(|e| e.into_inner());
    out.write_all(&bytes).and_then(|()| out.flush())
}

// ── the protocol ──────────────────────────────────────────────────────────

/// How long an answer is waited for when the caller names no time of its own.
const CALL_MS: u64 = 30_000;

/// What whoever listens to a browser is told when the line to it ends. Not
/// one of the browser's own words
const GONE: &str = "shikisha.gone";

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
        let _ = say(&self.out, &msg);
    }
}

type Sink = Box<dyn Fn(Event) + Send>;

/// One connection, carrying every page.
#[derive(Clone)]
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
    /// The DevTools screens open on its pages, and what of the browser's
    /// talk is theirs
    screens: Arc<Mutex<Screens>>,
}

impl Cdp {
    fn speaker(&self) -> Speaker {
        Speaker { out: Arc::clone(&self.out), next: Arc::clone(&self.next) }
    }

    /// Over the two pipes the browser was started holding the other ends of
    fn over(to: std::fs::File, from: std::fs::File) -> anyhow::Result<Self> {
        let waiting: Arc<Mutex<std::collections::HashMap<u64, Sender<serde_json::Value>>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let sink: Arc<Mutex<Option<Sink>>> = Arc::new(Mutex::new(None));
        let screens: Arc<Mutex<Screens>> = Default::default();

        let post = Arc::clone(&waiting);
        let still = Arc::clone(&alive);
        let heard = Arc::clone(&sink);
        let theirs = Arc::clone(&screens);
        std::thread::Builder::new()
            .name("shikisha-cdp".into())
            .spawn(move || {
                read_messages(from, post, Arc::clone(&heard), theirs);
                still.store(false, Ordering::Relaxed);
                // The browser is gone, and nothing more will be said about
                // its pages: whoever listens is told that once, in the
                // browser's own voice
                if let Some(sink) = heard.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    sink(Event { session: None, method: GONE.to_string(), params: serde_json::Value::Null });
                }
            })?;

        Ok(Self {
            out: Arc::new(Mutex::new(Box::new(to))),
            next: Arc::new(AtomicU64::new(1)),
            waiting,
            sink,
            alive,
            screens,
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
        crate::pagedialog::not_behind(session, method)?;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut msg = serde_json::json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = serde_json::Value::String(s.to_string());
        }

        let (tx, rx) = channel();
        self.waiting.lock().unwrap().insert(id, tx);
        if let Err(e) = say(&self.out, &msg) {
            self.waiting.lock().unwrap().remove(&id);
            return Err(e.into());
        }

        // A page is spoken to by its session, which is the name its box is
        // known by here (see where the opening is heard)
        let on = crate::pagedialog::watched(session, method);
        match crate::pagedialog::wait(&rx, std::time::Duration::from_millis(timeout_ms), on) {
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

/// Read what the browser says until it stops, handing each answer to whoever
/// asked, what belongs to a DevTools screen to that screen, and everything
/// else to the sink.
///
/// With no sink installed, events are dropped where they arrive: a queue
/// nobody drains is a memory leak with a schedule.
fn read_messages(
    from: std::fs::File,
    waiting: Arc<Mutex<std::collections::HashMap<u64, Sender<serde_json::Value>>>>,
    sink: Arc<Mutex<Option<Sink>>>,
    screens: Arc<Mutex<Screens>>,
) {
    use std::io::BufRead as _;
    let mut from = std::io::BufReader::with_capacity(64 * 1024, from);
    let mut one = Vec::new();
    loop {
        one.clear();
        // Each thing said ends with a zero; one that does not is the pipe
        // ending in the middle of it
        if !matches!(from.read_until(0, &mut one), Ok(n) if n > 0) || one.pop() != Some(0) {
            return;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&one) else { continue };
        let Some(v) = screens.lock().unwrap_or_else(|e| e.into_inner()).take(v) else { continue };
        match v.get("id").and_then(|i| i.as_u64()) {
            Some(id) => {
                if let Some(tx) = waiting.lock().unwrap().remove(&id) {
                    let _ = tx.send(v);
                }
            }
            None => {
                let held = sink.lock().unwrap_or_else(|e| e.into_inner());
                if let (Some(f), Some(method)) = (held.as_ref(), v.get("method").and_then(|m| m.as_str())) {
                    f(Event {
                        session: v.get("sessionId").and_then(|s| s.as_str()).map(str::to_string),
                        method: method.to_string(),
                        params: v.get("params").cloned().unwrap_or_default(),
                    });
                }
            }
        }
    }
}

// ── a page's DevTools screen ──────────────────────────────────────────────
//
// The browser carries its own DevTools screen, which talks the DevTools
// protocol over a WebSocket it is given the address of. The browser's own
// port would be that address, and it answers anybody on the machine about
// every page -- so the browser has none (see `Wire`), and the screen is
// given this instead: a door on the loopback that needs a key made for the
// one page the screen was asked for, is answered only for the screen itself
// (the origin it connects from), and reaches that page alone: through a
// session of its own on the line this program already has, with what
// reaches past the page refused (`devscreen::asked`).

use crate::devscreen::{BEHIND as SCREEN_BEHIND, ONE_COMMAND as SCREEN_COMMAND, ORIGIN as SCREEN_ORIGIN, STALLED as SCREEN_STALLED};

/// Where a browser's DevTools screens connect
struct ScreenDoor {
    port: u16,
    /// The key to each page a screen has been asked for, by the page's
    /// target. A key opens its own page and no other, so the address of one
    /// tab's screen is not a way into the next tab
    keys: Arc<Mutex<std::collections::HashMap<String, String>>>,
}


/// The DevTools screens open on a browser's pages, as the reading thread
/// needs them: which of what the browser says is whose
#[derive(Default)]
struct Screens {
    next: u64,
    lines: std::collections::HashMap<u64, crate::ws::Lane>,
    /// Whose each session is: the screen's own on its page (`true`), or one
    /// the page handed it -- a frame, a worker
    sessions: std::collections::HashMap<String, (u64, bool)>,
    /// What each screen is waiting to hear about: this line's number for the
    /// question, and the screen's own
    asked: std::collections::HashMap<u64, (u64, serde_json::Value)>,
}

impl Screens {
    /// A screen's line, begun: its number, and the session it speaks through
    fn begin(&mut self, session: &str, line: crate::ws::Lane) -> u64 {
        self.next += 1;
        self.lines.insert(self.next, line);
        self.sessions.insert(session.to_string(), (self.next, true));
        self.next
    }

    /// A screen gone: nothing more is kept for it or sent to it
    fn end(&mut self, screen: u64) {
        self.lines.remove(&screen);
        self.sessions.retain(|_, (s, _)| *s != screen);
        self.asked.retain(|_, (s, _)| *s != screen);
    }

    /// Something for a screen, on its way. One too far behind is let go
    fn hand(&mut self, screen: u64, said: &serde_json::Value) {
        let frame = crate::ws::encode(crate::ws::Op::Text, said.to_string().as_bytes());
        self.send(screen, frame);
    }

    fn send(&mut self, screen: u64, frame: Vec<u8>) {
        if self.lines.get(&screen).is_some_and(|line| !line.send(frame)) {
            self.end(screen);
        }
    }

    /// Something the browser said: handed to the screen it is for, or given
    /// back when it is for no screen
    fn take(&mut self, mut v: serde_json::Value) -> Option<serde_json::Value> {
        if self.lines.is_empty() {
            return Some(v);
        }
        let session = v.get("sessionId").and_then(|s| s.as_str()).map(str::to_string);
        let (screen, own) = match v.get("id").and_then(|i| i.as_u64()) {
            // An answer: to the screen that asked, under the number it asked by
            Some(id) => {
                let Some((screen, theirs)) = self.asked.remove(&id) else { return Some(v) };
                v["id"] = theirs;
                (screen, session.as_deref().and_then(|s| self.sessions.get(s)).is_none_or(|(_, own)| *own))
            }
            // Something a session of a screen's says by itself
            None => {
                let Some(&(screen, own)) = session.as_deref().and_then(|s| self.sessions.get(s)) else { return Some(v) };
                // The sessions the page hands the screen are the screen's too
                let named = v.get("params").and_then(|p| p.get("sessionId")).and_then(|s| s.as_str());
                match (v.get("method").and_then(|m| m.as_str()), named) {
                    (Some("Target.attachedToTarget"), Some(s)) => {
                        self.sessions.insert(s.to_string(), (screen, false));
                    }
                    (Some("Target.detachedFromTarget"), Some(s)) => {
                        self.sessions.remove(s);
                    }
                    _ => {}
                }
                (screen, own)
            }
        };
        // The screen believes it is connected to the page itself: what its
        // own session says carries no session's name
        if own && let Some(o) = v.as_object_mut() {
            o.remove("sessionId");
        }
        self.hand(screen, &v);
        None
    }
}

impl Cdp {
    /// Something a DevTools screen said, passed on to the browser as its
    /// session's
    fn from_screen(&self, screen: u64, own: &str, target: &str, text: &str) {
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(text) else { return };
        let Some(o) = v.as_object_mut() else { return };
        let theirs = o.get("id").cloned().unwrap_or_default();
        // A cookie set or deleted, a site's storage cleared: for the sites
        // the page is at, and no other (`devscreen::sites_written`). Asked
        // of the browser before anything here is locked, and only for these
        let elsewhere = crate::devscreen::sites_written(
            o.get("method").and_then(|m| m.as_str()).unwrap_or_default(),
            o.get("params").unwrap_or(&serde_json::Value::Null),
        )
        .is_some_and(|sites| !crate::devscreen::within(&sites, &self.frames_of(own)));
        let ours = self.next.fetch_add(1, Ordering::Relaxed);
        {
            let mut screens = self.screens.lock().unwrap_or_else(|e| e.into_inner());
            // The session it speaks through: its own on the page, or one of
            // its own the page handed it (a frame's, a worker's) -- and only
            // those. A name is not a way to another page of this browser
            let through = o.get("sessionId").and_then(|s| s.as_str()).map(str::to_string);
            let answer = |mut said: serde_json::Value| {
                if let (Some(s), Some(o)) = (&through, said.as_object_mut()) {
                    o.insert("sessionId".into(), s.as_str().into());
                }
                said
            };
            match &through {
                None => {
                    o.insert("sessionId".into(), own.into());
                }
                Some(s) if screens.sessions.get(s).is_some_and(|(of, _)| *of == screen) => {}
                Some(_) => {
                    let refused = serde_json::json!({ "id": theirs, "error": { "code": -32001, "message": "Session with given id not found." } });
                    screens.hand(screen, &refused);
                    return;
                }
            }
            // And what it asks stays with that page
            let method = o.get("method").and_then(|m| m.as_str()).unwrap_or_default();
            let none = serde_json::Value::Null;
            let handed = |s: &str| screens.sessions.get(s).is_some_and(|(of, top)| *of == screen && !*top);
            let method = method.to_string();
            let verdict = match elsewhere {
                true => crate::devscreen::Asked::Refused,
                false => crate::devscreen::asked(&method, o.get("params").unwrap_or(&none), Some(target), &handed),
            };
            let method = method.as_str();
            match verdict {
                // Passed on, cut down to this page where it asked for more
                crate::devscreen::Asked::Pass => {
                    if let Some(p) = o.get_mut("params") {
                        crate::devscreen::trim(method, p);
                    }
                }
                crate::devscreen::Asked::Nothing => {
                    let done = answer(serde_json::json!({ "id": theirs, "result": {} }));
                    screens.hand(screen, &done);
                    return;
                }
                crate::devscreen::Asked::Refused => {
                    crate::append_hook_log(&format!("chrome: a DevTools screen asked {method}, which reaches past its page; refused"));
                    let refused = answer(crate::devscreen::refusal(&theirs));
                    screens.hand(screen, &refused);
                    return;
                }
            }
            screens.asked.insert(ours, (screen, theirs));
        }
        o.insert("id".into(), ours.into());
        let _ = say(&self.out, &v);
    }
}

impl Cdp {
    /// The hosts the page spoken to through `session` is at: its own and
    /// those of the frames in it. Empty when the browser does not say
    fn frames_of(&self, session: &str) -> Vec<String> {
        fn walk(tree: &serde_json::Value, hosts: &mut Vec<String>) {
            if let Some(url) = tree.pointer("/frame/url").and_then(|u| u.as_str()) {
                let host = crate::devscreen::host_of(url);
                if !host.is_empty() && !hosts.contains(&host) {
                    hosts.push(host);
                }
            }
            for child in tree.get("childFrames").and_then(|c| c.as_array()).into_iter().flatten() {
                walk(child, hosts);
            }
        }
        let mut hosts = Vec::new();
        if let Ok(said) = self.call(Some(session), "Page.getFrameTree", serde_json::json!({}), CALL_MS) {
            walk(said.get("frameTree").unwrap_or(&serde_json::Value::Null), &mut hosts);
        }
        hosts
    }
}

impl ScreenDoor {
    /// Start answering on a port of the system's choosing, for screens on
    /// the pages of the browser `cdp` is the line to
    fn open(cdp: Cdp, closing: Arc<std::sync::atomic::AtomicBool>) -> std::io::Result<ScreenDoor> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let keys: Arc<Mutex<std::collections::HashMap<String, String>>> = Default::default();
        let want = Arc::clone(&keys);
        let serving = crate::devscreen::Serving::default();
        std::thread::Builder::new().name("shikisha-devtools-door".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                if closing.load(Ordering::Relaxed) {
                    return;
                }
                // Anybody on the machine can knock. One that knocks and says
                // nothing is not waited on, and past so many at once the
                // rest are not answered at all
                let Some(served) = serving.enter() else { continue };
                let (cdp, want) = (cdp.clone(), Arc::clone(&want));
                let _ = std::thread::Builder::new().name("shikisha-devtools-line".into()).spawn(move || {
                    let _served = served;
                    serve_screen(stream, port, &want, &cdp);
                });
            }
        })?;
        Ok(ScreenDoor { port, keys })
    }

    /// What every screen's address begins with; the page's key and target
    /// follow
    fn prefix(&self) -> String {
        format!("devtools://devtools/bundled/devtools_app.html?ws=127.0.0.1:{}/devtools/", self.port)
    }

    /// The address a screen for the page `target` is opened at. Its key is
    /// made the first time it is asked for: 16 bytes, which only have to
    /// outlast one run of the browser, and guessing them is a round trip to
    /// this machine per try
    fn address(&self, target: &str) -> String {
        let mut keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        let key = keys.entry(target.to_string()).or_insert_with(|| crate::random_hex(16));
        format!("{}{key}/{target}", self.prefix())
    }

    /// Whether an address is one this door gave out
    fn made(&self, url: &str) -> bool {
        let Some((key, target)) = url.strip_prefix(&self.prefix()).and_then(|rest| rest.split_once('/')) else { return false };
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).get(target).is_some_and(|k| crate::crypto::token_eq(k, key))
    }
}

/// The page a request asked for: when the key, the origin and the address it
/// was made to are all right, and nothing otherwise. The address as well as
/// the key, so a name that only resolves to this machine is not a way in
fn screen_admitted(asked: &crate::ws::Opening, port: u16, keys: &std::collections::HashMap<String, String>) -> Option<String> {
    let (given, target) = asked.path.strip_prefix("/devtools/")?.split_once('/')?;
    let here = asked.host == format!("127.0.0.1:{port}");
    // The key of the page it names, and of no other
    let keyed = keys.get(target).is_some_and(|key| crate::crypto::token_eq(given, key));
    (keyed && asked.origin == SCREEN_ORIGIN && here && !asked.key.is_empty()).then(|| target.to_string())
}

fn serve_screen(stream: std::net::TcpStream, port: u16, keys: &Mutex<std::collections::HashMap<String, String>>, cdp: &Cdp) {
    let Ok(read_side) = stream.try_clone() else { return };
    let mut reader = std::io::BufReader::new(read_side);
    let Some(asked) = crate::ws::read_opening_within(&mut reader, crate::ws::OPENING_WITHIN) else { return };
    let mut out = stream;
    // A session of the screen's own on the page, apart from the one
    // automation speaks through. A page that is not there is no session,
    // and the screen is turned away like any other caller
    let target = screen_admitted(&asked, port, &keys.lock().unwrap_or_else(|e| e.into_inner()));
    let session = target.as_ref().and_then(|target| {
        cdp.call(None, "Target.attachToTarget", serde_json::json!({ "targetId": target, "flatten": true }), CALL_MS)
            .ok()
            .and_then(|a| a.get("sessionId").and_then(|s| s.as_str()).map(str::to_string))
    });
    let (Some(target), Some(session)) = (target, session) else {
        let _ = out.write_all(crate::ws::TURNED_AWAY);
        // The address carries the key, so it is not written down
        crate::append_hook_log("chrome: a connection to the DevTools door was turned away");
        return;
    };
    let let_go = || cdp.speaker().tell(None, "Target.detachFromTarget", serde_json::json!({ "sessionId": session }));
    let Ok(write_side) = out.try_clone() else { return let_go() };
    if out.write_all(crate::ws::welcome(&asked.key).as_bytes()).is_err() {
        return let_go();
    }
    // What the page says goes out on a writer of its own, so a screen slow
    // to read never holds up the thread every page's talk arrives on. One
    // that takes nothing at all for this long is gone
    let _ = write_side.set_write_timeout(Some(SCREEN_STALLED));
    let Ok(whole) = out.try_clone() else { return let_go() };
    let line = crate::ws::Lane::open(write_side, SCREEN_BEHIND, move || {
        // Let go of -- too far behind, or gone -- so the reading below ends
        let _ = whole.shutdown(std::net::Shutdown::Both);
    });
    let screen = cdp.screens.lock().unwrap_or_else(|e| e.into_inner()).begin(&session, line);
    let mut partial = crate::ws::Partial::default();
    loop {
        match crate::ws::read_message_upto(&mut reader, SCREEN_COMMAND, &mut partial) {
            Ok((crate::ws::Op::Text, body)) => {
                if let Ok(text) = String::from_utf8(body) {
                    cdp.from_screen(screen, &session, &target, &text);
                }
            }
            Ok((crate::ws::Op::Ping, body)) => {
                let pong = crate::ws::encode(crate::ws::Op::Pong, &body);
                cdp.screens.lock().unwrap_or_else(|e| e.into_inner()).send(screen, pong);
            }
            Ok((crate::ws::Op::Close, _)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    cdp.screens.lock().unwrap_or_else(|e| e.into_inner()).end(screen);
    let_go();
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
    /// Whether a viewer has supplied the layout size; cleared with the cast.
    reshaped: bool,
    casting: bool,
    /// The latest desktop layout, restored after the relay releases it.
    bounds: Option<(i32, i32)>,
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
    auth: std::collections::HashMap<String, (String, String, Vec<String>)>,
    /// Pages whose console somebody is listening to
    console: std::collections::HashSet<String>,
    /// What the other pages have said since they last loaded. A page here
    /// has the protocol's `Runtime` on from the start (its bindings need it),
    /// so this is heard anyway, and kept so that listening begins with what
    /// the page in view has already said -- as it does in the window
    early: std::collections::HashMap<String, std::collections::VecDeque<serde_json::Value>>,
    /// Which page each target is, and the private storage it was opened in
    /// when it was. A download is reported by the browser itself, naming the
    /// frame it started in -- and a page's own frame has the id of its target
    targets: std::collections::HashMap<String, (String, Option<String>)>,
    /// Downloads still arriving, by the browser's id for them
    fetching: std::collections::HashMap<String, Fetching>,
    /// The windows the pages have opened (see [`Windows`])
    windows: Windows,
}

/// One download on its way, as this side keeps track of it.
///
/// The browser is told to save under its own id for the file (`allowAndName`)
/// in a folder of its own, and the file is moved to the downloads folder under
/// its real name once whole. So the downloads folder never holds half a file
/// with a number for a name, and the name it ends up with is decided here --
/// numbered the way a browser numbers a second file of the same name
struct Fetching {
    page: Option<String>,
    name: String,
    url: String,
    /// The profile's browser it is in, and its private storage if it has one:
    /// what stopping it has to name
    browser: String,
    context: Option<String>,
    staging: std::path::PathBuf,
    /// Somebody pressed Cancel. The browser says "canceled" for that and for
    /// a download the network dropped alike; this is how the two are told apart
    stopping: bool,
    /// When progress was last passed on. A large file reports every few
    /// kilobytes, and a line redrawn that often says nothing more
    said: Option<std::time::Instant>,
}

/// How often a download's progress is passed on, at most
const DOWNLOAD_SAY_MS: u128 = 250;

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
        // Windows the pages opened that have waited too long are closed
        // here as well as when the browser next says something of a window:
        // one left blank, with nothing more said, would never be looked at
        // again. (Any of the browsers is told: a window it does not have is
        // nothing to it)
        let stale = self.seen.lock().unwrap_or_else(|e| e.into_inner()).windows.stale();
        for (target, context) in stale {
            for chrome in self.browsers.borrow().values() {
                chrome.speaker().tell(None, "Target.closeTarget", serde_json::json!({ "targetId": target }));
                // The private storage it was in goes with the last of what
                // was in it (its opener may have closed while it waited)
                if let Some(c) = context.as_deref().filter(|c| !self.shared(c)) {
                    chrome.close_context(c);
                }
            }
        }
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

    /// Whether anything is still in the private storage `context`: a page
    /// that is open, or a window waiting for its tab
    fn shared(&self, context: &str) -> bool {
        self.open.borrow().values().any(|p| p.context.as_deref() == Some(context))
            || self.seen.lock().unwrap_or_else(|e| e.into_inner()).windows.in_context(context)
    }

    /// The browser this profile's pages live in, started if this is the first.
    fn browser_for(&self, profile: &shikisha_shared::BrowserProfile) -> anyhow::Result<Rc<Chrome>> {
        // A private page is given its own storage inside the ordinary browser
        // rather than a browser of its own: the isolation is the storage, and
        // a second process would buy nothing but a second process
        let key = if profile.private { "default".to_string() } else { folder_name(&profile.name) };
        let kept = self.browsers.borrow().get(&key).map(Rc::clone);
        match kept {
            Some(c) if c.alive() => return Ok(c),
            // It went on its own (crashed, killed for its memory). Kept, it
            // would answer "the browser is gone" to every page opened from
            // now until the program is started again: its pages are forgotten
            // -- they went with it -- and another is started in its place
            Some(dead) => {
                crate::append_hook_log(&format!("chrome: the browser for '{key}' is gone; starting another"));
                self.forget(&dead);
                self.browsers.borrow_mut().remove(&key);
            }
            None => {}
        }
        let chrome = Rc::new(Chrome::start_in(self.root.join(&key), self.temporary)?);
        let staging = self.staging(&key);
        self.listen(&chrome, &key, staging);
        // A browser with no window refuses every download unless it is told
        // where they go. Told here, once, for the profile's own storage; a
        // private page's storage is told when it is made
        self.let_downloads(&chrome, &key, None);
        self.browsers.borrow_mut().insert(key, Rc::clone(&chrome));
        Ok(chrome)
    }

    /// Forget the pages a browser had: it is gone, and they with it
    fn forget(&self, browser: &Rc<Chrome>) {
        let theirs: Vec<String> =
            self.open.borrow().iter().filter(|(_, p)| Rc::ptr_eq(&p.browser, browser)).map(|(n, _)| n.clone()).collect();
        let mut book = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        for name in theirs {
            let Some(page) = self.open.borrow_mut().remove(&name) else { continue };
            crate::pagedialog::gone(&name);
            crate::pagedialog::no_longer(&page.session);
            book.named.remove(&page.session);
            book.targets.remove(&page.target);
            book.cast.remove(&name);
            book.auth.remove(&name);
            self.refs.lock().unwrap_or_else(|e| e.into_inner()).remove(&Some(name));
        }
    }

    /// Where this profile's downloads are kept while they arrive
    fn staging(&self, key: &str) -> std::path::PathBuf {
        self.root.join(key).join("Downloading")
    }

    /// Let one storage of this browser save files, into the folder they are
    /// kept in while they arrive, and say how each one goes
    fn let_downloads(&self, chrome: &Chrome, key: &str, context: Option<&str>) {
        let staging = self.staging(key);
        let _ = std::fs::create_dir_all(&staging);
        let mut params = serde_json::json!({
            "behavior": "allowAndName",
            "downloadPath": staging.display().to_string(),
            "eventsEnabled": true,
        });
        if let Some(c) = context {
            params["browserContextId"] = serde_json::Value::String(c.to_string());
        }
        if let Err(e) = chrome.call("Browser.setDownloadBehavior", params) {
            crate::append_hook_log(&format!("[chrome] downloads stay refused here: {e:#}"));
        }
    }

    /// Take in everything this browser says of its own accord.
    fn listen(&self, chrome: &Rc<Chrome>, key: &str, staging: std::path::PathBuf) {
        let speaker = chrome.speaker();
        let seen = Arc::clone(&self.seen);
        let key = key.to_string();
        // The pages of this browser with a box open, by session: what is
        // left to clear if the browser goes with one still up
        let asking: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
        chrome.on_event(move |ev| {
            // The browser went (crashed, killed) with boxes open: nobody can
            // answer them now, and nothing will say they closed
            if ev.session.is_none() && ev.method == GONE {
                let left = std::mem::take(&mut *asking.lock().unwrap_or_else(|e| e.into_inner()));
                let mut book = seen.lock().unwrap_or_else(|e| e.into_inner());
                for (session, page) in left {
                    crate::pagedialog::gone(&page);
                    crate::pagedialog::no_longer(&session);
                    book.mail.push(shikisha_shared::Ev::PageDialogGone { from: Some(page) });
                }
                return;
            }
            // The browser itself, rather than a page in it: downloads are
            // reported there, whichever page started them
            if ev.session.is_none() {
                // A window one of the pages opened. This browser has no
                // window to show it in: nobody would ever see it, and it
                // would run on, unseen, for as long as the browser does. So
                // where it is going is said to the loop, which makes a tab
                // for it -- and the window itself is kept for that tab to
                // take as its page (`open_child`). Taken, not closed and
                // opened again: it has already asked its address of the
                // server, and asking twice spends a link that works once;
                // and it is still the window its opener opened, so what the
                // two say to each other, and what was posted into it, hold.
                //
                // Heard from the window itself and not from the page's asking
                // for one (`Page.windowOpen`): a window the browser refused
                // to open is asked for all the same, and would become a tab
                // nobody wanted
                if matches!(ev.method.as_str(), "Target.targetCreated" | "Target.targetInfoChanged")
                    && let Some(info) = ev.params.get("targetInfo")
                    && let Some(target) = info.get("targetId").and_then(|t| t.as_str())
                {
                    let close = |target: &str| speaker.tell(None, "Target.closeTarget", serde_json::json!({ "targetId": target }));
                    let mut book = seen.lock().unwrap_or_else(|e| e.into_inner());
                    // Whose it is: one already waiting for its address, or
                    // one just made by a page of ours
                    let asked = match book.windows.blank.remove(target) {
                        Some(w) => Some(w),
                        None if ev.method == "Target.targetCreated" && info.get("type").and_then(|t| t.as_str()) == Some("page") => info
                            .get("openerId")
                            .and_then(|o| o.as_str())
                            .filter(|_| !book.targets.contains_key(target) && !book.windows.kept.contains_key(target))
                            .and_then(|opener| book.targets.get(opener))
                            .map(|(page, context)| Blank {
                                by_hand: book.windows.pressed.get(page).is_some_and(|at| at.elapsed() < WINDOW_PRESSED),
                                page: page.clone(),
                                since: std::time::Instant::now(),
                                context: context.clone(),
                            }),
                        None => None,
                    };
                    let Some(asked) = asked else { return };
                    let url = info.get("url").and_then(|u| u.as_str()).unwrap_or_default();
                    if shikisha_shared::is_openable(url) {
                        if book.windows.waiting() >= WINDOW_KEPT_MOST {
                            book.windows.closing(target);
                            close(target);
                            return;
                        }
                        book.windows.kept.insert(
                            target.to_string(),
                            Kept { browser: key.clone(), since: std::time::Instant::now(), context: asked.context },
                        );
                        book.mail.push(shikisha_shared::Ev::PageWindow {
                            from: Some(asked.page),
                            url: url.to_string(),
                            by_hand: asked.by_hand,
                            window: target.to_string(),
                        });
                    } else if book.windows.blank.values().filter(|w| w.page == asked.page).count() >= WINDOW_BLANK_MOST {
                        book.windows.closing(target);
                        close(target);
                    } else {
                        // Opened blank, to be sent somewhere in a moment (a
                        // sign-in does this: the window first, its address
                        // once the page has asked its server for one). Kept
                        // until it says where it is going
                        book.windows.blank.insert(target.to_string(), asked);
                    }
                    return;
                }
                if ev.method == "Target.targetDestroyed" {
                    if let Some(target) = ev.params.get("targetId").and_then(|t| t.as_str()) {
                        let mut book = seen.lock().unwrap_or_else(|e| e.into_inner());
                        if book.windows.went(target) {
                            // It closed itself. Not yet said to the loop, it
                            // is not said at all; said, the tab made for it
                            // goes with it -- opened at its address instead,
                            // the tab would ask the server again for what the
                            // window had asked for and been done with (a
                            // sign-in's last step, good for one asking)
                            book.mail.retain(|said| !matches!(said, shikisha_shared::Ev::PageWindow { window, .. } if window == target));
                            book.mail.push(shikisha_shared::Ev::PageWindowGone { window: target.to_string() });
                        }
                    }
                    return;
                }
                heard_download(&seen, &key, &staging, &ev.method, &ev.params);
                return;
            }
            let Some(session) = ev.session else { return };
            let mut book = seen.lock().unwrap_or_else(|e| e.into_inner());
            let Some(page) = book.named.get(&session).cloned() else { return };
            match ev.method.as_str() {
                // A page's alert, confirm or prompt: held for whoever is
                // driving the page (`crate::pagedialog`), which is why
                // nothing is answered here but "leave this page?"
                "Page.javascriptDialogOpening" => match crate::pagedialog::report_of(&page, &ev.params) {
                    Some(said) => {
                        crate::pagedialog::heard(&said);
                        // Whatever waits on this session gives up behind it
                        crate::pagedialog::also(&session);
                        asking.lock().unwrap_or_else(|e| e.into_inner()).push((session.clone(), page.clone()));
                        book.mail.push(said);
                    }
                    None => speaker.tell(Some(&session), crate::pagedialog::ANSWER, serde_json::json!({ "accept": true })),
                },
                // A link that opens in a new tab, or `window.open`. Only
                // whether a press on the page asked is taken from this: the
                // window itself is heard of from the browser (above), which
                // does not say who asked
                "Page.windowOpen" => {
                    if ev.params.get("userGesture").and_then(|g| g.as_bool()).unwrap_or(false) {
                        book.windows.pressed.insert(page, std::time::Instant::now());
                    }
                }
                "Page.javascriptDialogClosed" => {
                    let said = shikisha_shared::Ev::PageDialogGone { from: Some(page) };
                    crate::pagedialog::heard(&said);
                    crate::pagedialog::no_longer(&session);
                    asking.lock().unwrap_or_else(|e| e.into_inner()).retain(|(s, _)| *s != session);
                    book.mail.push(said);
                }
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
                    // Each frame is the whole picture: one not handed on yet
                    // is only out of date, and a loop that is held up must
                    // not come back to a pile of them
                    book.mail.retain(|e| !matches!(e, shikisha_shared::Ev::Frame { from: Some(f), .. } if *f == page));
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
                    // ...and only for a challenge from a site the password
                    // belongs to: not another site's picture on the page,
                    // not a proxy, not wherever the page went since
                    let answer = match book.auth.get(&page) {
                        Some((user, pass, sites)) if crate::config::challenge_allowed(sites, &ev.params) => serde_json::json!({
                            "response": "ProvideCredentials", "username": user, "password": pass,
                        }),
                        _ => serde_json::json!({ "response": "Default" }),
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
                // What the page said on its console, for a page somebody is
                // listening to (`crate::console`)
                m if crate::console::EVENTS.contains(&m) => {
                    let Some(entry) = crate::console::entry_of(m, &ev.params, crate::sqlite::now_ms()) else {
                        return;
                    };
                    if book.console.contains(&page) {
                        book.mail.push(shikisha_shared::Ev::ConsoleLine { from: Some(page), entry });
                    } else {
                        let kept = book.early.entry(page).or_default();
                        kept.push_back(entry);
                        while kept.len() > crate::console::KEPT {
                            kept.pop_front();
                        }
                    }
                }
                // A new document in the page itself (not in a frame inside
                // it): what the old one said is not what "since it loaded" means
                "Page.frameNavigated" => {
                    if ev.params.pointer("/frame/parentId").is_none() {
                        book.early.remove(&page);
                    }
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
        // Told of every page there comes to be, so one a page opened by
        // itself is heard of (and closed, above)
        if let Err(e) = chrome.call("Target.setDiscoverTargets", serde_json::json!({ "discover": true })) {
            crate::append_hook_log(&format!("chrome: windows the pages open will not be heard of: {e}"));
        }
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
        Ev::Picked { item, .. } => Ev::Picked { from, item },
        // An answer to an evaluation. Nothing here asks through the binding
        // -- an evaluation is answered by the protocol call that made it --
        // so one arriving is a page volunteering an answer to a question that
        // was never put to it
        Ev::Result { .. } => return None,
        other => other,
    })
}

/// A window a page opened with no address yet: whose it is, whether a
/// press asked for it, and since when it has stood blank
struct Blank {
    page: String,
    by_hand: bool,
    since: std::time::Instant,
    /// The private storage it was opened in, when its opener is in one
    context: Option<String>,
}

/// A window a page opened, said to the loop and waiting for the tab made
/// for it: the browser it is in, and since when it has waited
struct Kept {
    browser: String,
    since: std::time::Instant,
    /// The private storage it was opened in, when its opener is in one. It
    /// is this window's as much as its opener's: whichever of them closes
    /// last takes it along
    context: Option<String>,
}

/// The windows the pages have opened, as far as they are kept track of
#[derive(Default)]
struct Windows {
    /// When a press on each page last asked for a window
    pressed: std::collections::HashMap<String, std::time::Instant>,
    /// The ones opened blank, by target, until they say where they are going
    blank: std::collections::HashMap<String, Blank>,
    /// The ones waiting for their tab, by target
    kept: std::collections::HashMap<String, Kept>,
    /// The ones a tab took as its page, by target
    taken: std::collections::HashSet<String>,
    /// The targets this program is closing: one of these going is not a
    /// window closing itself
    closing: std::collections::HashSet<String>,
    /// The ones that closed themselves, lately: a tab made for one of them
    /// has nothing to take, and is not to open its address instead
    gone: std::collections::VecDeque<String>,
}

impl Windows {
    /// The windows that have waited too long, taken off the books, each with
    /// the private storage it was in, to be closed by the caller: blank and
    /// going nowhere; said and never made a tab of (the loop turned it down:
    /// too many, a page that is no tab); and, of those a tab was made for
    /// that has not come for them -- a tab on a desk nobody is looking at
    /// opens its page when the desk is looked at, and its window waits for
    /// that -- the longest waiting, once there are too many
    fn stale(&mut self) -> Vec<(String, Option<String>)> {
        let mut old: Vec<String> = self.blank.iter().filter(|(_, w)| w.since.elapsed() > WINDOW_BLANK_FOR).map(|(t, _)| t.clone()).collect();
        old.extend(
            self.kept
                .iter()
                .filter(|(t, w)| w.since.elapsed() > WINDOW_KEPT_FOR && !crate::pagewindow::claimed(t))
                .map(|(t, _)| t.clone()),
        );
        let mut held: Vec<(&String, std::time::Instant)> =
            self.kept.iter().filter(|(t, _)| crate::pagewindow::claimed(t) && !old.contains(t)).map(|(t, w)| (t, w.since)).collect();
        held.sort_by_key(|(_, since)| *since);
        let over = held.len().saturating_sub(WINDOW_HELD_MOST);
        old.extend(held.into_iter().take(over).map(|(t, _)| t.clone()));
        old.into_iter()
            .map(|t| {
                // Its tab, if one was made, opens its address like any tab
                crate::pagewindow::let_go(&t);
                self.closing(&t);
                let context = self.blank.remove(&t).map(|w| w.context).or_else(|| self.kept.remove(&t).map(|w| w.context)).flatten();
                (t, context)
            })
            .collect()
    }

    /// How many windows wait with no tab made for them yet
    fn waiting(&self) -> usize {
        self.kept.keys().filter(|t| !crate::pagewindow::claimed(t)).count()
    }

    /// This program is about to close `target`
    fn closing(&mut self, target: &str) {
        if self.closing.len() >= 256 {
            self.closing.clear();
        }
        self.closing.insert(target.to_string());
    }

    /// `target` is gone. Whether it was a window said to the loop -- waiting
    /// for its tab, or taken by it -- that closed itself
    fn went(&mut self, target: &str) -> bool {
        let ours = self.closing.remove(target);
        self.blank.remove(target);
        let said = self.kept.remove(target).is_some() | self.taken.remove(target);
        if ours || !said {
            return false;
        }
        if self.gone.len() >= 64 {
            self.gone.pop_front();
        }
        self.gone.push_back(target.to_string());
        true
    }

    /// Whether a window still on the books is in the private storage `context`
    fn in_context(&self, context: &str) -> bool {
        self.blank.values().map(|w| &w.context).chain(self.kept.values().map(|w| &w.context)).any(|c| c.as_deref() == Some(context))
    }
}

/// How long after a press on a page a window it opens counts as pressed for
const WINDOW_PRESSED: std::time::Duration = std::time::Duration::from_secs(5);
/// How long a window may stand blank before it is closed, and how many of
/// one page's may at once
const WINDOW_BLANK_FOR: std::time::Duration = std::time::Duration::from_secs(60);
const WINDOW_BLANK_MOST: usize = 4;
/// How long a window waits for its tab, and how many may be waiting. The
/// tab is made within a second or two of its being said
const WINDOW_KEPT_FOR: std::time::Duration = std::time::Duration::from_secs(20);
const WINDOW_KEPT_MOST: usize = 8;
/// How many windows may wait for a tab that was made for them and has not
/// opened its page yet
const WINDOW_HELD_MOST: usize = 16;

/// One thing the browser said about a download, turned into a report.
///
/// Runs on the thread that reads the browser, so nothing here waits for the
/// browser. Moving a finished file can take a while (a copy, across two disks),
/// so that goes to a thread of its own and is reported when it is done.
fn heard_download(
    seen: &Arc<Mutex<Seen>>,
    browser: &str,
    staging: &std::path::Path,
    method: &str,
    params: &serde_json::Value,
) {
    use shikisha_shared::{Download, DownloadState, Ev};
    let text = |k: &str| params.get(k).and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
    let bytes = |k: &str| params.get(k).and_then(serde_json::Value::as_f64).map_or(0, |n| n.max(0.0) as u64);
    let guid = text("guid");
    if guid.is_empty() {
        return;
    }
    let mut book = seen.lock().unwrap_or_else(|e| e.into_inner());
    match method {
        "Browser.downloadWillBegin" => {
            let (page, context) = book.targets.get(&text("frameId")).cloned().map_or((None, None), |(p, c)| (Some(p), c));
            let name = crate::downloads::safe_name(&text("suggestedFilename"));
            let url = text("url");
            book.fetching.insert(guid.clone(), Fetching {
                page: page.clone(),
                name: name.clone(),
                url: url.clone(),
                browser: browser.to_string(),
                context,
                staging: staging.to_path_buf(),
                stopping: false,
                said: Some(std::time::Instant::now()),
            });
            book.mail.push(Ev::Download {
                from: page,
                item: Download { id: guid, name, url, path: String::new(), got: 0, total: 0,
                    state: DownloadState::Going, why: String::new(), far: false },
            });
        }
        "Browser.downloadProgress" => {
            let (got, total) = (bytes("receivedBytes"), bytes("totalBytes"));
            let state = text("state");
            let Some(f) = book.fetching.get_mut(&guid) else { return };
            let report = |f: &Fetching, path: String, state: DownloadState, why: &str| Ev::Download {
                from: f.page.clone(),
                item: Download { id: guid.clone(), name: f.name.clone(), url: f.url.clone(), path, got, total,
                    state, why: why.to_string(), far: false },
            };
            match state.as_str() {
                "inProgress" => {
                    if f.said.is_some_and(|t| t.elapsed().as_millis() < DOWNLOAD_SAY_MS) {
                        return;
                    }
                    f.said = Some(std::time::Instant::now());
                    let ev = report(f, String::new(), DownloadState::Going, "");
                    book.mail.push(ev);
                }
                "canceled" => {
                    let Some(f) = book.fetching.remove(&guid) else { return };
                    let _ = std::fs::remove_file(f.staging.join(&guid));
                    // Stopped by nobody here: the network or the server dropped it
                    let ev = match f.stopping {
                        true => report(&f, String::new(), DownloadState::Cancelled, ""),
                        false => report(&f, String::new(), DownloadState::Failed, "network"),
                    };
                    book.mail.push(ev);
                }
                "completed" => {
                    let Some(f) = book.fetching.remove(&guid) else { return };
                    drop(book);
                    // Where the browser says it put it, when it says; under its
                    // id in the folder it was told otherwise
                    let from = match text("filePath") {
                        p if !p.is_empty() => std::path::PathBuf::from(p),
                        _ => f.staging.join(&guid),
                    };
                    let seen = Arc::clone(seen);
                    std::thread::spawn(move || {
                        let placed = crate::downloads::take_place(&crate::downloads::folder(), &f.name)
                            .and_then(|to| crate::downloads::move_into(&from, &to).ok().map(|()| to));
                        let ev = match placed {
                            Some(to) => {
                                let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                Ev::Download {
                                    from: f.page.clone(),
                                    item: Download { id: guid, name, url: f.url, path: to.display().to_string(),
                                        got, total, state: DownloadState::Done, why: String::new(), far: false },
                                }
                            }
                            None => Ev::Download {
                                from: f.page.clone(),
                                item: Download { id: guid, name: f.name, url: f.url, path: String::new(), got, total,
                                    state: DownloadState::Failed, why: "disk".into(), far: false },
                            },
                        };
                        seen.lock().unwrap_or_else(|e| e.into_inner()).mail.push(ev);
                    });
                }
                _ => {}
            }
        }
        _ => {}
    }
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
    fn basic_auth(&self, to: Option<&str>, user: &str, pass: &str, sites: &[String]) -> anyhow::Result<()> {
        let name = to.unwrap_or_default().to_string();
        let (chrome, session) = self.at(to)?;
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .auth
            .insert(name, (user.to_string(), pass.to_string(), sites.to_vec()));
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
            crate::append_hook_log(&format!("chrome: no answer from {}: {e}", to.unwrap_or("?")));
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
            Input::Mouse { phase, x, y, down, clicks } => {
                let held = self.open.borrow().get(&name).is_some_and(|p| p.held);
                let (ev, now) = crate::cdp::mouse_event(&phase, x * cw, y * ch, down, held, clicks);
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
                // One page of a browser has the keyboard, and the rest take
                // a key and do nothing with it: each is made to believe it
                // has it (as before a field is filled)
                let _ = chrome.call_page(&session, "Emulation.setFocusEmulationEnabled", serde_json::json!({ "enabled": true }));
                for ev in crate::cdp::key_events(&named, ctrl, alt) {
                    chrome.call_page(&session, "Input.dispatchKeyEvent", ev)?;
                }
            }
            // A viewer said what shape its screen is. The page is re-shaped to
            // match, so a phone sees a full screen rather than a strip
            Input::View { w, h, .. } => {
                // CDP has no browser zoom: use the viewer's CSS size one to
                // one. A CSS zoom would break layouts that measure themselves.
                if let Some(m) = crate::cdp::view_metrics(w, h, 1.0) {
                    chrome.call_page(&session, "Emulation.setDeviceMetricsOverride", m)?;
                    if let Some(page) = self.open.borrow_mut().get_mut(&name) {
                        page.reshaped = true;
                    }
                    if self.open.borrow().get(&name).is_some_and(|p| p.casting) {
                        self.screencast(to, true)?;
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
        if let Some(page) = self.open.borrow_mut().get_mut(&name) {
            page.casting = on;
        }
        if on {
            // Chrome paints its active page. A later-opened tab can still
            // own that surface even though the board is showing this one;
            // reshaping a hidden page alone produces no relay frames.
            chrome.call_page(&session, "Page.bringToFront", serde_json::json!({}))?;
            // Starting an already-running cast need not emit a static page
            // again. A new viewer, or new metrics, needs a fresh first frame.
            chrome.call_page(&session, "Page.stopScreencast", serde_json::json!({}))?;
            let params: serde_json::Value =
                serde_json::from_str(crate::cdp::CAST_PARAMS).unwrap_or_default();
            chrome.call_page(&session, "Page.startScreencast", params)?;
            return Ok(());
        }
        chrome.call_page(&session, "Page.stopScreencast", serde_json::json!({}))?;
        // Give the page its own shape back, if a viewer had reshaped it
        let restore = self.open.borrow_mut().get_mut(&name)
            .and_then(|p| std::mem::take(&mut p.reshaped).then_some(p.bounds));
        if let Some(bounds) = restore {
            let metrics = bounds.and_then(|(w, h)| crate::cdp::view_metrics(w as f64, h as f64, 1.0));
            chrome.call_page(
                &session,
                if metrics.is_some() { "Emulation.setDeviceMetricsOverride" } else { "Emulation.clearDeviceMetricsOverride" },
                metrics.unwrap_or_else(|| serde_json::json!({})),
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
    fn fill_only_on(&self, to: Option<&str>, sel: &shikisha_shared::Sel, value: &str, sites: &[String], timeout_ms: u64) -> anyhow::Result<shikisha_shared::SecretFill> {
        crate::pageops::fill_only_on(self, to, sel, value, sites, timeout_ms)
    }
    fn text(&self, to: Option<&str>, sel: &shikisha_shared::Sel, timeout_ms: u64) -> anyhow::Result<Option<String>> {
        crate::pageops::text(self, to, sel, timeout_ms)
    }
    fn select(&self, to: Option<&str>, sel: &shikisha_shared::Sel, value: &str, timeout_ms: u64)
        -> anyhow::Result<shikisha_shared::OpReport> {
        crate::pageops::select(self, to, sel, value, timeout_ms)
    }
    fn scroll(
        &self,
        to: Option<&str>,
        sel: Option<&shikisha_shared::Sel>,
        amount: &serde_json::Value,
        timeout_ms: u64,
    ) -> anyhow::Result<(shikisha_shared::Found, String)> {
        crate::pageops::scroll(self, to, sel, amount, timeout_ms)
    }
    fn settle(&self, to: Option<&str>, expect: &str, cap_ms: u64, first_ms: u64, timeout_ms: u64) -> anyhow::Result<(u64, String)> {
        crate::pageops::settle(self, to, expect, cap_ms, first_ms, timeout_ms)
    }
    fn href(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String> {
        crate::pageops::href(self, to, timeout_ms)
    }
    /// A headless browser has no search of its own, so the page is searched
    /// by the script every page is given -- the same one the window falls
    /// back on
    fn seek(&self, to: Option<&str>, text: &str, step: shikisha_shared::Seek) -> anyhow::Result<Option<(u32, u32)>> {
        crate::pageops::seek(self, to, text, step, CALL_MS).map(Some)
    }

    fn cancel_download(&self, id: &str) -> anyhow::Result<()> {
        let (browser, context) = {
            let mut book = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            let Some(f) = book.fetching.get_mut(id) else {
                // Already over: there is nothing left to stop
                return Ok(());
            };
            f.stopping = true;
            (f.browser.clone(), f.context.clone())
        };
        let chrome = self.browsers.borrow().get(&browser).cloned();
        let Some(chrome) = chrome else { return Ok(()) };
        let mut params = serde_json::json!({ "guid": id });
        if let Some(c) = context {
            params["browserContextId"] = serde_json::Value::String(c);
        }
        chrome.call("Browser.cancelDownload", params)?;
        Ok(())
    }

    fn html(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String> {
        crate::pageops::html(self, to, timeout_ms)
    }
    fn source(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String> {
        crate::pageops::source(self, to, timeout_ms)
    }
    fn digest(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String> {
        crate::pageops::digest(self, to, timeout_ms)
    }
    fn elements(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<serde_json::Value> {
        crate::pageops::elements(self, to, timeout_ms)
    }
    fn snapshot(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<Vec<u8>> {
        crate::pageops::snapshot(self, to, timeout_ms)
    }
    fn pdf(&self, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<(Vec<u8>, String)> {
        crate::pageops::pdf(self, to, timeout_ms)
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
        // The web, or the DevTools screen of a page in one of this program's
        // own browsers -- no other `devtools://` address
        let own_screen = self.browsers.borrow().values().any(|c| c.made_screen(url));
        if !shikisha_shared::is_openable(url) && !own_screen {
            anyhow::bail!(crate::i18n::tp("err.browser.bad_url", &[("url", url)]));
        }
        let chrome = self.browser_for(&profile)?;
        let key = if profile.private { "default".to_string() } else { folder_name(&profile.name) };
        // The window this tab was made for (`pagewindow`), waiting for it:
        // taken as its page, as it stands. Known by the window's own name,
        // never by where it was going -- a tab that merely opens the same
        // address is not handed a window another page opened, in whatever
        // storage that page is in. It keeps the storage it was opened in --
        // its opener's, private or not -- and shares it with its opener:
        // whichever closes last takes the storage along
        let waiting = match crate::pagewindow::take(name) {
            None => None,
            Some(window) => {
                let mut book = self.seen.lock().unwrap_or_else(|e| e.into_inner());
                if book.windows.gone.contains(&window) {
                    anyhow::bail!(crate::i18n::t("err.browser.window_gone"));
                }
                match book.windows.kept.remove(&window) {
                    Some(kept) if kept.browser == key => Some((window, kept.context)),
                    // Not this tab's browser after all: nobody's to take
                    Some(_) => {
                        book.windows.closing(&window);
                        chrome.speaker().tell(None, "Target.closeTarget", serde_json::json!({ "targetId": window }));
                        None
                    }
                    None => None,
                }
            }
        };
        let taken = waiting.and_then(|(target, context)| {
            let att = chrome.call("Target.attachToTarget", serde_json::json!({ "targetId": target, "flatten": true })).ok()?;
            Some((target, att.get("sessionId")?.as_str()?.to_string(), context))
        });
        let adopted = taken.is_some();
        // A private storage made here, for a private tab that takes no window
        let made = match (profile.private, adopted) {
            (true, false) => Some(chrome.new_context()?),
            _ => None,
        };
        if let Some(c) = made.as_deref() {
            self.let_downloads(&chrome, &key, Some(c));
        }
        let (target, session, context) = match taken {
            Some(taken) => taken,
            None => {
                let (target, session) = chrome.open_blank(made.as_deref())?;
                (target, session, made.clone())
            }
        };
        {
            let mut book = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            book.named.insert(session.clone(), name.to_string());
            book.targets.insert(target.clone(), (name.to_string(), context.clone()));
            if adopted {
                book.windows.taken.insert(target.clone());
            }
        }

        let dress = || -> anyhow::Result<()> {
            chrome.call_page(&session, "Page.enable", serde_json::json!({}))?;
            chrome.call_page(&session, "Runtime.enable", serde_json::json!({}))?;
            chrome.call_page(&session, "Runtime.addBinding", serde_json::json!({ "name": BINDING }))?;
            chrome.call_page(
                &session,
                "Page.addScriptToEvaluateOnNewDocument",
                serde_json::json!({ "source": page_script() }),
            )?;
            // The name the tab chose, or else the browser's own without the
            // word that says it has no window -- and, either way, everything
            // said beside the name made to agree with it
            if let Some(ua) = profile.user_agent.as_deref().or(chrome.calls_itself.as_deref()) {
                chrome.call_page(&session, "Emulation.setUserAgentOverride", crate::useragent::said(ua))?;
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
            {
                let mut book = self.seen.lock().unwrap_or_else(|x| x.into_inner());
                book.named.remove(&session);
                book.targets.remove(&target);
                book.windows.taken.remove(&target);
                book.windows.closing(&target);
            }
            let _ = chrome.call("Target.closeTarget", serde_json::json!({ "targetId": target }));
            if let Some(c) = context.as_deref().filter(|c| !self.shared(c)) {
                chrome.close_context(c);
            }
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
                reshaped: false,
                casting: false,
                bounds: (rect.2 > 0 && rect.3 > 0).then_some((rect.2, rect.3)),
            },
        );
        match adopted {
            // Already where it was going: not sent there a second time. The
            // script every page is given was not there when its document
            // began, so it is given now, and says how far the page has got
            true => {
                let _ = chrome.call_page(&session, "Runtime.evaluate", serde_json::json!({ "expression": page_script() }));
            }
            false => {
                chrome.call_page(&session, "Page.navigate", serde_json::json!({ "url": url }))?;
            }
        }
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
        if let Some(page) = self.open.borrow_mut().get_mut(name) {
            page.bounds = Some((rect.2, rect.3));
            if page.reshaped {
                return Ok(());
            }
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
        {
            // Closed by this program: not a window closing itself
            let mut book = self.seen.lock().unwrap_or_else(|e| e.into_inner());
            book.windows.taken.remove(&page.target);
            book.windows.closing(&page.target);
        }
        let _ = page.browser.call(
            "Target.closeTarget",
            serde_json::json!({ "targetId": page.target }),
        );
        // A private page's storage goes with it. That is the whole promise
        // -- once nothing else is in it: a window the page opened is in the
        // same storage, and goes on after the page that opened it
        if let Some(c) = page.context.as_deref().filter(|c| !self.shared(c)) {
            page.browser.close_context(c);
        }
        // A box it had open went with it
        crate::pagedialog::gone(name);
        crate::pagedialog::no_longer(&page.session);
        let mut book = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        book.named.remove(&page.session);
        book.targets.remove(&page.target);
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

    fn answer_dialog(&self, to: Option<&str>, accept: bool, text: &str) -> anyhow::Result<()> {
        crate::pageops::answer_dialog(self, to, accept, text)
    }

    /// A page here already has `Runtime` on (its bindings need it); hearing
    /// its console adds the browser's own log, and marks it listened to
    fn console(&self, to: Option<&str>, on: bool) -> anyhow::Result<()> {
        let (chrome, session) = self.at(to)?;
        let name = to.unwrap_or_default().to_string();
        if on {
            // What the page said since it loaded goes first, then everything
            // from now on. The browser's own word (`Log`) is only heard from
            // here -- turning it on hands over what it had kept as well
            {
                let mut book = self.seen.lock().unwrap_or_else(|e| e.into_inner());
                let early = book.early.remove(&name).unwrap_or_default();
                for entry in early {
                    book.mail.push(shikisha_shared::Ev::ConsoleLine { from: Some(name.clone()), entry });
                }
                book.console.insert(name);
            }
            chrome.call_page(&session, "Log.enable", serde_json::json!({}))?;
        } else {
            self.seen.lock().unwrap_or_else(|e| e.into_inner()).console.remove(&name);
            let _ = chrome.call_page(&session, "Log.disable", serde_json::json!({}));
        }
        Ok(())
    }

    fn devtools_url(&self, to: Option<&str>) -> anyhow::Result<String> {
        let name = to.ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.chrome.no_front")))?;
        let open = self.open.borrow();
        let page = open.get(name).ok_or_else(|| {
            anyhow::anyhow!(crate::i18n::tp("err.chrome.no_such_page", &[("name", name)]))
        })?;
        page.browser.devtools_screen(&page.target)
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
            panic!("this machine has no browser (install one before running with --ignored)");
        };
        println!("browser: {}", exe.display());

        let chrome = Chrome::start().expect("the browser does not start");
        let version = chrome
            .call("Browser.getVersion", serde_json::json!({}))
            .expect("it does not say its version");
        println!("version: {}", version.get("product").and_then(|v| v.as_str()).unwrap_or("?"));

        // A page of our own making, so the test does not depend on the network
        // The charset is declared, because a `data:` URL without one is read
        // as windows-1252 and every character above ASCII comes back as
        // something else. Found by this test saying exactly that
        let page = "data:text/html;charset=utf-8,                    <title>shikisha</title><h1 id=t>ここに書いた</h1>";
        let session = chrome.open(page).expect("the page does not open");

        // The page has to have finished before it can be read
        chrome
            .call_page(&session, "Page.enable", serde_json::json!({}))
            .expect("Page cannot be enabled");
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
            .expect("cannot read it back");
        let text = got
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(text, "ここに書いた", "what was written cannot be read back: {got}");

        // And a picture of it, which is what a phone is shown
        let shot = chrome
            .call_page(&session, "Page.captureScreenshot", serde_json::json!({"format": "png"}))
            .expect("cannot take it");
        let png = shot.get("data").and_then(|v| v.as_str()).unwrap_or("");
        assert!(png.len() > 1000, "the picture is too small: {} characters", png.len());
        println!("screenshot: a PNG of {} characters", png.len());
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
        let exe = fetch().expect("cannot get the browser");
        println!("browser: {} ({:?})", exe.display(), started.elapsed());
        assert!(exe.is_file(), "it is not where it was fetched to");

        // Asked again, it hands back the same one without fetching anything
        let again = std::time::Instant::now();
        assert_eq!(fetch().expect("the second time"), exe);
        assert!(
            again.elapsed() < std::time::Duration::from_secs(2),
            "the second time also goes to download it: {:?}",
            again.elapsed()
        );

        // The finder prefers it over whatever else is on this machine --
        // a known version being the whole reason it was fetched
        assert_eq!(found().as_deref(), Some(exe.as_path()), "it does not choose its own copy");

        // And it is a browser: it starts, and it is the version that was asked for
        let chrome = Chrome::start().expect("the fetched browser does not start");
        let said = chrome.call("Browser.getVersion", serde_json::json!({})).expect("it does not say its version");
        let product = said.get("product").and_then(|v| v.as_str()).unwrap_or("");
        assert!(product.contains(PINNED), "not the pinned version: {product}");
        println!("version: {product}");
    }

    /// A page here that opens a confirm or a prompt is held, whatever is
    /// asked of it meanwhile gives up at once, and the answer it is given is
    /// the one it gets. Against a real browser, since what is being checked
    /// is what the browser does with a box nobody answers for it
    ///
    ///     cargo test -p shikisha-core --lib a_box_a_page_here -- --ignored --nocapture
    #[test]
    #[ignore = "needs a browser on this machine"]
    fn a_box_a_page_here_opens_is_held_and_answered() {
        use crate::pageops::Speaks as _;
        use shikisha_shared::{BrowserHost, BrowserProfile, Sel};

        assert!(found().is_some(), "this machine has no browser");
        let site = serve();
        let store = std::env::temp_dir().join(format!("shikisha-pages-{}", crate::random_hex(8)));
        let pages = Pages::under(store, true);
        // A name of its own: the boxes are kept by the page's name, and the
        // tests beside this one open and close pages called `p` meanwhile
        let name = "dialog-held";
        pages
            .open_child(name, &site, (0, 0, 900, 700), BrowserProfile::shared_default())
            .expect("the page does not open");
        waited(&pages, |ev| match ev {
            shikisha_shared::Ev::Ready { from: Some(n), complete: true, .. } if n == name => Some(()),
            _ => None,
        })
        .expect("the page does not report that loading finished");
        let title = || pages.eval(Some(name), "return document.title", 5_000).unwrap_or_default();
        let until = |what: &str, done: &dyn Fn() -> bool| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !done() {
                assert!(std::time::Instant::now() < end, "timed out waiting for {what}");
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        };

        // Asked a moment later, so the evaluation that arranges it comes back
        pages
            .eval(Some(name), "setTimeout(() => { document.title = confirm('Delete it?') ? 'yes' : 'no'; }, 0); return 1", 5_000)
            .expect("the page cannot be told to ask");
        until("the confirm to be heard", &|| crate::pagedialog::open_on(name).is_some());
        let open = crate::pagedialog::open_on(name).unwrap();
        assert_eq!((open.kind.as_str(), open.message.as_str()), ("confirm", "Delete it?"));
        assert!(
            pages.drain().iter().any(|e| matches!(e, shikisha_shared::Ev::PageDialog { from: Some(p), .. } if p == name)),
            "the loop is not told a box opened"
        );
        // Whatever is asked of the page now gives up at once, not at the end
        // of its time -- what a script asks, and what the program asks for
        // itself (a console being listened to, a tap being placed)
        let started = std::time::Instant::now();
        assert!(pages.text(Some(name), &Sel::Css("#here".into()), 20_000).is_err(), "the stopped page answered");
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "it waited out its time: {:?}", started.elapsed());
        let started = std::time::Instant::now();
        let _ = pages.console(Some(name), true);
        let _ = pages.inject(Some(name), shikisha_shared::Input::Wheel { x: 10.0, y: 10.0, dx: 0.0, dy: 40.0 });
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "the program's own asks waited out their time: {:?}", started.elapsed());
        assert!(crate::pagedialog::open_on(name).is_some(), "nobody answered it, and it is gone");
        pages.answer_dialog(Some(name), false, "").expect("it cannot be answered");
        until("the box to be gone", &|| crate::pagedialog::open_on(name).is_none());
        assert_eq!(title(), "\"no\"", "Cancel did not reach the page as Cancel");

        // A prompt takes the words it is given
        pages
            .eval(Some(name), "setTimeout(() => { document.title = 'p:' + prompt('Your name?', 'nobody'); }, 0); return 1", 5_000)
            .unwrap();
        until("the prompt to be heard", &|| crate::pagedialog::open_on(name).is_some());
        assert_eq!(crate::pagedialog::open_on(name).unwrap().preset, "nobody");
        pages.answer_dialog(Some(name), true, "Ada").unwrap();
        until("the prompt to be gone", &|| crate::pagedialog::open_on(name).is_none());
        assert_eq!(title(), "\"p:Ada\"");

        // "Leave this page?" is answered with yes, and kept by nobody
        pages
            .eval(Some(name), "window.onbeforeunload = e => { e.preventDefault(); e.returnValue = 'x'; return 'x'; }; return 1", 5_000)
            .unwrap();
        // A page only gets to ask once somebody has touched it
        pages.click(Some(name), &Sel::Css("#go".into()), 10_000).unwrap();
        let _ = pages.eval(Some(name), "setTimeout(() => { location.href = '/next'; }, 0); return 1", 5_000);
        until("the page to be left", &|| title().contains("つぎ") || pages.href(Some(name), 5_000).is_ok_and(|u| u.ends_with("/next")));
        assert_eq!(crate::pagedialog::open_on(name), None, "leaving was held as a question");
        let _ = pages.close_child(name);
    }

    /// A browser that dies is replaced when a page is next opened, and a
    /// browser left on a profile by a run that was killed does not keep the
    /// next run from starting one. Against a real browser; the second half
    /// is about what a machine does with a process nobody waits for, so it
    /// runs where the program runs as a server
    ///
    ///     cargo test -p shikisha-core --lib a_browser_that_died -- --ignored --nocapture
    #[test]
    #[ignore = "needs a browser on this machine"]
    fn a_browser_that_died_is_replaced_and_a_stray_one_is_ended() {
        use shikisha_shared::{BrowserHost, BrowserProfile, Sel};
        assert!(found().is_some(), "this machine has no browser");
        let site = serve();
        let store = std::env::temp_dir().join(format!("shikisha-pages-{}", crate::random_hex(8)));
        // Profiles that last, as on a server: a throwaway one is never met twice
        let pages = Pages::under(store.clone(), false);
        let ready = |name: &'static str| {
            waited(&pages, |ev| match ev {
                shikisha_shared::Ev::Ready { from: Some(n), complete: true, .. } if n == name => Some(()),
                _ => None,
            })
            .expect("the page does not report that loading finished")
        };
        pages.open_child("first", &site, (0, 0, 900, 700), BrowserProfile::shared_default()).expect("the page does not open");
        ready("first");

        // The browser is killed under it
        let browser = pages.browser_for(&BrowserProfile::shared_default()).unwrap();
        let pid = browser.child.id();
        #[cfg(unix)]
        // SAFETY: a signal to this test's own child
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
        }
        drop(browser);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while pages.browsers.borrow().values().any(|c| c.alive()) {
            assert!(std::time::Instant::now() < until, "the browser was killed and is still taken to be there");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // A page opened now gets a browser, not "the browser is gone"
        pages.open_child("second", &site, (0, 0, 900, 700), BrowserProfile::shared_default()).expect("no page opens after the browser died");
        ready("second");
        assert_eq!(pages.text(Some("second"), &Sel::Css("#here".into()), 10_000).unwrap().as_deref(), Some("ここにいる"));
        assert!(pages.text(Some("first"), &Sel::Css("#here".into()), 2_000).is_err(), "a page of the dead browser is still answered for");

        // A browser nobody owns, on a profile: started by a shell that has
        // gone, the way one outlives a run that was killed
        #[cfg(unix)]
        {
            let profile = store.join("stray");
            std::fs::create_dir_all(&profile).unwrap();
            let exe = found().unwrap();
            let line = format!(
                "'{}' --headless=new --remote-debugging-port=0 --user-data-dir='{}' --no-first-run --no-sandbox about:blank >/dev/null 2>&1 &",
                exe.display(),
                profile.display()
            );
            std::process::Command::new("sh").arg("-c").arg(&line).status().unwrap();
            let named = format!("--user-data-dir={}", profile.display());
            let strays = || {
                let table = crate::repo::process_table(None, sysinfo::ProcessRefreshKind::nothing().with_cmd(sysinfo::UpdateKind::Always));
                table.processes().values().filter(|p| is_the_browser_on(p.cmd(), &named)).count()
            };
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while strays() == 0 {
                assert!(std::time::Instant::now() < until, "the stray browser did not start");
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            // Let it take the profile
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let started = std::time::Instant::now();
            let chrome = Chrome::start_in(profile.clone(), false).expect("a stray browser on the profile keeps this one from starting");
            assert!(started.elapsed() < std::time::Duration::from_secs(15), "it started only after waiting out the stray: {:?}", started.elapsed());
            assert_eq!(strays(), 1, "the stray is still there beside the new one");
            drop(chrome);
        }
        drop(pages);
        let _ = std::fs::remove_dir_all(&store);
    }

    /// The browser on a profile is told from its helpers and from a browser
    /// on a profile whose name only starts the same, however the system
    /// hands the command line back
    #[test]
    fn the_browser_on_a_profile_is_told_from_its_helpers_and_its_neighbours() {
        let cmd = |words: &[&str]| words.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        let named = "--user-data-dir=/srv/p/work";
        assert!(is_the_browser_on(&cmd(&["/opt/chrome", "--headless=new", named, "about:blank"]), named));
        assert!(is_the_browser_on(&cmd(&["/opt/chrome --headless=new --user-data-dir=/srv/p/work about:blank"]), named), "run together");
        assert!(is_the_browser_on(&cmd(&["/opt/chrome", named]), named), "last on the line");
        assert!(!is_the_browser_on(&cmd(&["/opt/chrome --type=renderer --user-data-dir=/srv/p/work --lang=en"]), named), "a helper");
        assert!(!is_the_browser_on(&cmd(&["/opt/chrome", "--user-data-dir=/srv/p/work2"]), named), "another profile");
        assert!(!is_the_browser_on(&cmd(&["/opt/chrome", "--headless=new"]), named));
    }

    /// A profile name cannot become a different folder.
    #[test]
    fn a_profile_name_is_only_ever_one_folder() {
        assert_eq!(folder_name("work"), "work");
        assert_eq!(folder_name("../../etc"), "etc");
        assert_eq!(folder_name("a/b"), "ab");
        assert_eq!(folder_name(""), "default");
        assert_eq!(folder_name("..."), "default");
        assert_ne!(folder_name("work"), folder_name("home"), "different names end up in the same place");
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
            "the report is made to carry a different name: {told:?}"
        );
        // Asking for something is not a report
        assert!(from_page(Ev::Say { tab: 0, uid: "test".into(), text: "cat /etc/shadow".into() }, "p").is_none());
        // Nothing is asked through the binding, so nothing answers through it
        assert!(
            from_page(Ev::Result { id: 1, ok: true, value: "\"y\"".into() }, "p").is_none(),
            "it accepts an answer nobody asked for"
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

        assert!(found().is_some(), "this machine has no browser");
        let site = serve();
        let store = std::env::temp_dir().join(format!("shikisha-pages-{}", crate::random_hex(8)));
        let pages = Pages::under(store, true);
        let t = 10_000;

        pages
            .open_child("p", &site, (0, 0, 900, 700), BrowserProfile::shared_default())
            .expect("the page does not open");

        // The page reports for itself, through a binding that stands where
        // the window's channel stands. Nothing below works if this does not
        let (url, complete) = wait_ready(&pages).expect("the page does not report that loading finished");
        assert_eq!(url, site, "it reports a different address");
        assert!(complete, "it says it finished without waiting for images and CSS");

        // It calls itself a browser like any other: not one with no window,
        // in the name or in what is said beside the name
        use crate::pageops::Speaks as _;
        let name = pages.eval(Some("p"), "return navigator.userAgent", t).unwrap();
        assert!(name.contains("Chrome/") && !name.contains("Headless"), "it says it has no window: {name}");
        let beside = pages
            .eval(Some("p"), "return navigator.userAgentData ? navigator.userAgentData.brands.map(b => b.brand).join(',') : 'none'", t)
            .unwrap();
        assert!(!beside.contains("Headless"), "what it says beside its name gives it away: {beside}");

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
            "the press did not register"
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
            "the value put in comes back changed"
        );

        // ── the digest, and the refs it hands out ────────────────────────
        let digest = pages.digest(Some("p"), t).expect("the digest cannot be taken");
        assert!(digest.contains("押す"), "what is visible on the board is not in the digest: {digest}");
        let number = |label: &str| -> u32 {
            digest
                .lines()
                .find(|l| l.contains(label))
                .and_then(|l| l.split(']').next())
                .and_then(|l| l.trim_start_matches('[').trim().parse().ok())
                .unwrap_or_else(|| panic!("the ref for {label} is not in the digest:\n{digest}"))
        };
        // A genuine mouse event, which is the whole reason refs exist
        let rep = pages.click(Some("p"), &Sel::Ref(number("触る")), t).expect("the ref cannot be pressed");
        assert_eq!(rep.state, Found::Visible);
        assert!(rep.echo.unwrap_or_default().contains("触る"), "it does not say what it pressed");
        assert_eq!(
            pages.text(Some("p"), &Sel::Css("#tapped".into()), t).unwrap().as_deref(),
            Some("触られた"),
            "real input did not arrive"
        );
        // And a ref that no longer means anything says so
        let err = pages.click(Some("p"), &Sel::Ref(999), t).unwrap_err().to_string();
        assert!(!err.is_empty(), "it quietly presses a ref that does not exist");

        // ── carrying a login from one place to another ───────────────────
        let cookies = pages.cookies_out(Some("p"), t).expect("the cookies cannot be read");
        assert!(
            cookies.as_array().is_some_and(|c| {
                c.iter().any(|k| k.get("value").and_then(|n| n.as_str()) == Some("mark"))
            }),
            "the cookies the browser holds do not come out: {cookies}"
        );
        let store = pages.storage_out(Some("p"), t).expect("localStorage cannot be read");
        assert!(store.to_string().contains("しるし"), "localStorage does not come out: {store}");
        pages.cookies_in(Some("p"), &cookies, t).expect("the cookies cannot be put back");
        pages.storage_in(Some("p"), &store, t).expect("localStorage cannot be put back");

        // ── a picture of it, which is what a phone is shown ──────────────
        let png = pages.snapshot(Some("p"), t).expect("cannot take it");
        assert!(png.len() > 2_000, "the picture is too small: {} bytes", png.len());
        assert_eq!(&png[..4], b"\x89PNG", "not a PNG");

        // ── a request made from inside the page ──────────────────────────
        let got = pages
            .fetch(Some("p"), &format!("{site}next"), &serde_json::json!({}), t)
            .expect("fetch fails");
        assert!(got.contains("\"status\":200"), "fetch's answer is odd: {got}");
        assert!(got.contains("次の画面"), "the body is not in it: {got}");

        // ── being watched, and touched from wherever it is watched ───────
        pages.screencast(Some("p"), true).expect("the stream does not start");
        let (w, h) = wait_frame(&pages).expect("not a single picture arrives");
        assert!(w >= 1 && h >= 1, "the picture has no size");
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
                        Input::Mouse {
                            phase: phase.into(),
                            x: at[0],
                            y: at[1],
                            down: false,
                            clicks: 1,
                        },
                    )
                    .expect("cannot touch");
            }
        };
        tap(&spot("reach"));
        assert_eq!(
            pages.text(Some("p"), &Sel::Css("#reached".into()), t).unwrap().as_deref(),
            Some("届いた"),
            "the finger from afar did not arrive"
        );
        // Typing arrives the same way, and lands wherever the last touch put
        // the cursor -- which is what makes a phone able to fill in a form
        pages.fill(Some("p"), &Sel::Css("#q".into()), "", t).unwrap();
        tap(&spot("q"));
        pages.inject(Some("p"), Input::Text { text: "遠くから".into() }).expect("cannot type");
        assert_eq!(
            pages.text(Some("p"), &Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some("遠くから"),
            "the typed text did not go in"
        );
        pages.screencast(Some("p"), false).expect("the stream does not stop");

        // ── where it is, and where it has been ───────────────────────────
        pages.ask_where(Some("p")).unwrap();
        let (at, back) = wait_where(&pages).expect("it does not say where it is now");
        assert_eq!(at, site);
        assert!(!back, "it says it can go back on the first page");
        pages.go(Some("p"), Go::To(format!("{site}next"))).expect("cannot move");
        wait_ready(&pages).expect("the destination does not report that loading finished");
        assert!(pages.href(Some("p"), t).unwrap().ends_with("/next"));
        pages.ask_where(Some("p")).unwrap();
        let (_, back) = wait_where(&pages).expect("it does not say where it is after moving");
        assert!(back, "there is a previous page, but it says it cannot go back");
        pages.go(Some("p"), Go::Back).expect("cannot go back");
        wait_ready(&pages).expect("where it went back to does not report that loading finished");
        assert_eq!(pages.href(Some("p"), t).unwrap(), site, "it did not go back");

        pages.close_child("p").expect("cannot close it");
        assert!(
            pages.find(Some("p"), &Sel::Css("#here".into()), t).is_err(),
            "a closed page can still be driven"
        );
        println!("all passed");
    }

    /// The seam: a runtime with no window, asked for something that shows
    /// pages, is handed this -- and what a page says lands in the mailbox the
    /// loop reads, without the loop knowing any of the above.
    #[test]
    #[ignore = "needs a browser on this machine"]
    fn a_runtime_with_no_window_is_handed_this_browser() {
        use crate::host::Shell as _;
        use shikisha_shared::{BrowserProfile, Sel};

        assert!(found().is_some(), "this machine has no browser");
        let site = serve();
        let store = std::env::temp_dir().join(format!("shikisha-seam-{}", crate::random_hex(8)));
        let mut shell =
            crate::host::Headless::browsing(24, 80, Rc::new(crate::placed::Placed::under(store, true)));

        let (host, rect) = shell.host().expect("the runtime with no window refuses the page");
        assert!(rect.2 > 0 && rect.3 > 0, "the page has no size: {rect:?}");
        host.open_child("p", &site, rect, BrowserProfile::shared_default())
            .expect("the page does not open");

        // The loop's own move: wait a moment, and see what arrived
        let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < until && shell.mail().loads.is_empty() {
            shell.poll(std::time::Duration::from_millis(200), None).unwrap();
        }
        let loaded = std::mem::take(&mut shell.mail().loads);
        assert!(
            loaded.iter().any(|(name, url, _)| name == "p" && url == &site),
            "loading finished did not reach the mailbox: {loaded:?}"
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

    /// A stored password goes into the page it belongs to and nowhere else,
    /// whatever the page says about itself. Each case here is a way it got
    /// out before: a page that answers "where are you?" with a lie and takes
    /// what the helper is handed, a field inside another site's frame, a
    /// page that goes elsewhere while the field is waited for
    #[test]
    #[ignore = "needs a browser on this machine"]
    fn a_stored_password_goes_only_into_the_page_it_belongs_to() {
        use crate::pageops::Speaks as _;
        use shikisha_shared::{BrowserHost, BrowserProfile, Found, Go, SecretFill, Sel};

        assert!(found().is_some(), "this machine has no browser");
        // Two sites: the one the password belongs to, and somebody else's
        let other = site_of(|_, _| OTHERS.to_string());
        let theirs = other.clone();
        let mine = site_of(move |path, me| match path {
            p if p.starts_with("/hop") => {
                let to = format!("{theirs}?as={me}");
                format!("<!doctype html><body><script>setTimeout(() => location.href = {to:?}, 400)</script>")
            }
            _ => MINE.replace("OTHER", &theirs),
        });
        let sites = vec![mine.trim_end_matches('/').to_string()];
        let store = std::env::temp_dir().join(format!("shikisha-secret-{}", crate::random_hex(8)));
        let pages = Pages::under(store, true);
        let t = 10_000;
        let secret = "S3cret-not-for-others";
        let said = |js: &str| pages.eval(Some("p"), js, t).unwrap_or_default();
        // What a field holds, asked without any helper the page could have replaced
        let holds = |css: &str| {
            let ask = format!("(e => e ? String(e.value ?? e.textContent) : null)(document.querySelector({css:?}))");
            pages
                .cdp(Some("p"), "Runtime.evaluate", serde_json::json!({ "expression": ask, "returnByValue": true }), t)
                .ok()
                .and_then(|v| v.pointer("/result/value").and_then(|x| x.as_str()).map(str::to_string))
        };
        pages.open_child("p", &mine, (0, 0, 900, 700), BrowserProfile::shared_default()).expect("the page does not open");
        wait_ready(&pages).expect("the page does not report that loading finished");

        // The page has replaced the helpers every other operation calls, to
        // say it is somewhere allowed and to keep what it is handed. The
        // password is written all the same, and the page's script saw none
        // of it on the way
        assert!(said("return String(window.__shikisha_href())").contains("allowed.example"), "the page's lie is not in place");
        let done = pages.fill_only_on(Some("p"), &Sel::Css("#pw".into()), secret, &sites, t).unwrap();
        assert!(matches!(done, SecretFill::Done(ref r) if r.state == Found::Visible), "{done:?}");
        assert_eq!(holds("#pw").as_deref(), Some(secret));
        assert_eq!(said("return String(window.__taken)"), "\"undefined\"", "the page's own script was handed the password");
        assert_eq!(said("return String(window.__heard)"), "\"input\"", "the page was not told its field changed");

        // Not a field: said, not written around
        let err = pages.fill_only_on(Some("p"), &Sel::Css("#label".into()), secret, &sites, t).unwrap_err().to_string();
        assert!(!err.is_empty());
        assert_eq!(holds("#label").as_deref(), Some("a label"));
        // Nor a field for a number: handed words, the browser refuses them
        // and writes what it refused into its log
        assert!(pages.fill_only_on(Some("p"), &Sel::Css("#count".into()), secret, &sites, t).is_err(), "a password was handed to a number field");
        // A page that copies what it is given into the field's own
        // description: what comes back says which field, and not what went in
        let copied = pages.fill_only_on(Some("p"), &Sel::Css("#copies".into()), secret, &sites, t).unwrap();
        assert!(matches!(copied, SecretFill::Done(ref r) if !r.echo.clone().unwrap_or_default().contains(secret)), "{copied:?}");
        assert_eq!(holds("#copies").as_deref(), Some(secret));
        // ...the second time as well, when the description already holds
        // the first password
        let again = pages.fill_only_on(Some("p"), &Sel::Css("#copies".into()), "another-secret", &sites, t).unwrap();
        assert!(
            matches!(again, SecretFill::Done(ref r) if !r.echo.clone().unwrap_or_default().contains("ecret")),
            "the description handed back a password: {again:?}"
        );
        // A field that turns into one for a number as it takes the focus
        // is not written into: the browser would log what it refused
        assert!(pages.fill_only_on(Some("p"), &Sel::Css("#turns".into()), secret, &sites, t).is_err(), "a password went into a field that became a number field");
        assert_eq!(holds("#turns").as_deref(), Some(""));

        // A field inside the other site's frame, named by its ref: refused
        // (its node is looked up in the browser and handed out as ref 1, the
        // way a digest that reaches into frames hands it out)
        let framed_field = (0..50)
            .find_map(|_| {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let doc = pages.cdp(Some("p"), "DOM.getDocument", serde_json::json!({ "depth": -1, "pierce": true }), t).ok()?;
                fn find(n: &serde_json::Value) -> Option<i64> {
                    let named = n.get("attributes").and_then(|a| a.as_array()).is_some_and(|a| a.iter().any(|v| v == "in-the-frame"));
                    if named {
                        return n.get("backendNodeId").and_then(serde_json::Value::as_i64);
                    }
                    ["children", "contentDocument", "shadowRoots"].iter().find_map(|k| match n.get(*k) {
                        Some(serde_json::Value::Array(kids)) => kids.iter().find_map(find),
                        Some(one) => find(one),
                        None => None,
                    })
                }
                find(doc.get("root")?)
            })
            .expect("the frame never loaded its field");
        pages.refs().lock().unwrap().insert(Some("p".to_string()), vec![framed_field]);
        let n = 1;
        let framed = pages.fill_only_on(Some("p"), &Sel::Ref(n), secret, &sites, t);
        assert!(framed.is_err(), "a password went into another site's frame: {framed:?}");

        // A page that goes to the other site while the field is waited for:
        // the field turns up there, and stays empty
        pages.go(Some("p"), Go::To(format!("{mine}hop"))).unwrap();
        let went = pages.fill_only_on(Some("p"), &Sel::Css("#late".into()), secret, &sites, t).unwrap();
        assert!(matches!(went, SecretFill::WrongSite(ref at) if at.starts_with(&other)), "{went:?}");
        assert_eq!(holds("#late").as_deref(), Some(""), "the password followed the page to another site");

        // A number taken on one page means nothing on the next: refused, not
        // tried against whatever the new page has under that number
        pages.digest(Some("p"), t).expect("the digest cannot be taken");
        pages.go(Some("p"), Go::To(format!("{other}?as={mine}&again=1"))).unwrap();
        let stale = (0..50).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            pages.click(Some("p"), &Sel::Ref(1), 2_000).err().map(|e| e.to_string())
        });
        assert!(stale.is_some(), "a number from the page before was pressed on this one");

        // ...and on the other site, saying it is the first one changes nothing
        assert!(said("return String(window.__shikisha_href())").starts_with(&format!("\"{}", mine.trim_end_matches('/'))), "the page's lie is not in place");
        let lied = pages.fill_only_on(Some("p"), &Sel::Css("#late".into()), secret, &sites, 3_000).unwrap();
        assert!(matches!(lied, SecretFill::WrongSite(_)), "{lied:?}");
        assert_eq!(holds("#late").as_deref(), Some(""));
    }

    /// A site on 127.0.0.1 whose pages are whatever `page` writes for a path
    /// (and its own address, known only once it is up)
    fn site_of(page: impl Fn(&str, &str) -> String + Send + 'static) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let at = format!("http://127.0.0.1:{}/", server.server_addr().to_ip().unwrap().port());
        let me = at.clone();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = page(req.url(), &me).replace("SELF", me.trim_end_matches('/'));
                let _ = req.respond(tiny_http::Response::from_string(body).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
                ));
            }
        });
        at
    }

    /// The page the password belongs to -- which also does what a hostile
    /// page would: replaces the helpers, and embeds another site
    const MINE: &str = r#"<!doctype html><meta charset=utf-8><title>mine</title><body>
<input id=pw type=password oninput="window.__heard = 'input'">
<input id=count type=number>
<input id=turns type=password onfocus="this.type = 'number'">
<input id=copies placeholder="word" oninput="this.placeholder = this.value; this.name = this.value">
<div id=label tabindex=0>a label</div>
<iframe src="OTHERframe" width=300 height=80></iframe>
<script>
  window.__shikisha_href = () => "https://allowed.example/";
  for (const name of ["__shikisha_fill", "__shikisha_fill_at", "__shikisha_q", "__shikisha_resolve"]) {
    window[name] = function () { window.__taken = Array.from(arguments).join("|"); return "visible"; };
  }
</script>
"#;

    /// Somebody else's site: a field waiting to be filled, a frame's worth
    /// of one, and the claim to be the first site
    const OTHERS: &str = r#"<!doctype html><meta charset=utf-8><title>other</title><body>
<input id=late placeholder="in-the-frame">
<script>
  const first = new URLSearchParams(location.search).get("as") || document.referrer || "";
  window.__shikisha_href = () => (first || "http://127.0.0.1/").replace(/\/[^/]*$/, "") + "/";
</script>
"#;

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

    /// A page searched for words, and a file it saves, on the browser with
    /// no window: the search is the script every page is given, and the file
    /// lands in the downloads folder under its own name, numbered like a
    /// browser numbers a second one.
    ///
    ///     cargo test -p shikisha-core --lib chrome:: -- --ignored --nocapture
    #[test]
    #[ignore = "needs a browser on this machine"]
    fn a_page_here_is_searched_and_saves_what_it_downloads() {
        use shikisha_shared::{BrowserHost, BrowserProfile, DownloadState, Ev, Seek, Sel};
        assert!(found().is_some(), "this machine has no browser");
        let into = std::env::temp_dir().join(format!("shikisha-dl-{}", crate::random_hex(6)));
        // SAFETY: set before the browser starts, and read only by this test's
        // own browser; no other test reads it
        unsafe { std::env::set_var("SHIKISHA_DOWNLOADS", &into) };
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let resp = if req.url().starts_with("/report.txt") {
                    tiny_http::Response::from_string("いちご\n").with_header(
                        tiny_http::Header::from_bytes(&b"Content-Disposition"[..], &b"attachment; filename=\"report.txt\""[..]).unwrap(),
                    )
                } else {
                    tiny_http::Response::from_string(SEARCHED).with_header(
                        tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
                    )
                };
                let _ = req.respond(resp);
            }
        });
        let site = format!("http://127.0.0.1:{port}/");
        let store = std::env::temp_dir().join(format!("shikisha-pages-{}", crate::random_hex(8)));
        let pages = Pages::under(store, true);
        pages.open_child("p", &site, (0, 0, 900, 700), BrowserProfile::shared_default()).expect("the page does not open");
        wait_ready(&pages).expect("the page does not report that loading finished");

        // ── searching ────────────────────────────────────────────────────
        // Three shown, whatever their case, one of them split by a tag; the
        // hidden one and the one across two blocks are not matches
        let seek = |text: &str, step| pages.seek(Some("p"), text, step).unwrap().expect("no answer");
        assert_eq!(seek("apple pie", Seek::New), (1, 3), "the matches were not counted as a person sees them");
        assert_eq!(seek("apple pie", Seek::Next), (2, 3));
        assert_eq!(seek("apple pie", Seek::Next), (3, 3));
        assert_eq!(seek("apple pie", Seek::Next), (1, 3), "it does not go round after the last");
        assert_eq!(seek("apple pie", Seek::Prev), (3, 3), "it does not go round before the first");
        assert_eq!(seek("いちご", Seek::New), (1, 1));
        assert_eq!(seek("nothing like it", Seek::New), (0, 0));
        assert_eq!(seek("apple pie", Seek::Stop), (0, 0));

        // ── saving ───────────────────────────────────────────────────────
        let mut saved = Vec::new();
        for _ in 0..2 {
            pages.click(Some("p"), &Sel::Css("#dl".into()), 10_000).expect("the link was not pressed");
            let done = waited(&pages, |ev| match ev {
                Ev::Download { from, item } if item.state == DownloadState::Done => Some((from.clone(), item.clone())),
                Ev::Download { item, .. } if item.state != DownloadState::Going => panic!("the download ended {:?} ({})", item.state, item.why),
                _ => None,
            })
            .expect("the download was never reported done");
            assert_eq!(done.0.as_deref(), Some("p"), "the page that saved it was not named");
            saved.push(done.1);
        }
        assert_eq!(saved[0].name, "report.txt");
        assert_eq!(saved[1].name, "report (1).txt", "a second file of the same name replaced the first");
        for d in &saved {
            assert_eq!(std::fs::read_to_string(&d.path).unwrap(), "いちご\n");
            assert!(std::path::Path::new(&d.path).starts_with(&into), "it was saved somewhere else: {}", d.path);
        }
        // Nothing is left half-named in the folder it arrives in
        assert!(
            std::fs::read_dir(pages.staging("default")).map(|d| d.count()).unwrap_or(0) == 0,
            "a file was left where downloads wait"
        );
        let _ = pages.close_child("p");
        let _ = std::fs::remove_dir_all(&into);
    }

    const SEARCHED: &str = r#"<!doctype html><meta charset=utf-8><title>さがす</title><body>
<p>Apple pie, then APPLE   PIE again.</p>
<p>And ap<b>ple</b> pie.</p>
<p style="display:none">apple pie hidden</p>
<p>apple</p><p>pie</p>
<p>いちごのケーキ</p>
<a id=dl href="/report.txt">保存</a>
"#;

    /// A window a page opens is said to the loop and kept for the tab made
    /// for it, which takes it as it stands -- not closed and opened again --
    /// and one no tab is made for does not go on running unseen
    #[test]
    #[ignore = "needs a browser on this machine"]
    fn a_window_a_page_opens_becomes_the_tab_made_for_it() {
        use crate::pageops::Speaks as _;
        use shikisha_shared::{BrowserHost, BrowserProfile, Sel};

        assert!(found().is_some(), "this machine has no browser");
        let site = serve();
        let store = std::env::temp_dir().join(format!("shikisha-pages-{}", crate::random_hex(8)));
        let pages = Pages::under(store, true);
        let name = "opens-windows";
        pages.open_child(name, &site, (0, 0, 900, 700), BrowserProfile::shared_default()).expect("the page does not open");
        waited(&pages, |ev| match ev {
            shikisha_shared::Ev::Ready { from: Some(n), complete: true, .. } if n == name => Some(()),
            _ => None,
        })
        .expect("the page does not report that loading finished");
        let chrome = pages.browsers.borrow().values().next().map(Rc::clone).unwrap();
        // The pages the browser has
        let standing = || {
            let all = chrome.call("Target.getTargets", serde_json::json!({})).unwrap();
            all.get("targetInfos")
                .and_then(|t| t.as_array())
                .map(|t| t.iter().filter(|i| i.get("type").and_then(|k| k.as_str()) == Some("page")).count())
                .unwrap_or(0)
        };
        let before = standing();
        let said_now = |pages: &Pages| -> Vec<(String, bool)> {
            pages
                .drain()
                .into_iter()
                .filter_map(|ev| match ev {
                    shikisha_shared::Ev::PageWindow { from: Some(n), url, by_hand, .. } if n == name => Some((url, by_hand)),
                    _ => None,
                })
                .collect()
        };
        let said = |pages: &Pages| {
            waited(pages, |ev| match ev {
                shikisha_shared::Ev::PageWindow { from: Some(n), url, by_hand, window } if n == name => Some((url.clone(), *by_hand, window.clone())),
                _ => None,
            })
        };
        let until = |what: &str, done: &dyn Fn() -> bool| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !done() {
                assert!(std::time::Instant::now() < end, "{what} ({} pages, {before} before)", standing());
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        };
        let gone_now = |pages: &Pages| -> Vec<String> {
            pages
                .drain()
                .into_iter()
                .filter_map(|ev| match ev {
                    shikisha_shared::Ev::PageWindowGone { window } => Some(window),
                    _ => None,
                })
                .collect()
        };
        // Every window still waiting, as if it had waited too long
        let waited_out = |pages: &Pages| {
            let mut book = pages.seen.lock().unwrap();
            let long = WINDOW_BLANK_FOR + WINDOW_KEPT_FOR;
            for w in book.windows.kept.values_mut() {
                w.since = std::time::Instant::now().checked_sub(long).unwrap();
            }
            for w in book.windows.blank.values_mut() {
                w.since = std::time::Instant::now().checked_sub(long).unwrap();
            }
        };

        // `window.open` from a script by itself, before anything was pressed
        // on the page: a window the browser refuses to open is no tab; one
        // it lets open is said as a script's. Asked of the page as the
        // page's own script would -- what automation evaluates counts as a
        // press. No tab is made for it here, and it does not stay
        let (_, session) = pages.at(Some(name)).unwrap();
        chrome
            .call_page(&session, "Runtime.evaluate", serde_json::json!({ "expression": "setTimeout(() => window.open('/by-script'), 0); 1" }))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let opened = standing() != before;
        match said_now(&pages).as_slice() {
            [] => assert!(!opened, "a window opened and nothing was said of it"),
            [(url, by_hand)] => {
                assert_eq!(url, &format!("{site}by-script"));
                assert!(!by_hand, "a script by itself is taken for a press");
            }
            more => panic!("one window was said {} times: {more:?}", more.len()),
        }
        waited_out(&pages);
        let _ = pages.drain();
        until("a window no tab was made for is still running", &|| standing() == before);

        // A window opened by a press, which its opener then writes into:
        // said once, with the press, and still there for its tab
        pages
            .eval(Some(name), "window.w = window.open('/elsewhere?x=1'); return 1", 5_000)
            .unwrap();
        let (url, by_hand, window) = said(&pages).expect("a window a page opened was not said");
        assert_eq!(url, format!("{site}elsewhere?x=1"), "not the whole address");
        assert!(by_hand, "a press on the page is not told from a script");
        assert_eq!(standing(), before + 1, "the window was closed before its tab could take it");
        until("the opener cannot reach the window it opened", &|| {
            pages.eval(Some(name), "try { w.marked = 'by the opener'; return w.document.readyState } catch (e) { return String(e) }", 5_000).is_ok_and(|s| s.contains("complete"))
        });

        // A tab that merely opens the same address is not handed the window:
        // it is a page of its own, and the window still waits
        pages.open_child("stranger", &url, (0, 0, 900, 700), BrowserProfile::shared_default()).unwrap();
        assert_eq!(standing(), before + 2, "a tab at the same address took a window that was not made for it");
        waited(&pages, |ev| match ev {
            shikisha_shared::Ev::Ready { from: Some(n), complete: true, .. } if n == "stranger" => Some(()),
            _ => None,
        })
        .expect("the other tab at that address never loaded");
        assert_eq!(pages.eval(Some("stranger"), "return window.marked || 'a new document'", 5_000).unwrap(), "\"a new document\"");
        pages.close_child("stranger").unwrap();
        until("the other tab's page is still there after it closed", &|| standing() == before + 1);

        // The tab made for it takes that very window: the same document
        // (what the opener wrote is there), still its opener's, not fetched
        // a second time
        crate::pagewindow::claim("took", &window);
        pages.open_child("took", &url, (0, 0, 900, 700), BrowserProfile::shared_default()).expect("the tab made for the window does not open");
        assert_eq!(standing(), before + 1, "a second page was opened beside the window");
        assert_eq!(pages.eval(Some("took"), "return window.marked || 'a new document'", 5_000).unwrap(), "\"by the opener\"", "the window was opened again, not taken");
        assert_eq!(pages.eval(Some("took"), "return String(!!window.opener)", 5_000).unwrap(), "\"true\"", "it is no longer its opener's window");
        assert_eq!(pages.eval(Some("took"), "return String(performance.getEntriesByType('navigation').length)", 5_000).unwrap(), "\"1\"");
        // ...and it is a page like any other from here on
        waited(&pages, |ev| match ev {
            shikisha_shared::Ev::Ready { from: Some(n), .. } if n == "took" => Some(()),
            _ => None,
        })
        .expect("a window taken as a page never says how far it has got");
        assert!(pages.href(Some("took"), 5_000).unwrap().ends_with("elsewhere?x=1"));
        pages.close_child("took").unwrap();
        until("the tab's page is still there after it closed", &|| standing() == before);

        // A link that opens in a new tab, pressed: said, with the press
        pages
            .eval(
                Some(name),
                "const a = document.createElement('a'); a.id = 'blank'; a.target = '_blank'; a.href = '/linked'; a.textContent = 'linked'; \
                 a.style.cssText = 'position:fixed;left:10px;top:10px;font-size:30px;z-index:99999'; document.body.appendChild(a); return 1",
                5_000,
            )
            .unwrap();
        pages.click(Some(name), &Sel::Css("#blank".into()), 10_000).unwrap();
        let (url, by_hand, window) = said(&pages).expect("a link that opens a new tab was not said");
        assert_eq!(url, format!("{site}linked"));
        assert!(by_hand);
        // Its tab is on a desk nobody is looking at, and does not come for
        // it for a long while: the window waits for it all the same
        crate::pagewindow::claim("linked", &window);
        waited_out(&pages);
        let _ = pages.drain();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(standing(), before + 1, "a window its tab had not come for yet was closed");
        pages.open_child("linked", &url, (0, 0, 900, 700), BrowserProfile::shared_default()).unwrap();
        assert_eq!(standing(), before + 1);
        pages.close_child("linked").unwrap();
        until("the linked page is still there after it closed", &|| standing() == before);

        // A window opened blank and sent somewhere a moment later, as a
        // sign-in does it: followed to where it goes, said once, and taken
        pages
            .eval(Some(name), "const b = window.open('', 'login'); setTimeout(() => { b.location = '/signin?step=2'; }, 400); return 1", 5_000)
            .unwrap();
        let (url, by_hand, window) = said(&pages).expect("a window opened blank and then sent somewhere was not followed");
        assert_eq!(url, format!("{site}signin?step=2"));
        assert!(by_hand);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(said_now(&pages).is_empty(), "one window was said twice");
        crate::pagewindow::claim("signin", &window);
        pages.open_child("signin", &url, (0, 0, 900, 700), BrowserProfile::shared_default()).unwrap();
        assert_eq!(standing(), before + 1);
        // Done, it closes itself, as a sign-in's last step does: said, so
        // that its tab goes with it -- and a tab closed by the person is not
        // said to have closed itself
        let gone = |pages: &Pages| {
            waited(pages, |ev| match ev {
                shikisha_shared::Ev::PageWindowGone { window } => Some(window.clone()),
                _ => None,
            })
        };
        pages.eval(Some("signin"), "setTimeout(() => window.close(), 0); return 1", 5_000).unwrap();
        assert_eq!(gone(&pages).as_deref(), Some(window.as_str()), "a window that closed itself was not said to have");
        pages.close_child("signin").unwrap();
        until("the window that closed itself is still there", &|| standing() == before);

        // One that closes itself before its tab comes for it: the tab has
        // nothing to take, and does not ask the server for the address again
        pages.eval(Some(name), "window.once = window.open('/once?code=1'); return 1", 5_000).unwrap();
        let (url, _, window) = said(&pages).expect("the window was not said");
        crate::pagewindow::claim("late", &window);
        pages.eval(Some(name), "once.close(); return 1", 5_000).unwrap();
        assert_eq!(gone(&pages).as_deref(), Some(window.as_str()));
        assert!(pages.open_child("late", &url, (0, 0, 900, 700), BrowserProfile::shared_default()).is_err(), "a window that closed itself was opened again at its address");
        assert_eq!(standing(), before);

        // A private page's window is in the private page's storage, and goes
        // on after the page that opened it is closed
        let private = BrowserProfile { private: true, ..BrowserProfile::shared_default() };
        pages.open_child("private", &site, (0, 0, 900, 700), private.clone()).unwrap();
        waited(&pages, |ev| match ev {
            shikisha_shared::Ev::Ready { from: Some(n), complete: true, .. } if n == "private" => Some(()),
            _ => None,
        })
        .expect("the private page never loaded");
        pages.eval(Some("private"), "document.cookie = 'kept=private; path=/'; window.open('/in-private'); return 1", 5_000).unwrap();
        let (url, window) = waited(&pages, |ev| match ev {
            shikisha_shared::Ev::PageWindow { from: Some(n), url, window, .. } if n == "private" => Some((url.clone(), window.clone())),
            _ => None,
        })
        .expect("the private page's window was not said");
        crate::pagewindow::claim("private-window", &window);
        pages.open_child("private-window", &url, (0, 0, 900, 700), private).unwrap();
        pages.close_child("private").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let cookies = pages.eval(Some("private-window"), "return document.cookie", 5_000).expect("the window went with the page that opened it");
        assert!(cookies.contains("kept=private"), "the window is not in the storage of the page that opened it: {cookies}");
        pages.close_child("private-window").unwrap();
        until("the private window is still there after it closed", &|| standing() == before);
        assert!(gone_now(&pages).is_empty(), "a tab the person closed was said to have closed itself");

        // One left blank and never sent anywhere does not stay for ever,
        // though the browser says nothing more of it
        pages.eval(Some(name), "window.open('', 'nowhere'); return 1", 5_000).unwrap();
        until("a blank window was not opened", &|| standing() == before + 1);
        waited_out(&pages);
        let _ = pages.drain();
        until("a window left blank is still running", &|| standing() == before);

        // The page that asked goes on
        assert_eq!(pages.eval(Some(name), "return 1 + 1", 5_000).unwrap(), "2");
    }

    /// The browser answers on no port of its own, and a page's DevTools
    /// screen reaches that page -- and only through the door, with its key
    #[test]
    #[ignore = "needs a browser on this machine"]
    fn a_devtools_screen_reaches_its_page_through_the_door_and_the_browser_has_no_port() {
        use crate::pageops::Speaks as _;
        use shikisha_shared::{BrowserHost, BrowserProfile};
        use std::io::Read as _;

        assert!(found().is_some(), "this machine has no browser");
        let site = serve();
        let store = std::env::temp_dir().join(format!("shikisha-pages-{}", crate::random_hex(8)));
        let pages = Pages::under(store.clone(), true);
        let name = "inspected";
        pages.open_child(name, &site, (0, 0, 900, 700), BrowserProfile::shared_default()).expect("the page does not open");
        waited(&pages, |ev| match ev {
            shikisha_shared::Ev::Ready { from: Some(n), complete: true, .. } if n == name => Some(()),
            _ => None,
        })
        .expect("the page does not report that loading finished");

        // A browser that answers on a port writes the port down in its profile
        let written = |root: &std::path::Path| {
            std::fs::read_dir(root).into_iter().flatten().flatten().any(|d| d.path().join("DevToolsActivePort").exists())
        };
        assert!(!written(&store), "the browser is answering on a port of its own");

        let screen = pages.devtools_url(Some(name)).expect("no DevTools screen for the page");
        let rest = screen.strip_prefix("devtools://devtools/bundled/devtools_app.html?ws=").expect("not the browser's own screen");
        let (host, path) = rest.split_once('/').unwrap();
        let path = format!("/{path}");
        assert!(!written(&store), "asking for a screen opened the browser's own port");

        // One opening, and what came back first
        let open = |path: &str, origin: Option<&str>| {
            let mut s = std::net::TcpStream::connect(host).expect("the door is not there");
            s.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
            let origin = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
            write!(s, "GET {path} HTTP/1.1\r\nHost: {host}\r\n{origin}Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
            let mut head = Vec::new();
            let mut one = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && s.read(&mut one).is_ok_and(|n| n == 1) {
                head.push(one[0]);
            }
            (s, String::from_utf8_lossy(&head).into_owned())
        };
        // Another program on the machine, with or without the key; a web
        // page that learned the address; the browser's own way of asking
        for (path, origin) in [
            (path.as_str(), None),
            (path.as_str(), Some("https://example.com")),
            ("/devtools/page/x", Some(SCREEN_ORIGIN)),
            ("/json/version", None),
            (&format!("{}0", path.rsplit_once('/').unwrap().0), Some(SCREEN_ORIGIN)),
        ] {
            let (_, head) = open(path, origin);
            assert!(head.starts_with("HTTP/1.1 403"), "{path} from {origin:?} was answered: {head}");
        }

        // The screen itself: what it asks reaches the page, and the answer
        // comes back as if the page were all there is
        let (mut s, head) = open(&path, Some(SCREEN_ORIGIN));
        assert!(head.starts_with("HTTP/1.1 101"), "the screen was turned away: {head}");
        let ask = serde_json::json!({"id": 7, "method": "Runtime.evaluate", "params": {"expression": "document.title", "returnByValue": true}});
        s.write_all(&crate::ws::client_encode(crate::ws::Op::Text, ask.to_string().as_bytes())).unwrap();
        let (_, _, body) = crate::ws::read_server_frame(&mut s).expect("the page did not answer the screen");
        let answer: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(answer.get("id"), Some(&serde_json::json!(7)), "{answer}");
        assert!(answer.get("sessionId").is_none(), "{answer}");
        let title: String = serde_json::from_str(&pages.eval(Some(name), "return document.title", 5_000).unwrap()).unwrap();
        assert!(!title.is_empty());
        assert_eq!(answer.pointer("/result/result/value").and_then(|v| v.as_str()), Some(title.as_str()), "{answer}");
        // Asked one thing, and what came back under its number
        let ask = |s: &mut std::net::TcpStream, said: serde_json::Value| -> serde_json::Value {
            s.write_all(&crate::ws::client_encode(crate::ws::Op::Text, said.to_string().as_bytes())).unwrap();
            loop {
                let (_, _, body) = crate::ws::read_server_frame(s).expect("the door did not answer");
                let answer: serde_json::Value = serde_json::from_slice(&body).unwrap();
                if answer.get("id") == said.get("id") {
                    return answer;
                }
            }
        };
        // A name is not a way to another page of this browser
        let answer = ask(&mut s, serde_json::json!({"id": 8, "sessionId": "0123456789ABCDEF", "method": "Runtime.evaluate", "params": {"expression": "1"}}));
        assert!(answer.get("error").is_some(), "{answer}");

        // Nor is the protocol itself: a second page, and everything that
        // would reach it from the first one's screen
        pages.open_child("beside", &site, (0, 0, 900, 700), BrowserProfile::shared_default()).expect("the second page does not open");
        let beside = pages.open.borrow().get("beside").map(|p| p.target.clone()).unwrap();
        let before = pages.eval(Some("beside"), "return document.title", 5_000).unwrap();
        for (n, said) in [
            serde_json::json!({"method": "Target.attachToTarget", "params": {"targetId": beside, "flatten": true}}),
            serde_json::json!({"method": "Target.getTargets", "params": {}}),
            serde_json::json!({"method": "Target.getTargetInfo", "params": {"targetId": beside}}),
            serde_json::json!({"method": "Target.createTarget", "params": {"url": "about:blank"}}),
            serde_json::json!({"method": "Target.closeTarget", "params": {"targetId": beside}}),
            serde_json::json!({"method": "Browser.close", "params": {}}),
            serde_json::json!({"method": "Browser.setPermission", "params": {"permission": {"name": "geolocation"}, "setting": "granted", "origin": "https://example.com"}}),
            serde_json::json!({"method": "Network.getAllCookies", "params": {}}),
            serde_json::json!({"method": "Storage.getCookies", "params": {}}),
        ]
        .into_iter()
        .enumerate()
        {
            let mut said = said;
            said["id"] = serde_json::json!(100 + n);
            let answer = ask(&mut s, said.clone());
            assert!(answer.get("error").is_some(), "{said} was let through: {answer}");
        }
        // A cookie of the page's own site is the screen's to set and take
        // away; another site's is not, by any of the roads
        let here = crate::devscreen::host_of(&site);
        let answer = ask(&mut s, serde_json::json!({"id": 110, "method": "Network.setCookie", "params": {"name": "by-screen", "value": "1", "url": site}}));
        assert_eq!(answer.pointer("/result/success"), Some(&serde_json::json!(true)), "{answer}");
        assert!(pages.eval(Some(name), "return document.cookie", 5_000).unwrap().contains("by-screen=1"));
        let answer = ask(&mut s, serde_json::json!({"id": 111, "method": "Network.deleteCookies", "params": {"name": "by-screen", "domain": here}}));
        assert!(answer.get("result").is_some(), "{answer}");
        for (n, said) in [
            serde_json::json!({"method": "Network.setCookie", "params": {"name": "sid", "value": "x", "url": "https://bank.example.org/"}}),
            serde_json::json!({"method": "Network.setCookie", "params": {"name": "sid", "value": "x", "domain": "bank.example.org", "path": "/"}}),
            serde_json::json!({"method": "Network.setCookies", "params": {"cookies": [{"name": "sid", "value": "x", "url": "https://bank.example.org/"}]}}),
            serde_json::json!({"method": "Network.deleteCookies", "params": {"name": "sid", "url": "https://bank.example.org/"}}),
            serde_json::json!({"method": "Storage.clearDataForOrigin", "params": {"origin": "https://bank.example.org", "storageTypes": "cookies"}}),
        ]
        .into_iter()
        .enumerate()
        {
            let mut said = said;
            said["id"] = serde_json::json!(112 + n);
            let answer = ask(&mut s, said.clone());
            assert!(answer.get("error").is_some(), "{said} was let through: {answer}");
        }
        // What every screen asks as it opens is answered, with nothing told
        let answer = ask(&mut s, serde_json::json!({"id": 120, "method": "Target.setDiscoverTargets", "params": {"discover": true}}));
        assert!(answer.get("result").is_some(), "{answer}");
        let answer = ask(&mut s, serde_json::json!({"id": 121, "method": "Target.getTargetInfo", "params": {}}));
        assert_eq!(answer.pointer("/result/targetInfo/targetId").and_then(|t| t.as_str()), Some(rest.rsplit_once('/').unwrap().1), "{answer}");
        assert_eq!(pages.eval(Some("beside"), "return document.title", 5_000).unwrap(), before, "the page beside it was reached");
        // The first page's key does not open the second, which has a key of
        // its own once a screen is asked for it
        let (_, head) = open(&format!("{}/{beside}", path.rsplit_once('/').unwrap().0), Some(SCREEN_ORIGIN));
        assert!(head.starts_with("HTTP/1.1 403"), "one page's key opened another: {head}");
        let other = pages.devtools_url(Some("beside")).unwrap();
        assert_ne!(other.rsplit('/').nth(1), screen.rsplit('/').nth(1), "two pages share a key");

        // A long command arrives in pieces, as a browser sends one, and is
        // put together again
        let long = serde_json::json!({"id": 130, "method": "Runtime.evaluate", "params": {"expression": format!("'{}'.length", "x".repeat(300_000)), "returnByValue": true}}).to_string();
        let bytes = long.as_bytes();
        for (n, piece) in bytes.chunks(100_000).enumerate() {
            let mut frame = crate::ws::client_encode(crate::ws::Op::Text, piece);
            let last = (n + 1) * 100_000 >= bytes.len();
            frame[0] = (if last { 0x80 } else { 0x00 }) | (if n == 0 { 0x01 } else { 0x00 });
            s.write_all(&frame).unwrap();
        }
        let answer = loop {
            let (_, _, body) = crate::ws::read_server_frame(&mut s).expect("a command sent in pieces ended the line");
            let answer: serde_json::Value = serde_json::from_slice(&body).unwrap();
            if answer.get("id") == Some(&serde_json::json!(130)) {
                break answer;
            }
        };
        assert_eq!(answer.pointer("/result/result/value"), Some(&serde_json::json!(300_000)), "{answer}");
        drop(s);

        // And the browser's own screen, opened as a page: it connects, and
        // is talking to the page
        pages.open_child("its-screen", &screen, (0, 0, 900, 700), BrowserProfile::shared_default()).expect("the screen does not open");
        let chrome = pages.browsers.borrow().values().next().map(Rc::clone).unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let screens = chrome.cdp.screens.lock().unwrap();
            if !screens.lines.is_empty() && screens.next >= 2 {
                break;
            }
            drop(screens);
            assert!(std::time::Instant::now() < end, "the browser's own DevTools screen never connected");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // ...and stays: what it is refused does not make it give up
        std::thread::sleep(std::time::Duration::from_secs(3));
        assert!(!chrome.cdp.screens.lock().unwrap().lines.is_empty(), "the browser's own DevTools screen hung up");
        // Automation goes on beside it
        assert_eq!(pages.eval(Some(name), "return 1 + 1", 5_000).unwrap(), "2");
    }

    /// The door for DevTools screens lets in the screen, with the key, at
    /// the address it was given -- and nobody else
    #[test]
    fn the_devtools_door_is_opened_by_its_key_from_the_screen_alone() {
        let asked = |path: &str, origin: &str, host: &str| crate::ws::Opening {
            path: path.into(),
            origin: origin.into(),
            host: host.into(),
            key: "dGhlIHNhbXBsZSBub25jZQ==".into(),
        };
        let keys: std::collections::HashMap<String, String> =
            [("AB12CD", "k3y"), ("EF34", "0th3r")].into_iter().map(|(t, k)| (t.to_string(), k.to_string())).collect();
        let let_in = |a: &crate::ws::Opening| screen_admitted(a, 4000, &keys);
        let ok = asked("/devtools/k3y/AB12CD", SCREEN_ORIGIN, "127.0.0.1:4000");
        assert_eq!(let_in(&ok).as_deref(), Some("AB12CD"));
        // Another program on the machine: no key, or a guess at one
        assert_eq!(let_in(&asked("/devtools/page/AB12CD", SCREEN_ORIGIN, "127.0.0.1:4000")), None);
        assert_eq!(let_in(&asked("/devtools/k3/AB12CD", SCREEN_ORIGIN, "127.0.0.1:4000")), None);
        assert_eq!(let_in(&asked("/json/list", SCREEN_ORIGIN, "127.0.0.1:4000")), None);
        // One page's key is not another's: the address of one tab's screen
        // does not open the tab beside it, nor a page no screen was asked for
        assert_eq!(let_in(&asked("/devtools/k3y/EF34", SCREEN_ORIGIN, "127.0.0.1:4000")), None);
        assert_eq!(let_in(&asked("/devtools/k3y/9999", SCREEN_ORIGIN, "127.0.0.1:4000")), None);
        assert_eq!(let_in(&asked("/devtools/0th3r/EF34", SCREEN_ORIGIN, "127.0.0.1:4000")).as_deref(), Some("EF34"));
        // A web page that learned the address: it speaks from its own origin,
        // or from none, or through a name that only resolves to this machine
        assert_eq!(let_in(&asked("/devtools/k3y/AB12CD", "https://example.com", "127.0.0.1:4000")), None);
        assert_eq!(let_in(&asked("/devtools/k3y/AB12CD", "", "127.0.0.1:4000")), None);
        assert_eq!(let_in(&asked("/devtools/k3y/AB12CD", SCREEN_ORIGIN, "rebound.example.com:4000")), None);
        // Not an opening at all, and not a page's name
        assert_eq!(let_in(&crate::ws::Opening { key: String::new(), ..ok.clone() }), None);
        assert_eq!(let_in(&asked("/devtools/k3y/", SCREEN_ORIGIN, "127.0.0.1:4000")), None);
        assert_eq!(let_in(&asked("/devtools/k3y/a/../b", SCREEN_ORIGIN, "127.0.0.1:4000")), None);
    }

    /// What the browser says is handed to the DevTools screen it is for, as
    /// that screen expects to hear it, and to nobody else
    #[test]
    fn a_devtools_screen_hears_its_own_page_and_nothing_else() {
        let heard = |rx: &std::sync::mpsc::Receiver<Vec<u8>>| -> Vec<serde_json::Value> {
            // The lane writes on a thread of its own: given a moment
            std::thread::sleep(std::time::Duration::from_millis(100));
            rx.try_iter()
                .map(|frame| {
                    let (op, body) = crate::ws::read_frame_upto(&mut masked(&frame).as_slice(), SCREEN_COMMAND).unwrap();
                    assert_eq!(op, crate::ws::Op::Text);
                    serde_json::from_slice(&body).unwrap()
                })
                .collect()
        };
        let mut screens = Screens::default();
        let said = |v: serde_json::Value| v;
        // No screen: everything is this program's own
        assert!(screens.take(said(serde_json::json!({"id": 1, "result": {}}))).is_some());

        // Written into a pipe of the test's own, read back frame by frame
        let (tx, rx) = channel::<Vec<u8>>();
        struct Into(Sender<Vec<u8>>);
        impl Write for Into {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                let _ = self.0.send(b.to_vec());
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let one = screens.begin("S1", crate::ws::Lane::open(Into(tx), 4096, || {}));
        screens.asked.insert(70, (one, serde_json::json!(3)));

        // Its answer comes back under the number it asked by, with no
        // session's name on it; automation's answer is automation's
        assert!(screens.take(serde_json::json!({"id": 70, "sessionId": "S1", "result": {"ok": true}})).is_none());
        assert!(screens.take(serde_json::json!({"id": 71, "sessionId": "P", "result": {}})).is_some());
        assert_eq!(heard(&rx), [serde_json::json!({"id": 3, "result": {"ok": true}})]);
        assert!(screens.asked.is_empty());

        // What its session says by itself, and what another page says
        assert!(screens.take(serde_json::json!({"method": "DOM.documentUpdated", "sessionId": "S1", "params": {}})).is_none());
        assert!(screens.take(serde_json::json!({"method": "Page.frameNavigated", "sessionId": "P", "params": {}})).is_some());
        assert!(screens.take(serde_json::json!({"method": "Target.targetCreated", "params": {}})).is_some());
        assert_eq!(heard(&rx), [serde_json::json!({"method": "DOM.documentUpdated", "params": {}})]);

        // A frame or a worker the page hands it is its own too, and keeps
        // its name; once taken away it is nobody's
        assert!(screens.take(serde_json::json!({"method": "Target.attachedToTarget", "sessionId": "S1", "params": {"sessionId": "W1"}})).is_none());
        assert!(screens.take(serde_json::json!({"method": "Runtime.consoleAPICalled", "sessionId": "W1", "params": {}})).is_none());
        assert_eq!(heard(&rx).last(), Some(&serde_json::json!({"method": "Runtime.consoleAPICalled", "sessionId": "W1", "params": {}})));
        assert!(screens.take(serde_json::json!({"method": "Target.detachedFromTarget", "sessionId": "S1", "params": {"sessionId": "W1"}})).is_none());
        assert!(screens.take(serde_json::json!({"method": "Runtime.consoleAPICalled", "sessionId": "W1", "params": {}})).is_some());
        let _ = heard(&rx);

        // Too far behind, it is let go: nothing more is kept for it
        let much = "x".repeat(8192);
        assert!(screens.take(serde_json::json!({"method": "Runtime.consoleAPICalled", "sessionId": "S1", "params": {"text": much}})).is_none());
        assert!(screens.lines.is_empty() && screens.sessions.is_empty());
        assert!(screens.take(serde_json::json!({"method": "DOM.documentUpdated", "sessionId": "S1", "params": {}})).is_some());
    }

    /// A frame as a server writes it, turned into one a server can read
    fn masked(frame: &[u8]) -> Vec<u8> {
        let (head, len) = match frame[1] {
            126 => (4, u16::from_be_bytes([frame[2], frame[3]]) as usize),
            127 => (10, u64::from_be_bytes(frame[2..10].try_into().unwrap()) as usize),
            n => (2, n as usize),
        };
        let mut out = frame[..head].to_vec();
        out[1] |= 0x80;
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&frame[head..head + len]);
        out
    }
}

//! This PC's own resident process: the one that holds this PC's terminals
//! while the app is closed, updated, or gone in a crash (the local-keeper
//! plan).
//!
//! It is the bridge's resident process (`fardaemon`) with its terminals job
//! (`farterms`), run on this PC by this same program (`--keeper`), reached
//! through a named pipe instead of over SSH. Everything a terminal held there
//! promises -- the three answers about a terminal, one owner at a time, the
//! screen handed back as it is -- it promises here, because it is the same
//! code. This module is only the part that is this PC's: where its folder is,
//! how it is started, and the line to it.
//!
//! **Its folder is the account's own, never the install's.** The install may
//! live in a folder that is synchronised to other PCs and the cloud
//! (`C:\google` is), and the door's key must not travel with it. So the
//! folder is under `%LOCALAPPDATA%`, named by a mark of the install and the
//! account: two copies of the app on one PC each have their own.

use std::io::{BufRead as _, Read as _, Write as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::json;

use crate::farlink::Link;

/// The line to this PC's resident process is kept under this name, beside
/// the lines to other machines (which are kept under their machine keys)
pub const KEY: &str = "this-pc";

/// How long a resident process just started may take to open its door
const UP_WAIT: Duration = Duration::from_secs(15);

/// Which install and account this is: the folder's name
fn mark() -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(crate::config::root_dir().to_string_lossy().to_lowercase().as_bytes());
    h.update(b"\n");
    h.update(std::env::var("USERNAME").unwrap_or_default().to_lowercase().as_bytes());
    h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// The resident process's folder
pub fn home() -> Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or_else(|| anyhow!("LOCALAPPDATA is not set"))?;
    Ok(PathBuf::from(base).join("ShikishaTerm").join("keeper").join(mark()))
}

fn keep_door() -> Result<PathBuf> {
    Ok(home()?.join("run").join(crate::farlink::KEEP_SOCK))
}

/// The line to it, when it is up
pub fn link() -> Option<Arc<Link>> {
    crate::farlink::link_by_key(KEY)
}

/// Whether this PC's terminals are to be held by it: the person said so
/// (`keep_terminals`), and this is a system it runs on
pub fn wanted() -> bool {
    cfg!(windows) && crate::config::load().and_then(|c| c.keep_terminals).unwrap_or(false)
}

/// Make the line on a thread of its own, unless it is up or being made: a
/// tab that needs it waits for it (`farterm::open_later`), and does not hold
/// up the app while the resident process starts
pub fn connect_soon() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static MAKING: AtomicBool = AtomicBool::new(false);
    if link().is_some() || MAKING.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("this PC's resident process".into()).spawn(|| {
        if let Err(e) = connect() {
            crate::append_hook_log(&format!("this PC's resident process: no line to it ({e:#})"));
        }
        MAKING.store(false, Ordering::SeqCst);
    });
}

/// Whether a resident process is there now, without starting one
pub fn is_there() -> bool {
    keep_door().is_ok_and(|d| crate::fardaemon::probe(&d).is_some())
}

/// The line to this PC's resident process: the one there is, or one made now
/// -- starting the resident process first when nobody is at its door. Blocks
/// until it has named itself; call from a thread
pub fn connect() -> Result<Arc<Link>> {
    if let Some(l) = link() {
        return Ok(l);
    }
    let home = home()?;
    let door = keep_door()?;
    if matches!(crate::fardaemon::find(&door), crate::fardaemon::Found::Nobody) {
        start(&home)?;
    }
    let until = Instant::now() + UP_WAIT;
    while crate::fardaemon::probe(&door).is_none() {
        if Instant::now() >= until {
            bail!("this PC's resident process did not open its door");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let conn = crate::keepipe::connect(&door).context("this PC's resident process did not answer")?;
    let mut reader = std::io::BufReader::new(conn.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    if !matches!(serde_json::from_str::<crate::farlink::Frame>(first.trim()), Ok(crate::farlink::Frame::Hello { .. })) {
        bail!("this PC's resident process did not name itself");
    }
    let key = crate::fardaemon::door_key(&home)?;
    (&conn).write_all(format!("{}\n", json!({ "role": "port", "key": key })).as_bytes())?;
    // The line hears the resident process name itself first, as every line
    // to a bridge does
    let input = std::io::Cursor::new(first.into_bytes()).chain(reader);
    let shut = conn.try_clone()?;
    let closer: Box<dyn Fn() + Send + Sync> = Box::new(move || {
        let _ = shut.shutdown(std::net::Shutdown::Both);
    });
    crate::farlink::link_over(KEY, "this PC", Box::new(conn), input, None, Some(closer), home.to_string_lossy().into_owned())
}

/// Start the resident process
fn start(home: &std::path::Path) -> Result<()> {
    #[cfg(windows)]
    {
        crate::fardaemon::start_resident(home, &["--keeper".into(), home.to_string_lossy().into_owned()])
    }
    #[cfg(not(windows))]
    {
        let _ = home;
        bail!("this PC's resident process is only started on Windows")
    }
}

/// Ask this PC's resident process to end everything it holds and go
/// ("stop all and quit"). `false` when there is no line to ask on
pub fn end() -> bool {
    let Some(l) = link() else { return false };
    let ok = l.call("end_resident", json!({}), Duration::from_secs(5)).is_ok();
    crate::farlink::let_go_key(KEY);
    ok
}

/// Let go of the line as the app goes, leaving what it holds running
pub fn let_go() {
    crate::farlink::let_go_key(KEY);
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::Value;

    fn b64(v: &Value) -> Vec<u8> {
        v.as_str().and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok()).unwrap_or_default()
    }

    /// The whole road on this PC: the resident process in a folder of its own
    /// opens a terminal for the app, the app sees what the program says, lets
    /// go (as if it were closed), comes back on a new line, attaches again,
    /// and is handed the screen the program left -- and the program's end
    #[test]
    fn a_terminal_held_on_this_pc_outlives_the_line_and_comes_back_with_its_screen() {
        let home = std::env::temp_dir().join(format!("shikisha-localkeep-{}-{}", std::process::id(), crate::random_hex(4)));
        std::fs::create_dir_all(&home).unwrap();
        {
            let home = home.clone();
            std::thread::spawn(move || crate::fardaemon::daemon(home));
        }
        let door = home.join("run").join(crate::farlink::KEEP_SOCK);
        let until = Instant::now() + Duration::from_secs(10);
        while crate::fardaemon::probe(&door).is_none() {
            assert!(Instant::now() < until, "the resident process opened its door");
            std::thread::sleep(Duration::from_millis(50));
        }
        let line = |n: u32| {
            let conn = crate::keepipe::connect(&door).unwrap();
            let mut reader = std::io::BufReader::new(conn.try_clone().unwrap());
            let mut hello = String::new();
            reader.read_line(&mut hello).unwrap();
            let key = crate::fardaemon::door_key(&home).unwrap();
            (&conn).write_all(format!("{}\n", json!({ "role": "port", "key": key })).as_bytes()).unwrap();
            let _ = n;
            (conn, reader)
        };
        let say = |conn: &crate::keepipe::Conn, m: Value| {
            let f = crate::farlink::Frame::Job { job: "terms".into(), m };
            let text = format!("{}\n", serde_json::to_string(&f).unwrap());
            (&*conn).write_all(text.as_bytes()).unwrap();
        };
        let hear = |reader: &mut std::io::BufReader<crate::keepipe::Conn>, want: &str| -> Value {
            let until = Instant::now() + Duration::from_secs(20);
            loop {
                assert!(Instant::now() < until, "heard {want}");
                let mut l = String::new();
                if reader.read_line(&mut l).unwrap_or(0) == 0 {
                    panic!("the line ended before {want}");
                }
                if let Ok(crate::farlink::Frame::Job { m, .. }) = serde_json::from_str(&l)
                    && m["did"] == want
                {
                    return m;
                }
            }
        };

        // Opened, and attached; what the program prints arrives
        let (conn, mut reader) = line(1);
        say(&conn, json!({ "do": "open", "ref": 1, "tab": "t", "rows": 24, "cols": 80, "away": "always",
            "argv": ["cmd.exe", "/d", "/q", "/c", "echo held-on-this-pc&& ping -n 3 127.0.0.1 >nul"] }));
        let opened = hear(&mut reader, "opened");
        let (generation, term) = (opened["gen"].as_str().unwrap().to_string(), opened["term"].as_u64().unwrap());
        // What it printed is in the screen handed over when attaching, or in
        // the output after it: kept as the app would, in a parser of its own
        let mut screen = vt100::Parser::new(24, 80, 0);
        let until = Instant::now() + Duration::from_secs(20);
        while !screen.screen().contents().contains("held-on-this-pc") {
            assert!(Instant::now() < until, "the program's words arrived: {:?}", screen.screen().contents());
            let mut l = String::new();
            assert!(reader.read_line(&mut l).unwrap() > 0, "the line stayed up");
            if let Ok(crate::farlink::Frame::Job { m, .. }) = serde_json::from_str::<crate::farlink::Frame>(&l) {
                if m["did"] == "attached" {
                    screen.restore(vt100::Screen::from_snapshot(&b64(&m["state"])).expect("a state it can read"));
                    screen.process(&b64(&m["pending"]));
                } else if m["did"] == "out" {
                    let b = b64(&m["b"]);
                    // The pseudo console asks where the cursor is before it
                    // lets the program's output through, and the owner answers
                    // -- as a tab's own parser does
                    if b.windows(4).any(|w| w == b"\x1b[6n") {
                        let reply = base64::engine::general_purpose::STANDARD.encode(b"\x1b[1;1R");
                        say(&conn, json!({ "do": "in", "term": term, "owner": 1, "b": reply }));
                    }
                    screen.process(&b);
                }
            }
        }

        // The app goes; the program goes on
        let _ = conn.shutdown(std::net::Shutdown::Both);
        drop(reader);
        std::thread::sleep(Duration::from_millis(300));

        // Back on a new line: attached again, the screen in the state
        let (conn, mut reader) = line(2);
        say(&conn, json!({ "do": "attach", "term": term, "gen": generation, "tab": "t", "rows": 24, "cols": 80, "away": "always" }));
        let mut back = String::new();
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            assert!(Instant::now() < until, "attached again");
            let mut l = String::new();
            reader.read_line(&mut l).unwrap();
            if let Ok(crate::farlink::Frame::Job { m, .. }) = serde_json::from_str::<crate::farlink::Frame>(&l) {
                if m["did"] == "attached" {
                    let state = b64(&m["state"]);
                    back = vt100::Screen::from_snapshot(&state).map(|s| s.contents()).expect("a state it can read");
                    break;
                }
                assert_ne!(m["did"], "unknown", "the terminal was known: {m}");
            }
        }
        assert!(back.contains("held-on-this-pc"), "the screen came back: {back:?}");

        // And its end is told, with the code
        let ended = hear(&mut reader, "ended");
        assert_eq!(ended["code"], 0);
        say(&conn, json!({ "do": "end_all", "ref": 9 }));
        let _ = conn.shutdown(std::net::Shutdown::Both);
    }
}

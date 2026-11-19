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

/// The resident process's folder
pub fn home() -> Result<PathBuf> {
    let base = crate::machine_data_dir().ok_or_else(|| anyhow!("this system names no folder for a program's own things"))?;
    Ok(base.join("ShikishaTerm").join("keeper").join(crate::config::install_mark()))
}

fn keep_door() -> Result<PathBuf> {
    Ok(crate::fardaemon::run_dir(&home()?).join(crate::farlink::KEEP_SOCK))
}

/// The line to it, when it is up
pub fn link() -> Option<Arc<Link>> {
    crate::farlink::link_by_key(KEY)
}

/// What a tab held here is told in its environment so that its `shikisha`
/// command (and the AI CLIs' hooks) reach whichever app is running, through
/// the resident process -- not the app that started the tab, which may be
/// long gone: the resident process's tabs door, and the file its key is in.
/// The key is written there by the app each time it is running
/// (`give_keys`), so a command after a restart is let in with the key of the
/// app that is there now (local-keeper plan §6)
pub fn tab_env(tab: &str) -> Vec<(String, String)> {
    let Ok(home) = home() else { return Vec::new() };
    let door = crate::fardaemon::run_dir(&home).join(crate::farlink::TABS_SOCK);
    #[cfg(windows)]
    let door = crate::keepipe::pipe_name(&door);
    #[cfg(not(windows))]
    let door = door.to_string_lossy().into_owned();
    let key = home.join("keys").join(crate::farlink::key_name(tab));
    vec![
        (crate::farlink::ENV_SOCK.into(), door),
        (crate::farlink::ENV_KEY.into(), key.to_string_lossy().into_owned()),
    ]
}

/// Hand the resident process the key of each tab held here, by the tab's
/// name: given once per key for as long as the line stays up, and all again
/// on a new line (the resident process may be a new one)
pub fn give_keys(keys: Vec<(String, String)>) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static GIVEN: Mutex<Option<(usize, HashSet<String>)>> = Mutex::new(None);
    let Some(l) = link() else { return };
    let line = Arc::as_ptr(&l) as usize;
    let mut given = GIVEN.lock().unwrap_or_else(|e| e.into_inner());
    if given.as_ref().is_none_or(|(on, _)| *on != line) {
        *given = Some((line, HashSet::new()));
    }
    let Some((_, done)) = given.as_mut() else { return };
    for (tab, key) in keys {
        let mark = format!("{tab}|{key}");
        if done.contains(&mark) {
            continue;
        }
        match l.call("put_key", json!({ "tab": crate::farlink::key_name(&tab), "key": key }), Duration::from_secs(5)) {
            Ok(_) => {
                done.insert(mark);
            }
            Err(e) => crate::append_hook_log(&format!("this PC's resident process: the key of {tab} was not handed over ({e})")),
        }
    }
}

/// Whether this PC's terminals are to be held by it: the setting
/// (`keep_terminals`) says so -- unset is yes, as the person decided
/// 2026-10-02 (local-keeper plan §3) -- and this is a system it runs on
pub fn wanted() -> bool {
    if !cfg!(windows) {
        return false;
    }
    let set = crate::config::load().and_then(|c| c.keep_terminals);
    // This crate's own tests start tabs with no settings at all, to test the
    // tab; a resident process there would be a second program under test
    if cfg!(test) {
        return set == Some(true);
    }
    set.unwrap_or(true)
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
    // With the setting off, one that is there is gone back to, and none is
    // started: a terminal of a resident process that was stopped is over
    if !wanted() && !is_there() {
        MAKING.store(false, Ordering::SeqCst);
        return;
    }
    let _ = std::thread::Builder::new().name("this PC's resident process".into()).spawn(|| {
        // Asked again every couple of seconds for as long as a tab waits. The
        // same reason is written once and then every so often, with how many
        // times it has been: written each time, it was nine lines in ten of
        // the log, and pushed out of it what would have said why
        static WHY: std::sync::Mutex<(String, u32)> = std::sync::Mutex::new((String::new(), 0));
        const SAID_EVERY: u32 = 300;
        let made = connect_or(wanted());
        let mut why = WHY.lock().unwrap_or_else(|e| e.into_inner());
        match made {
            Ok(_) => *why = (String::new(), 0),
            Err(e) => {
                let e = format!("{e:#}");
                if why.0 != e {
                    *why = (e.clone(), 1);
                    crate::append_hook_log(&format!("this PC's resident process: no line to it ({e})"));
                } else {
                    why.1 += 1;
                    if why.1 % SAID_EVERY == 0 {
                        crate::append_hook_log(&format!("this PC's resident process: still no line to it after {} tries ({e})", why.1));
                    }
                }
            }
        }
        drop(why);
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
    connect_or(true)
}

/// The line to the resident process that is there, never starting one
fn connect_existing() -> Result<Arc<Link>> {
    connect_or(false)
}

fn connect_or(starting: bool) -> Result<Arc<Link>> {
    if let Some(l) = link() {
        return Ok(l);
    }
    let home = home()?;
    let door = keep_door()?;
    // Looked for each time round: one that is there but on its way out names
    // itself to nobody, and the one to speak to is the one started once it
    // has gone
    let until = Instant::now() + UP_WAIT;
    let mut started: Option<Instant> = None;
    loop {
        match crate::fardaemon::find(&door) {
            crate::fardaemon::Found::Named(_) => break,
            crate::fardaemon::Found::Nobody if !starting => bail!("this PC's resident process is not there"),
            crate::fardaemon::Found::Nobody if started.is_none_or(|at| at.elapsed() >= crate::fardaemon::START_AGAIN) => {
                start(&home)?;
                ENDING.store(NOT_ASKED, std::sync::atomic::Ordering::SeqCst);
                started = Some(Instant::now());
            }
            _ => {}
        }
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
    let key = match crate::fardaemon::door_key(&home) {
        Ok(key) => key,
        // One that answers and whose key is gone has outlived its folder
        // (`end_outlived`). Ended, the next try starts a new one
        Err(e) if starting && outlived(&home) => {
            drop(reader);
            drop(conn);
            // It answered before now: one started from now on is another
            let answered = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            end_outlived(&home, &door, answered).with_context(|| format!("{e:#}"))?;
            return connect_or(starting);
        }
        Err(e) => return Err(e),
    };
    (&conn).write_all(format!("{}\n", json!({ "role": "port", "key": key })).as_bytes())?;
    // The line hears the resident process name itself first, as every line
    // to a bridge does
    let input = std::io::Cursor::new(first.into_bytes()).chain(reader);
    let shut = conn.try_clone()?;
    let closer: Box<dyn Fn() + Send + Sync> = Box::new(move || {
        let _ = shut.shutdown(std::net::Shutdown::Both);
    });
    let link = crate::farlink::link_over(KEY, "this PC", Box::new(conn), input, None, Some(closer), home.to_string_lossy().into_owned())?;
    // A resident process started by an older version of the app keeps
    // running across an update -- that is what it is for -- and holds what it
    // holds the way that version did. Said, with what makes it new
    // (local-keeper plan §8)
    let theirs = link.version.lock().ok().and_then(|v| v.clone()).unwrap_or_default();
    if !theirs.is_empty() && theirs != env!("CARGO_PKG_VERSION") {
        crate::caps::tell(crate::i18n::tp("msg.localkeep.older", &[("theirs", &theirs), ("ours", env!("CARGO_PKG_VERSION"))]));
    }
    Ok(link)
}

/// How many resident processes this run may end for having outlived their
/// folder: one is the case this is for, and a second is allowed for a
/// machine where it happens again; past that something else is wrong, and
/// ending more would only hide it
const OUTLIVED_MOST: u32 = 2;

/// Whether the resident process that answers has outlived its folder: its
/// key is not where it wrote it, and still is not a few seconds on.
///
/// The key is written the moment a resident process opens its door, and
/// the folder it is in is the app's own data -- which goes when the data is
/// cleared, or the app is taken off and put on again. A resident process
/// that was running then runs on, with its door open, and nobody can ever
/// show it its key again: every tab waits for a line that cannot be made,
/// and the programs it holds run where nobody sees them
fn outlived(home: &std::path::Path) -> bool {
    let key = crate::fardaemon::run_dir(home).join("keep.key");
    for _ in 0..6 {
        if key.exists() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    true
}

/// End a resident process that has outlived its folder, with everything it
/// holds, so that a new one can be started.
///
/// It cannot be asked to end: asking takes the key. So it is found among
/// the processes -- a copy of this very program, started to keep this very
/// folder, before `answered` (the moment it was found answering) -- and
/// ended with what it started. What it held could not be reached by
/// anybody any more; said in the log and to the person all the same, since
/// programs of theirs ended with it.
///
/// Only one app runs from a folder at a time in a session
/// (`crate::instance`), and only the app makes this line, on one thread
/// (`connect_soon`): there is no second app still speaking to the resident
/// process that is ended, and no second thread ending the one started in
/// its place. A resident process of another session -- the same person
/// logged in twice -- is left alone: the app there may be speaking to it. One at a time all
/// the same, and the key looked for once more first: a resident process
/// that has its key by now is not the one this is for
#[cfg(windows)]
fn end_outlived(home: &std::path::Path, door: &std::path::Path, answered: u64) -> Result<()> {
    use std::os::windows::process::CommandExt as _;
    use std::sync::atomic::{AtomicU32, Ordering};
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    if crate::fardaemon::run_dir(home).join("keep.key").exists() {
        return Ok(());
    }
    static ENDED: AtomicU32 = AtomicU32::new(0);
    if ENDED.fetch_add(1, Ordering::SeqCst) >= OUTLIVED_MOST {
        bail!("this PC's resident process has outlived its folder, and too many were ended for that already");
    }
    let me = std::env::current_exe()?;
    let kept = home.to_string_lossy().to_lowercase();
    let mut all = System::new();
    all.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_exe(UpdateKind::Always).with_cmd(UpdateKind::Always));
    let mine: Vec<u32> = all
        .processes()
        .values()
        .filter(|p| p.pid().as_u32() != std::process::id())
        // Running since before it was found answering: not one started since
        .filter(|p| p.start_time() <= answered)
        // ...and in this session: where one person is logged in more than
        // once, the app of another session may still be speaking to its own
        .filter(|p| session_of(p.pid().as_u32()).is_some_and(|s| Some(s) == session_of(std::process::id())))
        .filter(|p| p.exe().is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(&me.to_string_lossy())))
        .filter(|p| {
            let said: Vec<String> = p.cmd().iter().map(|a| a.to_string_lossy().to_lowercase()).collect();
            said.iter().any(|a| a == "--keeper") && said.iter().any(|a| *a == kept)
        })
        .map(|p| p.pid().as_u32())
        .collect();
    if mine.is_empty() {
        bail!("this PC's resident process has outlived its folder, and was not found among the processes to be ended");
    }
    for pid in &mine {
        // With everything it started: the terminals it holds are its children
        let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).creation_flags(0x0800_0000).output();
    }
    let until = Instant::now() + Duration::from_secs(8);
    while !matches!(crate::fardaemon::find(door), crate::fardaemon::Found::Nobody) {
        if Instant::now() >= until {
            bail!("this PC's resident process has outlived its folder, and did not end when it was ended");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    crate::append_hook_log(&format!("this PC's resident process: {} that outlived its folder ended, with what it held; a new one is started", mine.len()));
    crate::caps::tell(crate::i18n::t("msg.localkeep.outlived"));
    Ok(())
}

/// The session a process runs in: each time a person is logged in is one
#[cfg(windows)]
fn session_of(pid: u32) -> Option<u32> {
    let mut session = 0u32;
    // SAFETY: the number is ours to be written into
    (unsafe { windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

#[cfg(not(windows))]
fn end_outlived(_home: &std::path::Path, _door: &std::path::Path, _answered: u64) -> Result<()> {
    bail!("this PC's resident process is only kept on Windows")
}

/// Start the resident process
fn start(home: &std::path::Path) -> Result<()> {
    // Through a go-between that starts it and leaves at once: the resident
    // process is then nobody's child, so ending the app's whole process tree
    // (as Task Manager's "End process tree" does) does not reach it
    #[cfg(windows)]
    {
        crate::fardaemon::start_resident(home, &["--keeper-launch".into(), home.to_string_lossy().into_owned()])
    }
    #[cfg(not(windows))]
    {
        let _ = home;
        bail!("this PC's resident process is only started on Windows")
    }
}

/// How long the resident process is given to say it heard "end"
const END_WAIT: Duration = Duration::from_secs(5);

/// Ask this PC's resident process to end everything it holds and go
/// ("stop all and quit"). An error when there is no line to ask on, or the
/// asking did not get its answer: then nothing is taken to have ended, and
/// the line is kept -- the terminals there may well still run
pub fn end() -> Result<()> {
    use std::sync::atomic::Ordering::SeqCst;
    let Some(l) = link() else { bail!("there is no line to this PC's resident process") };
    // Asked before the call goes: the resident process answers and goes at
    // once, and a tab that sees its line go in that moment must wait to hear
    // how the asking ended (`ended_on_purpose`), not take the line's going
    // for the answer
    ENDING.store(ASKED, SeqCst);
    match l.call("end_resident", json!({}), END_WAIT) {
        Ok(_) => {
            ENDING.store(ENDED, SeqCst);
            crate::farlink::let_go_key(KEY);
            Ok(())
        }
        Err(e) => {
            ENDING.store(NOT_ASKED, SeqCst);
            bail!("this PC's resident process did not answer \"end\": {e}")
        }
    }
}

/// Where asking the resident process to end everything it holds stands:
/// not asked (or a new one started since), asked and not yet answered, or
/// answered -- its terminals are over, and a tab whose line to it went does
/// not wait for the line to come back
static ENDING: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(NOT_ASKED);
const NOT_ASKED: u8 = 0;
const ASKED: u8 = 1;
const ENDED: u8 = 2;

/// Whether the resident process on `l` is another version's than this app:
/// one started by the version before an update, which runs on across it --
/// that is what it is for -- with that version's code. Not known before it
/// has named itself; one that names no version is older than any that does
pub fn of_another_version(l: &Link) -> bool {
    l.version.lock().ok().and_then(|v| v.clone()).is_some_and(|v| v != env!("CARGO_PKG_VERSION"))
}

/// Open a new terminal on this PC's resident process with `open`, the
/// resident process first replaced by one of this version when it is
/// another version's and holds nothing any more.
///
/// A resident process of another version holds the terminals that ran
/// across the update, and must hold them for as long as they run: replaced
/// only once there are none, and asked at the moment a new terminal is to
/// be opened -- the one moment that decides which version that terminal
/// runs under. Until then the new terminal is opened on the old one, as
/// before. Where how much it holds cannot be told, it is kept: ending
/// terminals nobody counted is the one thing this must never do.
///
/// One at a time, and `open` under the same turn while the old one stays:
/// a terminal being opened on it by another tab is not in its list until it
/// is open, and must not be ended under that tab by a replacement that did
/// not see it. With no line at all, too, the turn is waited for: that is
/// also what the line is while one is being replaced, and an open let
/// through then finds no line and takes its tab for ended
pub fn opening_new<R>(open: impl FnOnce() -> R) -> R {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let line = || match link() {
        Some(l) if !of_another_version(&l) => Line::Current,
        Some(_) => Line::Other,
        None => Line::None,
    };
    in_turn(&TURN, line, replace_if_drained, open)
}

/// What the line to this PC's resident process is, as a new terminal is
/// about to be opened
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    /// Up, to one of this version
    Current,
    /// Up, to one of another version
    Other,
    /// Not up: none there yet, or the one there being replaced
    None,
}

/// The order of `opening_new`, apart from what it opens and replaces: open
/// at once on one of this version; otherwise in the turn, the one there
/// `settle`d first if it is still another version's once the turn is had
fn in_turn<R>(turn: &std::sync::Mutex<()>, line: impl Fn() -> Line, settle: impl FnOnce(), open: impl FnOnce() -> R) -> R {
    if line() == Line::Current {
        return open();
    }
    let _turn = turn.lock().unwrap_or_else(|e| e.into_inner());
    // Looked at again in the turn: the tab before may have replaced it
    if line() == Line::Other {
        settle();
    }
    open()
}

/// The resident process of another version on the line now, replaced when it
/// holds nothing still wanted (`opening_new`); kept, with why, otherwise
fn replace_if_drained() {
    if let Some(l) = link().filter(|l| of_another_version(l)) {
        let theirs = l.version.lock().ok().and_then(|v| v.clone()).unwrap_or_default();
        match drained() {
            Ok(true) if wanted() => match replace(&l) {
                Ok(()) => crate::append_hook_log(&format!(
                    "this PC's resident process: the one of version {theirs} held nothing any more; replaced by one of {}",
                    env!("CARGO_PKG_VERSION")
                )),
                Err(e) => crate::append_hook_log(&format!("this PC's resident process: the one of version {theirs} was not replaced: {e:#}")),
            },
            // With the setting off no new one is to be started: the old one
            // is used for what it is still used for
            Ok(true) => {}
            Ok(false) => crate::append_hook_log(&format!(
                "this PC's resident process: the one of version {theirs} still holds terminals; the new one is opened on it"
            )),
            Err(e) => crate::append_hook_log(&format!(
                "this PC's resident process: the one of version {theirs} is kept, since what it holds could not be told ({e:#})"
            )),
        }
    }
}

/// A terminal opened by another tab while the one of another version is
/// being replaced -- the line is down then -- waits for the replacement, and
/// is opened on the new one; never let through to find no line
#[cfg(test)]
#[test]
fn an_open_while_the_resident_process_is_replaced_waits_for_the_new_one() {
    use std::sync::atomic::{AtomicU8, Ordering::SeqCst};
    use std::sync::{Arc, Mutex};
    const OTHER: u8 = 0;
    const DOWN: u8 = 1;
    const CURRENT: u8 = 2;
    let turn = Arc::new(Mutex::new(()));
    let state = Arc::new(AtomicU8::new(OTHER));
    let line_of = |state: Arc<AtomicU8>| {
        move || match state.load(SeqCst) {
            OTHER => Line::Other,
            DOWN => Line::None,
            _ => Line::Current,
        }
    };
    let (down_tx, down_rx) = std::sync::mpsc::channel();
    let replacing = {
        let (turn, state) = (Arc::clone(&turn), Arc::clone(&state));
        std::thread::spawn(move || {
            let settle = || {
                // Let go of the old one: the line is down until the new one is up
                state.store(DOWN, SeqCst);
                down_tx.send(()).unwrap();
                std::thread::sleep(Duration::from_millis(300));
                state.store(CURRENT, SeqCst);
            };
            in_turn(&turn, line_of(Arc::clone(&state)), settle, || state.load(SeqCst))
        })
    };
    down_rx.recv().unwrap();
    // Another tab opens in the middle of it
    let other = in_turn(&turn, line_of(Arc::clone(&state)), || panic!("settled twice"), || state.load(SeqCst));
    assert_eq!(other, CURRENT, "opened only once the new one was up");
    assert_eq!(replacing.join().unwrap(), CURRENT);
    // Once it is the current version's, nothing waits and nothing is settled
    assert_eq!(in_turn(&turn, line_of(state), || panic!("settled a current one"), || 7), 7);
}

/// Whether this PC's resident process holds nothing that is still wanted: no
/// terminal still running, and none ended whose end a tab of this app has
/// still to come back for (it is written down for it). An error when it did
/// not say what it holds
fn drained() -> Result<bool> {
    let held = crate::farterm::list_held(&crate::farterm::Place::Here).ok_or_else(|| anyhow!("it did not say what it holds"))?;
    drained_by(&held, crate::farterm::owed_here).ok_or_else(|| anyhow!("what it holds, or what this app's tabs are owed of it, could not be read"))
}

/// The same, read from what it said it holds (`held`), with `owed` saying
/// whether a tab will come back for an ended terminal (None: not known).
/// None when what it said is not a list of terminals, or what is owed could
/// not be told
fn drained_by(held: &serde_json::Value, owed: impl Fn(&str, u64) -> Option<bool>) -> Option<bool> {
    let generation = held["gen"].as_str().unwrap_or_default();
    for t in held["terms"].as_array()? {
        // A terminal not said to have ended is taken for running, and one
        // with no number for one a tab comes back for
        if t["ended"].as_bool() != Some(true) {
            return Some(false);
        }
        let Some(term) = t["term"].as_u64() else { return Some(false) };
        if owed(generation, term)? {
            return Some(false);
        }
    }
    Some(true)
}

#[cfg(test)]
#[test]
fn an_older_resident_process_is_replaced_only_once_it_holds_nothing_still_wanted() {
    let none = |_: &str, _: u64| Some(false);
    let said = |terms: serde_json::Value| json!({ "did": "list", "gen": "g1", "terms": terms });
    // Nothing held
    assert_eq!(drained_by(&said(json!([])), none), Some(true));
    // One still running, and one that says nothing of having ended
    assert_eq!(drained_by(&said(json!([{ "term": 1, "ended": false }])), none), Some(false));
    assert_eq!(drained_by(&said(json!([{ "term": 1 }])), none), Some(false));
    // Ended, and no tab of this app to come back for it
    assert_eq!(drained_by(&said(json!([{ "term": 1, "ended": true }])), none), Some(true));
    // Ended, and written down for a tab that has not taken its end yet
    let owed = |generation: &str, term: u64| Some(generation == "g1" && term == 1);
    assert_eq!(drained_by(&said(json!([{ "term": 1, "ended": true }, { "term": 2, "ended": true }])), owed), Some(false));
    assert_eq!(drained_by(&said(json!([{ "term": 2, "ended": true }])), owed), Some(true));
    // Ended, and the note of what is owed could not be read: not known, so kept
    assert_eq!(drained_by(&said(json!([{ "term": 2, "ended": true }])), |_: &str, _: u64| None), None);
    // ...but with nothing ended to ask about, the note is not needed
    assert_eq!(drained_by(&said(json!([])), |_: &str, _: u64| None), Some(true));
    // What it said is not a list: not known, so kept
    assert_eq!(drained_by(&json!({ "did": "list" }), none), None);
}

/// End the resident process on `l`, which holds nothing any more, and start
/// one of this version in its place. Not `end`: that is the person's "stop
/// them", and a tab waiting for its line in the meantime would take it for
/// its own terminal ended (`ended_on_purpose`)
fn replace(l: &Link) -> Result<()> {
    l.call("end_resident", json!({}), END_WAIT).map_err(|e| anyhow!("it did not answer \"end\": {e}"))?;
    crate::farlink::let_go_key(KEY);
    let door = keep_door()?;
    let until = Instant::now() + Duration::from_secs(8);
    while !matches!(crate::fardaemon::find(&door), crate::fardaemon::Found::Nobody) {
        if Instant::now() >= until {
            bail!("it did not go when it was asked to");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    connect().map(|_| ())
}

/// Whether the resident process was ended on purpose (`end`) and none started
/// since. While the asking is still out, waits to hear how it ended: a line
/// that went then is the answer's doing only if the answer was "ending"
pub fn ended_on_purpose() -> bool {
    use std::sync::atomic::Ordering::SeqCst;
    let until = Instant::now() + END_WAIT + Duration::from_secs(1);
    while ENDING.load(SeqCst) == ASKED && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    ENDING.load(SeqCst) == ENDED
}

/// Stop this PC's resident process as the app quits with everything to stop:
/// told to end what it holds and go, rather than keep the ended terminals'
/// codes for a start that is not coming. Nothing to do when none is there;
/// an error, with why, when one is there and did not say it ended
pub fn end_on_quit() -> Result<()> {
    if link().is_none() && !is_there() {
        return Ok(());
    }
    connect_existing()?;
    end()
}

/// Let go of the line as the app goes, leaving what it holds running
pub fn let_go() {
    crate::farlink::let_go_key(KEY);
}

/// How many terminals the resident process holds that are still running,
/// asked of it when it is there. Never starts one: a count is not a reason
/// to have a resident process. None there is none running; one there that
/// could not be reached or did not answer is an error, not a 0 -- a 0 hides
/// the terminals still running and the one press that stops them. Blocks
/// for an answer; call off the loop
pub fn held_count() -> Result<usize> {
    match crate::fardaemon::find(&keep_door()?) {
        crate::fardaemon::Found::Nobody => return Ok(0),
        crate::fardaemon::Found::Unreachable(why) => {
            bail!("this PC's resident process could not be reached: {why}")
        }
        _ => {}
    }
    connect_existing()?;
    crate::farterm::list_held(&crate::farterm::Place::Here)
        .and_then(|m| m["terms"].as_array().map(|t| t.iter().filter(|t| t["ended"] == false).count()))
        .ok_or_else(|| anyhow!("this PC's resident process did not say what it holds"))
}

/// Stop every terminal the resident process holds, and the resident process
/// with them: the person turned the setting off and asked for what it still
/// holds to end (the button beside the setting). An error, with why, when
/// there was no resident process to ask or it did not answer
pub fn stop_all() -> Result<()> {
    connect_existing()?;
    crate::append_hook_log("this PC's resident process: asked to stop everything it holds, from the settings");
    let ended = end();
    if let Err(e) = &ended {
        crate::append_hook_log(&format!("this PC's resident process: could not be stopped: {e:#}"));
    }
    ended
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use base64::Engine as _;
    use serde_json::Value;

    fn b64(v: &Value) -> Vec<u8> {
        v.as_str().and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok()).unwrap_or_default()
    }

    /// A line that went while "end" was out is the asking's doing only once
    /// it was answered: unanswered, the terminals are not taken to be over
    /// (they may still run, and the tab waits for its line instead)
    #[test]
    fn a_line_gone_while_end_is_asked_waits_for_the_answer() {
        use std::sync::atomic::Ordering::SeqCst;
        for (answer, over) in [(ENDED, true), (NOT_ASKED, false)] {
            ENDING.store(ASKED, SeqCst);
            let said = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                ENDING.store(answer, SeqCst);
            });
            let asked_at = Instant::now();
            assert_eq!(ended_on_purpose(), over);
            assert!(asked_at.elapsed() >= Duration::from_millis(150), "it did not wait for the answer");
            said.join().unwrap();
        }
        ENDING.store(NOT_ASKED, SeqCst);
        assert!(end().is_err(), "with no line, nothing was asked and nothing is ended");
        assert!(!ended_on_purpose());
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
        let door = crate::fardaemon::own_run(&home).join(crate::farlink::KEEP_SOCK);
        let until = Instant::now() + Duration::from_secs(10);
        while crate::fardaemon::probe(&door).is_none() {
            assert!(Instant::now() < until, "the resident process opened its door");
            std::thread::sleep(Duration::from_millis(50));
        }
        let line = |n: u32| {
            let conn = crate::keepipe::connect(&door).unwrap();
            // A read that waits for good is a test that never ends (a release
            // run sat on one for over an hour): a minute is the most any one
            // message here can take, and then the test fails saying which
            let reading = conn.try_clone().unwrap();
            reading.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
            let mut reader = std::io::BufReader::new(reading);
            let mut hello = String::new();
            reader.read_line(&mut hello).unwrap();
            let key = crate::fardaemon::door_key(&home).unwrap();
            (&conn).write_all(format!("{}\n", json!({ "role": "port", "key": key })).as_bytes()).unwrap();
            let _ = n;
            // Say it is there, as the app does: a line silent for a minute is
            // dropped, and on a busy machine this test waits longer than that
            let ticks = conn.try_clone().unwrap();
            std::thread::spawn(move || {
                let tick = format!("{}\n", serde_json::to_string(&crate::farlink::Frame::Tick).unwrap());
                // Ends once the line is shut from this side
                while (&ticks).write_all(tick.as_bytes()).is_ok() {
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
            (conn, reader)
        };
        let say = |conn: &crate::keepipe::Conn, m: Value| {
            let f = crate::farlink::Frame::Job { job: "terms".into(), m };
            let text = format!("{}\n", serde_json::to_string(&f).unwrap());
            (&*conn).write_all(text.as_bytes()).unwrap();
        };
        // Who owns each terminal on this line, as its "attached" said: the
        // owner answers its terminal's questions, under that name
        let owners = std::cell::RefCell::new(std::collections::HashMap::<u64, u64>::new());
        let hear = |reader: &mut std::io::BufReader<crate::keepipe::Conn>, want: &str| -> Value {
            let until = Instant::now() + Duration::from_secs(20);
            loop {
                assert!(Instant::now() < until, "heard {want}");
                let mut l = String::new();
                match reader.read_line(&mut l) {
                    Ok(0) => panic!("the line ended before {want}"),
                    Ok(_) => {}
                    Err(e) => panic!("nothing came for a minute while waiting for {want}: {e}"),
                }
                let Ok(crate::farlink::Frame::Job { m, .. }) = serde_json::from_str::<crate::farlink::Frame>(&l) else { continue };
                if let (Some(term), Some(owner)) = (m["term"].as_u64(), m["owner"].as_u64())
                    && m["did"] == "attached"
                {
                    owners.borrow_mut().insert(term, owner);
                }
                // A pseudo console asks where the cursor is and holds the
                // program back until it is told -- PowerShell does not start
                // what it was given until then. Answered as a tab's parser
                // does, whatever is being waited for here
                if m["did"] == "out"
                    && let Some(term) = m["term"].as_u64()
                    && b64(&m["b"]).windows(4).any(|w| w == b"\x1b[6n")
                {
                    let owner = owners.borrow().get(&term).copied().unwrap_or(1);
                    let reply = base64::engine::general_purpose::STANDARD.encode(b"\x1b[1;1R");
                    say(reader.get_ref(), json!({ "do": "in", "term": term, "owner": owner, "b": reply }));
                }
                if m["did"] == want {
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
        say(&conn, json!({ "do": "attach", "ref": 11, "term": term, "gen": generation, "tab": "t", "rows": 24, "cols": 80, "away": "always" }));
        let until = Instant::now() + Duration::from_secs(20);
        let back = loop {
            assert!(Instant::now() < until, "attached again");
            let mut l = String::new();
            reader.read_line(&mut l).unwrap();
            if let Ok(crate::farlink::Frame::Job { m, .. }) = serde_json::from_str::<crate::farlink::Frame>(&l) {
                if m["did"] == "attached" {
                    assert_eq!(m["gen"], generation);
                    assert_eq!(m["ref"], 11, "attach acknowledgement is addressed to its request");
                    let state = b64(&m["state"]);
                    break vt100::Screen::from_snapshot(&state).map(|s| s.contents()).expect("a state it can read");
                }
                assert_ne!(m["did"], "unknown", "the terminal was known: {m}");
            }
        };
        assert!(back.contains("held-on-this-pc"), "the screen came back: {back:?}");

        // And its end is told, with the code
        let ended = hear(&mut reader, "ended");
        assert_eq!(ended["code"], 0);
        assert_eq!(ended["gen"], generation);
        // A stale acknowledgement of another lifetime cannot erase this
        // terminal's exit record, even when its number is the same.
        say(&conn, json!({ "do": "forget", "term": term, "gen": "previous" }));
        say(&conn, json!({ "do": "attach", "ref": 12, "term": term, "gen": generation, "tab": "t" }));
        let over = hear(&mut reader, "over");
        assert_eq!(over["gen"], generation);
        assert_eq!(over["ref"], 12);

        // Which processes a held terminal's job has is told to its owner:
        // what the app counts as the tab's work in the background
        // Start the nested command after the terminal has been put in its
        // job. This tests stopping a held job, not launcher startup timing.
        let long = ["powershell.exe", "-NoLogo", "-NoProfile", "-Command", "Start-Sleep -Milliseconds 500; ping -n 30 127.0.0.1 | Out-Null"];
        say(&conn, json!({ "do": "open", "ref": 5, "tab": "p", "cwd": "", "rows": 24, "cols": 80, "away": "always", "reuse": true, "argv": long }));
        let id = hear(&mut reader, "opened")["term"].clone();
        let procs = loop {
            let p = hear(&mut reader, "procs");
            if p["term"] == id && p["pids"].as_array().is_some_and(|a| a.len() >= 2) {
                break p;
            }
        };
        let root = procs["root"].as_u64().expect("its first process");
        assert!(procs["pids"].as_array().is_some_and(|p| p.iter().any(|v| v.as_u64() == Some(root))), "its own process among them: {procs}");

        // Asked to open again for a tab whose terminal runs -- the app lost
        // its note of it -- the one running is handed back, not a second
        let long = ["cmd.exe", "/d", "/q", "/c", "ping -n 30 127.0.0.1 >nul"];
        say(&conn, json!({ "do": "open", "ref": 2, "tab": "u", "cwd": "", "rows": 24, "cols": 80, "away": "always", "reuse": true, "argv": long }));
        let one = hear(&mut reader, "opened");
        say(&conn, json!({ "do": "open", "ref": 3, "tab": "u", "cwd": "", "rows": 24, "cols": 80, "away": "always", "reuse": true, "argv": long }));
        let again = hear(&mut reader, "opened");
        assert_eq!(again["term"], one["term"], "the running one, not a second");
        assert_eq!(again["again"], true);
        // Told to stop (a restart), it is not handed out again: a new one opens
        let owner = hear(&mut reader, "attached")["owner"].clone();
        // The line, terminal number and owner number may all be reused
        // after a restart. A command from the old lifetime is refused.
        say(&conn, json!({ "do": "stop", "ref": 13, "term": again["term"], "gen": "previous", "owner": owner }));
        let refused = hear(&mut reader, "refused");
        assert_eq!(refused["gen"], "previous");
        assert_eq!(refused["ref"], 13);
        say(&conn, json!({ "do": "open", "ref": 14, "tab": "u", "cwd": "", "rows": 24, "cols": 80, "away": "always", "reuse": true, "argv": long }));
        assert_eq!(hear(&mut reader, "opened")["term"], one["term"], "the stale stop ended the current program");
        let owner = hear(&mut reader, "attached")["owner"].clone();
        say(&conn, json!({ "do": "stop", "term": again["term"], "owner": owner }));
        say(&conn, json!({ "do": "open", "ref": 4, "tab": "u", "cwd": "", "rows": 24, "cols": 80, "away": "always", "reuse": true, "argv": long }));
        let fresh = hear(&mut reader, "opened");
        assert_ne!(fresh["term"], one["term"], "a stopped one is not handed back");
        say(&conn, json!({ "do": "end_all", "ref": 9 }));
        // The terminal is still retained for its exit to be acknowledged.
        // Its launcher AND children must already release their resources.
        let until = Instant::now() + Duration::from_secs(5);
        while procs["pids"].as_array().unwrap().iter().any(|p| crate::guest::image_of(p.as_u64().unwrap() as u32).is_some()) {
            assert!(Instant::now() < until, "a closed terminal left a child running: {procs}");
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = conn.shutdown(std::net::Shutdown::Both);
    }
}

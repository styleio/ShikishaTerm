//! Noticing that the app has stopped responding, and keeping what a crash
//! leaves behind.
//!
//! Two parts of the app have to keep moving for it to answer: the loop that
//! runs the tabs (`runtime::run`, a pass every few milliseconds) and the
//! window's own thread, which draws and takes the keys. Each says it is alive
//! ([`beat`]); a thread of this module's own looks at when each last did. One
//! that has not for [`HUNG_AFTER`] has stopped: that is written down -- in the
//! crash log, where the next start's account of a run that ended badly reads
//! it, and on Windows as a dump of every thread's stack, which shows where it
//! stopped. Nothing is stopped or killed: a part that is only slow comes back,
//! and is written down as having come back, with how long it was gone.
//!
//! Time is counted only while the machine is awake, so a lid closed over a
//! running app is not a hang; and a part waiting on purpose for a person to
//! answer a dialog of the system's own ([`person_answers`]) is not one either.
//!
//! A native crash on Windows (one Rust's own panic does not catch: an access
//! violation in a library the window uses) is written as a dump too, before
//! Windows' own error reporting takes it.
//!
//! The dumps hold the memory of the program's threads. They stay on this PC
//! -- in its own local application data, never beside the program, which may
//! be in a folder a cloud drive copies elsewhere -- a bounded number of them,
//! and nothing sends them anywhere; the person can hand one over if they
//! choose to.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// How long a part may go without saying it is alive before it is taken to
/// have stopped. Longer than anything the loop does on purpose -- its slowest
/// work is put on threads of its own -- and short enough that a person still
/// looking at the frozen window learns why once it comes back
pub const HUNG_AFTER: Duration = Duration::from_secs(20);

/// How often the watching thread looks, and asks the window to answer
const LOOK_EVERY: Duration = Duration::from_secs(2);

/// How many dumps are kept, and how much room they may take together: the
/// newest are kept, so a crash that repeats at every start cannot fill a disk
const DUMPS_KEPT: usize = 8;
const DUMPS_ROOM: u64 = 200 * 1024 * 1024;

/// A dump still being written is named so; one left so by a run that ended
/// halfway through it is given up once it is this old
const UNFINISHED: &str = "part";
const UNFINISHED_GIVEN_UP: Duration = Duration::from_secs(10 * 60);

/// The parts that have to keep moving
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The loop that runs the tabs
    Work,
    /// The window's own thread
    Window,
}

const PARTS: [Part; 2] = [Part::Work, Part::Window];

impl Part {
    fn name(self) -> &'static str {
        match self {
            Part::Work => "work",
            Part::Window => "window",
        }
    }

    fn index(self) -> usize {
        match self {
            Part::Work => 0,
            Part::Window => 1,
        }
    }
}

/// When each part last said it is alive, in milliseconds of the machine
/// being awake. Zero: it has not been watched yet
static LAST: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)];
/// How many dialogs each part is waiting on a person to answer
static HELD: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

std::thread_local! {
    /// Which part runs on this thread, once it has said so
    static MINE: std::cell::Cell<Option<Part>> = const { std::cell::Cell::new(None) };
}

/// Milliseconds the machine has been awake, from 1 so that zero keeps
/// meaning "not yet". Windows' interrupt time without the time asleep; the
/// monotonic clocks elsewhere already leave sleep out
fn awake_ms() -> u64 {
    #[cfg(windows)]
    {
        let mut t = 0u64;
        // SAFETY: writes one number
        unsafe { windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime(&mut t) };
        t / 10_000 + 1
    }
    #[cfg(not(windows))]
    {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
    }
}

/// A part says it is alive, from its own thread
pub fn beat(part: Part) {
    MINE.with(|m| m.set(Some(part)));
    LAST[part.index()].store(awake_ms(), Ordering::SeqCst);
}

/// Start watching a part from now on, from any thread: one that does not
/// answer from here on has stopped, even if it never answered at all
fn arm(part: Part) {
    LAST[part.index()].store(awake_ms(), Ordering::SeqCst);
}

/// Wait for a person to answer a dialog of the system's own: the part on
/// this thread, if it is one that is watched, is not taken to have stopped
/// while they think
pub fn person_answers<R>(ask: impl FnOnce() -> R) -> R {
    struct Held(Part);
    impl Drop for Held {
        fn drop(&mut self) {
            beat(self.0);
            HELD[self.0.index()].fetch_sub(1, Ordering::SeqCst);
        }
    }
    let _held = MINE.with(|m| m.get()).map(|part| {
        HELD[part.index()].fetch_add(1, Ordering::SeqCst);
        Held(part)
    });
    ask()
}

/// Where the account of the parts that stopped is kept, as JSON
fn hang_file() -> PathBuf {
    crate::config::logs_dir().join("hang.json")
}

/// Where dumps are written: this machine's own application data, apart for
/// each copy of the program, so that tidying one copy's never takes
/// another's
pub fn dumps_dir() -> PathBuf {
    let root = crate::config::root_dir().display().to_string().to_lowercase();
    // FNV-1a: a short name that stays the same for the same folder
    let mut h: u32 = 0x811c_9dc5;
    for b in root.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    crate::config::machine_dir().join("dumps").join(format!("{h:08x}"))
}

/// A part that stopped, as written down
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Hang {
    pub pid: u32,
    pub part: String,
    /// When it was last heard from, in seconds since 1970
    pub since: u64,
    /// How long it had gone unheard when it was last looked at, in seconds
    pub secs: u64,
    /// Whether it came back
    pub back: bool,
    /// The dump written when it was found, once it is written
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dump: String,
}

/// The last time each part stopped, as the hang file says
pub fn last_hangs() -> Vec<Hang> {
    std::fs::read_to_string(hang_file()).map(|text| read_hangs(&text)).unwrap_or_default()
}

/// The hang file's accounts: a list, one for each part; or, as the first
/// version wrote it, one account alone
fn read_hangs(text: &str) -> Vec<Hang> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Written {
        Each(Vec<Hang>),
        One(Hang),
    }
    match serde_json::from_str(text) {
        Ok(Written::Each(all)) => all,
        Ok(Written::One(h)) => vec![h],
        Err(_) => Vec::new(),
    }
}

/// The part of a run that was still stopped when it was last looked at --
/// the longest stopped, if both were
pub fn still_stopped(hangs: &[Hang], pid: u32, started: u64) -> Option<&Hang> {
    hangs.iter().filter(|h| h.pid == pid && !h.back && h.since + 1 >= started).max_by_key(|h| h.secs)
}

/// Both parts' accounts, written together: one part coming back never
/// writes over the other still being stopped
fn write_hangs(open: &[Option<Hang>; 2]) {
    let all: Vec<&Hang> = open.iter().flatten().collect();
    if let Ok(text) = serde_json::to_string_pretty(&all) {
        let _ = crate::crypto::write_atomic(&hang_file(), &text);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn note(what: &str) {
    crate::append_hook_log(&format!("watchdog: {what}"));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(crate::config::logs_dir().join("crash.log")) {
        use std::io::Write as _;
        let _ = writeln!(f, "{}", crate::lastexit::crash_entry(what));
    }
}

/// A part that stopped and came back, for the screen to say once: which,
/// and for how many seconds
static CAME_BACK: std::sync::Mutex<Option<(Part, u64)>> = std::sync::Mutex::new(None);

/// What came back since this was last asked
pub fn take_came_back() -> Option<(Part, u64)> {
    CAME_BACK.lock().unwrap_or_else(|e| e.into_inner()).take()
}

type Ping = Box<dyn Fn() + Send + Sync>;
static PING: std::sync::Mutex<Option<Ping>> = std::sync::Mutex::new(None);

/// How the window is asked to say it is alive: a message to its own thread,
/// which says so when it takes it. Watched from now on, so a window that
/// never takes the first one has stopped too
pub fn watch_window(ping: impl Fn() + Send + Sync + 'static) {
    *PING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(ping));
    arm(Part::Window);
}

/// Start watching, once
pub fn start() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = awake_ms();
    #[cfg(windows)]
    native::catch_crashes();
    let _ = std::thread::Builder::new().name("shikisha-watchdog".into()).spawn(watch);
}

/// A dump asked for: of which part, for which time it stopped (the times
/// any part stopped are counted, so a dump that comes back late is put with
/// the time it was taken for)
struct DumpAsk {
    part: Part,
    time: u64,
}

/// A part that is stopped, as the watching thread holds it
struct Stopped {
    /// The awake time of the last beat before it stopped
    from: u64,
}

fn watch() {
    // Dumps are written by a thread of their own: one that is slow or never
    // ends cannot hold up writing down that the part stopped, nor the
    // watching of the other part
    let (ask_tx, ask_rx) = std::sync::mpsc::channel::<DumpAsk>();
    let (got_tx, got_rx) = std::sync::mpsc::channel::<(DumpAsk, Option<PathBuf>)>();
    let _ = std::thread::Builder::new().name("shikisha-dumps".into()).spawn(move || {
        move_out(&crate::config::logs_dir().join("dumps"), &dumps_dir());
        tidy_dumps(&dumps_dir());
        for ask in ask_rx {
            #[cfg(windows)]
            let got = native::dump_now(&format!("hang-{}", ask.part.name()), None);
            #[cfg(not(windows))]
            let got = None;
            if got_tx.send((ask, got)).is_err() {
                return;
            }
        }
    });
    let mut open: [Option<Hang>; 2] = [None, None];
    // Which time each part's account in `open` is of
    let mut open_time = [0u64; 2];
    let mut stopped: [Option<Stopped>; 2] = [None, None];
    let mut times = 0u64;
    loop {
        std::thread::sleep(LOOK_EVERY);
        if let Some(ping) = PING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            ping();
        }
        let mut changed = false;
        for (ask, got) in got_rx.try_iter() {
            let i = ask.part.index();
            let Some(path) = got else { continue };
            if let (Some(h), true) = (&mut open[i], open_time[i] == ask.time) {
                h.dump = path.display().to_string();
                changed = true;
            }
        }
        let now = awake_ms();
        for part in PARTS {
            let i = part.index();
            // Whether a person is being waited for is read before when it
            // last beat: a dialog that closes in between beats before it lets
            // go (`person_answers`), so the beat read after is the new one
            if HELD[i].load(Ordering::SeqCst) > 0 {
                continue;
            }
            let last = LAST[i].load(Ordering::SeqCst);
            if last == 0 {
                continue;
            }
            let quiet = Duration::from_millis(now.saturating_sub(last));
            match (&stopped[i], quiet >= HUNG_AFTER) {
                (None, true) => {
                    // And looked at once more: a dialog opened, or a beat
                    // given, since the two were read is not a part stopped
                    if HELD[i].load(Ordering::SeqCst) > 0 || LAST[i].load(Ordering::SeqCst) != last {
                        continue;
                    }
                    times += 1;
                    // Written down before any dump is tried: a run ended
                    // while the dump is being written still says it stopped
                    open[i] = Some(Hang {
                        pid: std::process::id(),
                        part: part.name().into(),
                        since: now_secs().saturating_sub(quiet.as_secs()),
                        secs: quiet.as_secs(),
                        back: false,
                        dump: String::new(),
                    });
                    open_time[i] = times;
                    stopped[i] = Some(Stopped { from: last });
                    write_hangs(&open);
                    changed = false;
                    note(&format!("the {} part has not answered for {} s", part.name(), quiet.as_secs()));
                    let _ = ask_tx.send(DumpAsk { part, time: times });
                }
                (Some(_), true) => {
                    // Still gone: how long is kept up to date, so a run ended
                    // while it was says for how long
                    if let Some(h) = &mut open[i] {
                        h.secs = quiet.as_secs();
                        changed = true;
                    }
                }
                (Some(s), false) => {
                    // Back only on a beat it gave since: awake time from its
                    // last beat before to its first after
                    let lasted = last.saturating_sub(s.from) / 1000;
                    if let Some(h) = &mut open[i] {
                        h.back = true;
                        h.secs = lasted;
                    }
                    stopped[i] = None;
                    write_hangs(&open);
                    changed = false;
                    note(&format!("the {} part answers again after {lasted} s", part.name()));
                    *CAME_BACK.lock().unwrap_or_else(|e| e.into_inner()) = Some((part, lasted));
                }
                (None, false) => {}
            }
        }
        if changed {
            write_hangs(&open);
        }
    }
}

/// For checking this module on a real window (`tools/debug/watchdog.win.mjs`),
/// in a debug build only: `SHIKISHA_DEBUG_STALL=<after>:<for>` holds the
/// work loop still once, for `<for>` seconds, `<after>` seconds into the run;
/// `<after>:0` makes it crash then instead
#[cfg(debug_assertions)]
pub fn debug_stall() {
    static DONE: AtomicBool = AtomicBool::new(false);
    static FROM: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    let from = FROM.get_or_init(std::time::Instant::now);
    let Some((after, hold)) = std::env::var("SHIKISHA_DEBUG_STALL").ok().and_then(|v| {
        let (a, b) = v.split_once(':')?;
        Some((a.trim().parse::<u64>().ok()?, b.trim().parse::<u64>().ok()?))
    }) else {
        DONE.store(true, Ordering::Relaxed);
        return;
    };
    if from.elapsed() >= Duration::from_secs(after) {
        DONE.store(true, Ordering::Relaxed);
        // `<for>` of 0: not held still but made to crash, as a library
        // reading memory it does not have would
        if hold == 0 {
            // SAFETY: none -- this is the crash being asked for
            unsafe { std::ptr::read_volatile(std::ptr::dangling::<u8>().wrapping_sub(1)) };
        }
        std::thread::sleep(Duration::from_secs(hold));
    }
}

/// Dumps the first version kept beside the program, moved to where dumps are
/// kept now: that folder may be one a cloud drive copies
fn move_out(old: &Path, new: &Path) {
    let Ok(found) = std::fs::read_dir(old) else { return };
    let _ = std::fs::create_dir_all(new);
    for e in found.flatten() {
        let p = e.path();
        if let Some(name) = p.file_name()
            && (p.extension().is_some_and(|x| x == "dmp") || unfinished(&p))
            && std::fs::rename(&p, new.join(name)).is_err()
        {
            let _ = std::fs::remove_file(&p);
        }
    }
    let _ = std::fs::remove_dir(old);
}

fn unfinished(p: &Path) -> bool {
    p.extension().is_some_and(|x| x == UNFINISHED)
}

/// The dumps newest first, and the oldest given up past [`DUMPS_KEPT`] or
/// [`DUMPS_ROOM`]; and any a run left half written, once it is plainly not
/// being written any more
pub fn tidy_dumps(dir: &Path) -> Vec<(PathBuf, u64)> {
    let now = std::time::SystemTime::now();
    let mut all: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        let Some((size, when)) = e.metadata().ok().and_then(|m| Some((m.len(), m.modified().ok()?))) else { continue };
        if unfinished(&path) {
            if now.duration_since(when).is_ok_and(|age| age >= UNFINISHED_GIVEN_UP) {
                let _ = std::fs::remove_file(&path);
            }
        } else if path.extension().is_some_and(|x| x == "dmp") {
            all.push((path, size, when));
        }
    }
    all.sort_by(|a, b| b.2.cmp(&a.2));
    let mut kept = Vec::new();
    let mut room = 0u64;
    for (i, (path, size, _)) in all.into_iter().enumerate() {
        // The newest is always kept, however big: it is the one that says
        // what just happened, and it alone is still one file
        if i > 0 && (i >= DUMPS_KEPT || room + size > DUMPS_ROOM) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        room += size;
        kept.push((path, size));
    }
    kept
}

/// The dump a run wrote, when it wrote one: by its process id, written after
/// it started -- the newest of the kind asked for (`crash` or `hang`), or
/// else the newest of any
pub fn dump_of(dir: &Path, pid: u32, started: u64, kind: &str) -> Option<PathBuf> {
    let tag = format!("-{pid}-");
    let mut found: Vec<(PathBuf, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "dmp"))
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().contains(&tag)))
        .filter_map(|p| {
            let when = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
            (when.as_secs() + 1 >= started).then_some((p, when.as_secs()))
        })
        .collect();
    found.sort_by_key(|(_, when)| std::cmp::Reverse(*when));
    let of_kind = found
        .iter()
        .position(|(p, _)| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{kind}-"))));
    of_kind.or((!found.is_empty()).then_some(0)).map(|i| found.swap_remove(i).0)
}

#[cfg(windows)]
mod native {
    //! Dumps, by Windows' own writer (`MiniDumpWriteDump`). Each holds every
    //! thread's stack and the list of loaded modules, which is enough to say
    //! where each thread was; not the whole of the program's memory
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        CONTEXT, EXCEPTION_POINTERS, EXCEPTION_RECORD, LPTOP_LEVEL_EXCEPTION_FILTER, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpNormal, MiniDumpWithThreadInfo,
        MiniDumpWithUnloadedModules, MiniDumpWriteDump, SetUnhandledExceptionFilter,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, INFINITE, SetEvent, WaitForSingleObject,
    };

    /// Windows' writer may not be called by two threads at once: a hang's
    /// dump and a crash's take turns
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A dump of this program, now, named for what it is of. With the
    /// exception that caused it, when there is one: the thread it happened
    /// on and where. Written under a name that says it is unfinished, and
    /// given its own name only once whole
    pub fn dump_now(what: &str, crash: Option<(u32, *mut EXCEPTION_POINTERS)>) -> Option<PathBuf> {
        let dir = super::dumps_dir();
        std::fs::create_dir_all(&dir).ok()?;
        super::tidy_dumps(&dir);
        let path = dir.join(format!("{what}-{}-{}.dmp", std::process::id(), super::now_secs()));
        let writing = path.with_extension(format!("dmp.{}", super::UNFINISHED));
        let wide: Vec<u16> = writing.as_os_str().encode_wide_z();
        let _turn = WRITING.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: a path ended by a zero, and handles closed below
        let file = unsafe {
            CreateFileW(wide.as_ptr(), GENERIC_WRITE, 0, std::ptr::null(), CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, std::ptr::null_mut())
        };
        if file == INVALID_HANDLE_VALUE {
            return None;
        }
        let info = crash.map(|(thread, pointers)| MINIDUMP_EXCEPTION_INFORMATION {
            ThreadId: thread,
            ExceptionPointers: pointers,
            ClientPointers: 0,
        });
        // SAFETY: this process, a file open for writing, and exception
        // pointers to a copy of the crash that stays for good
        let ok = unsafe {
            MiniDumpWriteDump(
                GetCurrentProcess(),
                GetCurrentProcessId(),
                file,
                MiniDumpNormal | MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules,
                info.as_ref().map_or(std::ptr::null(), |i| i as *const _),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        unsafe { CloseHandle(file) };
        if ok == 0 || std::fs::rename(&writing, &path).is_err() {
            let _ = std::fs::remove_file(&writing);
            return None;
        }
        // Again with the new one counted, so the dumps kept are within the
        // limits once it is there too
        super::tidy_dumps(&dir);
        Some(path)
    }

    trait Wide {
        fn encode_wide_z(&self) -> Vec<u16>;
    }
    impl Wide for std::ffi::OsStr {
        fn encode_wide_z(&self) -> Vec<u16> {
            use std::os::windows::ffi::OsStrExt;
            self.encode_wide().chain(std::iter::once(0)).collect()
        }
    }

    // The crash to write down, handed from the thread that crashed to one
    // that can still write: a thread whose stack has run out cannot. Only
    // the first thread to crash fills these in, so the exception and the
    // thread it names are always the same crash's
    static CLAIMED: AtomicBool = AtomicBool::new(false);
    static CRASH_POINTERS: AtomicPtr<EXCEPTION_POINTERS> = AtomicPtr::new(std::ptr::null_mut());

    /// The crash as copied from the thread that crashed. The thread's own
    /// record and registers live on its stack, and stop being there once a
    /// filter lets it go on -- which may happen before a writer held up
    /// behind a hang's dump gets to them -- so the writer reads this copy
    #[repr(C, align(16))]
    struct Copied {
        context: CONTEXT,
        record: EXCEPTION_RECORD,
        pointers: EXCEPTION_POINTERS,
    }
    struct Kept(std::cell::UnsafeCell<std::mem::MaybeUninit<Copied>>);
    // SAFETY: written once, by the one thread that claimed it, before the
    // writer is told to read it
    unsafe impl Sync for Kept {}
    static COPIED: Kept = Kept(std::cell::UnsafeCell::new(std::mem::MaybeUninit::uninit()));

    /// Copy the crash, and the pointers to the copy
    ///
    /// SAFETY: `from` is what Windows handed the filter, and only the thread
    /// that claimed the crash calls this, once
    unsafe fn copy_crash(from: *const EXCEPTION_POINTERS) -> *mut EXCEPTION_POINTERS {
        unsafe {
            let to = (*COPIED.0.get()).as_mut_ptr();
            if from.is_null() || (*from).ExceptionRecord.is_null() || (*from).ContextRecord.is_null() {
                return std::ptr::null_mut();
            }
            std::ptr::addr_of_mut!((*to).context).write(*(*from).ContextRecord);
            let mut record = *(*from).ExceptionRecord;
            // The record of an exception this one came from is not copied
            record.ExceptionRecord = std::ptr::null_mut();
            std::ptr::addr_of_mut!((*to).record).write(record);
            std::ptr::addr_of_mut!((*to).pointers).write(EXCEPTION_POINTERS {
                ExceptionRecord: std::ptr::addr_of_mut!((*to).record),
                ContextRecord: std::ptr::addr_of_mut!((*to).context),
            });
            std::ptr::addr_of_mut!((*to).pointers)
        }
    }
    static CRASH_THREAD: AtomicU32 = AtomicU32::new(0);
    static GO: AtomicIsize = AtomicIsize::new(0);
    static DONE: AtomicIsize = AtomicIsize::new(0);
    /// The filter that was there before this one, called after it
    static BEFORE: AtomicUsize = AtomicUsize::new(0);

    /// Have a native crash written down before Windows' own reporting runs
    pub fn catch_crashes() {
        // SAFETY: plain events, kept for the life of the process. Done is
        // left set once set, so every thread that crashed is let go
        let (go, done) = unsafe { (CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()), CreateEventW(std::ptr::null(), 1, 0, std::ptr::null())) };
        if go.is_null() || done.is_null() {
            return;
        }
        GO.store(go as isize, Ordering::SeqCst);
        DONE.store(done as isize, Ordering::SeqCst);
        let writer = std::thread::Builder::new().name("shikisha-crash-writer".into()).spawn(move || {
            let (go, done) = (GO.load(Ordering::SeqCst) as HANDLE, DONE.load(Ordering::SeqCst) as HANDLE);
            // SAFETY: the events made above
            unsafe { WaitForSingleObject(go, INFINITE) };
            let pointers = CRASH_POINTERS.load(Ordering::SeqCst);
            let thread = CRASH_THREAD.load(Ordering::SeqCst);
            let path = dump_now("crash", Some((thread, pointers)));
            super::note(&format!(
                "a native crash was written down{}",
                path.map(|p| format!(" in {}", p.display())).unwrap_or_default()
            ));
            unsafe { SetEvent(done) };
        });
        if writer.is_err() {
            return;
        }
        // SAFETY: a function of the shape Windows calls
        let before = unsafe { SetUnhandledExceptionFilter(Some(on_crash)) };
        BEFORE.store(before.map_or(0, |f| f as usize), Ordering::SeqCst);
    }

    /// Called by Windows on the thread that crashed: hands the crash to the
    /// writer, waits a while for it, then does what the filter before this
    /// one would have done, or lets Windows go on as it would have
    unsafe extern "system" fn on_crash(pointers: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: plain calls on this thread and on the events made at start
        unsafe {
            if !CLAIMED.swap(true, Ordering::SeqCst) {
                CRASH_POINTERS.store(copy_crash(pointers), Ordering::SeqCst);
                CRASH_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
                SetEvent(GO.load(Ordering::SeqCst) as HANDLE);
            }
            WaitForSingleObject(DONE.load(Ordering::SeqCst) as HANDLE, 15_000);
        }
        let before = BEFORE.load(Ordering::SeqCst);
        if before != 0 && before != on_crash as *const () as usize {
            // SAFETY: what SetUnhandledExceptionFilter gave back, a filter
            let f: LPTOP_LEVEL_EXCEPTION_FILTER = unsafe { std::mem::transmute::<usize, LPTOP_LEVEL_EXCEPTION_FILTER>(before) };
            if let Some(f) = f {
                return unsafe { f(pointers) };
            }
        }
        // EXCEPTION_CONTINUE_SEARCH: Windows' own reporting still runs
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sk-dumps-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn aged(p: &Path, secs_ago: u64) {
        let f = std::fs::File::options().write(true).open(p).unwrap();
        f.set_modified(std::time::SystemTime::now() - Duration::from_secs(secs_ago)).unwrap();
    }

    #[test]
    fn only_the_newest_dumps_are_kept() {
        let dir = scratch("kept");
        for i in 0..(DUMPS_KEPT + 3) {
            let p = dir.join(format!("crash-1-{i}.dmp"));
            std::fs::write(&p, b"x").unwrap();
            aged(&p, (100 - i) as u64);
        }
        std::fs::write(dir.join("notes.txt"), b"not a dump").unwrap();
        let kept = tidy_dumps(&dir);
        assert_eq!(kept.len(), DUMPS_KEPT);
        assert!(kept[0].0.ends_with(format!("crash-1-{}.dmp", DUMPS_KEPT + 2)), "newest first: {:?}", kept[0].0);
        assert!(!dir.join("crash-1-0.dmp").exists(), "the oldest went");
        assert!(dir.join("notes.txt").exists(), "only dumps are tidied");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dump_left_half_written_goes_once_it_is_old() {
        let dir = scratch("half");
        let old = dir.join("hang-work-1-5.dmp.part");
        let fresh = dir.join("hang-work-2-6.dmp.part");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&fresh, b"x").unwrap();
        aged(&old, UNFINISHED_GIVEN_UP.as_secs() + 60);
        let kept = tidy_dumps(&dir);
        assert!(kept.is_empty(), "a half-written dump is not one: {kept:?}");
        assert!(!old.exists(), "one left long ago went");
        assert!(fresh.exists(), "one perhaps still being written stays");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_dump_of_a_run_is_the_newest_of_its_kind() {
        let dir = scratch("of");
        let started = now_secs() - 50;
        let hang = dir.join(format!("hang-work-7-{}.dmp", started + 10));
        let crash = dir.join(format!("crash-7-{}.dmp", started + 30));
        let other = dir.join(format!("crash-8-{}.dmp", started + 40));
        let earlier = dir.join("crash-7-1.dmp");
        for (p, ago) in [(&hang, 40), (&crash, 20), (&other, 10), (&earlier, 5000)] {
            std::fs::write(p, b"x").unwrap();
            aged(p, ago);
        }
        assert_eq!(dump_of(&dir, 7, started, "crash"), Some(crash.clone()), "a crash after a hang it came back from");
        assert_eq!(dump_of(&dir, 7, started, "hang"), Some(hang.clone()));
        std::fs::remove_file(&hang).unwrap();
        assert_eq!(dump_of(&dir, 7, started, "hang"), Some(crash), "else the newest of the run");
        assert_eq!(dump_of(&dir, 9, started, "crash"), None, "another run's are not this one's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hangs_read_back_as_written() {
        let h = Hang { pid: 7, part: "work".into(), since: 100, secs: 25, back: false, dump: String::new() };
        let text = serde_json::to_string(&[&h]).unwrap();
        assert_eq!(read_hangs(&text), vec![h.clone()]);
        assert!(!text.contains("dump"), "no dump, nothing said of one");
        // As the first version wrote it: one account alone, its dump kept
        let one = Hang { dump: "C:\\x\\hang-work-7-1.dmp".into(), ..h };
        assert_eq!(read_hangs(&serde_json::to_string(&one).unwrap()), vec![one]);
        assert!(read_hangs("not json").is_empty());
    }

    #[test]
    fn dumps_beside_the_program_are_moved_out() {
        let dir = scratch("out");
        let (old, new) = (dir.join("old"), dir.join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("crash-7-1.dmp"), b"x").unwrap();
        move_out(&old, &new);
        assert!(new.join("crash-7-1.dmp").exists());
        assert!(!old.exists(), "the old folder goes once empty");
        move_out(&old, &new);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_part_back_does_not_hide_one_still_stopped() {
        let work = Hang { pid: 7, part: "work".into(), since: 100, secs: 40, back: false, dump: String::new() };
        let window = Hang { pid: 7, part: "window".into(), since: 100, secs: 22, back: true, dump: String::new() };
        let both = [window.clone(), work.clone()];
        assert_eq!(still_stopped(&both, 7, 90), Some(&work));
        assert_eq!(still_stopped(&[window], 7, 90), None, "back is not stopped");
        assert_eq!(still_stopped(&both, 8, 90), None, "another run's");
        assert_eq!(still_stopped(&both, 7, 200), None, "from before this run started");
    }

    #[test]
    fn a_person_answering_is_not_a_hang() {
        // On a thread of its own: the part is this thread's
        std::thread::spawn(|| {
            beat(Part::Window);
            let held = person_answers(|| HELD[Part::Window.index()].load(Ordering::SeqCst));
            assert_eq!(held, 1, "held while the person thinks");
            assert_eq!(HELD[Part::Window.index()].load(Ordering::SeqCst), 0, "and let go after");
        })
        .join()
        .unwrap();
        // A thread that is no part is not held
        std::thread::spawn(|| {
            let before = HELD.iter().map(|h| h.load(Ordering::SeqCst)).sum::<u32>();
            person_answers(|| assert_eq!(HELD.iter().map(|h| h.load(Ordering::SeqCst)).sum::<u32>(), before));
        })
        .join()
        .unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_dump_of_this_program_can_be_written() {
        let p = native::dump_now("test", None).expect("a dump");
        assert!(std::fs::metadata(&p).unwrap().len() > 1000);
        assert!(!p.with_extension("dmp.part").exists(), "nothing half written left");
        let _ = std::fs::remove_file(p);
    }
}

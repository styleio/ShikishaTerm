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
//! A native crash on Windows (one Rust's own panic does not catch: an access
//! violation in a library the window uses) is written as a dump too, before
//! Windows' own error reporting takes it.
//!
//! The dumps stay on this PC, in the logs folder, a bounded number of them.
//! They hold the memory of the program's threads, so nothing sends them
//! anywhere; the person can hand one over if they choose to.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

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

/// The parts that have to keep moving
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The loop that runs the tabs
    Work,
    /// The window's own thread
    Window,
}

impl Part {
    fn name(self) -> &'static str {
        match self {
            Part::Work => "work",
            Part::Window => "window",
        }
    }
}

/// When each part last said it is alive, in milliseconds since this module
/// started counting. Zero: it has not said so yet, and is not watched
static WORK: AtomicU64 = AtomicU64::new(0);
static WINDOW: AtomicU64 = AtomicU64::new(0);

fn clock() -> &'static Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    // From 1, so that zero keeps meaning "not yet"
    clock().elapsed().as_millis() as u64 + 1
}

fn cell(part: Part) -> &'static AtomicU64 {
    match part {
        Part::Work => &WORK,
        Part::Window => &WINDOW,
    }
}

/// A part says it is alive
pub fn beat(part: Part) {
    cell(part).store(now_ms(), Ordering::Relaxed);
}

/// Where the account of a part that stopped is kept: the last one, as JSON
fn hang_file() -> PathBuf {
    crate::config::logs_dir().join("hang.json")
}

/// Where dumps are written
pub fn dumps_dir() -> PathBuf {
    crate::config::logs_dir().join("dumps")
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
    /// The dump written when it was found, when there is one
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dump: String,
}

/// The last part that stopped, as the hang file says
pub fn last_hang() -> Option<Hang> {
    let text = std::fs::read_to_string(hang_file()).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_hang(h: &Hang) {
    if let Ok(text) = serde_json::to_string_pretty(h) {
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
/// which says so when it takes it
pub fn watch_window(ping: impl Fn() + Send + Sync + 'static) {
    *PING.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(ping));
}

/// Start watching, once
pub fn start() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = clock();
    #[cfg(windows)]
    native::catch_crashes();
    let _ = std::thread::Builder::new().name("shikisha-watchdog".into()).spawn(watch);
}

fn watch() {
    let mut stopped: [Option<Hang>; 2] = [None, None];
    let mut last_look = Instant::now();
    loop {
        std::thread::sleep(LOOK_EVERY);
        if let Some(ping) = PING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            ping();
        }
        // A machine that slept: nothing was alive, and nothing stopped
        let gap = last_look.elapsed();
        last_look = Instant::now();
        if gap > LOOK_EVERY * 3 {
            for part in [Part::Work, Part::Window] {
                let c = cell(part);
                if c.load(Ordering::Relaxed) != 0 {
                    c.store(now_ms(), Ordering::Relaxed);
                }
            }
            continue;
        }
        for (i, part) in [Part::Work, Part::Window].into_iter().enumerate() {
            let last = cell(part).load(Ordering::Relaxed);
            if last == 0 {
                continue;
            }
            let quiet = Duration::from_millis(now_ms().saturating_sub(last));
            match (&mut stopped[i], quiet >= HUNG_AFTER) {
                (None, true) => {
                    let mut h = Hang {
                        pid: std::process::id(),
                        part: part.name().into(),
                        since: now_secs().saturating_sub(quiet.as_secs()),
                        secs: quiet.as_secs(),
                        back: false,
                        dump: String::new(),
                    };
                    note(&format!("the {} part has not answered for {} s", part.name(), quiet.as_secs()));
                    #[cfg(windows)]
                    if let Some(p) = native::dump_now(&format!("hang-{}", part.name()), None) {
                        h.dump = p.display().to_string();
                    }
                    write_hang(&h);
                    stopped[i] = Some(h);
                }
                (Some(h), true) => {
                    // Still gone: how long is kept up to date, so a run ended
                    // while it was says for how long
                    h.secs = quiet.as_secs();
                    write_hang(h);
                }
                (Some(h), false) => {
                    let lasted = now_secs().saturating_sub(h.since);
                    h.back = true;
                    h.secs = lasted;
                    write_hang(h);
                    note(&format!("the {} part answers again after {lasted} s", part.name()));
                    *CAME_BACK.lock().unwrap_or_else(|e| e.into_inner()) = Some((part, lasted));
                    stopped[i] = None;
                }
                (None, false) => {}
            }
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
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    let Some((after, hold)) = std::env::var("SHIKISHA_DEBUG_STALL").ok().and_then(|v| {
        let (a, b) = v.split_once(':')?;
        Some((a.trim().parse::<u64>().ok()?, b.trim().parse::<u64>().ok()?))
    }) else {
        DONE.store(true, Ordering::Relaxed);
        return;
    };
    if clock().elapsed() >= Duration::from_secs(after) {
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

/// The dumps newest first, and the oldest given up past [`DUMPS_KEPT`] or
/// [`DUMPS_ROOM`]
pub fn tidy_dumps(dir: &Path) -> Vec<(PathBuf, u64)> {
    let mut all: Vec<(PathBuf, u64, std::time::SystemTime)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "dmp"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((e.path(), m.len(), m.modified().ok()?))
        })
        .collect();
    all.sort_by(|a, b| b.2.cmp(&a.2));
    let mut kept = Vec::new();
    let mut room = 0u64;
    for (i, (path, size, _)) in all.into_iter().enumerate() {
        // The newest is always kept, however big
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
/// it started
pub fn dump_of(pid: u32, started: u64) -> Option<PathBuf> {
    let tag = format!("-{pid}-");
    std::fs::read_dir(dumps_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(&tag))
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .is_some_and(|t| t.as_secs() + 1 >= started)
        })
        .map(|e| e.path())
        .max()
}

#[cfg(windows)]
mod native {
    //! Dumps, by Windows' own writer (`MiniDumpWriteDump`). Each holds every
    //! thread's stack and the list of loaded modules, which is enough to say
    //! where each thread was; not the whole of the program's memory
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicIsize, AtomicPtr, AtomicU32, Ordering};
    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpNormal, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules,
        MiniDumpWriteDump, SetUnhandledExceptionFilter,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, INFINITE, SetEvent, WaitForSingleObject,
    };

    /// A dump of this program, now, named for what it is of. With the
    /// exception that caused it, when there is one: the thread it happened
    /// on and where
    pub fn dump_now(what: &str, crash: Option<(u32, *mut EXCEPTION_POINTERS)>) -> Option<PathBuf> {
        let dir = super::dumps_dir();
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("{what}-{}-{}.dmp", std::process::id(), super::now_secs()));
        let wide: Vec<u16> = path.as_os_str().encode_wide_z();
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
        // pointers that stay valid while the crashed thread waits for this
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
        if ok == 0 {
            let _ = std::fs::remove_file(&path);
            return None;
        }
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
    // that can still write: a thread whose stack has run out cannot
    static CRASH_POINTERS: AtomicPtr<EXCEPTION_POINTERS> = AtomicPtr::new(std::ptr::null_mut());
    static CRASH_THREAD: AtomicU32 = AtomicU32::new(0);
    static GO: AtomicIsize = AtomicIsize::new(0);
    static DONE: AtomicIsize = AtomicIsize::new(0);

    /// Have a native crash written down before Windows' own reporting runs
    pub fn catch_crashes() {
        // SAFETY: plain events, kept for the life of the process
        let (go, done) = unsafe { (CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()), CreateEventW(std::ptr::null(), 0, 0, std::ptr::null())) };
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
        unsafe { SetUnhandledExceptionFilter(Some(on_crash)) };
    }

    /// Called by Windows on the thread that crashed: hands the crash to the
    /// writer, waits a while for it, and lets Windows go on as it would have
    unsafe extern "system" fn on_crash(pointers: *const EXCEPTION_POINTERS) -> i32 {
        CRASH_POINTERS.store(pointers as *mut _, Ordering::SeqCst);
        // SAFETY: plain calls on this thread and on the events made at start
        unsafe {
            CRASH_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
            SetEvent(GO.load(Ordering::SeqCst) as HANDLE);
            WaitForSingleObject(DONE.load(Ordering::SeqCst) as HANDLE, 15_000);
        }
        // EXCEPTION_CONTINUE_SEARCH: Windows' own reporting still runs
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_newest_dumps_are_kept() {
        let dir = std::env::temp_dir().join(format!("sk-dumps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..(DUMPS_KEPT + 3) {
            let p = dir.join(format!("crash-1-{i}.dmp"));
            std::fs::write(&p, b"x").unwrap();
            let when = std::time::SystemTime::now() - Duration::from_secs((100 - i) as u64);
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(when).unwrap();
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
    fn a_hang_reads_back_as_written() {
        let h = Hang { pid: 7, part: "work".into(), since: 100, secs: 25, back: false, dump: String::new() };
        let text = serde_json::to_string(&h).unwrap();
        assert_eq!(serde_json::from_str::<Hang>(&text).unwrap(), h);
        assert!(!text.contains("dump"), "no dump, nothing said of one");
    }

    #[cfg(windows)]
    #[test]
    fn a_dump_of_this_program_can_be_written() {
        let p = native::dump_now("test", None).expect("a dump");
        assert!(std::fs::metadata(&p).unwrap().len() > 1000);
        let _ = std::fs::remove_file(p);
    }
}

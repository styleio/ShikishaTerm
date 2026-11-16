//! How the machine was doing while the app ran, noted for a bug report.
//!
//! "It has been slow since Friday" is a report nobody can act on: by the time
//! somebody looks, the slowness is over, and what would say why -- what was
//! using the processors, how much memory was left, whether the app's own
//! parts were keeping up, how full the disk was -- was on a screen the person
//! closed. So it is written down as it happens, a line at a time, in
//! `logs/health.log`, and kept for a week. The diagnostics a person saves
//! (`crate::diagnose`) carry it.
//!
//! A line is written when something is off -- the processors nearly all in
//! use, little memory left, a part of the app answering late -- and then it
//! names the programs using the most of each. Otherwise one line in ten
//! minutes says that all was well, so a quiet week is a small file.
//!
//! Programs are named by their file name only (`node.exe`): never their
//! arguments or where they are, which is where a person's work would show.

use std::time::Duration;

/// How often the machine is looked at
const EVERY: Duration = Duration::from_secs(60);
/// With nothing off, one look in this many is written down
const QUIET_EVERY: u32 = 10;
/// How long a line is kept
pub const KEPT: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// The most the file holds: the newest lines, whatever their age
const ROOM: usize = 2 * 1024 * 1024;
/// How many looks pass between two trimmings of the file
const TRIM_EVERY: u32 = 6 * 60;

/// The processors in use, in hundredths, from which a look is worth a line
const BUSY: f32 = 85.0;
/// The share of the memory left, in hundredths, under which a look is
const SHORT: u64 = 10;
/// How late a part of the app answers, in milliseconds, before a look is
const LATE: u64 = 3_000;
/// How many programs are named for each of what they use
const NAMED: usize = 3;

const FILE: &str = "health.log";

pub fn file() -> std::path::PathBuf {
    crate::config::logs_dir().join(FILE)
}

/// What one look at the machine found
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Look {
    /// The processors in use, all of them together, in hundredths
    pub cpu: f32,
    /// Memory free for use, and all there is, in bytes
    pub free: u64,
    pub total: u64,
    /// What the system can still promise of memory and the most it can (its
    /// commit, `crate::pressure::room`), in bytes, where it says: a machine
    /// runs out of this with memory still free, and then refuses programs
    pub commit: Option<(u64, u64)>,
    /// How late the work part and the window part answered at worst since
    /// the last look, in milliseconds
    pub late: [u64; 2],
    /// Room left on the disk the app is on, and its size, in bytes
    pub disk: Option<(u64, u64)>,
}

impl Look {
    /// Whether something is off, and the programs behind it are worth naming
    pub fn off(&self) -> bool {
        self.cpu >= BUSY
            || (self.total > 0 && self.free.saturating_mul(100) / self.total < SHORT)
            || self.commit.is_some_and(|(left, most)| most > 0 && left.saturating_mul(100) / most < SHORT)
            || self.late.iter().any(|l| *l >= LATE)
    }
}

/// The programs using the most: of the processors (in hundredths of the
/// whole machine) and of the memory (in bytes), by file name, the copies of
/// one program taken together
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Top {
    pub cpu: Vec<(String, f32)>,
    pub memory: Vec<(String, u64)>,
}

fn secs(ms: u64) -> String {
    format!("{:.1} s", ms as f64 / 1000.0)
}

/// One line of the file: when, what the look found, and -- when something
/// was off -- the programs using the most
pub fn line(look: &Look, top: Option<&Top>) -> String {
    let size = crate::pressure::size;
    let mut s = format!(
        "cpu {:.0}%, memory {} free of {}, late: work {} window {}",
        look.cpu,
        size(look.free),
        size(look.total),
        secs(look.late[0]),
        secs(look.late[1]),
    );
    if let Some((left, most)) = look.commit {
        s.push_str(&format!(", commit {} left of {}", size(left), size(most)));
    }
    if let Some((free, total)) = look.disk {
        s.push_str(&format!(", disk {} free of {}", size(free), size(total)));
    }
    if let Some(top) = top {
        // A program on one of many processors is a few hundredths of the
        // machine: said to a tenth, or it would read as nothing at all
        let cpu: Vec<String> = top.cpu.iter().map(|(n, c)| if *c < 10.0 { format!("{n} {c:.1}%") } else { format!("{n} {c:.0}%") }).collect();
        let memory: Vec<String> = top.memory.iter().map(|(n, m)| format!("{n} {}", size(*m))).collect();
        s.push_str(&format!(" | most cpu: {} | most memory: {}", cpu.join(", "), memory.join(", ")));
    }
    s
}

/// The time a line of one of the app's own records was written, in seconds
/// since 1970: they begin `[pid N at SECONDS]` (`crate::lastexit::crash_entry`)
pub fn written_at(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("[pid ")?;
    let (_, rest) = rest.split_once(" at ")?;
    let (secs, _) = rest.split_once(']')?;
    secs.trim().parse().ok()
}

/// The lines of a record from the last `KEPT` before `now`, and no more of
/// them than `room` bytes, the newest kept. A line that does not say when it
/// was written -- the rest of a message that ran over several -- goes with
/// the line before it
pub fn recent(text: &str, now: u64, room: usize) -> String {
    let from = now.saturating_sub(KEPT.as_secs());
    let mut keep = false;
    let mut lines: Vec<&str> = Vec::new();
    for l in text.lines() {
        if let Some(at) = written_at(l) {
            keep = at >= from;
        }
        if keep {
            lines.push(l);
        }
    }
    let mut held = 0;
    let mut first = lines.len();
    for (i, l) in lines.iter().enumerate().rev() {
        held += l.len() + 1;
        if held > room {
            break;
        }
        first = i;
    }
    let mut out = lines[first..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Drop what is older than a week, and what the file has no room for
fn trim() {
    let path = file();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let kept = recent(&text, now_secs(), ROOM);
    if kept.len() != text.len() {
        // Written beside it and put in its place, so a run ended half way
        // through leaves the file whole
        let beside = path.with_extension("log.new");
        if std::fs::write(&beside, kept).is_ok() {
            let _ = std::fs::rename(&beside, &path);
        }
    }
}

fn note(what: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file()) {
        use std::io::Write as _;
        let stamp = crate::hooks::local_stamp("%Y-%m-%dT%H:%M:%S");
        let _ = writeln!(f, "{}", crate::lastexit::crash_entry(&format!("{stamp} {what}")));
    }
}

/// Room left for this user on the disk `at` is on, and the disk's size
#[cfg(windows)]
pub fn disk_room(at: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = at.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: `wide` ends in a zero and outlives the call; the two numbers are ours
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) };
    (ok != 0).then_some((free, total))
}

#[cfg(not(windows))]
pub fn disk_room(at: &std::path::Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt as _;
    let path = std::ffi::CString::new(at.as_os_str().as_bytes()).ok()?;
    let mut v: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` ends in a zero and outlives the call; `v` is ours to write into
    if unsafe { libc::statvfs(path.as_ptr(), &mut v) } != 0 {
        return None;
    }
    let block = v.f_frsize as u64;
    Some((v.f_bavail as u64 * block, v.f_blocks as u64 * block))
}

/// The most of `all`, by what `of` says each holds, the largest first
fn most<T: Copy + PartialOrd>(all: &std::collections::HashMap<String, T>) -> Vec<(String, T)> {
    let mut list: Vec<(String, T)> = all.iter().map(|(n, v)| (n.clone(), *v)).collect();
    list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    list.truncate(NAMED);
    list
}

/// The programs using the most now. Takes about half a second: what a
/// program uses of the processors is read as the change between two looks
fn top() -> Top {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let kind = ProcessRefreshKind::nothing().with_cpu().with_memory().without_tasks();
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(500)));
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;
    let mut cpu: std::collections::HashMap<String, f32> = Default::default();
    let mut memory: std::collections::HashMap<String, u64> = Default::default();
    for p in sys.processes().values() {
        let name = p.name().to_string_lossy().into_owned();
        if name.is_empty() {
            continue;
        }
        *cpu.entry(name.clone()).or_default() += p.cpu_usage() / cores;
        *memory.entry(name).or_default() += p.memory();
    }
    Top { cpu: most(&cpu), memory: most(&memory) }
}

/// Look at the machine once a minute for as long as the app runs, on a
/// thread of its own, and write down what is found
pub fn start() {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new().name("shikisha-health".into()).spawn(|| {
        trim();
        note(&format!("started {} ({})", env!("CARGO_PKG_VERSION"), env!("BUILD_REV")));
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_usage();
        // What was late before the first look is the app starting, not trouble
        let _ = crate::watchdog::take_late();
        let root = crate::config::root_dir();
        let mut looks = 0u32;
        loop {
            std::thread::sleep(EVERY);
            looks = looks.wrapping_add(1);
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            let look = Look {
                cpu: sys.global_cpu_usage(),
                free: sys.available_memory(),
                total: sys.total_memory(),
                commit: crate::pressure::room(),
                late: crate::watchdog::take_late(),
                disk: disk_room(&root),
            };
            if look.off() {
                note(&line(&look, Some(&top())));
            } else if looks % QUIET_EVERY == 0 {
                note(&line(&look, None));
            }
            if looks % TRIM_EVERY == 0 {
                trim();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    fn calm() -> Look {
        Look { cpu: 12.0, free: 20 * GB, total: 32 * GB, commit: Some((30 * GB, 40 * GB)), late: [0, 0], disk: Some((100 * GB, 476 * GB)) }
    }

    /// A look is worth naming programs for when the processors are nearly
    /// all in use, little memory is left, or a part of the app answers late
    #[test]
    fn a_look_is_off_when_the_machine_or_the_app_is_struggling() {
        assert!(!calm().off());
        assert!(Look { cpu: 93.0, ..calm() }.off());
        assert!(Look { free: 2 * GB, ..calm() }.off());
        assert!(Look { late: [0, 4_500], ..calm() }.off());
        assert!(Look { commit: Some((GB, 40 * GB)), ..calm() }.off(), "memory free, and none left to promise");
        assert!(!Look { commit: None, ..calm() }.off());
        assert!(!Look { late: [2_000, 0], ..calm() }.off(), "a part two seconds late is the window being asked");
        assert!(!Look { total: 0, free: 0, ..calm() }.off(), "a machine that does not say its memory is not short of it");
    }

    /// A line says what a person would have read off the task manager, and
    /// names programs by their file name only
    #[test]
    fn a_line_says_what_was_in_use_and_by_whom() {
        let look = Look { cpu: 93.4, late: [3_400, 0], ..calm() };
        let top = Top {
            cpu: vec![("rustc.exe".into(), 61.2), ("node.exe".into(), 3.14)],
            memory: vec![("rustc.exe".into(), 5 * GB + GB / 2)],
        };
        let said = line(&look, Some(&top));
        assert_eq!(
            said,
            "cpu 93%, memory 20.0 GB free of 32.0 GB, late: work 3.4 s window 0.0 s, commit 30.0 GB left of 40.0 GB, disk 100.0 GB free of 476.0 GB \
             | most cpu: rustc.exe 61%, node.exe 3.1% | most memory: rustc.exe 5.5 GB"
        );
        assert!(!line(&calm(), None).contains("most"), "a calm look names nobody");
    }

    /// What is older than a week goes, with the lines that belonged to it;
    /// and of what is left, the newest that fit
    #[test]
    fn only_the_last_week_is_kept_and_only_what_there_is_room_for() {
        let now = 10_000_000;
        let old = now - KEPT.as_secs() - 5;
        let text = format!(
            "[pid 1 at {old}] an old line\n  and the rest of it\n[pid 2 at {}] a line of this week\n  with more to it\n[pid 2 at {}] the newest\n",
            now - 60,
            now - 1
        );
        let kept = recent(&text, now, 1024);
        assert!(!kept.contains("old") && !kept.contains("the rest of it"), "{kept}");
        assert!(kept.contains("a line of this week\n  with more to it\n"), "{kept}");
        assert!(kept.ends_with("the newest\n"), "{kept}");
        let tight = recent(&text, now, 40);
        assert_eq!(tight, format!("[pid 2 at {}] the newest\n", now - 1));
        assert_eq!(recent("", now, 1024), "");
        assert_eq!(written_at("[pid 43560 at 1794618468] the work part has not answered"), Some(1794618468));
        assert_eq!(written_at("index out of bounds"), None);
    }

    /// The disk the app is on says how much room it has
    #[test]
    fn the_disk_of_a_folder_that_is_there_has_a_size() {
        let (free, total) = disk_room(&std::env::temp_dir()).expect("the temp folder is on a disk");
        assert!(total > 0 && free <= total, "{free} of {total}");
    }

    /// The programs using the most are named, the copies of one together
    #[test]
    fn the_programs_using_the_most_are_named() {
        // Something of this program is kept busy while it is looked at
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let busy = {
            let stop = stop.clone();
            std::thread::spawn(move || while !stop.load(std::sync::atomic::Ordering::Relaxed) { std::hint::spin_loop() })
        };
        let top = top();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = busy.join();
        assert!(top.cpu.first().is_some_and(|(_, c)| *c > 0.0), "nothing is using the processors: {top:?}");
        assert!(!top.memory.is_empty() && top.memory.len() <= NAMED, "{top:?}");
        assert!(top.memory.windows(2).all(|w| w[0].1 >= w[1].1), "not the largest first: {top:?}");
        let mut names: Vec<&String> = top.memory.iter().map(|(n, _)| n).collect();
        names.dedup();
        assert_eq!(names.len(), top.memory.len(), "a program is named twice: {top:?}");
    }
}

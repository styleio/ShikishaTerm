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
//! use, little memory left, memory being read back from the disk, the disk
//! busy all the time, a part of the app answering late -- and then it names
//! the programs using the most of each, and says how much of it is this app
//! and what it started. Otherwise one line in ten minutes says that all was
//! well, so a quiet week is a small file. A piece of the app's own work that
//! took long (`took`) gets a line too.
//!
//! The file is never the place for more: what can be read off the machine
//! when a person saves the diagnostics is read then (`crate::diagnose`).
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
/// How busy the disks are, in hundredths of the time, from which a look is
const DISK_BUSY: f32 = 90.0;
/// How many pages of memory a second are read back from the disk before a
/// look is: a machine with memory to spare reads a few when a program
/// starts, one without reads thousands and does little else
const PAGED: f32 = 500.0;
/// How many programs are named for the processors and the memory they use,
/// and for what they are promised and what they read and write
const NAMED: usize = 5;
const NAMED_FEW: usize = 3;
/// Over how long the programs are measured when something is off
const MEASURED: Duration = Duration::from_millis(1000);
/// From how long a piece of the app's own work is worth a line
pub const SLOW: Duration = Duration::from_secs(5);

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
    /// What the system says of the minute before the look, where it does
    /// (`crate::perf::Rates`): the processors' time spent in the system
    /// itself, how busy the disks were and how many requests waited for
    /// them, and the pages of memory read back from the disk a second
    pub kernel: Option<f32>,
    pub busy: Option<f32>,
    pub queue: Option<f32>,
    pub paged: Option<f32>,
}

impl Look {
    /// Whether something is off, and the programs behind it are worth naming
    pub fn off(&self) -> bool {
        self.busy_processors() || self.short_of_memory() || self.late_app() || self.disk_busy() || self.paging()
    }

    /// The processors nearly all in use
    pub fn busy_processors(&self) -> bool {
        self.cpu >= BUSY
    }

    /// Little memory left: free for use, or for the system to promise
    pub fn short_of_memory(&self) -> bool {
        (self.total > 0 && self.free.saturating_mul(100) / self.total < SHORT)
            || self.commit.is_some_and(|(left, most)| most > 0 && left.saturating_mul(100) / most < SHORT)
    }

    /// A part of the app answering late
    pub fn late_app(&self) -> bool {
        self.late.iter().any(|l| *l >= LATE)
    }

    /// The disks busy all the time
    pub fn disk_busy(&self) -> bool {
        self.busy.is_some_and(|b| b >= DISK_BUSY)
    }

    /// Memory being read back from the disk
    pub fn paging(&self) -> bool {
        self.paged.is_some_and(|p| p >= PAGED)
    }
}

/// The programs using the most: of the processors (in hundredths of the
/// whole machine) and of the memory (in bytes), by file name, the copies of
/// one program taken together
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Top {
    pub cpu: Vec<(String, f32)>,
    pub memory: Vec<(String, u64)>,
    /// Of the memory promised (the commit), where the system says
    pub commit: Vec<(String, u64)>,
    /// Of what is read and written -- files, pipes and the network taken
    /// together, which is how the system counts it for a program -- in
    /// bytes a second
    pub disk: Vec<(String, u64)>,
    /// This app and everything it started, taken together: how many
    /// programs, and what they use of the processors, the memory and the disk
    pub own: Option<(usize, f32, u64, u64)>,
}

/// A share of the processors. A program on one of many is a few hundredths
/// of the machine: said to a tenth, or it would read as nothing at all
fn share(c: f32) -> String {
    if c < 10.0 { format!("{c:.1}%") } else { format!("{c:.0}%") }
}

/// Bytes a second, as a person reads them
fn a_second(bytes: u64) -> String {
    format!("{:.1} MB/s", bytes as f64 / (1024.0 * 1024.0))
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
    if let Some(k) = look.kernel {
        s.push_str(&format!(", kernel {k:.0}%"));
    }
    if let Some(b) = look.busy {
        s.push_str(&format!(", disk busy {b:.0}%"));
    }
    if let Some(q) = look.queue {
        s.push_str(&format!(", disk queue {q:.1}"));
    }
    if let Some(p) = look.paged {
        s.push_str(&format!(", paged in {p:.0}/s"));
    }
    if let Some(top) = top {
        let cpu: Vec<String> = top.cpu.iter().map(|(n, c)| format!("{n} {}", share(*c))).collect();
        let memory: Vec<String> = top.memory.iter().map(|(n, m)| format!("{n} {}", size(*m))).collect();
        s.push_str(&format!(" | most cpu: {} | most memory: {}", cpu.join(", "), memory.join(", ")));
        if !top.commit.is_empty() {
            let commit: Vec<String> = top.commit.iter().map(|(n, m)| format!("{n} {}", size(*m))).collect();
            s.push_str(&format!(" | most committed: {}", commit.join(", ")));
        }
        if !top.disk.is_empty() {
            let disk: Vec<String> = top.disk.iter().map(|(n, d)| format!("{n} {}", a_second(*d))).collect();
            s.push_str(&format!(" | most io: {}", disk.join(", ")));
        }
        if let Some((programs, cpu, memory, disk)) = top.own {
            s.push_str(&format!(" | this app's own: {programs} programs, cpu {}, memory {}, io {}", share(cpu), size(memory), a_second(disk)));
        }
    }
    s
}

/// What a line of the file said, read back: when it was written, what the
/// look found (a line that is not a look -- the app starting, a piece of work
/// that took long -- has none, and its words are in `other`), and the
/// programs it named, under each heading
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    pub at: u64,
    pub when: String,
    pub look: Option<Look>,
    pub named: Vec<(String, Vec<String>)>,
    pub other: String,
}

/// A size as `crate::pressure::size` writes it, in bytes
pub(crate) fn unsize(s: &str) -> Option<u64> {
    let (n, unit) = s.trim().split_once(' ')?;
    let n: f64 = n.parse().ok()?;
    let by = match unit {
        "GB" => 1024.0 * 1024.0 * 1024.0,
        "MB" => 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * by) as u64)
}

/// Read a line of the file back (`line` wrote it)
pub fn seen(text: &str) -> Option<Seen> {
    let at = written_at(text)?;
    let (_, rest) = text.split_once("] ")?;
    let (when, rest) = rest.split_once(' ').unwrap_or((rest, ""));
    let mut seen = Seen { at, when: when.to_string(), ..Default::default() };
    if !rest.starts_with("cpu ") {
        seen.other = rest.to_string();
        return Some(seen);
    }
    let mut parts = rest.split(" | ");
    let mut look = Look::default();
    let two = |s: &str, mid: &str| s.split_once(mid).and_then(|(a, b)| Some((unsize(a)?, unsize(b)?)));
    let number = |s: &str| s.trim_end_matches(['%', 's']).trim_end_matches('/').trim().parse::<f32>().ok();
    for part in parts.next().unwrap_or_default().split(", ") {
        if let Some(v) = part.strip_prefix("cpu ") {
            look.cpu = number(v).unwrap_or(0.0);
        } else if let Some(v) = part.strip_prefix("memory ") {
            (look.free, look.total) = two(v, " free of ").unwrap_or((0, 0));
        } else if let Some(v) = part.strip_prefix("late: work ") {
            if let Some((work, window)) = v.split_once(" s window ") {
                let ms = |s: &str| (s.trim_end_matches(" s").parse::<f64>().unwrap_or(0.0) * 1000.0) as u64;
                look.late = [ms(work), ms(window)];
            }
        } else if let Some(v) = part.strip_prefix("commit ") {
            look.commit = two(v, " left of ");
        } else if let Some(v) = part.strip_prefix("disk busy ") {
            look.busy = number(v);
        } else if let Some(v) = part.strip_prefix("disk queue ") {
            look.queue = number(v);
        } else if let Some(v) = part.strip_prefix("disk ") {
            look.disk = two(v, " free of ");
        } else if let Some(v) = part.strip_prefix("kernel ") {
            look.kernel = number(v);
        } else if let Some(v) = part.strip_prefix("paged in ") {
            look.paged = number(v);
        }
    }
    seen.look = Some(look);
    for part in parts {
        if let Some((heading, items)) = part.split_once(": ") {
            seen.named.push((heading.to_string(), items.split(", ").map(str::to_string).collect()));
        }
    }
    Some(seen)
}

/// Write down a piece of the app's own work that took long: what it was, how
/// long, and `how` it went in a word or a number -- never a name or a
/// place, which this file does not hold. Work that was quick writes nothing
pub fn took(what: &str, since: std::time::Instant, how: &str) {
    let for_ = since.elapsed();
    if for_ < SLOW {
        return;
    }
    let how = if how.is_empty() { String::new() } else { format!(" ({how})") };
    note(&format!("slow: {what} took {:.0} s{how}", for_.as_secs_f64()));
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

/// The programs using the most now. Takes a second: what a program uses of
/// the processors and the disk is read as the change between two looks
fn top() -> Top {
    top_of(&crate::perf::sample(MEASURED).1)
}

/// The same, out of every program there is: the copies of one program taken
/// together and said with how many they are, the largest first
pub fn top_of(procs: &[crate::perf::Proc]) -> Top {
    #[derive(Default, Clone, Copy)]
    struct Sum {
        copies: u32,
        cpu: f32,
        memory: u64,
        commit: u64,
        disk: u64,
    }
    let mut by: std::collections::BTreeMap<&str, Sum> = Default::default();
    for p in procs {
        let s = by.entry(p.name.as_str()).or_default();
        s.copies += 1;
        s.cpu += p.cpu;
        s.memory += p.memory;
        s.commit += p.commit;
        s.disk += p.disk;
    }
    fn most<T: Copy + PartialOrd + Default>(by: &std::collections::BTreeMap<&str, Sum>, of: impl Fn(&Sum) -> T, least: T, named: usize) -> Vec<(String, T)> {
        let mut list: Vec<(&str, &Sum)> = by.iter().map(|(n, s)| (*n, s)).filter(|(_, s)| of(s) > least).collect();
        list.sort_by(|a, b| of(b.1).partial_cmp(&of(a.1)).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(b.0)));
        list.truncate(named);
        list.into_iter().map(|(n, s)| (if s.copies > 1 { format!("{n} x{}", s.copies) } else { n.to_string() }, of(s))).collect()
    }
    let ours: Vec<&crate::perf::Proc> = procs.iter().filter(|p| p.ours).collect();
    Top {
        cpu: most(&by, |s| s.cpu, 0.0, NAMED),
        memory: most(&by, |s| s.memory, 0, NAMED),
        commit: most(&by, |s| s.commit, 0, NAMED_FEW),
        // Under a tenth of a megabyte a second is nobody working the disk
        disk: most(&by, |s| s.disk, 100 * 1024, NAMED_FEW),
        own: (!ours.is_empty()).then(|| (ours.len(), ours.iter().map(|p| p.cpu).sum(), ours.iter().map(|p| p.memory).sum(), ours.iter().map(|p| p.disk).sum())),
    }
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
        // What the system itself counts, each reading the minute since the last
        let mut meter = crate::perf::Meter::start();
        let mut looks = 0u32;
        loop {
            std::thread::sleep(EVERY);
            looks = looks.wrapping_add(1);
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            let rates = meter.as_mut().map(|m| m.read()).unwrap_or_default();
            let look = Look {
                cpu: sys.global_cpu_usage(),
                free: sys.available_memory(),
                total: sys.total_memory(),
                commit: crate::pressure::room(),
                late: crate::watchdog::take_late(),
                disk: disk_room(&root),
                kernel: rates.kernel,
                busy: rates.busy,
                queue: rates.queue,
                paged: rates.paged,
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
        Look { cpu: 12.0, free: 20 * GB, total: 32 * GB, commit: Some((30 * GB, 40 * GB)), late: [0, 0], disk: Some((100 * GB, 476 * GB)), ..Default::default() }
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
        assert!(Look { busy: Some(97.0), ..calm() }.off(), "the disk busy all the time");
        assert!(Look { paged: Some(4_200.0), ..calm() }.off(), "memory being read back from the disk");
        assert!(!Look { busy: Some(40.0), paged: Some(30.0), ..calm() }.off());
    }

    /// A line says what a person would have read off the task manager, and
    /// names programs by their file name only
    #[test]
    fn a_line_says_what_was_in_use_and_by_whom() {
        let look = Look { cpu: 93.4, late: [3_400, 0], ..calm() };
        let top = Top {
            cpu: vec![("rustc.exe".into(), 61.2), ("node.exe".into(), 3.14)],
            memory: vec![("rustc.exe".into(), 5 * GB + GB / 2)],
            ..Default::default()
        };
        let said = line(&look, Some(&top));
        assert_eq!(
            said,
            "cpu 93%, memory 20.0 GB free of 32.0 GB, late: work 3.4 s window 0.0 s, commit 30.0 GB left of 40.0 GB, disk 100.0 GB free of 476.0 GB \
             | most cpu: rustc.exe 61%, node.exe 3.1% | most memory: rustc.exe 5.5 GB"
        );
        assert!(!line(&calm(), None).contains("most"), "a calm look names nobody");
    }

    /// What the system counts and what the app itself uses are said too, and
    /// a line is read back as what it said
    #[test]
    fn a_line_is_read_back_as_what_it_said() {
        let look = Look { cpu: 41.0, free: 300 * 1024 * 1024, total: 8 * GB, kernel: Some(22.4), busy: Some(98.6), queue: Some(4.3), paged: Some(5210.0), ..calm() };
        let top = Top {
            cpu: vec![("MsMpEng".into(), 14.0)],
            memory: vec![("chrome x23".into(), 2 * GB), ("Memory Compression".into(), GB)],
            commit: vec![("chrome x23".into(), 3 * GB)],
            disk: vec![("System".into(), 30 * 1024 * 1024)],
            own: Some((14, 3.2, 2 * GB, 512 * 1024)),
        };
        let said = line(&look, Some(&top));
        assert!(said.ends_with(
            ", kernel 22%, disk busy 99%, disk queue 4.3, paged in 5210/s \
             | most cpu: MsMpEng 14% | most memory: chrome x23 2.0 GB, Memory Compression 1.0 GB | most committed: chrome x23 3.0 GB \
             | most io: System 30.0 MB/s | this app's own: 14 programs, cpu 3.2%, memory 2.0 GB, io 0.5 MB/s"
        ), "{said}");
        let back = seen(&format!("[pid 7 at 1794963433] 2026-11-18T09:57:13 {said}")).unwrap();
        assert_eq!((back.at, back.when.as_str()), (1794963433, "2026-11-18T09:57:13"));
        let read = back.look.unwrap();
        assert_eq!((read.cpu, read.free, read.total), (41.0, 300 * 1024 * 1024, 8 * GB));
        assert_eq!((read.commit, read.disk), (Some((30 * GB, 40 * GB)), Some((100 * GB, 476 * GB))));
        assert_eq!((read.kernel, read.busy, read.queue, read.paged), (Some(22.0), Some(99.0), Some(4.3), Some(5210.0)));
        assert!(read.off());
        assert_eq!(back.named[1], ("most memory".to_string(), vec!["chrome x23 2.0 GB".to_string(), "Memory Compression 1.0 GB".to_string()]));
        assert_eq!(back.named.len(), 5);
        // A line written before the system's own counts were kept reads back too
        let old = seen("[pid 14804 at 1794963192] 2026-11-18T09:53:12 cpu 8%, memory 1.2 GB free of 7.8 GB, late: work 0.0 s window 2.0 s, commit 4.0 GB left of 17.7 GB, disk 18.4 GB free of 117.8 GB").unwrap();
        let read = old.look.unwrap();
        assert_eq!((read.cpu, read.late, read.busy), (8.0, [0, 2000], None));
        // ...and one that is not a look keeps its words
        let other = seen("[pid 1 at 5] 2026-11-18T09:43:12 slow: removing a folder took 95 s (failed)").unwrap();
        assert_eq!((other.look, other.other.as_str()), (None, "slow: removing a folder took 95 s (failed)"));
    }

    /// The copies of one program are taken together and counted, and what
    /// the app started is added up
    #[test]
    fn the_copies_of_a_program_are_taken_together() {
        let p = |name: &str, cpu: f32, memory: u64, disk: u64, ours: bool| crate::perf::Proc { name: name.into(), cpu, memory, commit: memory * 2, disk, ours, ..Default::default() };
        let top = top_of(&[p("chrome", 1.0, GB, 0, false), p("chrome", 2.0, GB, 0, false), p("node", 40.0, GB / 2, 5 * 1024 * 1024, true), p("app", 0.5, GB / 4, 10, true)]);
        assert_eq!(top.cpu[0], ("node".to_string(), 40.0));
        assert_eq!(top.cpu[1], ("chrome x2".to_string(), 3.0));
        assert_eq!(top.memory[0], ("chrome x2".to_string(), 2 * GB));
        assert_eq!(top.commit[0], ("chrome x2".to_string(), 4 * GB));
        assert_eq!(top.disk, [("node".to_string(), 5 * 1024 * 1024)], "a program that reads next to nothing is not named");
        assert_eq!(top.own, Some((2, 40.5, GB / 2 + GB / 4, 5 * 1024 * 1024 + 10)));
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
        // ...and has been for a moment: a program that has used no time yet
        // has nothing for its next look to be measured against
        std::thread::sleep(Duration::from_millis(300));
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

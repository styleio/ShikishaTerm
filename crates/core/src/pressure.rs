//! Standing through a machine running out of memory, and saying who ran it out.
//!
//! When Windows runs out of commit -- the memory it has promised to programs,
//! RAM and page file together -- the next program to ask for any is refused,
//! and a Rust program refused an allocation ends on the spot. Nothing chooses
//! which program that is. On 2026-09-22 and 23 it was this one, three times:
//! a python holding 17 GB and a build asking for more left nothing, and the
//! terminal went with every AI at work in it, although it was itself using a
//! few tens of megabytes.
//!
//! Two answers, and neither of them touches the screen. Taking the screen away
//! to make room was tried and put back the same day: on a small machine the
//! line is crossed before the window has even opened, and on any machine it is
//! crossed while somebody is using it (`.private/doc/crash-resilience-trials.ja.md`).
//!
//! * **The reserve** (`crate::reserve`): memory set aside at the start and
//!   given back the instant an allocation is refused, so the refusal a build
//!   causes for a second is ridden out instead of ending the program. This
//!   module puts it back once there is room again.
//! * **Saying so**: when the room is getting small, or the reserve has just
//!   been spent, the notification-area icon says what is going on and names
//!   the program holding the most. The person can do something about a python
//!   holding 17 GB; they can do nothing about a terminal that vanished.

/// Below this much room the machine is short, on a machine with a commit
/// limit of 20 GB or more. Smaller machines scale it down to a tenth of
/// their limit (see [`lines`]): a 4 GB laptop lives all day below two
/// gigabytes and must not be told about it all day
pub const LOW: u64 = 2 * GIB;

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// How often the room is looked at. Asking costs next to nothing (one call,
/// no allocation), and a build eats a gigabyte in seconds
pub const EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// The least time between two notices about memory. Said once, a notice is
/// read; said every few minutes on a machine that hovers at the line, it is
/// a nuisance that teaches people to ignore the icon
pub const QUIET_FOR: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// What changed about the room, when something did
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// It fell below the low line
    Low,
    /// It came back above the eased line after being low
    Eased,
}

/// The room left, followed over time. Answers only when the answer changes,
/// so the one who asks can act on each answer once
#[derive(Debug)]
pub struct Gauge {
    low: bool,
    /// Where the lines are (see [`lines`])
    lines: (u64, u64),
}

impl Gauge {
    /// Lines for a machine whose commit limit is `limit`
    pub fn new(limit: u64) -> Self {
        Self { low: false, lines: lines(limit) }
    }

    /// `room` is the commit still available, in bytes
    pub fn read(&mut self, room: u64) -> Option<Turn> {
        let (low, eased) = self.lines;
        if !self.low && room < low {
            self.low = true;
            return Some(Turn::Low);
        }
        if self.low && room > eased {
            self.low = false;
            return Some(Turn::Eased);
        }
        None
    }
}

/// Where the low and eased lines are, for a machine whose commit limit is
/// `limit`: a tenth of it, at most [`LOW`], and twice that to count as eased.
///
/// `SHIKISHA_MEMORY_LOW_MB` and `SHIKISHA_MEMORY_EASED_MB` move them, for
/// trying this on a machine that has plenty: running a real machine out of
/// memory to see what happens is the very harm this exists to prevent
pub fn lines(limit: u64) -> (u64, u64) {
    let mb = |name: &str| {
        std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok()).map(|v| v * MIB)
    };
    let low = (limit / 10).min(LOW);
    (
        mb("SHIKISHA_MEMORY_LOW_MB").unwrap_or(low),
        mb("SHIKISHA_MEMORY_EASED_MB").unwrap_or(low * 2),
    )
}

/// The room on this machine: the commit still available, and the limit it is
/// part of, in bytes.
///
/// Commit rather than free RAM, because running out of RAM only makes a
/// machine slow -- it pages -- while running out of commit is what refuses
/// the allocation
#[cfg(windows)]
pub fn room() -> Option<(u64, u64)> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut s: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    s.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut s) } != 0).then_some((s.ullAvailPageFile, s.ullTotalPageFile))
}

#[cfg(not(windows))]
pub fn room() -> Option<(u64, u64)> {
    // Elsewhere the kernel overcommits and ends a program of its own choosing
    // instead of refusing it, so there is no line to read ahead of time
    None
}

/// The program on this machine holding the most memory, other than this one:
/// its name and what it holds, in bytes.
///
/// What is counted is what a program has committed for itself (private
/// bytes), which is what running the machine out is made of. Programs this
/// one cannot ask about are passed over rather than guessed at
#[cfg(windows)]
pub fn heaviest() -> Option<(String, u64)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    let me = std::process::id();
    let mut best: Option<(String, u64)> = None;
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snap, &mut e) != 0;
        while more {
            if e.th32ProcessID != me && e.th32ProcessID != 0 {
                let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, e.th32ProcessID);
                if !h.is_null() {
                    let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
                    c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
                    if GetProcessMemoryInfo(h, &mut c as *mut _ as *mut _, c.cb) != 0 {
                        let held = c.PrivateUsage as u64;
                        if best.as_ref().is_none_or(|(_, b)| held > *b) {
                            let len = e.szExeFile.iter().position(|&u| u == 0).unwrap_or(e.szExeFile.len());
                            best = Some((String::from_utf16_lossy(&e.szExeFile[..len]), held));
                        }
                    }
                    CloseHandle(h);
                }
            }
            more = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    best
}

#[cfg(not(windows))]
pub fn heaviest() -> Option<(String, u64)> {
    None
}

/// An amount of memory the way a person reads it: `17.5 GB`, `640 MB`
pub fn size(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.1} GB", bytes as f64 / GIB as f64)
    } else {
        format!("{} MB", bytes / MIB)
    }
}

/// What the icon says, as (title, text). `rescued` means the reserve has just
/// been spent to keep this program running, which is worth saying even when
/// the room has already come back
pub fn notice(rescued: bool, heaviest: Option<(String, u64)>) -> (String, String) {
    let title = crate::i18n::t(if rescued { "tray.memory_rescued.title" } else { "tray.memory_low.title" });
    let who = match heaviest {
        Some((name, held)) => crate::i18n::tp(
            "tray.memory.heaviest",
            &[("name", &name), ("size", &size(held))],
        ),
        None => crate::i18n::t("tray.memory.nobody"),
    };
    (title, format!("{} {}", crate::i18n::t("tray.memory.still_working"), who))
}

/// Follow the room for as long as `say` wants to hear, on a thread of its
/// own: put the reserve back once there is room for it, and hand `say` a
/// notice when the room gets small or the reserve is spent. `say` answers
/// whether to go on: false once whoever it tells has gone
pub fn watch(say: impl Fn(String, String) -> bool + Send + 'static) {
    let Some((_, limit)) = room() else { return };
    let _ = std::thread::Builder::new().name("shikisha-pressure".into()).spawn(move || {
        let mut gauge = Gauge::new(limit);
        let mut last_said: Option<std::time::Instant> = None;
        loop {
            std::thread::sleep(EVERY);
            let Some((left, _)) = room() else { continue };
            // Read first, before anything here allocates: the log line
            // below is the first thing that would be refused
            let rescued = crate::reserve::take_spent();
            let turn = gauge.read(left);
            if rescued {
                crate::append_hook_log(&format!(
                    "memory: an allocation was refused and the reserve was spent to keep going ({} left)",
                    size(left)
                ));
            }
            if let Some(turn) = turn {
                crate::append_hook_log(&format!("memory: {turn:?} with {} of commit left", size(left)));
            }
            // Put back once the machine could lose it and still have room
            if !crate::reserve::armed() && left > crate::reserve::SIZE as u64 * 8 && crate::reserve::arm() {
                crate::append_hook_log("memory: the reserve is set aside again");
            }
            let worth_saying = rescued || turn == Some(Turn::Low);
            let quiet = last_said.is_some_and(|t| t.elapsed() < QUIET_FOR);
            if worth_saying && (!quiet || rescued) {
                let who = heaviest();
                if let Some((name, held)) = &who {
                    crate::append_hook_log(&format!("memory: the most is held by {name} ({})", size(*held)));
                }
                let (title, text) = notice(rescued, who);
                last_said = Some(std::time::Instant::now());
                if !say(title, text) {
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Said once on the way down and once on the way back, however many
    /// times the room is looked at in between
    #[test]
    fn each_turn_is_said_once() {
        let mut g = Gauge { low: false, lines: (LOW, LOW * 2) };
        assert_eq!(g.read(10 * GIB), None, "plenty of room says nothing");
        assert_eq!(g.read(LOW - 1), Some(Turn::Low));
        assert_eq!(g.read(LOW - 1), None, "still low is not news");
        assert_eq!(g.read(100), None);
        assert_eq!(g.read(LOW * 2 + 1), Some(Turn::Eased));
        assert_eq!(g.read(LOW * 2 + 1), None);
        assert_eq!(g.read(LOW - 1), Some(Turn::Low), "and a second time is a second turn");
    }

    /// A small machine is not told it is short all day long. The 2 GB line
    /// the first attempt used was crossed before the window of a 4 GB laptop
    /// had even opened
    #[test]
    fn the_line_follows_the_size_of_the_machine() {
        assert_eq!(lines(48 * GIB), (LOW, LOW * 2), "a large machine gets the full line");
        let (low, eased) = lines(6 * GIB);
        assert!(low < GIB, "a 6 GB limit is not short at 1 GB: {low}");
        assert_eq!(eased, low * 2);
    }

    /// Nobody is named when nobody could be found, and the work carrying on
    /// is said either way
    #[test]
    fn the_notice_names_who_holds_the_memory() {
        let (_, text) = notice(false, Some(("python.exe".into(), 17_478_479_872)));
        assert!(text.contains("python.exe") && text.contains("16.3 GB"), "{text}");
        let (_, text) = notice(true, None);
        assert!(!text.is_empty());
    }

    #[test]
    fn sizes_read_the_way_people_read_them() {
        assert_eq!(size(17_478_479_872), "16.3 GB");
        assert_eq!(size(640 * MIB), "640 MB");
    }

    /// This machine answers, and names somebody
    #[cfg(windows)]
    #[test]
    fn the_room_and_the_heaviest_are_read() {
        let (left, limit) = room().expect("GlobalMemoryStatusEx answered nothing");
        assert!(left > 0 && limit >= left);
        let (name, held) = heaviest().expect("no program could be asked about");
        assert!(!name.is_empty() && held > 0);
    }
}

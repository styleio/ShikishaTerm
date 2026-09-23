//! Letting go of the screen before the machine runs out of memory.
//!
//! When Windows runs out of commit -- the memory it has promised to programs,
//! RAM and page file together -- the next program to ask for any is refused,
//! and a Rust program refused an allocation ends on the spot. Nothing chooses
//! which program that is. On 2026-09-22 and 23 it was this one, three times:
//! a python holding 17 GB and a build asking for 4 more left nothing, and the
//! terminal went with every AI at work in it, although the terminal itself
//! was using a few tens of megabytes.
//!
//! What this program can give back quickly is its screen. The board is drawn
//! by a browser engine, and the engine's processes hold hundreds of megabytes
//! that the work in the tabs does not need: the terminals, the agents and the
//! runtime go on the same with nothing drawing them. So when the room left is
//! getting small, the screen is let go of *first*, while there is still room
//! to do it cleanly, and the icon in the notification area says so. Rebuilt
//! only when somebody asks, because rebuilding it takes back what was freed.
//!
//! Waiting for the screen to die of it instead -- which is all `revive` could
//! do -- is waiting for a lottery this program keeps losing.

/// Below this much room, the screen is let go of.
///
/// Enough to still be doing it cleanly: letting go of a page asks the engine
/// for a little memory of its own, and a build can take a gigabyte in a few
/// seconds. Measured against how fast the room went on the afternoons this
/// was written for, two is comfortably ahead of the refusal
pub const LOW: u64 = 2 * GIB;

/// Above this much room, the danger is over and the screen can come back.
///
/// Well clear of [`LOW`], so a screen brought back -- which takes a few
/// hundred megabytes again -- does not by itself push the room back under
/// the line and have it let go of again at once
pub const EASED: u64 = 4 * GIB;

const GIB: u64 = 1024 * 1024 * 1024;

/// How often the room is looked at. Asking costs next to nothing (one call,
/// no allocation), and a build eats a gigabyte in seconds
pub const EVERY: std::time::Duration = std::time::Duration::from_millis(500);

/// What changed about the room, when something did
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// It fell below [`LOW`]: let go of the screen now
    Low,
    /// It came back above [`EASED`] after being low
    Eased,
}

/// The room left, followed over time. Answers only when the answer changes,
/// so the one who asks can act on each answer once
#[derive(Debug)]
pub struct Gauge {
    low: bool,
    /// [`LOW`] and [`EASED`], unless a trial run moved them (see [`lines`])
    lines: (u64, u64),
}

impl Default for Gauge {
    fn default() -> Self {
        Self { low: false, lines: lines() }
    }
}

/// Where the two lines are. `SHIKISHA_MEMORY_LOW_MB` and
/// `SHIKISHA_MEMORY_EASED_MB` move them, for trying this on a machine that
/// has plenty: running a real machine out of memory to see the screen go is
/// the very harm this exists to prevent
pub fn lines() -> (u64, u64) {
    let mb = |name: &str| {
        std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok()).map(|v| v * 1024 * 1024)
    };
    (
        mb("SHIKISHA_MEMORY_LOW_MB").unwrap_or(LOW),
        mb("SHIKISHA_MEMORY_EASED_MB").unwrap_or(EASED),
    )
}

impl Gauge {
    pub fn new() -> Self {
        Self::default()
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

/// The commit still available on this machine, in bytes: what the next
/// program to ask for memory can be given before somebody is refused.
///
/// Commit rather than free RAM, because running out of RAM only makes a
/// machine slow -- it pages -- while running out of commit is what refuses
/// the allocation
#[cfg(windows)]
pub fn room() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut s: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    s.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    (unsafe { GlobalMemoryStatusEx(&mut s) } != 0).then_some(s.ullAvailPageFile)
}

#[cfg(not(windows))]
pub fn room() -> Option<u64> {
    // Elsewhere the kernel overcommits and ends a program of its own choosing
    // instead of refusing it, so there is no line to read ahead of time
    None
}

/// Look at the room for as long as `told` wants to hear about it, on a thread
/// of its own. `told` answers whether to go on: false once whoever it tells
/// has gone
pub fn watch(told: impl Fn(Turn, u64) -> bool + Send + 'static) {
    if room().is_none() {
        return;
    }
    let _ = std::thread::Builder::new().name("shikisha-pressure".into()).spawn(move || {
        let mut gauge = Gauge::new();
        loop {
            std::thread::sleep(EVERY);
            let Some(left) = room() else { continue };
            if let Some(turn) = gauge.read(left) {
                crate::append_hook_log(&format!(
                    "memory: {turn:?} with {} MB of commit left",
                    left / (1024 * 1024)
                ));
                if !told(turn, left) {
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
        let mut g = Gauge::new();
        assert_eq!(g.read(10 * GIB), None, "plenty of room says nothing");
        assert_eq!(g.read(LOW - 1), Some(Turn::Low));
        assert_eq!(g.read(LOW - 1), None, "still low is not news");
        assert_eq!(g.read(100), None);
        assert_eq!(g.read(EASED + 1), Some(Turn::Eased));
        assert_eq!(g.read(EASED + 1), None);
        assert_eq!(g.read(LOW - 1), Some(Turn::Low), "and a second time is a second turn");
    }

    /// The screen coming back takes a few hundred megabytes. Between the two
    /// lines that must not count as the danger returning, or the screen would
    /// be let go of again the moment it was back
    #[test]
    fn coming_back_between_the_lines_is_not_a_new_danger() {
        let mut g = Gauge::new();
        assert_eq!(g.read(LOW - 1), Some(Turn::Low));
        assert_eq!(g.read(3 * GIB), None, "room between the lines is not yet eased");
        assert_eq!(g.read(EASED + 1), Some(Turn::Eased));
        assert_eq!(g.read(3 * GIB), None, "and after easing, the same room is not low");
    }

    /// This machine answers, and in a unit that makes sense
    #[cfg(windows)]
    #[test]
    fn the_room_is_read() {
        let left = room().expect("GlobalMemoryStatusEx answered nothing");
        assert!(left > 0);
    }
}

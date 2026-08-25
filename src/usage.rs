//! What each tab is costing the machine right now.
//!
//! A window running six agents is a window where one of them can quietly pin a
//! core or grow to a gigabyte, and the only tell is a fan that will not stop.
//! "Which one" is a question the machine can already answer; it just never gets
//! asked, because asking it by hand means a task manager and a hunt through a
//! process tree that all say the same program's name.
//!
//! A tab's cost is not the shell we launched — that sits idle. It is the agent
//! and everything the agent started, summed. So this walks the same process
//! tree the ports come from, and adds up the processor time and the memory of
//! the whole subtree under each tab.
//!
//! Processor use is a rate, so it is measured the only way a rate can be: two
//! readings and the time between them. That is what the meter keeps — last
//! time's totals — so that each new reading turns into a percentage rather than
//! a meaningless running sum.

use std::collections::HashMap;
use std::time::Instant;

/// What one tab is costing.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    /// Processor use across the whole subtree, as a percentage of one core's
    /// worth of time. 100 means "a full core"; on an eight-core machine the
    /// ceiling is 800
    pub cpu: u32,
    /// Resident memory of the whole subtree, in bytes
    pub mem: u64,
}

impl Usage {
    /// The short way it reads on the dashboard: `40% · 512MB`.
    ///
    /// Rounded to whole percents and whole units, because a row that flickered
    /// through decimals would draw the eye to noise. Nothing shown until there
    /// is something to show — a tab costing nothing says nothing
    pub fn line(&self) -> Option<String> {
        if self.cpu == 0 && self.mem == 0 {
            return None;
        }
        Some(format!("{}% · {}", self.cpu, human(self.mem)))
    }
}

/// Bytes as a person reads them: MB up to a point, then GB.
fn human(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else {
        format!("{}MB", (bytes + MB / 2) / MB)
    }
}

/// Keeps just enough of last time to turn processor totals into a rate.
pub struct Meter {
    /// Per process, the total processor time it had used as of the last look
    prev: HashMap<u32, u64>,
    /// When that look was
    at: Option<Instant>,
    /// How many cores, so a subtree spread across all of them can be shown
    /// against one core rather than pretending the machine is single-core
    cores: u32,
}

impl Default for Meter {
    fn default() -> Self {
        Meter {
            prev: HashMap::new(),
            at: None,
            cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1),
        }
    }
}

impl Meter {
    /// What each tab is costing, given each tab's own process.
    ///
    /// The first call has nothing to compare against, so it reports memory but
    /// no processor use — a percentage needs two points in time, and inventing
    /// one from a single reading would be a lie the size of the whole subtree
    pub fn sample(&mut self, roots: &[(usize, u32)]) -> HashMap<usize, Usage> {
        let mut out = HashMap::new();
        if roots.is_empty() {
            self.prev.clear();
            self.at = None;
            return out;
        }
        let children = crate::repo::child_map();
        let now = Instant::now();
        let elapsed = self.at.map(|t| now.saturating_duration_since(t).as_secs_f64());

        // Read each live process once, even a subtree that shares one. A pid
        // that appears under two tabs (it should not, but ids get reused) is
        // read once and counted where it is found
        let mut cost: HashMap<u32, (u64, u64)> = HashMap::new();
        let mut fresh: HashMap<u32, u64> = HashMap::new();

        for (key, root) in roots {
            let mut cpu_delta: u64 = 0;
            let mut mem: u64 = 0;
            for pid in crate::repo::descendants(*root, &children) {
                let (ctime, m) = *cost.entry(pid).or_insert_with(|| read(pid));
                mem += m;
                fresh.insert(pid, ctime);
                // The rate is the change since last time. A process we have
                // not seen before contributes memory now and processor use
                // only once there is a gap to measure it over
                if let Some(prev) = self.prev.get(&pid) {
                    cpu_delta += ctime.saturating_sub(*prev);
                }
            }
            let cpu = match elapsed {
                // 100ns ticks over seconds, as a share of one core
                Some(sec) if sec > 0.0 => {
                    ((cpu_delta as f64 / 1e7) / sec * 100.0).round() as u32
                }
                _ => 0,
            };
            out.insert(*key, Usage { cpu: cpu.min(100 * self.cores), mem });
        }

        self.prev = fresh;
        self.at = Some(now);
        out
    }
}

// ── The one thing only the operating system knows ─────────────────

/// One process's total processor time (100ns units) and resident memory.
///
/// A process we cannot open — one that ended between listing and asking, or
/// one owned by another account — reads as nothing rather than as an error:
/// the tree it belonged to still has an honest total from the rest
fn read(pid: u32) -> (u64, u64) {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return (0, 0);
        }
        let mut cpu = 0u64;
        let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) =
            (blank(), blank(), blank(), blank());
        if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0 {
            cpu = ticks(&k) + ticks(&u);
        }
        let mut mem = 0u64;
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(h, &mut pmc, pmc.cb) != 0 {
            mem = pmc.WorkingSetSize as u64;
        }
        CloseHandle(h);
        (cpu, mem)
    }
}

fn blank() -> windows_sys::Win32::Foundation::FILETIME {
    windows_sys::Win32::Foundation::FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 }
}

fn ticks(ft: &windows_sys::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_shown_for_a_tab_that_costs_nothing() {
        assert_eq!(Usage::default().line(), None);
        assert_eq!(Usage { cpu: 40, mem: 512 * 1024 * 1024 }.line().as_deref(), Some("40% · 512MB"));
        assert_eq!(Usage { cpu: 0, mem: 3 * 1024 * 1024 }.line().as_deref(), Some("0% · 3MB"));
    }

    #[test]
    fn big_numbers_turn_into_gigabytes() {
        assert_eq!(human(0), "0MB");
        assert_eq!(human(512 * 1024 * 1024), "512MB");
        assert_eq!(human(1024 * 1024 * 1024), "1.0GB");
        assert_eq!(human(1536 * 1024 * 1024), "1.5GB");
    }

    #[test]
    fn the_first_look_reports_memory_but_not_a_made_up_rate() {
        // A rate needs two points in time. Our own process is a real subtree
        // to measure, and on the first sample its cpu must be zero rather than
        // an invented number, while its memory is real
        let me = std::process::id();
        let mut meter = Meter::default();
        let first = meter.sample(&[(0, me)]);
        let u = first.get(&0).copied().unwrap_or_default();
        assert_eq!(u.cpu, 0, "最初の一回で率をでっち上げない");
        assert!(u.mem > 0, "自分自身のメモリが読めていない: {}", u.mem);
        // A second look has a gap to measure over; cpu is allowed to be
        // anything from zero up, but memory stays real
        std::thread::sleep(std::time::Duration::from_millis(30));
        let second = meter.sample(&[(0, me)]);
        assert!(second.get(&0).is_some_and(|u| u.mem > 0));
    }

    #[test]
    fn no_tabs_forgets_the_last_reading_rather_than_holding_it() {
        let mut meter = Meter::default();
        meter.sample(&[(0, std::process::id())]);
        assert!(meter.at.is_some());
        let empty = meter.sample(&[]);
        assert!(empty.is_empty());
        assert!(meter.at.is_none(), "タブが無くなったら過去も忘れる");
        assert!(meter.prev.is_empty());
    }
}

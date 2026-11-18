//! What the machine and its programs are using, read the way the system's
//! own task manager reads it.
//!
//! "Which program is it" has to be answered for every program, not only the
//! ones this app may open: the ones it may not -- the system itself, the
//! virus scanner, another account's -- are as often the answer. On Windows
//! the system's performance counters say it for all of them, with what each
//! reads and writes and how hard the disk and the paging are working; a
//! process opened by hand says none of that for a protected one, and reads
//! as using nothing.
//!
//! Programs are named by their file name only: never their arguments or
//! where they are, which is where a person's work would show.

use std::time::Duration;

/// One program that is running
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Proc {
    /// Its file name, as the system says it
    pub name: String,
    pub pid: u32,
    pub parent: u32,
    /// The processors it uses, in hundredths of the whole machine
    pub cpu: f32,
    /// The memory it alone holds, in bytes
    pub memory: u64,
    /// The memory it has been promised (its commit), in bytes, where the
    /// system says
    pub commit: u64,
    /// What it reads and writes, in bytes a second: files, pipes and the
    /// network taken together, which is how the system counts it
    pub disk: u64,
    pub threads: u32,
    pub handles: u32,
    /// How long it has been running, in seconds
    pub age: u64,
    /// Whether it is this app, or something this app started
    pub ours: bool,
}

/// How hard the machine is working, over the time between two looks
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    /// The processors in use, in hundredths
    pub cpu: Option<f32>,
    /// The part of that spent in the system itself, in hundredths
    pub kernel: Option<f32>,
    /// Pages of memory read back from the disk, a second: what a machine
    /// short of memory spends its time on
    pub paged: Option<f32>,
    /// How much of the time the disks were busy, in hundredths
    pub busy: Option<f32>,
    /// How many requests were waiting for the disks, on average
    pub queue: Option<f32>,
    /// What the disks read and wrote, in bytes a second
    pub disk: Option<u64>,
}

/// How many processors the machine has, all of them
pub fn cores() -> usize {
    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
    sys.cpus().len().max(1)
}

/// The running copies of this very program: this process, and every other
/// one started from the same file -- the resident process that holds this
/// PC's terminals is one, started on its own and nobody's child. A copy of
/// the app installed somewhere else is another app, and is not counted
#[cfg(windows)]
fn own_copies(procs: &[Proc]) -> std::collections::HashSet<u32> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut own = std::collections::HashSet::from([std::process::id()]);
    let Ok(me) = std::env::current_exe() else { return own };
    let stem = me.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let same = |p: &std::path::Path| p.to_string_lossy().to_lowercase() == me.to_string_lossy().to_lowercase();
    // Only the few that carry this program's name are opened to be asked
    let named: Vec<Pid> = procs.iter().filter(|p| p.name.to_lowercase() == stem).map(|p| Pid::from_u32(p.pid)).collect();
    let mut sys = System::new();
    sys.refresh_processes_specifics(ProcessesToUpdate::Some(&named), true, ProcessRefreshKind::nothing().with_exe(UpdateKind::Always));
    own.extend(named.iter().filter(|pid| sys.process(**pid).and_then(|p| p.exe()).is_some_and(same)).map(|pid| pid.as_u32()));
    own
}

/// Mark this app's own: the copies of this program (`own`, by their
/// numbers) and whatever they started, however far down. A program's parent
/// is known by a number the system hands out again once the parent is gone,
/// so one that is younger than its "child" is somebody else
pub fn mark_ours(procs: &mut [Proc], own: &std::collections::HashSet<u32>) {
    for p in procs.iter_mut() {
        p.ours = own.contains(&p.pid);
    }
    loop {
        let parents: std::collections::HashMap<u32, u64> = procs.iter().filter(|p| p.ours).map(|p| (p.pid, p.age)).collect();
        let mut more = false;
        for p in procs.iter_mut() {
            if !p.ours && p.pid != 0 && parents.get(&p.parent).is_some_and(|age| *age >= p.age) {
                p.ours = true;
                more = true;
            }
        }
        if !more {
            break;
        }
    }
}

#[cfg(windows)]
mod counters {
    use super::{Proc, Rates};
    use windows_sys::Win32::System::Performance::*;

    /// A value is taken as it is, past a hundred too: a program on four
    /// processors uses four hundred hundredths of one
    const AS_NUMBER: u32 = PDH_FMT_DOUBLE | 0x0000_8000;

    pub struct Query(PDH_HQUERY);
    // SAFETY: a query is a handle the system keeps; it is used by one thread at a time
    unsafe impl Send for Query {}

    #[derive(Clone, Copy)]
    pub struct Counter(PDH_HCOUNTER);
    // SAFETY: the same, and it lives as long as its query
    unsafe impl Send for Counter {}

    impl Query {
        pub fn open() -> Option<Query> {
            let mut q: PDH_HQUERY = std::ptr::null_mut();
            // SAFETY: `q` is ours to be written into
            (unsafe { PdhOpenQueryW(std::ptr::null(), 0, &mut q) } == 0).then_some(Query(q))
        }

        /// A counter by its English name, which every Windows knows whatever
        /// language it speaks
        pub fn add(&self, path: &str) -> Option<Counter> {
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let mut c: PDH_HCOUNTER = std::ptr::null_mut();
            // SAFETY: `wide` ends in a zero and outlives the call
            (unsafe { PdhAddEnglishCounterW(self.0, wide.as_ptr(), 0, &mut c) } == 0).then_some(Counter(c))
        }

        pub fn collect(&self) -> bool {
            // SAFETY: the query is open
            unsafe { PdhCollectQueryData(self.0) == 0 }
        }
    }

    impl Drop for Query {
        fn drop(&mut self) {
            // SAFETY: the query is open, and closed once
            unsafe { PdhCloseQuery(self.0) };
        }
    }

    fn good(status: u32) -> bool {
        // PDH_CSTATUS_VALID_DATA and PDH_CSTATUS_NEW_DATA
        status == 0 || status == 1
    }

    pub fn one(c: Option<Counter>) -> Option<f64> {
        let c = c?;
        let mut v = PDH_FMT_COUNTERVALUE::default();
        // SAFETY: `v` is ours to be written into
        let said = unsafe { PdhGetFormattedCounterValue(c.0, AS_NUMBER, std::ptr::null_mut(), &mut v) };
        // SAFETY: a number was asked for, and that is the field filled
        (said == 0 && good(v.CStatus)).then(|| unsafe { v.Anonymous.doubleValue })
    }

    /// Every instance of a counter asked with a `*`, by its name
    pub fn many(c: Option<Counter>) -> Vec<(String, f64)> {
        let Some(c) = c else { return Vec::new() };
        let (mut bytes, mut count) = (0u32, 0u32);
        // SAFETY: asked with no room, it says how much it wants
        let said = unsafe { PdhGetFormattedCounterArrayW(c.0, AS_NUMBER, &mut bytes, &mut count, std::ptr::null_mut()) };
        if said != PDH_MORE_DATA || bytes == 0 {
            return Vec::new();
        }
        // Whole words of eight bytes, so what is written is aligned
        let mut room = vec![0u64; bytes as usize / 8 + 1];
        let items = room.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        // SAFETY: `room` is at least `bytes` long
        if unsafe { PdhGetFormattedCounterArrayW(c.0, AS_NUMBER, &mut bytes, &mut count, items) } != 0 {
            return Vec::new();
        }
        (0..count as usize)
            .map(|i| {
                // SAFETY: the system wrote `count` items, each naming itself inside `room`
                let item = unsafe { &*items.add(i) };
                let name = unsafe {
                    let mut len = 0;
                    while *item.szName.add(len) != 0 {
                        len += 1;
                    }
                    String::from_utf16_lossy(std::slice::from_raw_parts(item.szName, len))
                };
                let value = if good(item.FmtValue.CStatus) { unsafe { item.FmtValue.Anonymous.doubleValue } } else { 0.0 };
                (name, value)
            })
            .collect()
    }

    /// The counters that say how hard the machine is working
    pub struct Machine {
        cpu: Option<Counter>,
        kernel: Option<Counter>,
        paged: Option<Counter>,
        idle: Option<Counter>,
        queue: Option<Counter>,
        bytes: Option<Counter>,
    }

    impl Machine {
        pub fn on(q: &Query) -> Machine {
            Machine {
                cpu: q.add(r"\Processor(_Total)\% Processor Time"),
                kernel: q.add(r"\Processor(_Total)\% Privileged Time"),
                paged: q.add(r"\Memory\Pages Input/sec"),
                idle: q.add(r"\PhysicalDisk(_Total)\% Idle Time"),
                queue: q.add(r"\PhysicalDisk(_Total)\Avg. Disk Queue Length"),
                bytes: q.add(r"\PhysicalDisk(_Total)\Disk Bytes/sec"),
            }
        }

        pub fn read(&self) -> Rates {
            Rates {
                cpu: one(self.cpu).map(|v| v.clamp(0.0, 100.0) as f32),
                kernel: one(self.kernel).map(|v| v.clamp(0.0, 100.0) as f32),
                paged: one(self.paged).map(|v| v.max(0.0) as f32),
                busy: one(self.idle).map(|v| (100.0 - v).clamp(0.0, 100.0) as f32),
                queue: one(self.queue).map(|v| v.max(0.0) as f32),
                disk: one(self.bytes).map(|v| v.max(0.0) as u64),
            }
        }
    }

    /// The counters that say what each program is using
    pub struct Programs {
        /// Whether each process has a name of its own ("chrome:1234"), as
        /// Windows 11 gives them. Before it, the copies of one program are
        /// numbered in the order they stand in ("chrome#3"), and when one of
        /// them ends the others move up: what a number named at the first
        /// look is another process at the second
        apart: bool,
        pid: Option<Counter>,
        parent: Option<Counter>,
        cpu: Option<Counter>,
        memory: Option<Counter>,
        commit: Option<Counter>,
        disk: Option<Counter>,
        threads: Option<Counter>,
        handles: Option<Counter>,
        age: Option<Counter>,
    }

    impl Programs {
        pub fn on(q: &Query) -> Programs {
            let apart = q.add(r"\Process V2(*)\Process ID");
            let set = if apart.is_some() { "Process V2" } else { "Process" };
            let of = |what: &str| q.add(&format!(r"\{set}(*)\{what}"));
            Programs {
                apart: apart.is_some(),
                pid: apart.or_else(|| of("ID Process")),
                parent: of("Creating Process ID"),
                cpu: of("% Processor Time"),
                memory: of("Working Set - Private"),
                commit: of("Private Bytes"),
                disk: of("IO Data Bytes/sec"),
                threads: of("Thread Count"),
                handles: of("Handle Count"),
                age: of("Elapsed Time"),
            }
        }

        /// The processes there are now, by name and number
        pub fn seen(&self) -> Vec<(String, f64)> {
            many(self.pid)
        }

        /// What each program used between two looks; `before` is what
        /// `seen` answered at the first. A process that was not the same
        /// one at both has nothing to be measured against: what it used of
        /// the processors and the disk is left at nothing, not worked out
        /// from another process's count
        pub fn read(&self, cores: f32, before: &[(String, f64)]) -> Vec<Proc> {
            let ids = many(self.pid);
            let known: std::collections::HashSet<(&str, u64)> = before.iter().map(|(n, p)| (n.as_str(), *p as u64)).collect();
            let steady = |i: usize| match self.apart {
                true => known.contains(&(ids[i].0.as_str(), ids[i].1 as u64)),
                false => before.get(i).is_some_and(|was| was.0 == ids[i].0 && was.1 as u64 == ids[i].1 as u64),
            };
            // The same programs in the same order under every counter of one
            // look; one that is not is left at nothing rather than given
            // another program's number
            let beside = |c: Option<Counter>| {
                let all = many(c);
                let same = all.len() == ids.len() && all.iter().zip(&ids).all(|(a, b)| a.0 == b.0);
                move |i: usize| if same { all[i].1 } else { 0.0 }
            };
            let (parent, cpu, memory, commit) = (beside(self.parent), beside(self.cpu), beside(self.memory), beside(self.commit));
            let (disk, threads, handles, age) = (beside(self.disk), beside(self.threads), beside(self.handles), beside(self.age));
            ids.iter()
                .enumerate()
                .map(|(i, (name, pid))| {
                    // Copies of one program are told apart by what comes
                    // after a `:` or a `#`
                    let name = match self.apart {
                        true => name.rsplit_once(':').map_or(name.as_str(), |(n, _)| n),
                        false => name.split('#').next().unwrap_or(name),
                    };
                    (i, name.to_string(), *pid)
                })
                // The sum of them all, and the time nobody used, are not programs
                .filter(|(_, name, _)| !name.eq_ignore_ascii_case("_Total") && !name.eq_ignore_ascii_case("Idle"))
                .map(|(i, name, pid)| Proc {
                    name,
                    pid: pid as u32,
                    parent: parent(i) as u32,
                    cpu: if steady(i) { (cpu(i).max(0.0) as f32) / cores } else { 0.0 },
                    memory: memory(i).max(0.0) as u64,
                    commit: commit(i).max(0.0) as u64,
                    disk: if steady(i) { disk(i).max(0.0) as u64 } else { 0 },
                    threads: threads(i) as u32,
                    handles: handles(i) as u32,
                    age: age(i).max(0.0) as u64,
                    ours: false,
                })
                .collect()
        }
    }
}

/// The machine's rates, read again and again by whoever keeps this: each
/// reading is the average since the one before
pub struct Meter {
    #[cfg(windows)]
    inner: (counters::Query, counters::Machine),
}

impl Meter {
    /// None where the system has no such counters to ask
    pub fn start() -> Option<Meter> {
        #[cfg(windows)]
        {
            let q = counters::Query::open()?;
            let machine = counters::Machine::on(&q);
            q.collect();
            Some(Meter { inner: (q, machine) })
        }
        #[cfg(not(windows))]
        None
    }

    pub fn read(&mut self) -> Rates {
        #[cfg(windows)]
        {
            if !self.inner.0.collect() {
                return Rates::default();
            }
            self.inner.1.read()
        }
        #[cfg(not(windows))]
        Rates::default()
    }
}

/// Every program on the machine and what it used over `over`, with how hard
/// the machine was working meanwhile. Blocks for that long
#[cfg(windows)]
pub fn sample(over: Duration) -> (Rates, Vec<Proc>) {
    let Some(q) = counters::Query::open() else { return Default::default() };
    let (machine, programs) = (counters::Machine::on(&q), counters::Programs::on(&q));
    q.collect();
    let before = programs.seen();
    std::thread::sleep(over);
    if !q.collect() {
        return Default::default();
    }
    let mut procs = programs.read(cores() as f32, &before);
    let own = own_copies(&procs);
    mark_ours(&mut procs, &own);
    (machine.read(), procs)
}

#[cfg(not(windows))]
pub fn sample(over: Duration) -> (Rates, Vec<Proc>) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    // A program can call itself anything here -- a service named after the
    // customer it serves -- so the file it was started from is read instead
    let kind = ProcessRefreshKind::nothing().with_cpu().with_memory().with_disk_usage().with_exe(sysinfo::UpdateKind::OnlyIfNotSet).without_tasks();
    let mut sys = System::new();
    // All the machine's processors, known before the programs are looked at
    sys.refresh_cpu_list(sysinfo::CpuRefreshKind::nothing());
    let cores = sys.cpus().len().max(1) as f32;
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    let over = over.max(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
    std::thread::sleep(over);
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    // The copies of this very program: the ones started from its file
    let me = std::env::current_exe().ok();
    let mut own = std::collections::HashSet::from([std::process::id()]);
    own.extend(sys.processes().values().filter(|p| me.is_some() && p.exe() == me.as_deref()).map(|p| p.pid().as_u32()));
    let mut procs: Vec<Proc> = sys
        .processes()
        .values()
        .map(|p| {
            let name = p.exe().and_then(|e| e.file_name()).map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty());
            let used = p.disk_usage();
            Proc {
                name: name.unwrap_or_else(|| UNNAMED.to_string()),
                pid: p.pid().as_u32(),
                parent: p.parent().map(|x| x.as_u32()).unwrap_or(0),
                cpu: p.cpu_usage() / cores,
                memory: p.memory(),
                commit: 0,
                disk: ((used.read_bytes + used.written_bytes) as f64 / over.as_secs_f64()) as u64,
                threads: 0,
                handles: 0,
                age: p.run_time(),
                ours: false,
            }
        })
        .collect();
    mark_ours(&mut procs, &own);
    (Rates::default(), procs)
}

/// What stands for the programs whose file cannot be read
pub const UNNAMED: &str = "(other programs)";

/// The time each thread of this program has used so far, by the thread's
/// number: its name and its time in milliseconds. Asked twice, the
/// difference says which part of the app is working
pub fn own_threads() -> std::collections::HashMap<u64, (String, u64)> {
    let mut all = std::collections::HashMap::new();
    #[cfg(windows)]
    // SAFETY: every handle opened is closed, every pointer is ours or freed as the system asks
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE, LocalFree};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
        use windows_sys::Win32::System::Threading::*;
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap == INVALID_HANDLE_VALUE {
            return all;
        }
        let me = GetCurrentProcessId();
        let mut t: THREADENTRY32 = std::mem::zeroed();
        t.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snap, &mut t) != 0;
        while more {
            if t.th32OwnerProcessID == me {
                let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, t.th32ThreadID);
                if !h.is_null() {
                    let (mut made, mut ended, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) = std::mem::zeroed();
                    if GetThreadTimes(h, &mut made, &mut ended, &mut kernel, &mut user) != 0 {
                        let of = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
                        let mut name = String::new();
                        let mut said: windows_sys::core::PWSTR = std::ptr::null_mut();
                        if GetThreadDescription(h, &mut said) >= 0 && !said.is_null() {
                            let mut len = 0;
                            while *said.add(len) != 0 {
                                len += 1;
                            }
                            name = String::from_utf16_lossy(std::slice::from_raw_parts(said, len));
                            LocalFree(said as _);
                        }
                        all.insert(t.th32ThreadID as u64, (name, (of(kernel) + of(user)) / 10_000));
                    }
                    CloseHandle(h);
                }
            }
            more = Thread32Next(snap, &mut t) != 0;
        }
        CloseHandle(snap);
    }
    #[cfg(target_os = "linux")]
    if let Ok(tasks) = std::fs::read_dir("/proc/self/task") {
        for task in tasks.flatten() {
            let Ok(id) = task.file_name().to_string_lossy().parse::<u64>() else { continue };
            let name = std::fs::read_to_string(task.path().join("comm")).unwrap_or_default().trim().to_string();
            // The numbers come after the name, which may itself hold spaces
            let Ok(stat) = std::fs::read_to_string(task.path().join("stat")) else { continue };
            let Some(rest) = stat.rsplit_once(") ").map(|(_, r)| r) else { continue };
            let n: Vec<&str> = rest.split(' ').collect();
            let ticks = n.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) + n.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            // A tick is a hundredth of a second on every Linux this runs on
            all.insert(id, (name, ticks * 10));
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, pid: u32, parent: u32, age: u64) -> Proc {
        Proc { name: name.into(), pid, parent, age, ..Default::default() }
    }

    /// The app's own are its copies and whatever they started, and not a
    /// program that only has the number a parent of ours once had
    #[test]
    fn the_apps_own_are_what_it_started_however_far_down() {
        let mut all = vec![
            p("app", 10, 1, 500),
            p("shell", 11, 10, 400),
            p("node", 12, 11, 300),
            p("browser", 20, 1, 900),
            // Older than the program whose number it names as its parent
            p("stranger", 21, 11, 800),
            // The resident process: a copy of the program, and nobody's child
            p("app", 30, 1, 100),
            p("held shell", 31, 30, 90),
            // The same app installed somewhere else, and what it started
            p("app", 40, 1, 700),
            p("its shell", 41, 40, 600),
        ];
        mark_ours(&mut all, &std::collections::HashSet::from([10, 30]));
        let ours: Vec<&str> = all.iter().filter(|p| p.ours).map(|p| p.name.as_str()).collect();
        assert_eq!(ours, ["app", "shell", "node", "app", "held shell"]);
    }

    /// Every program on the machine is seen, this one among them and marked
    /// as the app's own, and something of it is using the processors
    #[test]
    fn a_sample_sees_this_program_and_what_it_uses() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let busy = {
            let stop = stop.clone();
            std::thread::spawn(move || while !stop.load(std::sync::atomic::Ordering::Relaxed) { std::hint::spin_loop() })
        };
        std::thread::sleep(Duration::from_millis(300));
        let (rates, procs) = sample(Duration::from_millis(1200));
        let before = own_threads();
        std::thread::sleep(Duration::from_millis(200));
        let after = own_threads();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = busy.join();
        let me = procs.iter().find(|p| p.pid == std::process::id()).unwrap_or_else(|| panic!("this program is not among {} programs", procs.len()));
        assert!(me.ours, "this program is not marked as the app's own: {me:?}");
        assert!(me.cpu > 0.0 && me.memory > 0, "this program uses nothing: {me:?}");
        assert!(procs.len() > 3, "{procs:?}");
        #[cfg(windows)]
        {
            assert!(rates.cpu.is_some() && rates.busy.is_some() && rates.paged.is_some(), "{rates:?}");
            // The system's own process, which no program may open, is read too
            assert!(procs.iter().any(|p| p.name.eq_ignore_ascii_case("System")), "the system itself is not among them");
            assert!(!procs.iter().any(|p| p.name.contains(':') || p.name.contains('#')), "a program is named with its number");
        }
        let _ = rates;
        #[cfg(any(windows, target_os = "linux"))]
        assert!(
            after.iter().any(|(id, (_, ms))| before.get(id).is_some_and(|(_, was)| ms > was)),
            "no thread of this program used any time: {before:?} {after:?}"
        );
        let _ = (before, after);
    }
}

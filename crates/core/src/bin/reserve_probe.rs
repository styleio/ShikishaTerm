//! Does the reserve keep a program alive when its memory is refused? Asked
//! of a real refusal, made safely, with a rival taking every byte it can.
//!
//!   cargo run -p shikisha-core --bin reserve_probe
//!
//! Running the whole machine out of commit to find out would take everything
//! else on it down too, which is the harm being guarded against. So the probe
//! puts itself in a job object whose processes may hold, between them, only a
//! little more than they already do, and Windows refuses their allocations
//! exactly as it refuses them on a machine that is out -- the same null from
//! the heap -- while nothing else notices.
//!
//! Beside it in the job runs a rival, standing for the build that runs a real
//! machine dry: it takes all the room there is and keeps asking for more, so
//! anything let go of is gone to it. A probe alone in its limit once showed a
//! reserve that was given back to Windows as a rescue; on a real machine a
//! link growing by the gigabyte took it, and the program ended a second later.
//!
//! Once the rival holds everything, the probe asks for memory a megabyte at a
//! time. With the reserve in place the refusals are answered from it and the
//! probe goes on by about the reserve's size; it says so. The refusal the
//! reserve has no room left for ends it the way Rust ends a refused program --
//! which is the point the probe stops at on purpose rather than crashing into.

#[cfg(windows)]
#[global_allocator]
static ALLOC: shikisha_core::reserve::Reserve = shikisha_core::reserve::Reserve;

/// The rival: told to start, it takes what the job has room for, says when it
/// has it all, and goes on asking so that nothing let go of stays free
#[cfg(windows)]
fn rival() {
    use std::io::{BufRead as _, Write as _};
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
    // Said before anything is taken, so the line can still be written once
    // there is no room left to write it
    println!("ready");
    let _ = std::io::stdout().flush();
    let mut go = String::new();
    let _ = std::io::stdin().lock().read_line(&mut go);
    let mut full = false;
    loop {
        let got = unsafe { VirtualAlloc(std::ptr::null(), 64 * 1024, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        if got.is_null() && !full {
            full = true;
            println!("full");
            let _ = std::io::stdout().flush();
        }
        std::hint::spin_loop();
    }
}

#[cfg(windows)]
fn main() {
    use shikisha_core::reserve;
    use std::io::{BufRead as _, Write as _};
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    if std::env::args().nth(1).as_deref() == Some("--rival") {
        return rival();
    }

    const MB: usize = 1024 * 1024;
    let committed = |process| unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(process, &mut c as *mut _ as *mut _, c.cb);
        c.PrivateUsage
    };
    let limit_job = |job, flags, limit| unsafe {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = flags;
        info.JobMemoryLimit = limit;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) != 0
    };

    assert!(reserve::arm(), "no reserve could be set aside");
    // Where the refusal the reserve cannot answer is written down
    let notes = std::env::temp_dir().join(format!("reserve-probe-{}.log", std::process::id()));
    assert!(reserve::write_refusals_to(&notes), "the notes could not be opened");
    // The rival goes with the probe however the probe ends: closing the job
    // ends everything in it
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    assert!(!job.is_null(), "no job object");
    assert!(limit_job(job, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, 0), "the job would not take its first terms");
    assert!(unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) } != 0, "could not join the job");
    // Started from inside the job, so it is in it too
    let mut rival = std::process::Command::new(std::env::current_exe().expect("no path to this program"))
        .arg("--rival")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the rival did not start");
    let mut said = std::io::BufReader::new(rival.stdout.take().expect("no line from the rival"));
    let mut line = String::new();
    let _ = said.read_line(&mut line);
    assert_eq!(line.trim(), "ready", "the rival did not get going");

    let headroom = 200 * MB;
    let rival_holds = committed(rival.as_raw_handle() as _);
    let limit = committed(unsafe { GetCurrentProcess() }) + rival_holds + headroom;
    assert!(
        limit_job(job, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_JOB_MEMORY, limit),
        "the limit was not taken"
    );
    println!(
        "the probe and its rival may hold {} MB between them ({} MB above what they hold, reserve {} MB included)",
        limit / MB,
        headroom / MB,
        reserve::SIZE / MB
    );
    let mut to_rival = rival.stdin.take().expect("no way to tell the rival");
    let _ = writeln!(to_rival, "go");
    let _ = to_rival.flush();
    line.clear();
    let _ = said.read_line(&mut line);
    assert_eq!(line.trim(), "full", "the rival did not take the room");
    println!("the rival holds all the room there is, and is asking for more");

    let mut kept: Vec<Vec<u8>> = Vec::with_capacity(1024);
    let mut rescued_at = None;
    loop {
        // Asked for without the abort, so the probe can say where it stopped
        let mut chunk: Vec<u8> = Vec::new();
        if chunk.try_reserve_exact(MB).is_err() {
            println!("refused again at {} MB with no room left in the reserve: the end of the road", kept.len());
            break;
        }
        chunk.resize(MB, 1);
        kept.push(chunk);
        if rescued_at.is_none() && reserve::take_spent() {
            rescued_at = Some(kept.len() - 1);
            println!("refused at {} MB, and answered from the reserve: rescued", kept.len() - 1);
        }
    }
    let _ = rival.kill();
    let written = std::fs::read_to_string(&notes).unwrap_or_default();
    match written.lines().last() {
        Some(line) => println!("written down as it happened: {line}"),
        None => {
            println!("NOT WRITTEN DOWN: the last refusal left no line in {}", notes.display());
            std::process::exit(1);
        }
    }
    // Half the reserve's worth is the line: a block of the probe's own going
    // on to its next block, or a reserve given away, gets a megabyte or two
    // before the next refusal, and a reserve kept gets nearly all of it
    let went_on = rescued_at.map(|at| kept.len() - at).unwrap_or(0);
    match rescued_at {
        Some(at) if went_on >= reserve::SIZE / MB / 2 => println!(
            "RESCUED: without the reserve this program would have ended at {at} MB; it went on to {} MB \
             while its rival took everything it could",
            kept.len()
        ),
        Some(at) => {
            println!(
                "NOT RESCUED: refused at {at} MB, it went on by only {went_on} MB before being refused again \
                 -- what the reserve was meant to hold went to the rival"
            );
            std::process::exit(1);
        }
        None => {
            println!("NOT RESCUED: the refusal never reached the reserve");
            std::process::exit(1);
        }
    }
}

#[cfg(not(windows))]
fn main() {
    println!("the reserve only matters where allocations are refused (Windows)");
}

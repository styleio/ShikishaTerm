//! Does the reserve keep a program alive when its memory is refused? Asked
//! of a real refusal, made safely.
//!
//!   cargo run -p shikisha-core --bin reserve_probe
//!
//! Running the whole machine out of commit to find out would take everything
//! else on it down too, which is the harm being guarded against. So the probe
//! puts itself in a job object that limits *this process* to a little more
//! than it already holds, and Windows refuses its allocations exactly as it
//! refuses them on a machine that is out -- the same null from the heap --
//! while nothing else notices.
//!
//! It then asks for memory a megabyte at a time until refused. With the
//! reserve in place the first refusal is ridden out and the probe goes on
//! past the limit by about the reserve's size; it says so, and says where it
//! would have ended without it. The second refusal, with nothing left to give
//! back, ends it the way Rust ends a refused program -- which is the point
//! the probe stops at on purpose rather than crashing into.

#[global_allocator]
static ALLOC: shikisha_core::reserve::Reserve = shikisha_core::reserve::Reserve;

#[cfg(windows)]
fn main() {
    use shikisha_core::reserve;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    const MB: usize = 1024 * 1024;
    let committed = || unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb);
        c.PrivateUsage
    };

    assert!(reserve::arm(), "no reserve could be set aside");
    let headroom = 200 * MB;
    let limit = committed() + headroom;
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        assert!(!job.is_null(), "no job object");
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.ProcessMemoryLimit = limit;
        assert!(
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0,
            "the limit was not taken"
        );
        assert!(AssignProcessToJobObject(job, GetCurrentProcess()) != 0, "could not join the job");
    }
    println!(
        "limited to {} MB of commit ({} MB above what is held, reserve {} MB included)",
        limit / MB,
        headroom / MB,
        reserve::SIZE / MB
    );

    let mut kept: Vec<Vec<u8>> = Vec::new();
    let mut rescued_at = None;
    loop {
        // Asked for without the abort, so the probe can say where it stopped
        let mut chunk: Vec<u8> = Vec::new();
        if chunk.try_reserve_exact(MB).is_err() {
            println!(
                "refused again at {} MB with nothing left to give back: the end of the road",
                kept.len()
            );
            break;
        }
        chunk.resize(MB, 1);
        kept.push(chunk);
        if rescued_at.is_none() && reserve::take_spent() {
            rescued_at = Some(kept.len());
            println!(
                "refused at {} MB, reserve given back, and the allocation went through: rescued",
                kept.len()
            );
        }
    }
    match rescued_at {
        Some(at) => println!(
            "RESCUED: without the reserve this program would have ended at {at} MB; it went on to {} MB",
            kept.len()
        ),
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

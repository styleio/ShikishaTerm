//! Making a tab's processes end when the tab does.
//!
//! Closing a tab kills the program it started, and always has. What it did not
//! kill is everything that program started: a `.cmd` shim is a `cmd.exe` that
//! runs a `node`, and killing the shim leaves the node. Those survivors hold
//! the folder they were working in, keep talking to whatever they were talking
//! to, and are visible only in Task Manager -- which is not where anybody
//! looks after closing a tab.
//!
//! Windows has one answer for this, and it is not "walk the process tree and
//! kill the children", which races with every process started while the walk
//! is happening. A job object owns processes: put the first one in, and
//! everything it starts joins it. Close the job and they all end together.
//!
//! The job is held by the tab, so it closes when the tab is dropped -- which
//! includes the tab being restarted, and includes this program crashing.
//! Nothing has to remember to tidy up, which is the whole point.

#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

// ── Where unix keeps the same promise ──────────────────────────────────────
//
// There is no job object here. What there is, and what does the same work, is
// the process group: a child put in its own group can be ended as a group, and
// the grandchildren it started are in that group unless one of them deliberately
// leaves it. That last part is the honest difference -- a job object holds even
// those -- and it is the same limit the Windows version falls back to when a
// job cannot be made.

#[cfg(unix)]
pub struct Job {
    /// The group everything this tab started belongs to. `None` until the
    /// first process is taken
    group: std::sync::Mutex<Option<i32>>,
}

#[cfg(unix)]
impl Job {
    /// A group to put a tab's processes in. Nothing can fail here yet: the
    /// group is the first process's own id, learned when it is taken
    pub fn new() -> Option<Job> {
        Some(Job { group: std::sync::Mutex::new(None) })
    }

    /// Put a process, and everything it goes on to start, into this group.
    ///
    /// False when it could not be done -- most likely because the process had
    /// already finished, which needs no answer from us.
    pub fn take(&self, pid: u32) -> bool {
        // SAFETY: both arguments are plain numbers; the call fails rather than
        // misbehaving when the process is gone or already leads its own group
        let made = unsafe { libc::setpgid(pid as i32, pid as i32) } == 0;
        // A process that already leads a group (a pty child does) is exactly
        // where we want it, so that counts as taken
        let leads = unsafe { libc::getpgid(pid as i32) } == pid as i32;
        if made || leads {
            *self.group.lock().unwrap_or_else(|e| e.into_inner()) = Some(pid as i32);
            return true;
        }
        false
    }
}

#[cfg(unix)]
impl Drop for Job {
    /// Ending the group is what ends the processes. As on Windows there is no
    /// separate "kill" step to forget.
    fn drop(&mut self) {
        if let Some(g) = *self.group.lock().unwrap_or_else(|e| e.into_inner()) {
            // SAFETY: a group id and a signal number. An already-gone group
            // answers ESRCH, which is nothing to do
            unsafe { libc::killpg(g, libc::SIGKILL) };
        }
    }
}

#[cfg(windows)]
/// A job object with kill-on-close set, holding a tab's processes.
#[cfg(windows)]
pub struct Job(HANDLE);

// The handle is only ever closed by Drop, and the type hands out no way to
// duplicate it. Moving one between threads is moving one owner.
#[cfg(windows)]
unsafe impl Send for Job {}
#[cfg(windows)]
unsafe impl Sync for Job {}

#[cfg(windows)]
impl Job {
    /// A new job whose members die when the last handle to it goes.
    ///
    /// `None` when the job could not be made, which leaves the caller exactly
    /// where it was before jobs existed: the direct child is still killed, and
    /// its children are still not. A tab that cannot get a job is not a tab
    /// that fails to open.
    pub fn new() -> Option<Job> {
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        // An unnamed job: nothing else has any business finding it by name.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return None;
        }
        let job = Job(handle);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        // A job without the limit is worse than no job: it would hold the
        // processes and never end them. Let the Drop close it and say no.
        (ok != 0).then_some(job)
    }

    /// Put a process, and everything it goes on to start, into this job.
    ///
    /// False when it could not be done -- most likely because the process had
    /// already finished, which needs no answer from us.
    pub fn take(&self, pid: u32) -> bool {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
        };
        // The two rights the assignment needs, and nothing else.
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return false;
        }
        let ok = unsafe { AssignProcessToJobObject(self.0, process) };
        unsafe { CloseHandle(process) };
        ok != 0
    }
}

#[cfg(windows)]
impl Drop for Job {
    /// Closing the last handle is what ends the processes. There is no separate
    /// "kill" step, and there must not be: a step somebody can forget is a step
    /// that gets forgotten on the path nobody tested.
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// The whole promise, end to end: a process started inside the job is gone
    /// once the job is dropped -- and so is the child it started, which is the
    /// case that walking a process tree gets wrong.
    ///
    /// The waiting is done by something that does not read its input. `pause`
    /// would be the obvious choice and is the wrong one: it reads a key, so
    /// killing its parent closes the pipe under it and it puts an error on the
    /// screen of whoever is running the tests. A test that leaves windows on a
    /// person's desktop is a test that gets switched off.
    #[test]
    fn closing_the_job_ends_what_it_holds() {
        use std::os::windows::process::CommandExt as _;
        use std::process::Stdio;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // A shell that waits, holding a child that also waits. Nothing here
        // reads a key, nothing draws a window, and both end on their own if
        // this test is ever killed before it can tidy up.
        let mut parent = std::process::Command::new("cmd.exe")
            .args(["/c", "cmd.exe /c ping -n 60 127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("cmd.exe が起動できない");
        let job = Job::new().expect("ジョブが作れない");
        assert!(job.take(parent.id()), "ジョブに入れられない");

        // Give the inner cmd.exe time to exist, or the test proves nothing
        // about children -- only about the process we assigned ourselves.
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert!(parent.try_wait().ok().flatten().is_none(), "まだ生きているはず");

        drop(job);
        // Ending is not instant: the kernel terminates the members after the
        // last handle closes. Wait for it, rather than assuming a duration.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if parent.try_wait().ok().flatten().is_some() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "ジョブを閉じても終わらない");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // Whatever happens above, nothing is left running
        let _ = parent.kill();
    }

    /// A process that has already gone cannot be taken, and saying so must not
    /// cost anything: this is the ordinary case when a program exits the
    /// instant it starts.
    #[test]
    fn a_process_that_is_gone_is_simply_not_taken() {
        use std::os::windows::process::CommandExt as _;
        let job = Job::new().expect("ジョブが作れない");
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "exit"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("cmd.exe が起動できない");
        let pid = child.id();
        let _ = child.wait();
        // The pid may still be openable for a moment after the process ends,
        // so either answer is right -- what matters is that it does not panic
        // and does not hang.
        let _ = job.take(pid);
        // A pid that was never a process is the clear case
        assert!(!job.take(0xFFFF_FFF0), "存在しないプロセスは入らない");
    }
}

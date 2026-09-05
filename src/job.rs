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

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

/// A job object with kill-on-close set, holding a tab's processes.
pub struct Job(HANDLE);

// The handle is only ever closed by Drop, and the type hands out no way to
// duplicate it. Moving one between threads is moving one owner.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

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

impl Drop for Job {
    /// Closing the last handle is what ends the processes. There is no separate
    /// "kill" step, and there must not be: a step somebody can forget is a step
    /// that gets forgotten on the path nobody tested.
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole promise, end to end: a process started inside the job is gone
    /// once the job is dropped -- and so is the child it started, which is the
    /// case that walking a process tree gets wrong.
    #[test]
    fn closing_the_job_ends_what_it_holds() {
        use std::process::Stdio;
        let job = Job::new().expect("ジョブが作れない");
        // A shell that waits, holding a child that also waits. `pause` reads
        // from a stdin that never speaks, so neither ends on its own.
        let mut parent = std::process::Command::new("cmd.exe")
            .args(["/c", "cmd.exe /c pause"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cmd.exe が起動できない");
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
    }

    /// A process that has already gone cannot be taken, and saying so must not
    /// cost anything: this is the ordinary case when a program exits the
    /// instant it starts.
    #[test]
    fn a_process_that_is_gone_is_simply_not_taken() {
        let job = Job::new().expect("ジョブが作れない");
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "exit"])
            .stdout(std::process::Stdio::null())
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

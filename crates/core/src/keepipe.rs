//! The resident process's doors, the same to the code on every system.
//!
//! The resident process (`fardaemon`) is reached through two doors, each a
//! name in its own folder: `keep.sock` and `tabs.sock`. On a unix machine that
//! is a socket file. On Windows there are no socket files worth trusting for
//! this, and the system's own way for programs of one account to talk is a
//! named pipe -- so each door's path is turned into a pipe name, the same
//! path always into the same name, and everything above this module goes on
//! speaking of paths.
//!
//! What this module promises on both:
//!
//!   - [`Listener::bind`] succeeds for exactly one process at a time. On unix
//!     the caller decides that under its lock, as before; on Windows the
//!     first instance of the pipe is made with `FILE_FLAG_FIRST_PIPE_INSTANCE`,
//!     so a second process trying finds the name taken and is refused
//!   - nobody else gets in: the pipe's security descriptor names this account
//!     alone, and callers from another machine are turned away
//!     (`PIPE_REJECT_REMOTE_CLIENTS`); [`same_user`] checks the caller again
//!   - a [`Conn`] can be read on one thread while another writes to it. On
//!     Windows that needs overlapped handles: a handle opened for plain
//!     blocking I/O lets one operation through at a time, so a reader parked
//!     waiting for the next line would hold every write up behind it

use std::io;
use std::path::Path;
#[cfg(windows)]
use std::time::Duration;

#[cfg(unix)]
pub use std::os::unix::net::UnixStream as Conn;

/// Connect to the door at `at`
#[cfg(unix)]
pub fn connect(at: &Path) -> io::Result<Conn> {
    Conn::connect(door(at))
}

/// Where a door named `at` really is.
///
/// A socket's name is held in a field of fixed size -- 104 bytes on a Mac
/// and 108 on Linux, the end mark included -- and a folder deep enough does
/// not fit: a long account name under `~/Library/Application Support`, a
/// portable copy several folders down. Such a door is opened instead in a
/// folder of this account's own under `/tmp`, under a name made from the
/// whole path, so the same path always comes to the same door. That folder is
/// used only when it is this account's and nobody else may enter it; a
/// folder of that name someone else made leaves the long name as it is, and
/// the door is refused rather than opened where another account could reach it
#[cfg(unix)]
pub fn door(at: &Path) -> std::path::PathBuf {
    use sha2::Digest as _;
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    const FITS: usize = if cfg!(target_os = "linux") { 107 } else { 103 };
    let long = at.as_os_str().as_bytes();
    if long.len() <= FITS {
        return at.to_path_buf();
    }
    // SAFETY: only reads this process's own user id
    let me = unsafe { libc::getuid() };
    let dir = std::path::PathBuf::from(format!("/tmp/shikisha-{me}"));
    let _ = std::fs::create_dir(&dir);
    let ours = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir() && m.uid() == me);
    if !ours || std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).is_err() {
        return at.to_path_buf();
    }
    let name: String = sha2::Sha256::digest(long).iter().take(12).map(|b| format!("{b:02x}")).collect();
    dir.join(format!("{name}.sock"))
}

/// A pipe's name is made from the whole path, whatever its length
#[cfg(windows)]
pub fn door(at: &Path) -> std::path::PathBuf {
    at.to_path_buf()
}

/// The door at `at`, for one process to answer
#[cfg(unix)]
pub struct Listener(std::os::unix::net::UnixListener);

#[cfg(unix)]
impl Listener {
    pub fn bind(at: &Path) -> io::Result<Self> {
        std::os::unix::net::UnixListener::bind(door(at)).map(Self)
    }

    /// Every caller, as it comes
    pub fn incoming(&self) -> impl Iterator<Item = io::Result<Conn>> + '_ {
        self.0.incoming()
    }
}

/// Whether the program at the other end runs as this account
#[cfg(unix)]
pub fn same_user(conn: &Conn) -> bool {
    use std::os::unix::io::AsRawFd as _;
    let fd = conn.as_raw_fd();
    // SAFETY: each call is given a buffer of the size it says, and the
    // descriptor belongs to a socket that is open for the length of the call
    unsafe {
        #[cfg(target_os = "linux")]
        {
            let mut cred: libc::ucred = std::mem::zeroed();
            let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
            if libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len) != 0 {
                return false;
            }
            cred.uid == libc::getuid()
        }
        #[cfg(not(target_os = "linux"))]
        {
            let (mut uid, mut gid) = (0, 0);
            libc::getpeereid(fd, &mut uid, &mut gid) == 0 && uid == libc::getuid()
        }
    }
}

// ── Windows ────────────────────────────────────────────────────────────────

/// The pipe a door's path stands for. The whole path goes in, so two
/// installations side by side never share a door; hashed, because a pipe
/// name has a length limit a path does not
#[cfg(windows)]
pub fn pipe_name(at: &Path) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(at.to_string_lossy().to_lowercase().as_bytes());
    let sum = h.finalize();
    let hex: String = sum.iter().take(12).map(|b| format!("{b:02x}")).collect();
    let door = at.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    format!(r"\\.\pipe\shikisha-{door}-{hex}")
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, ERROR_PIPE_CONNECTED,
        ERROR_PIPE_NOT_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    /// A handle that closes when the last one holding it lets go
    pub struct Handle(pub HANDLE);
    // A handle is a number the system keeps; every thread may use it
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle was opened by this module and is closed once
            unsafe { CloseHandle(self.0) };
        }
    }

    /// An event for one operation, closed with it
    struct Event(HANDLE);
    impl Event {
        fn new() -> io::Result<Self> {
            // SAFETY: a manual-reset event, unnamed, with default security
            let e = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
            if e.is_null() { Err(io::Error::last_os_error()) } else { Ok(Self(e)) }
        }
    }
    impl Drop for Event {
        fn drop(&mut self) {
            // SAFETY: made above, closed once
            unsafe { CloseHandle(self.0) };
        }
    }

    /// One overlapped operation started by `start`, waited for here, and
    /// given up on when `wait` runs out (cancelled and waited out, so the
    /// buffer it was writing into is no longer in use when this returns)
    pub fn run(
        h: HANDLE,
        wait: Option<Duration>,
        start: impl FnOnce(*mut OVERLAPPED) -> i32,
    ) -> io::Result<u32> {
        let ev = Event::new()?;
        // SAFETY: an all-zero OVERLAPPED is the documented starting point
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = ev.0;
        let ok = start(&mut ov);
        if ok == 0 {
            // SAFETY: reads this thread's last error
            let err = unsafe { GetLastError() };
            if err != ERROR_IO_PENDING {
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            let ms = wait.map_or(u32::MAX, |d| u32::try_from(d.as_millis()).unwrap_or(u32::MAX - 1));
            // SAFETY: the event belongs to this operation
            let got = unsafe { WaitForSingleObject(ev.0, ms) };
            if got == WAIT_TIMEOUT {
                // SAFETY: cancels this operation only, then waits for it to
                // finish being cancelled so `ov` and the buffer are free
                unsafe {
                    CancelIoEx(h, &ov);
                    let mut n = 0u32;
                    GetOverlappedResult(h, &ov, &mut n, 1);
                }
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
            }
            if got != WAIT_OBJECT_0 {
                return Err(io::Error::last_os_error());
            }
        }
        let mut n = 0u32;
        // SAFETY: the operation has finished; this only reads its result
        if unsafe { GetOverlappedResult(h, &ov, &mut n, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n)
    }

    /// Whether an error means the other end is gone (read as the end)
    pub fn is_end(e: &io::Error) -> bool {
        matches!(
            e.raw_os_error().map(|c| c as u32),
            Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_OPERATION_ABORTED)
        )
    }

    /// One end of a connected pipe
    pub struct Conn {
        pub h: Arc<Handle>,
        pub read_wait: std::sync::Mutex<Option<Duration>>,
        /// Shut from this side: reads end and writes fail from now on
        pub shut: Arc<AtomicBool>,
    }

    impl Conn {
        pub fn new(h: HANDLE) -> Self {
            Self { h: Arc::new(Handle(h)), read_wait: std::sync::Mutex::new(None), shut: Arc::new(AtomicBool::new(false)) }
        }

        pub fn read_into(&self, buf: &mut [u8]) -> io::Result<usize> {
            use windows_sys::Win32::Storage::FileSystem::ReadFile;
            if self.shut.load(Ordering::SeqCst) || buf.is_empty() {
                return Ok(0);
            }
            let wait = *self.read_wait.lock().unwrap_or_else(|e| e.into_inner());
            let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
            let h = self.h.0;
            // SAFETY: the buffer outlives the operation (`run` waits it out)
            match run(h, wait, |ov| unsafe { ReadFile(h, buf.as_mut_ptr(), len, std::ptr::null_mut(), ov) }) {
                Ok(n) => Ok(n as usize),
                Err(e) if is_end(&e) => Ok(0),
                Err(e) => Err(e),
            }
        }

        pub fn write_from(&self, buf: &[u8]) -> io::Result<usize> {
            use windows_sys::Win32::Storage::FileSystem::WriteFile;
            if self.shut.load(Ordering::SeqCst) {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            let len = u32::try_from(buf.len()).unwrap_or(u32::MAX);
            let h = self.h.0;
            // SAFETY: the buffer outlives the operation (`run` waits it out)
            run(h, None, |ov| unsafe { WriteFile(h, buf.as_ptr(), len, std::ptr::null_mut(), ov) }).map(|n| n as usize)
        }

        /// Shut the connection: what is waiting on it ends, on every thread
        pub fn shut(&self) {
            self.shut.store(true, Ordering::SeqCst);
            // SAFETY: cancels every operation on this handle, from any thread;
            // the handle stays open until the last holder lets it go
            unsafe { CancelIoEx(self.h.0, std::ptr::null()) };
        }
    }

    pub fn invalid(h: HANDLE) -> bool {
        h.is_null() || h == INVALID_HANDLE_VALUE
    }

    pub const PIPE_CONNECTED: u32 = ERROR_PIPE_CONNECTED;
}

/// One end of a door, connected
#[cfg(windows)]
pub struct Conn(win::Conn);

#[cfg(windows)]
impl Conn {
    /// A second view of the same connection, for another thread
    pub fn try_clone(&self) -> io::Result<Conn> {
        Ok(Conn(win::Conn {
            h: std::sync::Arc::clone(&self.0.h),
            read_wait: std::sync::Mutex::new(*self.0.read_wait.lock().unwrap_or_else(|e| e.into_inner())),
            shut: std::sync::Arc::clone(&self.0.shut),
        }))
    }

    /// How long a read may wait before it gives up (`None`: for as long as it takes)
    pub fn set_read_timeout(&self, wait: Option<Duration>) -> io::Result<()> {
        *self.0.read_wait.lock().unwrap_or_else(|e| e.into_inner()) = wait;
        Ok(())
    }

    /// Shut it. A pipe has no half-closed state worth relying on, so either
    /// half shuts the whole: a line the app stops writing to has nothing more
    /// to read either, as far as these doors go
    pub fn shutdown(&self, _how: std::net::Shutdown) -> io::Result<()> {
        self.0.shut();
        Ok(())
    }
}

#[cfg(windows)]
impl io::Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read_into(buf)
    }
}

#[cfg(windows)]
impl io::Read for &Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read_into(buf)
    }
}

#[cfg(windows)]
impl io::Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_from(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(windows)]
impl io::Write for &Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_from(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Connect to the door at `at`. Busy for a moment (every instance taken) is
/// waited out; nobody there is `NotFound`
#[cfg(windows)]
pub fn connect(at: &Path) -> io::Result<Conn> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, GetLastError};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    };
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;
    let name = pipe_name(at);
    let wide: Vec<u16> = format!("{name}\0").encode_utf16().collect();
    for _ in 0..10 {
        // SAFETY: a plain open of a pipe by name; the identity it may use on
        // our behalf is limited to identifying us (SECURITY_IDENTIFICATION)
        let h = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                std::ptr::null_mut(),
            )
        };
        if !win::invalid(h) {
            return Ok(Conn(win::Conn::new(h)));
        }
        // SAFETY: reads this thread's last error
        match unsafe { GetLastError() } {
            ERROR_FILE_NOT_FOUND => return Err(io::Error::from(io::ErrorKind::NotFound)),
            // SAFETY: waits for an instance of this name to be free
            ERROR_PIPE_BUSY => unsafe {
                WaitNamedPipeW(wide.as_ptr(), 2000);
            },
            e => return Err(io::Error::from_raw_os_error(e as i32)),
        }
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "every instance of the door stayed busy"))
}

/// The door at `at`, answered by this process alone
#[cfg(windows)]
pub struct Listener {
    name: Vec<u16>,
    sd: crate::api::SecurityDescriptor,
    /// The instance waiting for the next caller
    next: std::sync::Mutex<Option<windows_sys::Win32::Foundation::HANDLE>>,
}

// The handle and the descriptor are only touched under the lock or by the
// one thread that accepts
#[cfg(windows)]
unsafe impl Send for Listener {}
#[cfg(windows)]
unsafe impl Sync for Listener {}

#[cfg(windows)]
impl Listener {
    fn instance(name: &[u16], sd: &crate::api::SecurityDescriptor, first: bool) -> io::Result<windows_sys::Win32::Foundation::HANDLE> {
        use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX};
        use windows_sys::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
        };
        let sa = sd.attributes();
        let mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        // SAFETY: the name is NUL-terminated and the attributes outlive the call
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                &sa,
            )
        };
        if win::invalid(h) { Err(io::Error::last_os_error()) } else { Ok(h) }
    }

    /// Take the door. Refused when another process has it
    pub fn bind(at: &Path) -> io::Result<Self> {
        let name: Vec<u16> = format!("{}\0", pipe_name(at)).encode_utf16().collect();
        let sd = crate::api::SecurityDescriptor::only_me()
            .ok_or_else(|| io::Error::other("could not describe who may use the door"))?;
        let first = Self::instance(&name, &sd, true)?;
        Ok(Self { name, sd, next: std::sync::Mutex::new(Some(first)) })
    }

    fn accept(&self) -> io::Result<Conn> {
        use windows_sys::Win32::System::Pipes::ConnectNamedPipe;
        let h = match self.next.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(h) => h,
            None => Self::instance(&self.name, &self.sd, false)?,
        };
        let conn = win::Conn::new(h);
        // SAFETY: an overlapped wait for a caller on an instance of our own
        let joined = win::run(h, None, |ov| unsafe { ConnectNamedPipe(h, ov) });
        // The next instance is made at once, so a caller is never told the
        // door is not there while this one is being served
        if let Ok(next) = Self::instance(&self.name, &self.sd, false) {
            *self.next.lock().unwrap_or_else(|e| e.into_inner()) = Some(next);
        }
        match joined {
            Ok(_) => Ok(Conn(conn)),
            Err(e) if e.raw_os_error() == Some(win::PIPE_CONNECTED as i32) => Ok(Conn(conn)),
            Err(e) => Err(e),
        }
    }

    /// Every caller, as it comes
    pub fn incoming(&self) -> impl Iterator<Item = io::Result<Conn>> + '_ {
        std::iter::from_fn(move || Some(self.accept()))
    }
}

#[cfg(windows)]
impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(h) = self.next.lock().unwrap_or_else(|e| e.into_inner()).take() {
            // SAFETY: the waiting instance is ours and closed once
            unsafe { windows_sys::Win32::Foundation::CloseHandle(h) };
        }
    }
}

/// Whether the program at the other end runs as this account. The door's
/// security descriptor already says so; this asks the caller's own process,
/// so a mistake in one is not the only thing in the way
#[cfg(windows)]
pub fn same_user(conn: &Conn) -> bool {
    use windows_sys::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeServerProcessId};
    let mut pid = 0u32;
    // SAFETY: reads which process is at the other end of our own pipe
    let ok = unsafe { GetNamedPipeClientProcessId(conn.0.h.0, &mut pid) } != 0
        // The caller's side asks about the server instead
        || unsafe { GetNamedPipeServerProcessId(conn.0.h.0, &mut pid) } != 0;
    ok && crate::api::process_user_sid(pid).is_some_and(|theirs| crate::api::current_user_sid().is_some_and(|mine| mine == theirs))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::io::{BufRead as _, BufReader, Write as _};

    /// One process takes a door; a second asking for the same door is refused,
    /// and a caller is let in, read on one thread while written on another
    #[test]
    fn a_door_is_one_processs_and_reads_and_writes_go_both_ways_at_once() {
        let at = std::env::temp_dir().join(format!("shikisha-keepipe-{}-{}", std::process::id(), crate::random_hex(4))).join("keep.sock");
        let door = Listener::bind(&at).unwrap();
        assert!(Listener::bind(&at).is_err(), "the second taker is refused");
        let server = std::thread::spawn(move || {
            let conn = door.incoming().next().unwrap().unwrap();
            assert!(same_user(&conn));
            let mut lines = BufReader::new(conn.try_clone().unwrap()).lines();
            // Written while the other thread is parked reading: the write
            // must not wait behind the read
            (&conn).write_all(b"hello\n").unwrap();
            let got = lines.next().unwrap().unwrap();
            (&conn).write_all(format!("you said {got}\n").as_bytes()).unwrap();
            got
        });
        let conn = connect(&at).unwrap();
        let mut lines = BufReader::new(conn.try_clone().unwrap()).lines();
        assert_eq!(lines.next().unwrap().unwrap(), "hello");
        (&conn).write_all(b"ping\n").unwrap();
        assert_eq!(lines.next().unwrap().unwrap(), "you said ping");
        assert_eq!(server.join().unwrap(), "ping");
        assert!(lines.next().is_none(), "the other end gone is the end");
    }

    /// A read with a time limit gives up, and the connection stays usable
    #[test]
    fn a_read_with_a_limit_gives_up_and_the_door_stays_usable() {
        let at = std::env::temp_dir().join(format!("shikisha-keepipe-{}-{}", std::process::id(), crate::random_hex(4))).join("keep.sock");
        let door = Listener::bind(&at).unwrap();
        let server = std::thread::spawn(move || {
            let conn = door.incoming().next().unwrap().unwrap();
            std::thread::sleep(Duration::from_millis(400));
            (&conn).write_all(b"late\n").unwrap();
            std::thread::sleep(Duration::from_millis(200));
        });
        let conn = connect(&at).unwrap();
        conn.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let mut buf = [0u8; 8];
        let err = (&conn).read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        conn.set_read_timeout(None).unwrap();
        let mut line = String::new();
        BufReader::new(conn.try_clone().unwrap()).read_line(&mut line).unwrap();
        assert_eq!(line, "late\n");
        server.join().unwrap();
    }

    /// Nobody at a door is told as such, not waited on
    #[test]
    fn nobody_at_a_door_is_not_found() {
        let at = std::env::temp_dir().join("shikisha-keepipe-nobody").join(format!("{}.sock", crate::random_hex(6)));
        assert_eq!(connect(&at).err().map(|e| e.kind()), Some(io::ErrorKind::NotFound));
    }

    use std::io::Read as _;
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;
    use std::io::{BufRead as _, BufReader, Write as _};

    /// A door in a folder too deep for a socket's name still opens, and is
    /// found again by the same long name; a short name is left as it is
    #[test]
    fn a_door_too_deep_for_its_name_still_opens() {
        let base = crate::test_temp("keepipe-deep");
        let deep = base.join("a-folder-name-long-enough".repeat(5)).join("run");
        std::fs::create_dir_all(&deep).unwrap();
        let at = deep.join("keep.sock");
        assert!(at.as_os_str().len() > 108, "the test's folder is not deep enough: {}", at.display());
        let short = base.join("s.sock");
        assert_eq!(door(&short), short, "a name that fits was moved");
        assert_eq!(door(&at), door(&at), "one path came to two doors");
        assert!(door(&at).as_os_str().len() <= 103, "{}", door(&at).display());

        let _ = std::fs::remove_file(door(&at));
        let listener = Listener::bind(&at).expect("the deep door did not open");
        let answering = std::thread::spawn(move || {
            let mut conn = listener.incoming().next().unwrap().unwrap();
            conn.write_all(b"here\n").unwrap();
        });
        let conn = connect(&at).expect("the deep door was not found by its long name");
        let mut said = String::new();
        BufReader::new(conn).read_line(&mut said).unwrap();
        assert_eq!(said, "here\n");
        answering.join().unwrap();
        let _ = std::fs::remove_file(door(&at));
        let _ = std::fs::remove_dir_all(&base);
    }
}

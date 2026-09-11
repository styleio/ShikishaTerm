//! Getting the master password to a runtime with no window.
//!
//! The secrets belong to the machine the agents run on, because a login on a
//! laptop does not carry to a server. That settles where they live; it leaves
//! open how the key to them arrives, and a server has no dialog to put up.
//!
//! Three ways, tried in this order, and the order is the point.
//!
//! 1. **A credential the service manager handed over.** systemd's
//!    `LoadCredential=` passes a file the service can read and nothing else on
//!    the machine can: not another program of the same user, not the process
//!    list, not the unit file. This is the only one that works when nobody is
//!    there, which is the case a server spends its life in.
//! 2. **A person at the terminal.** Started by hand over SSH, the runtime asks,
//!    with the echo turned off. This is the gesture somebody expects when they
//!    run a program that needs a password.
//! 3. **Nothing.** No credential, no terminal: the runtime comes up with the
//!    secrets still locked and says so plainly, naming what to do about it. It
//!    does not guess, and it does not read the password out of the
//!    environment -- an environment block is readable by every other process
//!    that user owns, which is exactly the property a master password must not
//!    have.

use std::io::BufRead as _;
use std::io::Write as _;

/// The password, if this machine can supply one without being asked twice.
///
/// `None` means "there is no way to get it here", which the caller turns into a
/// runtime that runs with its secrets locked. It never means "wrong password" --
/// that judgement belongs to whoever can try to decrypt with it.
pub fn master(title: &str, note: &str) -> Option<String> {
    if let Some(pw) = from_service_credential() {
        return Some(pw);
    }
    if let Some(pw) = from_terminal(title, note) {
        return Some(pw);
    }
    // Only this function knows that every way was tried, so only this function
    // can say what is left to do. The runtime's own note says the secrets are
    // locked; it cannot say how to unlock them on a machine it knows nothing
    // about
    crate::append_hook_log(&crate::i18n::t("prompt.password.headless"));
    None
}

/// What systemd (or anything following the same convention) left for us.
///
/// `LoadCredential=shikisha-master:/path/to/file` in the unit, and the file
/// arrives in a directory only this service can read. The trailing newline a
/// text editor leaves is not part of the password.
fn from_service_credential() -> Option<String> {
    let dir = std::env::var_os("CREDENTIALS_DIRECTORY")?;
    let file = std::path::Path::new(&dir).join("shikisha-master");
    let text = std::fs::read_to_string(file).ok()?;
    let pw = text.trim_end_matches(['\n', '\r']).to_string();
    (!pw.is_empty()).then_some(pw)
}

/// Ask the person who started this, if there is one.
fn from_terminal(title: &str, note: &str) -> Option<String> {
    if !attended() {
        return None;
    }
    // The prompt goes to the error stream on purpose: whoever is piping this
    // program's output into something wants the output, not the question
    let mut err = std::io::stderr();
    let _ = writeln!(err, "{title} — {note}");
    let _ = write!(err, "> ");
    let _ = err.flush();

    let quiet = Hush::on();
    let mut line = String::new();
    let read = std::io::stdin().lock().read_line(&mut line);
    drop(quiet);
    // The newline the person pressed was swallowed with the echo, so the next
    // thing printed would otherwise land on the prompt's line
    let _ = writeln!(err);

    if read.ok()? == 0 {
        return None; // the input ended -- nobody is there after all
    }
    let pw = line.trim_end_matches(['\n', '\r']).to_string();
    (!pw.is_empty()).then_some(pw)
}

/// Whether somebody is sitting in front of this.
///
/// Asked of the input, not the output: a service started by systemd has both
/// redirected, and a person running this over SSH has both on a terminal. The
/// question being answered is "can anybody type", which is the input's.
#[cfg(unix)]
fn attended() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

#[cfg(windows)]
fn attended() -> bool {
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE};
    let mut mode = 0u32;
    unsafe { GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode) != 0 }
}

/// Turns the echo off while a password is being typed, and puts it back
/// however this ends -- including the ways that are not returning.
struct Hush(Option<Mode>);

#[cfg(unix)]
type Mode = libc::termios;
#[cfg(windows)]
type Mode = u32;

#[cfg(unix)]
impl Hush {
    fn on() -> Self {
        unsafe {
            let mut was: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut was) != 0 {
                return Self(None);
            }
            let mut quiet = was;
            quiet.c_lflag &= !libc::ECHO;
            // TCSAFLUSH rather than TCSANOW: anything typed ahead of the
            // prompt is thrown away instead of being taken as the password
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &quiet) != 0 {
                return Self(None);
            }
            Self(Some(was))
        }
    }
}

#[cfg(unix)]
impl Drop for Hush {
    fn drop(&mut self) {
        if let Some(was) = self.0 {
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &was);
            }
        }
    }
}

#[cfg(windows)]
impl Hush {
    fn on() -> Self {
        use windows_sys::Win32::System::Console::{
            ENABLE_ECHO_INPUT, GetConsoleMode, GetStdHandle, SetConsoleMode, STD_INPUT_HANDLE,
        };
        unsafe {
            let h = GetStdHandle(STD_INPUT_HANDLE);
            let mut was = 0u32;
            if GetConsoleMode(h, &mut was) == 0 {
                return Self(None);
            }
            if SetConsoleMode(h, was & !ENABLE_ECHO_INPUT) == 0 {
                return Self(None);
            }
            Self(Some(was))
        }
    }
}

#[cfg(windows)]
impl Drop for Hush {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Console::{GetStdHandle, SetConsoleMode, STD_INPUT_HANDLE};
        if let Some(was) = self.0 {
            unsafe {
                SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), was);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests share one environment, and this one changes it
    static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// What a service manager leaves is taken as it is, minus the newline a
    /// text editor adds. A password with spaces in it keeps them -- trimming
    /// both ends would quietly change somebody's password into a different one
    #[test]
    fn a_credential_is_read_exactly_as_written() {
        let _one = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("shikisha-cred-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shikisha-master"), "  two words \n").unwrap();
        unsafe { std::env::set_var("CREDENTIALS_DIRECTORY", &dir) };

        assert_eq!(from_service_credential().as_deref(), Some("  two words "));

        unsafe { std::env::remove_var("CREDENTIALS_DIRECTORY") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An empty file is nothing, not an empty password. Somebody who meant to
    /// write a password and wrote nothing should get "the secrets are locked",
    /// not a runtime trying to open them with ""
    #[test]
    fn an_empty_credential_is_no_credential() {
        let _one = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("shikisha-cred-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("shikisha-master"), "\n").unwrap();
        unsafe { std::env::set_var("CREDENTIALS_DIRECTORY", &dir) };

        assert!(from_service_credential().is_none());

        unsafe { std::env::remove_var("CREDENTIALS_DIRECTORY") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// No credential directory at all is the ordinary case on a machine that
    /// is not running this as a service
    #[test]
    fn no_credential_directory_is_no_credential() {
        let _one = ENV.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::env::remove_var("CREDENTIALS_DIRECTORY") };
        assert!(from_service_credential().is_none());
    }

    /// A test run has nothing on its input, so this must not sit waiting for
    /// somebody to type. It is the property that keeps a service from hanging
    /// at boot, which is the failure nobody would see until the box was
    /// rebooted at three in the morning
    #[test]
    fn nobody_at_the_terminal_means_no_password_rather_than_a_wait() {
        assert!(from_terminal("t", "n").is_none());
    }
}

//! The PATH a person's own terminal has, for a program a Mac started.
//!
//! A program a Mac starts from the Finder, the Dock or launchd is not started
//! by a shell, and is given almost no PATH: `/usr/bin:/bin:/usr/sbin:/sbin`.
//! Everything a person installs lives elsewhere -- Homebrew's `/opt/homebrew/bin`,
//! `~/.local/bin`, a Node version manager's folder -- so `claude`, `node`,
//! `gh` and `git` from Homebrew were "not installed", and a tab started from
//! here could not find them either. The person's shell knows where they are:
//! it is asked once, at the start, as their terminal would set it up, and
//! what it says goes first.
//!
//! Linux desktops start programs from a session that already read the
//! person's profile, and Windows keeps PATH in the registry, so this only
//! does anything on a Mac.

/// What the login shell's PATH is wrapped in, so whatever its startup files
/// print around it is left out
#[cfg(any(target_os = "macos", test))]
const MARK: &str = "__SHIKISHA_PATH__";

/// Take the PATH the person's login shell sets up, ahead of the one this
/// process was given. Called first thing, before any thread is started:
/// changing the environment is only safe while nothing else can be reading it
pub fn adopt() {
    #[cfg(target_os = "macos")]
    if let Some(theirs) = from_login_shell() {
        let ours = std::env::var_os("PATH").unwrap_or_default();
        if let Some(merged) = merge(&theirs, &ours) {
            // SAFETY: called at the start of main; the one other thread there
            // has been (the shell's reader above) never reads the environment
            unsafe { std::env::set_var("PATH", merged) };
        }
    }
}

/// The PATH the person's shell sets up when it starts as a login shell, the
/// way their terminal starts it. A shell that has not answered in a few
/// seconds (a startup file that waits for something) is let go: the PATH this
/// process was given still works for what is in it
#[cfg(target_os = "macos")]
fn from_login_shell() -> Option<std::ffi::OsString> {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    let shell = crate::config::machine_shell().1;
    // fish keeps PATH as a list, and a quoted list comes out with spaces in it
    let say = match shell.ends_with("/fish") {
        true => format!("printf '%s%s%s' {MARK} (string join : $PATH) {MARK}"),
        false => format!("printf '%s%s%s' {MARK} \"$PATH\" {MARK}"),
    };
    let mut child = Command::new(&shell)
        .args(["-l", "-i", "-c", &say])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = out.read_to_string(&mut text);
        let _ = tx.send(text);
    });
    let said = rx.recv_timeout(std::time::Duration::from_secs(5));
    let _ = child.kill();
    let _ = child.wait();
    marked(&said.ok()?).map(std::ffi::OsString::from)
}

/// What sits between the two marks
#[cfg(any(target_os = "macos", test))]
fn marked(text: &str) -> Option<&str> {
    let (_, rest) = text.split_once(MARK)?;
    let (path, _) = rest.split_once(MARK)?;
    (!path.trim().is_empty()).then_some(path)
}

/// The shell's PATH first, then whatever of this process's own it lacks
#[cfg(any(target_os = "macos", test))]
fn merge(theirs: &std::ffi::OsStr, ours: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
    let mut all: Vec<std::path::PathBuf> = Vec::new();
    for dir in std::env::split_paths(theirs).chain(std::env::split_paths(ours)) {
        if !dir.as_os_str().is_empty() && !all.contains(&dir) {
            all.push(dir);
        }
    }
    std::env::join_paths(all).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only what the shell printed between the marks is taken: a greeting
    /// a startup file prints is not part of the PATH
    #[test]
    fn the_path_is_read_from_between_the_marks() {
        let said = format!("Welcome!\n{MARK}/opt/homebrew/bin:/usr/bin{MARK}\nbye");
        assert_eq!(marked(&said), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(marked("no marks at all"), None);
        assert_eq!(marked(&format!("{MARK}  {MARK}")), None, "an empty PATH is no answer");
    }

    /// The shell's own order wins, and nothing of this process's is lost
    #[test]
    fn the_shells_path_goes_first_and_ours_follows() {
        let dirs = |v: &[&str]| std::env::join_paths(v).unwrap();
        let (a, b, c) = (crate::local_path(r"C:\a"), crate::local_path(r"C:\b"), crate::local_path(r"C:\c"));
        let merged = merge(&dirs(&[&a, &b]), &dirs(&[&b, &c])).unwrap();
        let got: Vec<String> = std::env::split_paths(&merged).map(|p| p.display().to_string()).collect();
        assert_eq!(got, vec![a, b, c]);
    }
}

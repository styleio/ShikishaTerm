//! A project that is not on this PC yet: cloned from a URL, or made new.
//!
//! Both end the same way the folder picker does -- a git repository added to
//! the desk -- so what follows (its first worktree) is the same whichever door
//! somebody came in by. What this module owns is only the getting there: the
//! folder, and git, with nobody at a prompt to answer anything.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where a project that is cloned or made new goes, until somebody says
/// otherwise: beside the worktrees, in a folder named for what it holds
pub fn projects_root() -> PathBuf {
    let home = ["USERPROFILE", "HOME"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|h| PathBuf::from(h.trim()))
        .find(|h| h.is_dir());
    match home {
        Some(h) => h.join("SHIKISHA-TERM").join("projects"),
        None => std::env::temp_dir().join("SHIKISHA-TERM").join("projects"),
    }
}

/// The folder name a clone of `url` gets: the last part of its path, without
/// `.git`. None for something that does not end in a name
pub fn repo_name_of(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches(['/', '\\']);
    // An address that is only a server has no repository in it to name
    if let Some((_, rest)) = trimmed.split_once("://")
        && !rest.contains('/')
    {
        return None;
    }
    let last = trimmed.rsplit(['/', '\\', ':']).next()?;
    let name = last.strip_suffix(".git").unwrap_or(last).trim();
    valid_name(name).then(|| name.to_string())
}

/// A name a folder can have on every system this runs on: no separators, no
/// characters Windows refuses, not `.` or `..`, not one of the names Windows
/// reserves for devices
pub fn valid_name(name: &str) -> bool {
    let n = name.trim();
    if n.is_empty() || n == "." || n == ".." || n.ends_with('.') || n.ends_with(' ') {
        return false;
    }
    if n.chars().any(|c| c.is_control() || "<>:\"/\\|?*".contains(c)) {
        return false;
    }
    let stem = n.split('.').next().unwrap_or(n).to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"];
    let numbered = ["COM", "LPT"];
    !(reserved.contains(&stem.as_str())
        || numbered.iter().any(|p| stem.len() == 4 && stem.starts_with(p) && stem.as_bytes()[3].is_ascii_digit()))
}

/// Where a clone or a new project would land, or why it cannot
pub fn target(parent: &str, name: &str) -> Result<PathBuf, String> {
    let parent = parent.trim();
    if parent.is_empty() {
        return Err(crate::i18n::t("err.addproj.no_parent"));
    }
    if !valid_name(name) {
        return Err(crate::i18n::t("err.addproj.bad_name"));
    }
    let at = Path::new(parent).join(name.trim());
    if at.exists() {
        return Err(crate::i18n::tp("err.addproj.exists", &[("path", &at.display().to_string())]));
    }
    Ok(at)
}

/// How far a clone has got, as git says it
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    /// git's own word for the stage it is at (`Receiving objects`)
    pub phase: String,
    pub percent: Option<u8>,
}

/// One line of what `git clone --progress` writes, read for its stage and its
/// percentage. Lines that say neither are not progress
pub fn progress_of(line: &str) -> Option<Progress> {
    let line = line.trim().trim_start_matches("remote:").trim();
    let (phase, rest) = line.split_once(':')?;
    let phase = phase.trim();
    if phase.is_empty() || phase.len() > 40 {
        return None;
    }
    let percent = rest
        .split_whitespace()
        .find_map(|w| w.strip_suffix('%'))
        .and_then(|n| n.trim().parse::<u8>().ok());
    percent.map(|p| Progress { phase: phase.to_string(), percent: Some(p.min(100)) })
}

/// What a job has come to
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Running(Progress),
    Done(PathBuf),
    Failed(String),
}

/// A clone on its way, watched from the loop and stoppable from the dialog
#[derive(Clone)]
pub struct Job {
    state: Arc<Mutex<Outcome>>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Job {
    pub fn outcome(&self) -> Outcome {
        self.state.lock().map(|s| s.clone()).unwrap_or_else(|e| e.into_inner().clone())
    }

    /// Stop it. The watching thread then takes away the half-made folder: a
    /// clone that did not finish is not a project, and leaving it would make
    /// the same name refuse the next attempt
    pub fn stop(&self) {
        if let Ok(mut c) = self.child.lock()
            && let Some(child) = c.as_mut()
        {
            let _ = child.kill();
        }
    }
}

/// Start cloning `url` into a new folder under `parent`
pub fn start_clone(url: &str, parent: &str) -> Result<Job, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err(crate::i18n::t("err.addproj.no_url"));
    }
    let name = repo_name_of(&url).ok_or_else(|| crate::i18n::t("err.addproj.bad_url"))?;
    let at = target(parent, &name)?;
    std::fs::create_dir_all(Path::new(parent.trim())).map_err(|e| e.to_string())?;
    let mut cmd = Command::new("git");
    cmd.arg("clone").arg("--progress").arg("--").arg(&url).arg(&at)
        // Nobody can see a prompt from here: a repository that needs signing
        // in fails and says so, rather than waiting for ever
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = crate::detach_console(&mut cmd).spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => crate::i18n::t("err.addproj.no_git"),
        _ => e.to_string(),
    })?;
    let stderr = child.stderr.take();
    let job = Job {
        state: Arc::new(Mutex::new(Outcome::Running(Progress::default()))),
        child: Arc::new(Mutex::new(Some(child))),
    };
    let watch = job.clone();
    std::thread::spawn(move || {
        let mut said = String::new();
        if let Some(mut err) = stderr {
            // git redraws its progress line with a carriage return, so a
            // line is whatever lies between either kind of break
            let mut buf = [0u8; 1024];
            let mut line = Vec::new();
            loop {
                let n = match std::io::Read::read(&mut err, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                for &b in &buf[..n] {
                    if b == b'\r' || b == b'\n' {
                        let text = String::from_utf8_lossy(&line).to_string();
                        line.clear();
                        if let Some(p) = progress_of(&text) {
                            if let Ok(mut s) = watch.state.lock() {
                                *s = Outcome::Running(p);
                            }
                        } else if !text.trim().is_empty() {
                            said = text;
                        }
                    } else {
                        line.push(b);
                    }
                }
            }
        }
        let status = watch.child.lock().ok().and_then(|mut c| c.as_mut().and_then(|c| c.wait().ok()));
        let ok = status.is_some_and(|s| s.success());
        let end = match ok && at.is_dir() {
            true => Outcome::Done(at.clone()),
            false => {
                let _ = std::fs::remove_dir_all(&at);
                let why = said.trim().trim_start_matches("fatal:").trim().to_string();
                Outcome::Failed(match why.is_empty() {
                    true => crate::i18n::t("err.addproj.clone_stopped"),
                    false => why,
                })
            }
        };
        if let Ok(mut s) = watch.state.lock() {
            *s = end;
        }
    });
    Ok(job)
}

/// Make a new project: its folder, a repository in it, and one empty commit.
///
/// The commit is what makes it a project worktrees can be cut from -- git will
/// not start a worktree from a repository with no commit at all. It needs a
/// name to be made under, so a PC where git has none is told before anything
/// is made, rather than left with a folder that is half a project
pub fn create(name: &str, parent: &str) -> Result<PathBuf, String> {
    let at = target(parent, name)?;
    let here = std::env::temp_dir();
    let asked = |key: &str| {
        crate::git::run(&here, &["config", "--get", key]).map(|v| !v.trim().is_empty()).unwrap_or(false)
    };
    if !asked("user.name") || !asked("user.email") {
        return Err(crate::i18n::t("err.addproj.no_identity"));
    }
    std::fs::create_dir_all(&at).map_err(|e| e.to_string())?;
    let steps: [&[&str]; 2] = [&["init", "-b", "main"], &["commit", "--allow-empty", "-m", "Initial commit"]];
    for args in steps {
        if let Err(e) = crate::git::run_as(&at, args, "", Duration::from_secs(30), &crate::git::As::default()) {
            let _ = std::fs::remove_dir_all(&at);
            bail_str(&e)?;
        }
    }
    Ok(at)
}

/// A git that failed, in words: a missing git is named as missing
fn bail_str(e: &anyhow::Error) -> Result<(), String> {
    Err(match e.downcast_ref::<crate::git::NotInstalled>() {
        Some(_) => crate::i18n::t("err.addproj.no_git"),
        None => format!("{e:#}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clone_is_named_after_the_end_of_its_url() {
        assert_eq!(repo_name_of("https://github.com/user/repo.git").as_deref(), Some("repo"));
        assert_eq!(repo_name_of("https://github.com/user/repo/").as_deref(), Some("repo"));
        assert_eq!(repo_name_of("git@github.com:user/my-app.git").as_deref(), Some("my-app"));
        assert_eq!(repo_name_of("https://github.com/"), None, "a host alone is named after the host");
        assert_eq!(repo_name_of(""), None);
    }

    #[test]
    fn a_folder_name_windows_would_refuse_is_refused_here_first() {
        assert!(valid_name("my-project"));
        for bad in ["", ".", "..", "a/b", "a\\b", "con", "NUL.txt", "COM1", "name.", "what?"] {
            assert!(!valid_name(bad), "{bad:?} was accepted");
        }
        assert!(valid_name("COM10") && valid_name("console"), "a name that only starts like a device was refused");
    }

    #[test]
    fn gits_progress_is_read_for_its_stage_and_percentage() {
        let p = progress_of("Receiving objects:  45% (450/1000), 1.20 MiB | 2.00 MiB/s").unwrap();
        assert_eq!((p.phase.as_str(), p.percent), ("Receiving objects", Some(45)));
        let r = progress_of("remote: Counting objects: 100% (12/12), done.").unwrap();
        assert_eq!((r.phase.as_str(), r.percent), ("Counting objects", Some(100)));
        assert_eq!(progress_of("Cloning into 'repo'..."), None);
        assert_eq!(progress_of("fatal: repository not found"), None);
    }

    #[test]
    fn a_project_is_never_made_over_a_folder_that_is_there() {
        let parent = std::env::temp_dir().join(format!("shikisha-addproj-{}", crate::random_hex(6)));
        std::fs::create_dir_all(parent.join("taken")).unwrap();
        let p = parent.display().to_string();
        assert!(target(&p, "taken").is_err(), "a folder that is there was offered");
        assert_eq!(target(&p, "fresh").unwrap(), parent.join("fresh"));
        assert!(target("", "fresh").is_err(), "no parent was accepted");
        let _ = std::fs::remove_dir_all(&parent);
    }
}

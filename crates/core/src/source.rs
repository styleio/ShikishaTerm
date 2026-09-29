//! This program's own source, fetched for the ? to search when the manual has
//! no answer.
//!
//! The answer to "how do I ..." is sometimes only written in the code: a key,
//! a button that appears in one state, a thing the manual never got round to.
//! The ? reads the manual first; a person who did not find what they wanted
//! there can ask it to look in the source, and waits longer for it.
//!
//! **The version that is running.** What is fetched is the commit this build
//! was made from ([`build_sha`]), so the code read is the code that runs. It is
//! fetched shallow -- that one commit and nothing of its history, about 30 MB
//! -- into the app's state folder, once. A later build is a different commit:
//! that one is fetched into the same folder and checked out, and the one
//! before is left for git to forget. No branch is made; there is nothing to
//! commit to and nothing to merge.
//!
//! A build whose commit is not in the public repository (a build of work not
//! pushed yet) reads `main` instead, and says so.
//!
//! Read-only from start to end: the AI that searches it is given tools that
//! read and nothing else (`webui::ask_reading`).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

/// Where the source comes from: the public repository
pub const REPOSITORY: &str = "https://github.com/styleio/ShikishaTerm.git";

/// How long one fetch may take before it is given up on
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// The whole commit this build was made from, when the build knew it
pub fn build_sha() -> &'static str {
    env!("BUILD_SHA")
}

/// Where the source is kept
pub fn folder() -> PathBuf {
    crate::config::state_path("source")
}

/// The source, checked out at a commit
#[derive(Clone, Debug)]
pub struct Checkout {
    pub folder: PathBuf,
    /// The commit that is checked out
    pub commit: String,
    /// Whether that is not the commit this build was made from, but `main`:
    /// this build's commit is not in the public repository
    pub instead: bool,
}

/// The source of this version, fetched if it is not here yet or is another
/// version's. Waits on the network the first time and after an update
pub fn ready() -> Result<Checkout> {
    let dir = folder();
    let want = build_sha();
    let git = which_git().ok_or_else(|| anyhow::anyhow!(crate::i18n::t("guide.source.no_git")))?;
    if !dir.join(".git").exists() {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        run(&git, &dir, &["init", "--quiet"], FETCH_TIMEOUT)?;
        run(&git, &dir, &["remote", "add", "origin", REPOSITORY], FETCH_TIMEOUT)?;
    }
    // Already this version: nothing to fetch
    if !want.is_empty() && head(&git, &dir).as_deref() == Some(want) {
        return Ok(Checkout { folder: dir, commit: want.to_string(), instead: false });
    }
    // This version, when the repository has it; otherwise the newest main
    let fetched = !want.is_empty() && run(&git, &dir, &["fetch", "--quiet", "--depth", "1", "origin", want], FETCH_TIMEOUT).is_ok();
    if !fetched {
        run(&git, &dir, &["fetch", "--quiet", "--depth", "1", "origin", "main"], FETCH_TIMEOUT)
            .context(crate::i18n::t("guide.source.fetch_failed"))?;
    }
    run(&git, &dir, &["-c", "advice.detachedHead=false", "checkout", "--quiet", "--force", "FETCH_HEAD"], FETCH_TIMEOUT)?;
    // What an earlier version left behind is not kept around
    let _ = run(&git, &dir, &["clean", "-fdq"], FETCH_TIMEOUT);
    let commit = head(&git, &dir).unwrap_or_default();
    Ok(Checkout { folder: dir, commit, instead: !fetched })
}

/// The commit a folder has checked out
fn head(git: &Path, dir: &Path) -> Option<String> {
    run(git, dir, &["rev-parse", "HEAD"], FETCH_TIMEOUT).ok().map(|s| s.trim().to_string())
}

/// git on this machine, found the way a tab's command is
fn which_git() -> Option<PathBuf> {
    crate::tab::resolve_command("git")
}

/// One git command in `dir`, with its output, or why it failed
fn run(git: &Path, dir: &Path, args: &[&str], timeout: std::time::Duration) -> Result<String> {
    use std::io::Read as _;
    let mut cmd = std::process::Command::new(git);
    cmd.args(args)
        .current_dir(dir)
        // Nothing to answer: no password prompt, no pager
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    crate::detach_console(&mut cmd);
    let mut child = cmd.spawn().with_context(|| format!("git {}", args.join(" ")))?;
    let until = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            let mut out = String::new();
            let mut err = String::new();
            if let Some(mut o) = child.stdout.take() {
                let _ = o.read_to_string(&mut out);
            }
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            if !status.success() {
                bail!("git {}: {}", args.join(" "), err.trim());
            }
            return Ok(out);
        }
        if std::time::Instant::now() > until {
            let _ = child.kill();
            bail!("git {}: {}", args.join(" "), crate::i18n::t("guide.source.too_slow"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

//! Running git, and reading what it says.
//!
//! The other half of what this app knows about git lives in `repo.rs`, which
//! **never launches git** -- it reads `.git/HEAD` directly, so the sidebar can
//! still name the branch while a rebase holds `index.lock`. That path stays as
//! it is. This module is the other job: the list and the diffs a person asked
//! to see, which cannot be read correctly without git itself.
//!
//! Two rules hold here:
//!
//! - **Nothing runs without a folder.** Every invocation carries `-C <folder>`,
//!   and the folder comes from a tab, never from a string a script assembled.
//! - **Nothing waits for a person.** `GIT_TERMINAL_PROMPT=0` and an empty
//!   askpass mean git fails instead of sitting on a credential prompt, and a
//!   wall-clock ceiling kills whatever is left. The engine runs on the main
//!   loop: a git that never returns is a window that never redraws.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

/// How long any one git may take before it is killed.
///
/// Everything the sugar runs is local and finishes in tens of milliseconds.
/// The ceiling is for `git_run`, where someone can reach a command that talks
/// to a network -- and for the day a repository is on a disconnected share
const LIMIT: Duration = Duration::from_secs(20);

/// The ceiling for the ones that talk to a server. A fetch of a large
/// repository over a slow line is not a hang, and killing it at twenty seconds
/// would be this app deciding the network is wrong. These never run on the main
/// loop -- the panel hands them to a thread -- so waiting costs nobody a redraw
const NETWORK_LIMIT: Duration = Duration::from_secs(180);

/// Branches that do not take a direct commit unless the caller says so.
///
/// Not a security boundary -- it is the difference between "committed to main
/// by accident" and "meant it". The offer to make a branch instead lives in
/// whoever catches the refusal
const PROTECTED: [&str; 2] = ["main", "master"];

/// One line of `git status`, in git's own vocabulary.
///
/// `index` and `work` are the two letters git prints: the staged side and the
/// working-tree side. They are handed on as they are -- inventing a word for
/// each combination would mean a table that has to be kept in step with git,
/// and everyone who knows git already reads these
pub struct Change {
    pub index: char,
    pub work: char,
    pub path: String,
    /// Where a renamed or copied file came from
    pub from: Option<String>,
}

pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

/// Read a child's whole stream on a thread of its own.
///
/// Polling for exit while a pipe fills is a deadlock: git blocks writing, we
/// block waiting for it to finish. A diff is easily larger than a pipe buffer
fn drain(mut s: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = s.read_to_end(&mut buf);
        buf
    })
}

/// Run one git in `dir` and hand back what it printed.
///
/// Failure carries git's own words: `stderr` says what was wrong far better
/// than a status code, and the person reading it is writing automation
pub fn run(dir: &Path, args: &[&str]) -> Result<String> {
    run_within(dir, args, LIMIT)
}

/// The same, with the caller saying how long it is willing to wait
pub fn run_within(dir: &Path, args: &[&str], limit: Duration) -> Result<String> {
    run_stdin(dir, args, "", limit)
}

/// ...and with something to hand it on the way in. `git apply` reads the patch
/// from here rather than from a file nobody asked us to write
pub fn run_stdin(dir: &Path, args: &[&str], input: &str, limit: Duration) -> Result<String> {
    if !dir.is_dir() {
        bail!(crate::i18n::tp(
            "err.git.no_folder",
            &[("p", &dir.display().to_string())]
        ));
    }
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        // Never sit waiting for a person who cannot see the prompt
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .stdin(if input.is_empty() { Stdio::null() } else { Stdio::piped() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::detach_console(&mut cmd).spawn()?;
    if !input.is_empty() {
        use std::io::Write as _;
        if let Some(mut w) = child.stdin.take() {
            w.write_all(input.as_bytes())?;
        }
    }
    let out = child.stdout.take().map(drain);
    let err = child.stderr.take().map(drain);
    let started = Instant::now();
    let status = loop {
        match child.try_wait()? {
            Some(s) => break s,
            None if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                bail!(crate::i18n::tp(
                    "err.git.timeout",
                    &[("cmd", &args.join(" "))]
                ));
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let stdout = out.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = err.and_then(|h| h.join().ok()).unwrap_or_default();
    if !status.success() {
        let said = String::from_utf8_lossy(&stderr).trim().to_string();
        bail!(crate::i18n::tp(
            "err.git.failed",
            &[("cmd", &args.join(" ")), ("said", &said)]
        ));
    }
    Ok(String::from_utf8_lossy(&stdout).to_string())
}

/// The top of the working tree the folder belongs to, or an error saying it
/// belongs to none. Everything else in this module starts here, so "not a
/// repository" is said once, in one wording
pub fn root(dir: &Path) -> Result<PathBuf> {
    let out = run(dir, &["rev-parse", "--show-toplevel"]).map_err(|_| {
        anyhow::anyhow!(crate::i18n::tp(
            "err.git.no_repo",
            &[("p", &dir.display().to_string())]
        ))
    })?;
    let line = out.trim();
    if line.is_empty() {
        bail!(crate::i18n::tp(
            "err.git.no_repo",
            &[("p", &dir.display().to_string())]
        ));
    }
    Ok(PathBuf::from(line))
}

/// What has changed. Untracked files are included, one per file rather than
/// one per folder: a list that says "a folder changed" cannot be staged from
pub fn status(dir: &Path) -> Result<Vec<Change>> {
    // -z because a path may contain anything, including a newline; without it
    // git quotes and escapes, and every reader has to unescape it again
    let out = run(
        dir,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut fields = out.split('\0').filter(|f| !f.is_empty());
    let mut changes = Vec::new();
    while let Some(record) = fields.next() {
        let mut chars = record.chars();
        let (Some(index), Some(work)) = (chars.next(), chars.next()) else {
            continue;
        };
        let path = record.get(3..).unwrap_or_default().to_string();
        // A rename or copy is followed by where it came from, as its own field
        let from = if matches!(index, 'R' | 'C') || matches!(work, 'R' | 'C') {
            fields.next().map(str::to_string)
        } else {
            None
        };
        changes.push(Change { index, work, path, from });
    }
    Ok(changes)
}

/// The files with a conflict in them, and nothing else
pub fn conflicts(dir: &Path) -> Result<Vec<String>> {
    let out = run(dir, &["diff", "--name-only", "--diff-filter=U", "-z"])?;
    Ok(out.split('\0').filter(|f| !f.is_empty()).map(str::to_string).collect())
}

/// The diff, as text. `staged` reads the staged side instead of the working
/// tree; `path` narrows it to one file
pub fn diff(dir: &Path, path: Option<&str>, staged: bool) -> Result<String> {
    let mut args: Vec<&str> = vec!["diff", "--no-color"];
    if staged {
        args.push("--cached");
    }
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    run(dir, &args)
}

pub fn log(dir: &Path, count: u32) -> Result<Vec<Commit>> {
    let n = format!("-{}", count.clamp(1, 1000));
    // Unit separator between fields, record separator between commits: a
    // subject can hold anything a person typed, tabs and pipes included
    let out = run(
        dir,
        &[
            "log",
            &n,
            "--date=short",
            "--pretty=format:%H\x1f%h\x1f%an\x1f%ad\x1f%s\x1e",
        ],
    )?;
    Ok(out
        .split('\x1e')
        .map(str::trim_start)
        .filter(|r| !r.is_empty())
        .filter_map(|record| {
            let mut f = record.split('\x1f');
            Some(Commit {
                hash: f.next()?.to_string(),
                short: f.next()?.to_string(),
                author: f.next()?.to_string(),
                date: f.next()?.to_string(),
                subject: f.next().unwrap_or_default().to_string(),
            })
        })
        .collect())
}

/// The branch checked out, or `None` when the head is detached.
///
/// `symbolic-ref` rather than `rev-parse`: it reads the ref HEAD points at, so
/// it still answers on a branch that has no commits yet. `rev-parse` has
/// nothing to resolve there and fails, which would have turned "your first
/// commit" into an error about an ambiguous argument. Callers reach this after
/// `root()` has agreed there is a repository, so a failure here means a
/// detached head rather than a missing one
pub fn branch(dir: &Path) -> Result<Option<String>> {
    match run(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        Ok(out) => {
            let name = out.trim().to_string();
            Ok((!name.is_empty()).then_some(name))
        }
        Err(_) => Ok(None),
    }
}

/// One piece of a diff: the smallest thing a person says yes or no to.
///
/// `patch` is a whole, valid patch on its own -- the file's header and this one
/// hunk -- so it can be handed straight back to `git apply`. Everything else
/// here is for the screen
pub struct Hunk {
    pub file: String,
    /// The `@@ ... @@` line, as git wrote it
    pub header: String,
    /// The lines this hunk covers on the new side
    pub start: u32,
    pub end: u32,
    pub patch: String,
}

/// Cut a diff into hunks.
///
/// Split here rather than in the screen: a patch that is one line out does not
/// apply, and "which lines is this" is the sort of thing that should be got
/// right once, where it can be tested, instead of in every place that draws it
pub fn split_hunks(diff: &str) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    let mut file = String::new();
    let mut head = String::new();
    let mut cur: Option<Hunk> = None;
    let finish = |cur: &mut Option<Hunk>, out: &mut Vec<Hunk>| {
        if let Some(h) = cur.take() {
            out.push(h);
        }
    };
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            finish(&mut cur, &mut out);
            head = format!("{line}\n");
            // "diff --git a/x b/x" -- the name after "b/" is the one it is now
            file = line
                .rsplit(" b/")
                .next()
                .unwrap_or_default()
                .trim()
                .to_string();
            continue;
        }
        if cur.is_none() && !line.starts_with("@@") {
            // Everything between the two is the file's header: index, mode,
            // ---/+++, and the "Binary files" line when there is nothing to cut
            head.push_str(line);
            head.push('\n');
            continue;
        }
        if line.starts_with("@@") {
            finish(&mut cur, &mut out);
            let (start, count) = new_range(line);
            cur = Some(Hunk {
                file: file.clone(),
                header: line.to_string(),
                start,
                end: start + count.saturating_sub(1).max(0),
                patch: format!("{head}{line}\n"),
            });
            continue;
        }
        if let Some(h) = cur.as_mut() {
            h.patch.push_str(line);
            h.patch.push('\n');
        }
    }
    finish(&mut cur, &mut out);
    out
}

/// The `+start,count` half of a hunk header. A missing count means one line,
/// which is how git writes a single-line hunk
fn new_range(header: &str) -> (u32, u32) {
    let plus = header
        .split_whitespace()
        .find(|w| w.starts_with('+'))
        .unwrap_or("+0,0");
    let mut parts = plus.trim_start_matches('+').split(',');
    let start = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let count = parts.next().and_then(|n| n.parse().ok()).unwrap_or(1);
    (start, count)
}

/// Put one patch back into the tree, or into what is staged.
///
/// This is how a hunk is staged, unstaged, or thrown away -- the same call each
/// time, with the two switches saying which. git decides whether the patch
/// still fits; if the file moved on since it was drawn, it refuses, and that
/// refusal is the truth rather than something to work around
pub fn apply(dir: &Path, patch: &str, cached: bool, reverse: bool) -> Result<()> {
    if patch.trim().is_empty() {
        bail!(crate::i18n::t("err.git.empty_patch"));
    }
    let mut args: Vec<&str> = vec!["apply"];
    if cached {
        args.push("--cached");
    }
    if reverse {
        args.push("--reverse");
    }
    // Whitespace is somebody's file, not something to tidy on the way past
    args.push("--whitespace=nowarn");
    args.push("-");
    let mut body = patch.to_string();
    if !body.ends_with('\n') {
        body.push('\n');
    }
    run_stdin(dir, &args, &body, LIMIT).map(|_| ())
}

/// Every local branch, and which one is checked out.
///
/// `for-each-ref` rather than `branch`: `branch` writes for people (a `*` in
/// front, colours, a leading two spaces) and its output has changed shape
/// before. This one is the plumbing, and says exactly what was asked for
pub fn branches(dir: &Path) -> Result<Vec<(String, bool)>> {
    let out = run(dir, &["for-each-ref", "--format=%(refname:short)", "refs/heads/"])?;
    let here = branch(dir)?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| (l.to_string(), here.as_deref() == Some(l)))
        .collect())
}

/// Move to a branch that already exists. git refuses when the move would take
/// uncommitted work with it into a conflict, and says so better than we could
pub fn checkout(dir: &Path, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!(crate::i18n::t("err.git.empty_branch"));
    }
    run(dir, &["checkout", "-q", name]).map(|_| ())
}

/// Bring the merge in. A conflict is not an error to hide: git stops, the files
/// are marked, and the panel is about to list them
pub fn merge(dir: &Path, name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!(crate::i18n::t("err.git.empty_branch"));
    }
    run(dir, &["merge", "--no-edit", name])
}

pub fn fetch(dir: &Path) -> Result<String> {
    run_within(dir, &["fetch", "--prune"], NETWORK_LIMIT)
}

pub fn pull(dir: &Path) -> Result<String> {
    run_within(dir, &["pull"], NETWORK_LIMIT)
}

/// Send it. A branch made here has never been pushed, so the first push is the
/// common one rather than the exception -- and answering "set an upstream and
/// try again" to somebody who just pressed a button called Push is asking them
/// to type the thing the button was for. The retry says out loud what it did
pub fn push(dir: &Path) -> Result<String> {
    match run_within(dir, &["push"], NETWORK_LIMIT) {
        Ok(out) => Ok(out),
        Err(first) => {
            let Some(here) = branch(dir)? else { return Err(first) };
            if !first.to_string().contains("--set-upstream") {
                return Err(first);
            }
            let said = run_within(
                dir,
                &["push", "--set-upstream", "origin", &here],
                NETWORK_LIMIT,
            )?;
            Ok(format!(
                "{}\n{said}",
                crate::i18n::tp("msg.git.first_push", &[("branch", &here)])
            ))
        }
    }
}

pub fn stage(dir: &Path, paths: &[String]) -> Result<()> {
    let mut args: Vec<&str> = vec!["add", "--"];
    args.extend(paths.iter().map(String::as_str));
    run(dir, &args).map(|_| ())
}

pub fn unstage(dir: &Path, paths: &[String]) -> Result<()> {
    let mut args: Vec<&str> = vec!["restore", "--staged", "--"];
    args.extend(paths.iter().map(String::as_str));
    // `restore` arrived in git 2.23, and it is the spelling git itself
    // suggests. Older ones still answer to the older spelling
    if run(dir, &args).is_ok() {
        return Ok(());
    }
    let mut older: Vec<&str> = vec!["reset", "-q", "HEAD", "--"];
    older.extend(paths.iter().map(String::as_str));
    run(dir, &older).map(|_| ())
}

/// Commit what is staged.
///
/// A protected branch refuses unless the caller says it meant it. The refusal
/// is the whole point: whoever catches it can offer to make a branch instead,
/// which is a better answer than either committing or a wall
pub fn commit(dir: &Path, message: &str, allow_protected: bool, amend: bool) -> Result<String> {
    if message.trim().is_empty() {
        bail!(crate::i18n::t("err.git.empty_message"));
    }
    if !allow_protected {
        if let Some(b) = branch(dir)? {
            if PROTECTED.contains(&b.as_str()) {
                bail!(crate::i18n::tp("err.git.protected", &[("branch", &b)]));
            }
        }
    }
    // Amend rewrites the commit that is already there. On a branch nobody else
    // has, that is tidying; on a shared one it is rewriting what other people
    // have -- which is why it goes through the same refusal as a plain commit
    let mut args = vec!["commit", "-m", message];
    if amend {
        args.push("--amend");
    }
    run(dir, &args)?;
    Ok(run(dir, &["rev-parse", "--short", "HEAD"])?.trim().to_string())
}

/// Make a branch and move onto it, carrying whatever is staged.
///
/// This exists so that refusing a commit on a shared branch has somewhere to
/// go. A wall tells someone they were wrong; this hands them the road they
/// wanted in the first place, with their work still in their hands
pub fn branch_create(dir: &Path, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!(crate::i18n::t("err.git.empty_branch"));
    }
    // git has its own rules about what a ref may be called, and they are the
    // ones that matter -- checking them again here would be a second opinion
    // that can only ever disagree
    run(dir, &["checkout", "-q", "-b", name]).map(|_| ())
}

/// Whether this branch is one that refuses a direct commit. Read by whoever
/// wants to offer the alternative before the refusal happens
pub fn is_protected(name: &str) -> bool {
    PROTECTED.contains(&name)
}

/// Split a command line the way a shell would, minus the shell.
///
/// `git_run` takes one string because that is how people write git, but
/// handing it to a shell would mean `;` and `&&` reach the machine. The words
/// are split here and passed to git directly, quotes honoured so a commit
/// message with a space in it survives
pub fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                any = true;
            }
            None if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            None => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_is_split_without_a_shell() {
        assert_eq!(split_args("status --porcelain"), vec!["status", "--porcelain"]);
        assert_eq!(
            split_args("commit -m \"a message with spaces\""),
            vec!["commit", "-m", "a message with spaces"]
        );
        // An empty quoted word is a word: `git log --grep=""` means something
        assert_eq!(split_args("log --grep \"\""), vec!["log", "--grep", ""]);
        assert!(split_args("   ").is_empty());
        // Nothing here treats `;` or `&&` as a separator -- they are just
        // characters, and git will refuse them as arguments
        assert_eq!(split_args("status; rm -rf /").len(), 4);
    }

    #[test]
    fn protected_branches_are_the_ones_people_share() {
        assert!(is_protected("main"));
        assert!(is_protected("master"));
        assert!(!is_protected("feature/two"));
    }

    /// A repository of its own, thrown away afterwards. `None` when this
    /// machine has no git, which is a reason to skip rather than to fail
    fn scratch_repo(tag: &str) -> Option<std::path::PathBuf> {
        let dir = std::env::temp_dir()
            .join(format!("shikisha-git-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        run(&dir, &["init", "-b", "main"]).ok()?;
        run(&dir, &["config", "user.email", "test@example.invalid"]).ok()?;
        run(&dir, &["config", "user.name", "test"]).ok()?;
        Some(dir)
    }

    #[test]
    fn a_shared_branch_says_no_before_it_commits() {
        let Some(dir) = scratch_repo("protected") else { return };
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();

        // On main it refuses, and says which branch it is refusing about --
        // whoever catches this offers to make a branch instead
        let refused = commit(&dir, "first", false, false).unwrap_err().to_string();
        assert!(refused.contains("main"), "断る理由にブランチ名が入る: {refused}");
        // Nothing was committed by the refusal
        assert!(log(&dir, 1).map(|l| l.is_empty()).unwrap_or(true));

        // ...and it goes through for someone who says they meant it
        let hash = commit(&dir, "first", true, false).expect("承知のうえなら通る");
        assert!(!hash.is_empty());
        assert_eq!(log(&dir, 1).unwrap()[0].subject, "first");

        // On a branch of its own, nothing is in the way
        run(&dir, &["checkout", "-q", "-b", "feature"]).unwrap();
        std::fs::write(dir.join("a.txt"), "hello again").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "second", false, false).expect("自分のブランチなら止まらない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_way_out_of_a_refusal_keeps_the_work() {
        let Some(dir) = scratch_repo("branchnew") else { return };
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "start", true, false).unwrap();

        // Something staged, on a branch that will not take it
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        assert!(commit(&dir, "next", false, false).is_err());

        // The offer: a branch, and the staged work still staged on it
        branch_create(&dir, "work/next").expect("枝を作れる");
        assert_eq!(branch(&dir).unwrap().as_deref(), Some("work/next"));
        let rows = status(&dir).unwrap();
        assert_eq!(rows.iter().find(|c| c.path == "a.txt").unwrap().index, 'M',
            "ステージしたものは持ったまま移る");
        commit(&dir, "next", false, false).expect("移った先では通る");

        assert!(branch_create(&dir, "  ").is_err(), "名前が空なら断る");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_staged_and_what_is_not_are_told_apart() {
        let Some(dir) = scratch_repo("status") else { return };
        std::fs::write(dir.join("kept.txt"), "one").unwrap();
        stage(&dir, &["kept.txt".to_string()]).unwrap();
        commit(&dir, "start", true, false).unwrap();

        std::fs::write(dir.join("kept.txt"), "two").unwrap();
        std::fs::write(dir.join("fresh.txt"), "new").unwrap();
        stage(&dir, &["kept.txt".to_string()]).unwrap();

        let rows = status(&dir).unwrap();
        let staged = rows.iter().find(|c| c.path == "kept.txt").expect("変更した行が出る");
        assert_eq!(staged.index, 'M', "ステージ側は変更済み");
        let untracked = rows.iter().find(|c| c.path == "fresh.txt").expect("新しいファイルも出る");
        assert_eq!((untracked.index, untracked.work), ('?', '?'));

        // ...and taking it back out moves it to the other side
        unstage(&dir, &["kept.txt".to_string()]).unwrap();
        let after = status(&dir).unwrap();
        let back = after.iter().find(|c| c.path == "kept.txt").unwrap();
        assert_eq!(back.index, ' ', "ステージから外れた");
        assert_eq!(back.work, 'M', "作業ツリー側には残っている");

        // The diff has the words that changed in it
        let d = diff(&dir, Some("kept.txt"), false).unwrap();
        assert!(d.contains("+two"), "差分に変更後の行がある: {d}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_branches_are_listed_with_the_one_you_are_on() {
        let Some(dir) = scratch_repo("branches") else { return };
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "start", true, false).unwrap();
        branch_create(&dir, "side").unwrap();

        let list = branches(&dir).unwrap();
        let names: Vec<&str> = list.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"main") && names.contains(&"side"), "{names:?}");
        assert_eq!(
            list.iter().filter(|(_, here)| *here).map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["side"],
            "今いる枝はひとつだけ印が付く"
        );

        checkout(&dir, "main").expect("戻れる");
        assert_eq!(branch(&dir).unwrap().as_deref(), Some("main"));
        assert!(checkout(&dir, "no-such-branch").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn amend_rewrites_rather_than_adds() {
        let Some(dir) = scratch_repo("amend") else { return };
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "frist", true, false).unwrap();

        commit(&dir, "first", true, true).expect("書き直せる");
        let log = log(&dir, 5).unwrap();
        assert_eq!(log.len(), 1, "コミットは増えない");
        assert_eq!(log[0].subject, "first", "言い直したほうが残る");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_diff_is_cut_where_a_person_would_cut_it() {
        let diff = concat!(
            "diff --git a/f.txt b/f.txt\n",
            "index 111..222 100644\n",
            "--- a/f.txt\n",
            "+++ b/f.txt\n",
            "@@ -1,3 +1,4 @@\n",
            " one\n",
            "+two\n",
            " three\n",
            " four\n",
            "@@ -20,2 +21,2 @@\n",
            "-old\n",
            "+new\n",
        );
        let hunks = split_hunks(diff);
        assert_eq!(hunks.len(), 2, "@@ ごとに1つ");
        assert_eq!(hunks[0].file, "f.txt");
        assert_eq!((hunks[0].start, hunks[0].end), (1, 4));
        assert_eq!((hunks[1].start, hunks[1].end), (21, 22));
        // Each piece carries the file's header, so it stands on its own as a
        // patch -- which is the whole point of cutting it this way
        assert!(hunks[1].patch.starts_with("diff --git a/f.txt b/f.txt\n"));
        assert!(hunks[1].patch.contains("+++ b/f.txt\n"));
        assert!(hunks[1].patch.contains("@@ -20,2 +21,2 @@\n-old\n+new\n"));
        assert!(!hunks[1].patch.contains("+two"), "隣の hunk は混ざらない");
        // Nothing to cut is not an error
        assert!(split_hunks("").is_empty());
    }

    #[test]
    fn one_hunk_can_be_staged_without_the_others() {
        let Some(dir) = scratch_repo("hunks") else { return };
        // Far apart enough that git's three lines of context cannot join them
        let start = (1..=24).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
        std::fs::write(dir.join("f.txt"), format!("{start}\n")).unwrap();
        stage(&dir, &["f.txt".to_string()]).unwrap();
        commit(&dir, "start", true, false).unwrap();

        // Two changes, far enough apart to be two hunks
        let edited = format!(
            "{}\n",
            start.replace("line 1\n", "LINE ONE\n").replace("line 20", "LINE TWENTY")
        );
        std::fs::write(dir.join("f.txt"), edited).unwrap();
        let hunks = split_hunks(&diff(&dir, Some("f.txt"), false).unwrap());
        assert_eq!(hunks.len(), 2, "離れた2箇所は2つの hunk: {hunks:?}",
            hunks = hunks.iter().map(|h| h.header.clone()).collect::<Vec<_>>());

        // Stage the first one only
        apply(&dir, &hunks[0].patch, true, false).expect("hunk を1つだけ載せられる");
        let staged = diff(&dir, Some("f.txt"), true).unwrap();
        assert!(staged.contains("LINE ONE"), "選んだほうは入っている");
        assert!(!staged.contains("LINE TWENTY"), "選ばなかったほうは入っていない");
        // ...and the other is still waiting in the tree
        let left = diff(&dir, Some("f.txt"), false).unwrap();
        assert!(left.contains("LINE TWENTY") && !left.contains("LINE ONE"));

        // Taking it back out again
        apply(&dir, &hunks[0].patch, true, true).expect("戻せる");
        assert!(diff(&dir, Some("f.txt"), true).unwrap().trim().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_conflict_is_listed_as_one() {
        let Some(dir) = scratch_repo("conflict") else { return };
        std::fs::write(dir.join("c.txt"), "base").unwrap();
        stage(&dir, &["c.txt".to_string()]).unwrap();
        commit(&dir, "base", true, false).unwrap();

        run(&dir, &["checkout", "-q", "-b", "other"]).unwrap();
        std::fs::write(dir.join("c.txt"), "theirs").unwrap();
        stage(&dir, &["c.txt".to_string()]).unwrap();
        commit(&dir, "theirs", false, false).unwrap();

        run(&dir, &["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("c.txt"), "ours").unwrap();
        stage(&dir, &["c.txt".to_string()]).unwrap();
        commit(&dir, "ours", true, false).unwrap();

        // The merge fails, which is the point: what matters is that the file
        // can then be found by name rather than by reading git's message
        assert!(run(&dir, &["merge", "other"]).is_err());
        assert_eq!(conflicts(&dir).unwrap(), vec!["c.txt".to_string()]);
        let rows = status(&dir).unwrap();
        let row = rows.iter().find(|c| c.path == "c.txt").unwrap();
        assert!(
            row.index == 'U' || row.work == 'U' || (row.index == 'A' && row.work == 'A'),
            "衝突の印が付いている: {}{}",
            row.index,
            row.work
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn this_repository_answers_about_itself() {
        // The app is developed in a git repository, so the read side can be
        // tried against something real rather than a fixture that has to be
        // built and torn down
        let here = std::path::Path::new(".");
        let Ok(top) = root(here) else {
            return; // built from a tarball: nothing to say
        };
        assert!(top.join(".git").exists() || top.join(".git").is_file());
        let head = branch(here).expect("HEAD reads");
        assert!(head.is_none() || !head.unwrap().is_empty());
        // status parses into records rather than one blob
        let changes = status(here).expect("status reads");
        for c in &changes {
            assert!(!c.path.is_empty(), "パスの無い行が出ている");
            assert!(!c.index.is_whitespace() || !c.work.is_whitespace());
        }
        assert!(log(here, 3).expect("log reads").len() <= 3);
    }
}

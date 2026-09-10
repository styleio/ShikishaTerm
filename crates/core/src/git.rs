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

/// The branches that refuse a direct commit until somebody says otherwise.
///
/// Not a security boundary -- it is the difference between "committed to main
/// by accident" and "meant it". The offer to make a branch instead lives in
/// whoever catches the refusal.
///
/// Only the starting point: which branches to guard is a question each project
/// answers differently, so the answer travels in from the settings (see
/// [`crate::config::GitSpec::protected`]) and this is what an answer nobody has
/// given yet amounts to
pub const DEFAULT_PROTECTED: [&str; 2] = ["main", "master"];

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
    /// For a file git has marked as conflicted: whether the markers are still
    /// in it.
    ///
    /// git says "unmerged" until the file is staged, whatever is inside it, so
    /// on its own that cannot tell "nobody has touched this yet" from "this is
    /// sorted out and waiting to be added". Those want different words on
    /// screen, and only one of them wants help
    pub tangled: bool,
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
        let conflict = index == 'U'
            || work == 'U'
            || (index == 'A' && work == 'A')
            || (index == 'D' && work == 'D');
        // Only the conflicted ones are opened, and only up to a size worth
        // reading: this runs every time the list is drawn
        let tangled = conflict && has_markers(&dir.join(&path));
        changes.push(Change { index, work, path, from, tangled });
    }
    Ok(changes)
}

/// Whether git's conflict markers are still in this file.
///
/// A file too large to read is called tangled: saying "sorted out" about
/// something nobody looked at is the wrong way to be wrong
fn has_markers(path: &Path) -> bool {
    const ROOM: u64 = 4 * 1024 * 1024;
    match std::fs::metadata(path).map(|m| m.len()) {
        Ok(n) if n <= ROOM => std::fs::read_to_string(path)
            .map(|body| body.contains("<<<<<<<") && body.contains(">>>>>>>"))
            .unwrap_or(true),
        _ => true,
    }
}

/// The files with a conflict in them, and nothing else
pub fn conflicts(dir: &Path) -> Result<Vec<String>> {
    let out = run(dir, &["diff", "--name-only", "--diff-filter=U", "-z"])?;
    Ok(out.split('\0').filter(|f| !f.is_empty()).map(str::to_string).collect())
}

/// The ones that still have both sides in them. What is left after somebody --
/// or the AI -- has been through is waiting to be staged, not to be untangled
pub fn tangled(dir: &Path) -> Result<Vec<String>> {
    Ok(conflicts(dir)?
        .into_iter()
        .filter(|f| has_markers(&dir.join(f)))
        .collect())
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
/// One commit as the history view shows it: the drawing on the left, and the
/// four things a person scans for
pub struct Line {
    /// git's own graph art for this row (`*`, `|\`, `|/` and so on)
    pub graph: String,
    pub hash: String,
    pub short: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

/// Everything about one commit, as it is shown when a row is picked
pub struct Detail {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub author_date: String,
    pub committer: String,
    pub commit_date: String,
    pub subject: String,
    pub body: String,
    pub files: Vec<String>,
}

/// The history, with git drawing the graph.
///
/// The art on the left is git's own: it knows which branches were where, and
/// redrawing that from a list of parents is a way to be subtly wrong about
/// somebody's history. Rows without a commit on them (the `|/` that closes a
/// merge) are kept -- they are the shape of the thing
pub fn graph(
    dir: &Path,
    all: bool,
    remotes: bool,
    count: u32,
    branch: Option<&str>,
) -> Result<Vec<Line>> {
    let n = format!("-{}", count.clamp(1, 2000));
    let mut args: Vec<&str> = vec![
        "log",
        "--graph",
        "--date=format:%Y/%m/%d %H:%M",
        "--pretty=format:%x1e%H%x1f%h%x1f%an%x1f%ad%x1f%s",
        &n,
    ];
    // Naming one branch beats the two switches: it is the narrower question,
    // and it is the one somebody asked by pointing at it
    match branch.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => args.push(b),
        None if remotes => args.push("--all"),
        None if all => args.push("--branches"),
        None => {}
    }
    let out = run(dir, &args)?;
    Ok(out
        .lines()
        .map(|line| match line.split_once('\x1e') {
            None => Line {
                graph: line.trim_end().to_string(),
                hash: String::new(),
                short: String::new(),
                author: String::new(),
                date: String::new(),
                subject: String::new(),
            },
            Some((art, record)) => {
                let mut f = record.split('\x1f');
                Line {
                    graph: art.trim_end().to_string(),
                    hash: f.next().unwrap_or_default().to_string(),
                    short: f.next().unwrap_or_default().to_string(),
                    author: f.next().unwrap_or_default().to_string(),
                    date: f.next().unwrap_or_default().to_string(),
                    subject: f.next().unwrap_or_default().to_string(),
                }
            }
        })
        .collect())
}

/// One commit, in full, and the files it touched
pub fn detail(dir: &Path, hash: &str) -> Result<Detail> {
    let hash = hash.trim();
    if hash.is_empty() {
        bail!(crate::i18n::t("err.git.no_commit"));
    }
    let out = run(
        dir,
        &[
            "show",
            "--no-patch",
            "--date=format:%Y/%m/%d %H:%M",
            "--pretty=format:%H%x1f%P%x1f%an%x1f%ad%x1f%cn%x1f%cd%x1f%s%x1f%b",
            hash,
        ],
    )?;
    let mut f = out.split('\x1f');
    let full = f.next().unwrap_or_default().trim().to_string();
    let parents: Vec<String> = f
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    let mut take = || f.next().unwrap_or_default().to_string();
    let (author, author_date, committer, commit_date, subject) =
        (take(), take(), take(), take(), take());
    let body = take().trim().to_string();
    // A merge has more than one parent, and `show` says nothing about what it
    // changed unless it is told which side to compare against. First parent is
    // the one people mean: "what did this merge bring in"
    let files = run(
        dir,
        &["show", "--name-only", "--pretty=format:", "-m", "--first-parent", hash],
    )?
    .lines()
    .map(str::trim)
    .filter(|l| !l.is_empty())
    .map(str::to_string)
    .collect();
    Ok(Detail {
        hash: full,
        parents,
        author,
        author_date,
        committer,
        commit_date,
        subject,
        body,
        files,
    })
}

/// What one commit did to one file, as a patch that can be cut into hunks and
/// walked back one at a time
pub fn show(dir: &Path, hash: &str, path: &str) -> Result<String> {
    let hash = hash.trim();
    if hash.is_empty() {
        bail!(crate::i18n::t("err.git.no_commit"));
    }
    let mut args: Vec<&str> = vec!["show", "--no-color", "--pretty=format:", "-m", "--first-parent", hash];
    if !path.is_empty() {
        args.push("--");
        args.push(path);
    }
    run(dir, &args)
}

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
pub fn commit(
    dir: &Path,
    message: &str,
    protect: &[String],
    allow_protected: bool,
    amend: bool,
) -> Result<String> {
    if message.trim().is_empty() {
        bail!(crate::i18n::t("err.git.empty_message"));
    }
    if !allow_protected {
        if let Some(b) = branch(dir)? {
            if is_protected(&b, protect) {
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

/// Whether this branch is one of the ones being guarded. Read by whoever wants
/// to offer the alternative before the refusal happens.
///
/// `*` stands for any run of characters, so a whole shelf of branches can be
/// named at once (`release/*`). Everything else is the name itself: branch
/// names are what people type, and a name that quietly meant something else
/// would guard the wrong branch
pub fn is_protected(name: &str, protect: &[String]) -> bool {
    protect.iter().any(|p| name_matches(p.trim(), name))
}

/// One pattern against one branch name, `*` matching any run of characters.
///
/// Walked rather than turned into a regular expression: the whole language is
/// one character, and a regex would also give `.` `+` `(` a meaning nobody
/// asked for -- a branch really can be called `v1.0+fix`
fn name_matches(pattern: &str, name: &str) -> bool {
    let Some((head, rest)) = pattern.split_once('*') else {
        return pattern == name;
    };
    if !name.starts_with(head) {
        return false;
    }
    // What is left of the pattern has to be found somewhere in what is left of
    // the name, at the end if the pattern ends there
    let mut at = &name[head.len()..];
    loop {
        if name_matches(rest, at) {
            return true;
        }
        // A `*` may stand for nothing at all, so this walks forward one
        // character at a time until the rest of the name is gone
        match at.chars().next() {
            Some(c) => at = &at[c.len_utf8()..],
            None => return false,
        }
    }
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

    /// The list as the settings hand it over
    fn guarded() -> Vec<String> {
        DEFAULT_PROTECTED.iter().map(|s| s.to_string()).collect()
    }

    /// Which branches are guarded is the project's answer, not ours.
    ///
    /// It began as two names written into this file, which is fine right up
    /// until somebody works alone on their own repository -- there, "make a
    /// branch first" is a rule with nobody on the other side of it.
    #[test]
    fn protected_branches_are_the_ones_the_project_named() {
        let out_of_the_box = guarded();
        assert!(is_protected("main", &out_of_the_box));
        assert!(is_protected("master", &out_of_the_box));
        assert!(!is_protected("feature/two", &out_of_the_box));

        // Alone on your own repository, nothing is guarded -- said by leaving
        // the list empty
        assert!(!is_protected("main", &[]));

        // A project that shares other branches names its own
        let theirs = ["develop".to_string(), "release/*".to_string()];
        assert!(is_protected("develop", &theirs));
        assert!(is_protected("release/1.0", &theirs));
        assert!(is_protected("release/", &theirs), "* は何も無くても当たる");
        assert!(!is_protected("main", &theirs), "書いていないものは守らない");
        assert!(!is_protected("hotfix/release/1.0", &theirs), "頭から見る");

        // `*` is the whole of the language. Everything else is the name
        // itself, because a branch really can be called `v1.0+fix`
        let odd = ["v1.0+fix".to_string(), "*/wip".to_string()];
        assert!(is_protected("v1.0+fix", &odd));
        assert!(!is_protected("v1Z0+fix", &odd), ". は . でしかない");
        assert!(is_protected("team/wip", &odd));
        assert!(!is_protected("wip", &odd), "* の前の / まで含めて名前");
        assert!(is_protected("anything at all", &["*".to_string()]), "* だけなら全部守る");

        // Spaces around a name are somebody typing a list, not a branch
        assert!(is_protected("main", &[" main ".to_string()]));
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
        let refused = commit(&dir, "first", &guarded(), false, false).unwrap_err().to_string();
        assert!(refused.contains("main"), "断る理由にブランチ名が入る: {refused}");
        // Nothing was committed by the refusal
        assert!(log(&dir, 1).map(|l| l.is_empty()).unwrap_or(true));

        // ...and it goes through for someone who says they meant it
        let hash = commit(&dir, "first", &guarded(), true, false).expect("承知のうえなら通る");
        assert!(!hash.is_empty());
        assert_eq!(log(&dir, 1).unwrap()[0].subject, "first");

        // On a branch of its own, nothing is in the way
        run(&dir, &["checkout", "-q", "-b", "feature"]).unwrap();
        std::fs::write(dir.join("a.txt"), "hello again").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "second", &guarded(), false, false).expect("自分のブランチなら止まらない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_way_out_of_a_refusal_keeps_the_work() {
        let Some(dir) = scratch_repo("branchnew") else { return };
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "start", &guarded(), true, false).unwrap();

        // Something staged, on a branch that will not take it
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        assert!(commit(&dir, "next", &guarded(), false, false).is_err());

        // The offer: a branch, and the staged work still staged on it
        branch_create(&dir, "work/next").expect("枝を作れる");
        assert_eq!(branch(&dir).unwrap().as_deref(), Some("work/next"));
        let rows = status(&dir).unwrap();
        assert_eq!(rows.iter().find(|c| c.path == "a.txt").unwrap().index, 'M',
            "ステージしたものは持ったまま移る");
        commit(&dir, "next", &guarded(), false, false).expect("移った先では通る");

        assert!(branch_create(&dir, "  ").is_err(), "名前が空なら断る");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_staged_and_what_is_not_are_told_apart() {
        let Some(dir) = scratch_repo("status") else { return };
        std::fs::write(dir.join("kept.txt"), "one").unwrap();
        stage(&dir, &["kept.txt".to_string()]).unwrap();
        commit(&dir, "start", &guarded(), true, false).unwrap();

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
    fn the_history_comes_back_drawn_and_in_pieces() {
        let Some(dir) = scratch_repo("history") else { return };
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "first", &guarded(), true, false).unwrap();
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        std::fs::write(dir.join("b.txt"), "new").unwrap();
        stage(&dir, &["a.txt".to_string(), "b.txt".to_string()]).unwrap();
        commit(&dir, "second\n\nwith a reason", &guarded(), true, false).unwrap();

        let rows = graph(&dir, false, false, 10, None).unwrap();
        let commits: Vec<&Line> = rows.iter().filter(|r| !r.hash.is_empty()).collect();
        assert_eq!(commits.len(), 2, "コミットの数だけ行がある");
        assert!(commits[0].graph.contains('*'), "git の描いた絵が付いてくる");
        assert_eq!(commits[0].subject, "second");
        assert!(commits[0].date.starts_with("20"), "日時が読める形: {}", commits[0].date);

        let d = detail(&dir, &commits[0].hash).unwrap();
        assert_eq!(d.subject, "second");
        assert_eq!(d.body, "with a reason");
        assert_eq!(d.parents, vec![commits[1].hash.clone()], "親を1つ持っている");
        assert_eq!(d.files, vec!["a.txt".to_string(), "b.txt".to_string()]);
        assert!(!d.committer.is_empty() && !d.commit_date.is_empty());

        // What it did to one file, cut into pieces that can be walked back
        let patch = show(&dir, &commits[0].hash, "a.txt").unwrap();
        let hunks = split_hunks(&patch);
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].patch.contains("+two"));
        // ...and walking one back leaves the file as it was before that commit
        apply(&dir, &hunks[0].patch, false, true).expect("取り消せる");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap().trim(), "one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_branches_are_listed_with_the_one_you_are_on() {
        let Some(dir) = scratch_repo("branches") else { return };
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "start", &guarded(), true, false).unwrap();
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
        commit(&dir, "frist", &guarded(), true, false).unwrap();

        commit(&dir, "first", &guarded(), true, true).expect("書き直せる");
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
        commit(&dir, "start", &guarded(), true, false).unwrap();

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
        commit(&dir, "base", &guarded(), true, false).unwrap();

        run(&dir, &["checkout", "-q", "-b", "other"]).unwrap();
        std::fs::write(dir.join("c.txt"), "theirs").unwrap();
        stage(&dir, &["c.txt".to_string()]).unwrap();
        commit(&dir, "theirs", &guarded(), false, false).unwrap();

        run(&dir, &["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("c.txt"), "ours").unwrap();
        stage(&dir, &["c.txt".to_string()]).unwrap();
        commit(&dir, "ours", &guarded(), true, false).unwrap();

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
        assert!(row.tangled, "まだ両方の側が入ったまま");
        assert_eq!(tangled(&dir).unwrap(), vec!["c.txt".to_string()]);

        // Sorted out by hand: git still calls it unmerged until it is staged,
        // and that is exactly the state that must not ask for help again
        std::fs::write(dir.join("c.txt"), "ours and theirs").unwrap();
        let after = status(&dir).unwrap();
        let row = after.iter().find(|c| c.path == "c.txt").unwrap();
        assert!(!row.tangled, "印が消えたら、もう解くものは無い");
        assert!(conflicts(&dir).unwrap().contains(&"c.txt".to_string()), "git はまだ未マージ扱い");
        assert!(tangled(&dir).unwrap().is_empty(), "解くべきものは残っていない");
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

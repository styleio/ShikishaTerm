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
pub const LIMIT: Duration = Duration::from_secs(20);

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

/// How a git signs in to a server, when it has to.
///
/// Deliberately not `Debug`: a token is in here, and a value that can be
/// printed is a value that ends up in a log
#[derive(Clone, Default, PartialEq, Eq)]
pub enum Auth {
    /// Whatever git on this machine is set up to do: its credential helper,
    /// its SSH keys. What a person typing git in a terminal gets
    #[default]
    Own,
    /// The same, told which of the accounts this machine holds for `host` to
    /// sign in as. With two held, git would otherwise ask which
    PcAs { host: String, login: String },
    /// No credential at all. Every helper git has been given is set aside and
    /// SSH is not allowed to start, so nothing reaches a server as anybody
    Sealed,
    /// A token over HTTPS, handed only to `host`
    Token { host: String, login: String, token: String },
    /// This key file over SSH, and no other key
    Ssh { key: PathBuf },
}

/// Who a git runs as: the credentials it signs in with and the name its
/// commits carry. `Default` is git's own answer to both
#[derive(Clone, Default)]
pub struct As {
    pub auth: Auth,
    /// The account's name in the settings, for messages
    pub account: Option<String>,
    /// The commit identity. Absent is whatever git itself is set to
    pub name: Option<String>,
    pub email: Option<String>,
}

/// A credential helper that answers with the token from the environment, and
/// only for the one server. Written for the shell git runs helpers with, and
/// handed the token through the environment rather than the command line, where
/// anything listing processes could read it
const TOKEN_HELPER: &str = "!f() { test \"$1\" = get || return 0; h=; \
while IFS= read -r l && test -n \"$l\"; do case \"$l\" in host=*) h=\"${l#host=}\";; esac; done; \
test \"$h\" = \"$SHIKISHA_GIT_HOST\" || return 0; \
printf 'username=%s\\npassword=%s\\n' \"$SHIKISHA_GIT_LOGIN\" \"$SHIKISHA_GIT_TOKEN\"; }; f";

impl As {
    /// Nothing to sign in with, and git's own name on commits
    pub fn sealed() -> As {
        As { auth: Auth::Sealed, ..Default::default() }
    }

    /// Put this on a git about to start: `-c` settings go before the
    /// subcommand, which is why this is given the command before its args
    fn apply(&self, cmd: &mut std::process::Command) {
        if let Some(n) = &self.name {
            cmd.arg("-c").arg(format!("user.name={n}"));
        }
        if let Some(e) = &self.email {
            cmd.arg("-c").arg(format!("user.email={e}"));
        }
        let quote = |p: &Path| format!("'{}'", p.display().to_string().replace('\\', "/").replace('\'', "'\\''"));
        match &self.auth {
            Auth::Own => {}
            // Only for that server: the name a helper is handed for any other
            // one stays whatever git on this machine would hand it
            Auth::PcAs { host, login } => {
                cmd.arg("-c").arg(format!("credential.https://{host}.username={login}"));
            }
            // An empty helper throws away every helper configured before it --
            // the machine's, the user's, a per-server one -- so what follows is
            // the whole list
            Auth::Sealed => {
                cmd.arg("-c").arg("credential.helper=");
                cmd.env("GIT_SSH_COMMAND", "false");
            }
            Auth::Token { host, login, token } => {
                cmd.arg("-c").arg("credential.helper=");
                cmd.arg("-c").arg(format!("credential.helper={TOKEN_HELPER}"));
                cmd.env("SHIKISHA_GIT_HOST", host)
                    .env("SHIKISHA_GIT_LOGIN", login)
                    .env("SHIKISHA_GIT_TOKEN", token)
                    // An HTTPS account does not quietly go out over SSH as
                    // whoever this machine's keys belong to
                    .env("GIT_SSH_COMMAND", "false");
            }
            Auth::Ssh { key } => {
                cmd.arg("-c").arg("credential.helper=");
                cmd.env(
                    "GIT_SSH_COMMAND",
                    format!(
                        "ssh -i {} -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=accept-new",
                        quote(key)
                    ),
                );
            }
        }
    }
}

/// git itself is not on this PC: the one failure no folder or setting can fix.
#[derive(Debug)]
pub struct NotInstalled;

impl std::fmt::Display for NotInstalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::i18n::t("err.git.not_installed"))
    }
}

impl std::error::Error for NotInstalled {}

/// Whether a failure is git not being on this PC
pub fn is_not_installed(e: &anyhow::Error) -> bool {
    e.downcast_ref::<NotInstalled>().is_some()
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
    run_as(dir, args, input, limit, &As::default())
}

/// ...as somebody in particular
pub fn run_as(dir: &Path, args: &[&str], input: &str, limit: Duration, who: &As) -> Result<String> {
    run_bytes_as(dir, args, input.as_bytes(), limit, who).map(|out| String::from_utf8_lossy(&out).into_owned())
}

/// What git wrote, as the bytes it wrote. For the one kind of output that is
/// somebody's file rather than git's own words: a diff, whose lines are in
/// whatever encoding the file is, and can go back into `git apply` only as
/// those same bytes
pub fn run_bytes(dir: &Path, args: &[&str], input: &[u8], limit: Duration) -> Result<Vec<u8>> {
    run_bytes_as(dir, args, input, limit, &As::default())
}

fn run_bytes_as(dir: &Path, args: &[&str], input: &[u8], limit: Duration, who: &As) -> Result<Vec<u8>> {
    if !dir.is_dir() {
        bail!(crate::i18n::tp(
            "err.git.no_folder",
            &[("p", &dir.display().to_string())]
        ));
    }
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir);
    who.apply(&mut cmd);
    cmd.args(args)
        // Never sit waiting for a person who cannot see the prompt
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never")
        .stdin(if input.is_empty() { Stdio::null() } else { Stdio::piped() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::detach_console(&mut cmd).spawn().map_err(|e| match e.kind() {
        // Said as what it is. Left as the OS's "file not found", every caller
        // that turns a failure into words -- "not a repository" first of all --
        // was describing a folder when the program itself is missing
        std::io::ErrorKind::NotFound => anyhow::Error::new(NotInstalled),
        _ => e.into(),
    })?;
    if !input.is_empty() {
        use std::io::Write as _;
        if let Some(mut w) = child.stdin.take() {
            w.write_all(input)?;
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
        let said = without_line_ending_notes(&String::from_utf8_lossy(&stderr));
        if let Some(why) = pc_sign_in_said(&who.auth, &said) {
            bail!(why);
        }
        bail!(crate::i18n::tp(
            "err.git.failed",
            &[("cmd", &args.join(" ")), ("said", &said)]
        ));
    }
    Ok(stdout)
}

/// git's words for a failure, less its notes about line endings.
///
/// On Windows every command that touches a file checked out with LF prints
/// "LF will be replaced by CRLF" for it -- a dozen lines that are not the
/// failure, stacked on top of the one line that is
fn without_line_ending_notes(said: &str) -> String {
    said.lines()
        .filter(|l| !(l.starts_with("warning: in the working copy of ") && l.contains(" will be replaced by ")))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// A push git will not make because the branch follows one of another name:
/// `feature` following `origin/main` could mean either, and git asks rather
/// than guesses
#[derive(Debug)]
pub struct PushNameMismatch {
    pub branch: String,
    pub remote: String,
    pub target: String,
}

impl std::fmt::Display for PushNameMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::i18n::tp(
            "err.git.push_other_name",
            &[("branch", &self.branch), ("remote", &self.remote), ("target", &self.target)],
        ))
    }
}

impl std::error::Error for PushNameMismatch {}

/// The name of the branch this one follows on its server (`main` for one
/// following `origin/main`), from the branch's own settings. None when it
/// follows nothing
fn followed_name(dir: &Path, here: &str) -> Option<String> {
    let merge = run(dir, &["config", "--get", &format!("branch.{here}.merge")]).ok()?;
    let name = merge.trim().trim_start_matches("refs/heads/").to_string();
    (!name.is_empty()).then_some(name)
}

/// A pull that would write over files somebody has not committed yet.
///
/// Worked out from git's own lists rather than read out of its message: the
/// message is in whatever language git speaks on the PC, and the lists are the
/// same everywhere. The paths are the files in both -- changed here and not
/// committed, and changed by what the pull brings in
#[derive(Debug)]
pub struct PullBlocked(pub Vec<String>);

impl std::fmt::Display for PullBlocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const SHOWN: usize = 3;
        let mut names = self.0.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
        if self.0.len() > SHOWN {
            names.push_str(&crate::i18n::tp(
                "err.git.pull_blocked.more",
                &[("n", &(self.0.len() - SHOWN).to_string())],
            ));
        }
        f.write_str(&crate::i18n::tp("err.git.pull_blocked", &[("files", &names)]))
    }
}

impl std::error::Error for PullBlocked {}

/// The uncommitted files a pull would change, once the pull has fetched.
/// `theirs` is the branch it is pulling from. Empty when there are none or it
/// cannot be told
fn in_the_way(dir: &Path, theirs: &str) -> Vec<String> {
    let Ok(coming) = run(dir, &["diff", "--name-only", "-z", &format!("HEAD...{theirs}")]) else {
        return Vec::new();
    };
    let coming: std::collections::HashSet<&str> = coming.split('\0').filter(|p| !p.is_empty()).collect();
    let Ok(here) = status(dir) else { return Vec::new() };
    let mut both: Vec<String> = here
        .into_iter()
        .flat_map(|c| std::iter::once(c.path).chain(c.from))
        .filter(|p| coming.contains(p.as_str()))
        .collect();
    both.sort();
    both.dedup();
    both
}

/// What to say when git on this PC wanted to ask somebody how to sign in.
///
/// Git's own words are about a prompt that could not be shown, which says
/// nothing about what to do. The usual reason is a credential manager holding
/// two GitHub accounts and wanting to ask which, or one told to use an account
/// it does not hold -- and both are put right in the menu that chose it. None
/// when it was something else
fn pc_sign_in_said(auth: &Auth, said: &str) -> Option<String> {
    if !said.contains("user interactivity has been disabled") {
        return None;
    }
    match auth {
        Auth::PcAs { login, .. } if !crate::pr::pc_accounts().iter().any(|a| a == login) => {
            Some(crate::i18n::tp("err.git.pc_gone", &[("login", login)]))
        }
        Auth::Own => {
            let held = crate::pr::pc_accounts();
            (held.len() > 1).then(|| crate::i18n::tp("err.git.pc_many", &[("names", &held.join(", "))]))
        }
        _ => None,
    }
}

/// The top of the working tree the folder belongs to, or an error saying it
/// belongs to none. Everything else in this module starts here, so "not a
/// repository" is said once, in one wording
pub fn root(dir: &Path) -> Result<PathBuf> {
    let out = run(dir, &["rev-parse", "--show-toplevel"]).map_err(|e| match is_not_installed(&e) {
        true => e,
        false => anyhow::anyhow!(crate::i18n::tp(
            "err.git.no_repo",
            &[("p", &dir.display().to_string())]
        )),
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
/// tree; `path` narrows it to one file. Each file's lines are read in that
/// file's own encoding
pub fn diff(dir: &Path, path: Option<&str>, staged: bool) -> Result<String> {
    Ok(diff_text(&diff_bytes(dir, path, staged)?, None))
}

/// The same diff, as git wrote it
pub fn diff_bytes(dir: &Path, path: Option<&str>, staged: bool) -> Result<Vec<u8>> {
    let mut args: Vec<&str> = vec!["diff", "--no-color"];
    if staged {
        args.push("--cached");
    }
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    run_bytes(dir, &args, b"", LIMIT)
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
/// walked back one at a time. As git wrote it: the file's own bytes
pub fn show_bytes(dir: &Path, hash: &str, path: &str) -> Result<Vec<u8>> {
    let hash = hash.trim();
    if hash.is_empty() {
        bail!(crate::i18n::t("err.git.no_commit"));
    }
    let mut args: Vec<&str> = vec!["show", "--no-color", "--pretty=format:", "-m", "--first-parent", hash];
    if !path.is_empty() {
        args.push("--");
        args.push(path);
    }
    run_bytes(dir, &args, b"", LIMIT)
}

/// The branch on the server this one is compared with, and how far apart the
/// two are
#[derive(Debug, Clone, PartialEq)]
pub struct Follows {
    /// The branch there, as git spells it (`origin/main`)
    pub name: String,
    /// Commits here that are not there yet
    pub ahead: u32,
    /// Commits there that are not here
    pub behind: u32,
    /// Whether the branch is set to follow it. False for one found by its name
    /// alone: it was pushed from somewhere that did not set the upstream (a
    /// terminal without `--set-upstream`), and the two are still the same
    /// branch on the same server
    pub tracked: bool,
}

/// Where the branch checked out sends its commits, and how far apart the two
/// are. As of the last fetch -- asking the server is what fetch is for, and a
/// count that talked to it would make every look at the panel a network wait.
///
/// The branch it is set to follow, else the branch of its own name on the
/// server it would push to. The second because "this branch follows nothing"
/// is not the same as "this work is not on the server": a push from a terminal
/// without `--set-upstream` sends the commits and writes no setting, and a
/// screen that only asked git what the branch follows then said the work had
/// never been pushed while the server had every commit of it.
///
/// `None` when neither exists (never pushed anywhere) or the head is detached
pub fn upstream(dir: &Path) -> Result<Option<Follows>> {
    let set = run(dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"])
        .ok()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    if let Some(name) = set {
        let (ahead, behind) = apart(dir, "@{upstream}")?;
        return Ok(Some(Follows { name, ahead, behind, tracked: true }));
    }
    let Some(here) = branch(dir)? else { return Ok(None) };
    let ref_name = format!("refs/remotes/{}/{here}", push_remote(dir));
    if run(dir, &["rev-parse", "--verify", "--quiet", &ref_name]).is_err() {
        return Ok(None);
    }
    let name = ref_name.trim_start_matches("refs/remotes/").to_string();
    let (ahead, behind) = apart(dir, &name)?;
    Ok(Some(Follows { name, ahead, behind, tracked: false }))
}

/// How far HEAD is from `rev`: commits here it does not have, and commits it
/// has that are not here
fn apart(dir: &Path, rev: &str) -> Result<(u32, u32)> {
    // "behind<TAB>ahead": the left side is the other branch's own commits
    let counts = run(dir, &["rev-list", "--left-right", "--count", &format!("{rev}...HEAD")])?;
    let mut parts = counts.split_whitespace().map(|n| n.parse::<u32>().unwrap_or(0));
    let behind = parts.next().unwrap_or(0);
    let ahead = parts.next().unwrap_or(0);
    Ok((ahead, behind))
}

/// The server this branch's commits would go to: the one it is set to use,
/// else the one every push goes to unless told otherwise, else `origin`
fn push_remote(dir: &Path) -> String {
    let here = branch(dir).ok().flatten().unwrap_or_default();
    let asked = [
        (!here.is_empty()).then(|| format!("branch.{here}.pushRemote")),
        Some("remote.pushDefault".to_string()),
        (!here.is_empty()).then(|| format!("branch.{here}.remote")),
    ];
    for key in asked.into_iter().flatten() {
        if let Ok(v) = run(dir, &["config", "--get", &key]) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return v;
            }
        }
    }
    "origin".to_string()
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
#[derive(Debug)]
pub struct Hunk {
    pub file: String,
    /// The `@@ ... @@` line, as git wrote it
    pub header: String,
    /// The lines this hunk covers on the new side
    pub start: u32,
    pub end: u32,
    pub patch: String,
    /// What the file's lines in `patch` were read as, and so what they are
    /// written back as when it is applied
    pub encoding: &'static encoding_rs::Encoding,
    /// Whether writing `patch` back gives the bytes git wrote. A hunk read
    /// in the wrong encoding is still worth reading, but handed back it would
    /// put the misreading into the file
    pub exact: bool,
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
    // Cut at the newline and nothing else: in a file saved with Windows line
    // endings the carriage return is part of the line, and a patch without it
    // no longer matches the file it came from
    for line in diff.split_terminator('\n') {
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
                end: start + count.saturating_sub(1),
                patch: format!("{head}{line}\n"),
                encoding: encoding_rs::UTF_8,
                exact: true,
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

/// A diff as git wrote it, cut into hunks: each file's lines read in the
/// encoding they most likely are, or in `encoding` when somebody has said
pub fn split_hunks_bytes(diff: &[u8], encoding: Option<&'static encoding_rs::Encoding>) -> Vec<Hunk> {
    files_of(diff, encoding)
        .into_iter()
        .flat_map(|f| {
            split_hunks(&f.text).into_iter().map(move |mut h| {
                h.encoding = f.encoding;
                h.exact = f.exact;
                h
            })
        })
        .collect()
}

/// ...or kept whole, as text
pub fn diff_text(diff: &[u8], encoding: Option<&'static encoding_rs::Encoding>) -> String {
    files_of(diff, encoding).into_iter().map(|f| f.text).collect()
}

/// A diff, one file at a time.
///
/// Read per file because a commit can hold a UTF-8 source file and a
/// Shift_JIS CSV side by side. Only the file's own lines -- from the first
/// `@@` on -- are in the file's encoding; the header above them is git's
fn files_of(diff: &[u8], encoding: Option<&'static encoding_rs::Encoding>) -> Vec<crate::charset::Reading> {
    let mut out = Vec::new();
    let mut head: Vec<u8> = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    for line in diff.split_inclusive(|b| *b == b'\n') {
        if line.starts_with(b"diff --git ") {
            read_file(&mut head, &mut body, encoding, &mut out);
        }
        if body.is_empty() && !line.starts_with(b"@@") {
            head.extend_from_slice(line);
        } else {
            body.extend_from_slice(line);
        }
    }
    read_file(&mut head, &mut body, encoding, &mut out);
    out
}

fn read_file(
    head: &mut Vec<u8>,
    body: &mut Vec<u8>,
    encoding: Option<&'static encoding_rs::Encoding>,
    out: &mut Vec<crate::charset::Reading>,
) {
    if head.is_empty() && body.is_empty() {
        return;
    }
    let mut file = match encoding {
        Some(e) => crate::charset::read_as(body, e),
        None => crate::charset::read(body),
    };
    // The header goes back as UTF-8, so it is exact only if it was UTF-8
    file.exact &= std::str::from_utf8(head).is_ok();
    file.text.insert_str(0, &String::from_utf8_lossy(head));
    out.push(file);
    head.clear();
    body.clear();
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
pub fn apply(dir: &Path, patch: &str, encoding: &'static encoding_rs::Encoding, cached: bool, reverse: bool) -> Result<()> {
    if patch.trim().is_empty() {
        bail!(crate::i18n::t("err.git.empty_patch"));
    }
    // The file's lines go back in the file's encoding, and git's own lines as
    // git wrote them. A line that cannot be written exactly stops it: a
    // replacement mark is what a misread line looks like, and applied it would
    // become part of the file
    let mut body: Vec<u8> = Vec::with_capacity(patch.len() + 1);
    let mut in_file = false;
    for line in patch.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            in_file = false;
        } else if line.starts_with("@@") {
            in_file = true;
        }
        if !in_file {
            body.extend_from_slice(line.as_bytes());
            continue;
        }
        match crate::charset::write_as(line, encoding).filter(|_| !line.contains('\u{FFFD}')) {
            Some(bytes) => body.extend_from_slice(&bytes),
            None => bail!(crate::i18n::tp("err.git.unwritable_patch", &[("enc", encoding.name())])),
        }
    }
    if !body.ends_with(b"\n") {
        body.push(b'\n');
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
    run_bytes(dir, &args, &body, LIMIT).map(|_| ())
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
pub fn merge(dir: &Path, name: &str, who: &As) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!(crate::i18n::t("err.git.empty_branch"));
    }
    // A merge can make a commit, and that commit carries a name
    run_as(dir, &["merge", "--no-edit", name], "", LIMIT, who)
}

/// Whether this account can reach this repository's remotes at all.
///
/// A token cannot sign in over SSH and a key cannot sign in over HTTPS. Asked
/// before running rather than left to git, whose answer to either is a
/// sentence about the network that sends somebody looking in the wrong place.
/// Only when every remote is the other kind: a repository with both can still
/// be reached
fn fits(dir: &Path, who: &As) -> Result<()> {
    let wants_ssh = match &who.auth {
        Auth::Token { .. } => false,
        Auth::Ssh { .. } => true,
        Auth::Own | Auth::PcAs { .. } | Auth::Sealed => return Ok(()),
    };
    let listed = run(dir, &["remote", "-v"]).unwrap_or_default();
    let urls: Vec<&str> = listed.lines().filter_map(|l| l.split_whitespace().nth(1)).collect();
    if urls.is_empty() || urls.iter().any(|u| is_ssh_url(u) == wants_ssh) {
        return Ok(());
    }
    let name = who.account.clone().unwrap_or_default();
    bail!(crate::i18n::tp(
        if wants_ssh { "err.git.account.wants_token" } else { "err.git.account.wants_ssh" },
        &[("name", &name)]
    ))
}

/// Whether git would reach this remote over SSH: `ssh://…`, or the short
/// `user@host:path` form, which has no scheme and a colon before any slash.
/// A drive letter (`C:/…`) is a folder, not a server
pub fn is_ssh_url(url: &str) -> bool {
    let u = url.trim();
    if let Some((scheme, _)) = u.split_once("://") {
        return scheme.eq_ignore_ascii_case("ssh") || scheme.eq_ignore_ascii_case("git+ssh");
    }
    match (u.find(':'), u.find('/')) {
        (Some(c), slash) => c > 1 && slash.is_none_or(|s| c < s),
        _ => false,
    }
}

/// The branch a branch was cut from, as written when its worktree was made
/// (`origin/main`, `origin/develop`). None when nothing was written
pub fn recorded_base(dir: &Path, branch: &str) -> Option<String> {
    run(dir, &["config", "--get", &format!("branch.{branch}.shikishaBase")])
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Write down what a branch was cut from, so it is not asked for again
pub fn record_base(dir: &Path, branch: &str, base: &str) -> Result<()> {
    let base = base.trim();
    if base.is_empty() {
        bail!(crate::i18n::t("err.git.empty_branch"));
    }
    run(dir, &["config", &format!("branch.{branch}.shikishaBase"), base]).map(|_| ())
}

/// The branches the servers have, as this PC last heard: `origin/main` and so
/// on, without the pointer each server keeps to its default
pub fn remote_branches(dir: &Path) -> Result<Vec<String>> {
    let out = run(dir, &["for-each-ref", "--format=%(refname:short)", "refs/remotes"])?;
    let remotes = run(dir, &["remote"]).unwrap_or_default();
    Ok(out
        .lines()
        .map(str::trim)
        // `origin/HEAD` is written short as the remote's own name
        .filter(|r| !r.is_empty() && !r.ends_with("/HEAD") && !remotes.lines().any(|m| m.trim() == *r))
        .map(str::to_string)
        .collect())
}

/// The commands that bring the latest of a branch's base in: fetch that one
/// branch from its server, then merge what was fetched -- never the local copy
/// of the base, which can be days old. Built once, and both shown on screen and
/// run from here, so what is shown is what runs
pub fn catch_up_steps(dir: &Path, base: &str) -> Vec<Vec<String>> {
    catch_up_steps_for(dir, None, base)
}

/// The same for a pull request's branch, which is on the server as well: that
/// branch is fetched first, so a folder behind what was pushed is told apart
/// before anything is merged into it. Every step but the last is a fetch
pub fn catch_up_steps_for(dir: &Path, head: Option<&str>, base: &str) -> Vec<Vec<String>> {
    let base = base.trim();
    let remotes = run(dir, &["remote"]).unwrap_or_default();
    // `origin/develop` names its server; a bare `develop` is origin's
    let (remote, branch) = match base.split_once('/') {
        Some((r, b)) if remotes.lines().any(|m| m.trim() == r) => (r.to_string(), b.to_string()),
        _ => ("origin".to_string(), base.to_string()),
    };
    let fetch = |b: &str| vec!["fetch".into(), remote.clone(), format!("+refs/heads/{b}:refs/remotes/{remote}/{b}")];
    let mut steps = Vec::new();
    if let Some(h) = head.map(str::trim).filter(|h| !h.is_empty() && *h != branch) {
        steps.push(fetch(h));
    }
    steps.push(fetch(&branch));
    steps.push(vec!["merge".into(), "--no-edit".into(), format!("{remote}/{branch}")]);
    steps
}

/// The same commands as a person would type them
pub fn catch_up_said(steps: &[Vec<String>]) -> Vec<String> {
    steps.iter().map(|s| format!("git {}", s.join(" "))).collect()
}

/// Why bringing the latest in did not happen, or stopped
#[derive(Debug)]
pub enum CatchUpStop {
    /// Changes nobody has committed: a merge on top of them is not started
    Dirty,
    /// The merge stopped on these files, and is left as it stopped
    Conflict { base: String, files: Vec<String> },
    /// What was pushed of the branch is ahead of this folder: merged here, it
    /// could not be pushed back. Nothing is started
    Behind { branch: String, count: u64 },
}

impl std::fmt::Display for CatchUpStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&match self {
            CatchUpStop::Dirty => crate::i18n::t("err.git.catch_up.dirty"),
            CatchUpStop::Conflict { base, files } => crate::i18n::tp(
                "err.git.catch_up.conflict",
                &[("base", base), ("files", &files.join(", "))],
            ),
            CatchUpStop::Behind { branch, count } => crate::i18n::tp(
                "err.git.catch_up.behind",
                &[("branch", branch), ("n", &count.to_string())],
            ),
        })
    }
}

impl std::error::Error for CatchUpStop {}

/// Whether the merge left half done in this folder is one bringing `rev` in:
/// what it was merging is the very commit `rev` names. So a page opened after
/// the merge stopped can still say what stopped and offer the way on
pub fn merging_in(dir: &Path, rev: &str) -> bool {
    let at = |r: &str| run(dir, &["rev-parse", "-q", "--verify", &format!("{r}^{{commit}}")]).ok().map(|s| s.trim().to_string());
    matches!((at("MERGE_HEAD"), at(rev)), (Some(a), Some(b)) if !a.is_empty() && a == b)
}

/// Bring the latest of `base` into the branch in front: refused while anything
/// is uncommitted, and on a conflict left where the merge stopped. Answers with
/// how many commits came in -- 0 when there was nothing new
pub fn catch_up(dir: &Path, base: &str, who: &As) -> Result<u64> {
    catch_up_for(dir, None, base, who)
}

/// [`catch_up`] for a pull request's branch: what was pushed of `head` is
/// fetched too, and a folder behind it is refused before the merge
pub fn catch_up_for(dir: &Path, head: Option<&str>, base: &str, who: &As) -> Result<u64> {
    fits(dir, who)?;
    // Untracked files are nobody's work in progress as far as a merge goes; a
    // change to a file git follows is
    if status(dir)?.iter().any(|c| c.index != '?') {
        return Err(anyhow::Error::new(CatchUpStop::Dirty));
    }
    let steps = catch_up_steps_for(dir, head, base);
    let Some((merge, fetches)) = steps.split_last() else { bail!(crate::i18n::t("err.git.empty_branch")) };
    fn args(s: &[String]) -> Vec<&str> {
        s.iter().map(String::as_str).collect()
    }
    for fetch in fetches {
        run_as(dir, &args(fetch), "", NETWORK_LIMIT, who)?;
    }
    // The pull request's branch as pushed: its own fetch names where it landed
    if fetches.len() > 1
        && let Some(pushed) = fetches[0].last().and_then(|r| r.rsplit_once(':')).map(|(_, to)| to.trim_start_matches("refs/remotes/").to_string())
    {
        let count = run(dir, &["rev-list", "--count", &format!("HEAD..{pushed}")])?.trim().parse::<u64>().unwrap_or(0);
        if count > 0 {
            return Err(anyhow::Error::new(CatchUpStop::Behind { branch: pushed, count }));
        }
    }
    let theirs = merge.last().cloned().unwrap_or_default();
    let taken = run(dir, &["rev-list", "--count", &format!("HEAD..{theirs}")])?
        .trim()
        .parse::<u64>()
        .unwrap_or(0);
    if taken == 0 {
        return Ok(0);
    }
    if let Err(e) = run_as(dir, &args(merge), "", LIMIT, who) {
        let files = conflicts(dir).unwrap_or_default();
        if files.is_empty() {
            return Err(e);
        }
        return Err(anyhow::Error::new(CatchUpStop::Conflict { base: theirs, files }));
    }
    Ok(taken)
}

pub fn fetch(dir: &Path, who: &As) -> Result<String> {
    fits(dir, who)?;
    run_as(dir, &["fetch", "--prune"], "", NETWORK_LIMIT, who)
}

pub fn pull(dir: &Path, who: &As) -> Result<String> {
    fits(dir, who)?;
    // A branch that follows nothing leaves a bare `git pull` with nothing to
    // read, so where to pull from is said outright -- the branch of this one's
    // name on the server (see [`upstream`]), which is what a person would type.
    // Its name is `<remote>/<branch>`, and a remote's name holds no slash,
    // while a branch's may
    let named = match upstream(dir)? {
        Some(f) if !f.tracked => f.name.split_once('/').map(|(r, b)| (r.to_string(), b.to_string())),
        _ => None,
    };
    let args: Vec<&str> = match &named {
        Some((remote, branch)) => vec!["pull", remote, branch],
        None => vec!["pull"],
    };
    let theirs = match &named {
        Some((remote, branch)) => format!("{remote}/{branch}"),
        None => "@{upstream}".to_string(),
    };
    run_as(dir, &args, "", NETWORK_LIMIT, who).map_err(|e| {
        let both = in_the_way(dir, &theirs);
        if both.is_empty() { e } else { anyhow::Error::new(PullBlocked(both)) }
    })
}

/// Send it. A branch made here has never been pushed, so the first push is the
/// common one rather than the exception -- and answering "set an upstream and
/// try again" to somebody who just pressed a button called Push is asking them
/// to type the thing the button was for. The retry says out loud what it did
pub fn push(dir: &Path, who: &As) -> Result<String> {
    fits(dir, who)?;
    match run_as(dir, &["push"], "", NETWORK_LIMIT, who) {
        Ok(out) => Ok(out),
        Err(first) => {
            let Some(here) = branch(dir)? else { return Err(first) };
            // Following a branch of another name, git will not choose between
            // sending to that one and sending under this name. Said in those
            // words, worked out from the branch's own settings rather than
            // read out of git's message
            if let Some(target) = followed_name(dir, &here)
                && target != here
            {
                let remote = run(dir, &["config", "--get", &format!("branch.{here}.remote")])
                    .map(|r| r.trim().to_string())
                    .ok()
                    .filter(|r| !r.is_empty())
                    .unwrap_or_else(|| "origin".to_string());
                return Err(anyhow::Error::new(PushNameMismatch { branch: here, remote, target }));
            }
            if !first.to_string().contains("--set-upstream") {
                return Err(first);
            }
            let said = run_as(
                dir,
                &["push", "--set-upstream", "origin", &here],
                "",
                NETWORK_LIMIT,
                who,
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
    who: &As,
) -> Result<String> {
    if message.trim().is_empty() {
        bail!(crate::i18n::t("err.git.empty_message"));
    }
    if !allow_protected
        && let Some(b) = branch(dir)?
            && is_protected(&b, protect) {
                bail!(crate::i18n::tp("err.git.protected", &[("branch", &b)]));
            }
    // Amend rewrites the commit that is already there. On a branch nobody else
    // has, that is tidying; on a shared one it is rewriting what other people
    // have -- which is why it goes through the same refusal as a plain commit
    let mut args = vec!["commit", "-m", message];
    if amend {
        args.push("--amend");
    }
    run_as(dir, &args, "", LIMIT, who)?;
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
        assert!(is_protected("release/", &theirs), "* matches even when there is nothing");
        assert!(!is_protected("main", &theirs), "what was not written is not guarded");
        assert!(!is_protected("hotfix/release/1.0", &theirs), "it matches from the start");

        // `*` is the whole of the language. Everything else is the name
        // itself, because a branch really can be called `v1.0+fix`
        let odd = ["v1.0+fix".to_string(), "*/wip".to_string()];
        assert!(is_protected("v1.0+fix", &odd));
        assert!(!is_protected("v1Z0+fix", &odd), ". is only ever .");
        assert!(is_protected("team/wip", &odd));
        assert!(!is_protected("wip", &odd), "the / before * is part of the name");
        assert!(is_protected("anything at all", &["*".to_string()]), "* alone guards everything");

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

    /// What git's credential machinery hands out for a server, asked the way
    /// git asks it before a fetch
    fn credential_for(dir: &Path, host: &str, who: &As) -> Option<String> {
        run_as(
            dir,
            &["credential", "fill"],
            &format!("protocol=https\nhost={host}\n\n"),
            LIMIT,
            who,
        )
        .ok()
    }

    /// Asking a folder about a repository when git is not on the PC says that
    /// git is not on the PC, not that the folder is not a repository
    #[test]
    fn a_missing_git_is_said_as_a_missing_git() {
        let err = anyhow::Error::new(NotInstalled);
        assert!(is_not_installed(&err));
        assert!(!is_not_installed(&anyhow::anyhow!("anything else")));
        assert!(!err.to_string().is_empty());
    }

    /// A bare server, a clone of it to work in, and a branch cut from its main
    /// the way a worktree's is -- for the catching-up tests below
    fn catch_up_setup(tag: &str) -> Option<(std::path::PathBuf, std::path::PathBuf, std::path::PathBuf)> {
        let seed = scratch_repo(&format!("{tag}-seed"))?;
        std::fs::write(seed.join("a.txt"), "one\n").unwrap();
        run(&seed, &["add", "."]).unwrap();
        run(&seed, &["commit", "-m", "one"]).unwrap();
        let tmp = std::env::temp_dir();
        let far = tmp.join(format!("shikisha-git-{}-{tag}-far", std::process::id()));
        let near = tmp.join(format!("shikisha-git-{}-{tag}-near", std::process::id()));
        let _ = std::fs::remove_dir_all(&far);
        let _ = std::fs::remove_dir_all(&near);
        run(&seed, &["clone", "-q", "--bare", &seed.display().to_string(), &far.display().to_string()]).unwrap();
        run(&seed, &["clone", "-q", &far.display().to_string(), &near.display().to_string()]).unwrap();
        for d in [&near, &seed] {
            run(d, &["config", "user.email", "test@example.invalid"]).unwrap();
            run(d, &["config", "user.name", "test"]).unwrap();
        }
        run(&seed, &["remote", "add", "far", &far.display().to_string()]).unwrap();
        run(&near, &["checkout", "-q", "-b", "work", "--no-track", "origin/main"]).unwrap();
        Some((seed, far, near))
    }

    /// The server's main moves on while the local main stays behind
    fn catch_up_advance(seed: &Path, file: &str, text: &str) {
        std::fs::write(seed.join(file), text).unwrap();
        run(seed, &["add", "."]).unwrap();
        run(seed, &["commit", "-qm", "far"]).unwrap();
        run(seed, &["push", "-q", "far", "main"]).unwrap();
    }

    #[test]
    fn catching_up_is_refused_with_uncommitted_work_and_changes_nothing() {
        let Some((seed, far, near)) = catch_up_setup("cu-dirty") else { return };
        record_base(&near, "work", "origin/main").unwrap();
        catch_up_advance(&seed, "b.txt", "new\n");
        std::fs::write(near.join("a.txt"), "mine\n").unwrap();
        let head = run(&near, &["rev-parse", "HEAD"]).unwrap();
        let err = catch_up(&near, "origin/main", &As::default()).unwrap_err();
        assert!(matches!(err.downcast_ref::<CatchUpStop>(), Some(CatchUpStop::Dirty)), "{err}");
        assert_eq!(run(&near, &["rev-parse", "HEAD"]).unwrap(), head, "the branch moved");
        assert_eq!(std::fs::read_to_string(near.join("a.txt")).unwrap(), "mine\n", "the work was touched");
        for d in [&near, &far, &seed] { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    fn a_base_not_written_down_is_none_and_one_chosen_is_kept() {
        let Some((seed, far, near)) = catch_up_setup("cu-base") else { return };
        assert_eq!(recorded_base(&near, "work"), None);
        assert!(remote_branches(&near).unwrap().contains(&"origin/main".to_string()));
        record_base(&near, "work", "origin/main").unwrap();
        assert_eq!(recorded_base(&near, "work").as_deref(), Some("origin/main"));
        assert_eq!(
            catch_up_said(&catch_up_steps(&near, "origin/main")),
            vec!["git fetch origin +refs/heads/main:refs/remotes/origin/main".to_string(),
                 "git merge --no-edit origin/main".to_string()]
        );
        for d in [&near, &far, &seed] { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    fn the_latest_on_the_server_comes_in_even_when_the_local_main_is_old() {
        let Some((seed, far, near)) = catch_up_setup("cu-latest") else { return };
        catch_up_advance(&seed, "b.txt", "new\n");
        catch_up_advance(&seed, "c.txt", "newer\n");
        // The local main is where it was cloned, two commits behind the server
        let local_main = run(&near, &["rev-parse", "main"]).unwrap();
        assert_eq!(catch_up(&near, "origin/main", &As::default()).unwrap(), 2);
        assert!(near.join("c.txt").exists(), "the server's latest did not come in");
        assert_eq!(run(&near, &["rev-parse", "main"]).unwrap(), local_main, "the local main was used or moved");
        assert_eq!(catch_up(&near, "origin/main", &As::default()).unwrap(), 0, "a second time there is nothing new");
        for d in [&near, &far, &seed] { let _ = std::fs::remove_dir_all(d); }
    }

    #[test]
    fn a_conflict_stops_where_the_merge_stopped_and_names_the_files() {
        let Some((seed, far, near)) = catch_up_setup("cu-conflict") else { return };
        std::fs::write(near.join("a.txt"), "mine\n").unwrap();
        run(&near, &["commit", "-qam", "mine"]).unwrap();
        catch_up_advance(&seed, "a.txt", "theirs\n");
        let err = catch_up(&near, "origin/main", &As::default()).unwrap_err();
        match err.downcast_ref::<CatchUpStop>() {
            Some(CatchUpStop::Conflict { base, files }) => {
                assert_eq!(base, "origin/main");
                assert_eq!(files, &vec!["a.txt".to_string()]);
            }
            other => panic!("not a conflict: {other:?} {err}"),
        }
        let merging = run(&near, &["rev-parse", "-q", "--verify", "MERGE_HEAD"]);
        assert!(merging.is_ok(), "the merge was not left where it stopped");
        assert!(merging_in(&near, "origin/main"), "the stopped merge is not told apart as this one");
        assert!(!merging_in(&near, "main"), "another branch was taken for the one being merged");
        for d in [&near, &far, &seed] { let _ = std::fs::remove_dir_all(d); }
    }

    /// A pull request's branch: what was pushed of it is fetched first, and
    /// shown as a step of its own
    #[test]
    fn a_pull_requests_branch_is_fetched_before_its_base_is_merged() {
        let Some((seed, far, near)) = catch_up_setup("cu-pr-steps") else { return };
        assert_eq!(
            catch_up_said(&catch_up_steps_for(&near, Some("work"), "origin/main")),
            vec!["git fetch origin +refs/heads/work:refs/remotes/origin/work".to_string(),
                 "git fetch origin +refs/heads/main:refs/remotes/origin/main".to_string(),
                 "git merge --no-edit origin/main".to_string()]
        );
        for d in [&near, &far, &seed] { let _ = std::fs::remove_dir_all(d); }
    }

    /// Merged into a folder behind what was pushed, the result could not be
    /// pushed back: refused, and nothing is touched
    #[test]
    fn a_folder_behind_its_pushed_branch_is_refused_before_the_merge() {
        let Some((seed, far, near)) = catch_up_setup("cu-pr-behind") else { return };
        std::fs::write(near.join("w.txt"), "pushed\n").unwrap();
        run(&near, &["add", "."]).unwrap();
        run(&near, &["commit", "-qm", "pushed"]).unwrap();
        run(&near, &["push", "-q", "origin", "work"]).unwrap();
        run(&near, &["reset", "-q", "--hard", "HEAD~1"]).unwrap();
        catch_up_advance(&seed, "b.txt", "new\n");
        let head = run(&near, &["rev-parse", "HEAD"]).unwrap();
        let err = catch_up_for(&near, Some("work"), "origin/main", &As::default()).unwrap_err();
        match err.downcast_ref::<CatchUpStop>() {
            Some(CatchUpStop::Behind { branch, count }) => {
                assert_eq!(branch, "origin/work");
                assert_eq!(*count, 1);
            }
            other => panic!("not refused as behind: {other:?} {err}"),
        }
        assert_eq!(run(&near, &["rev-parse", "HEAD"]).unwrap(), head, "the branch moved");
        assert!(!near.join("b.txt").exists(), "the base was merged anyway");
        // Up to date with what was pushed, the same call brings the base in
        run(&near, &["merge", "-q", "--ff-only", "origin/work"]).unwrap();
        assert_eq!(catch_up_for(&near, Some("work"), "origin/main", &As::default()).unwrap(), 1);
        for d in [&near, &far, &seed] { let _ = std::fs::remove_dir_all(d); }
    }

    /// A push refused because the branch follows one of another name says
    /// both names and both ways out, not git's page about push.default
    #[test]
    fn a_push_to_a_branch_of_another_name_is_said_plainly() {
        let Some(seed) = scratch_repo("othername-seed") else { return };
        std::fs::write(seed.join("a.txt"), "one\n").unwrap();
        run(&seed, &["add", "."]).unwrap();
        run(&seed, &["commit", "-m", "one"]).unwrap();
        let far = std::env::temp_dir().join(format!("shikisha-git-{}-othername-far", std::process::id()));
        let near = std::env::temp_dir().join(format!("shikisha-git-{}-othername-near", std::process::id()));
        let _ = std::fs::remove_dir_all(&far);
        let _ = std::fs::remove_dir_all(&near);
        run(&seed, &["clone", "-q", "--bare", &seed.display().to_string(), &far.display().to_string()]).unwrap();
        run(&seed, &["clone", "-q", &far.display().to_string(), &near.display().to_string()]).unwrap();
        run(&near, &["config", "user.email", "test@example.invalid"]).unwrap();
        run(&near, &["config", "user.name", "test"]).unwrap();
        run(&near, &["config", "push.default", "simple"]).unwrap();
        run(&near, &["checkout", "-q", "-b", "feature", "--track", "origin/main"]).unwrap();
        std::fs::write(near.join("a.txt"), "two\n").unwrap();
        run(&near, &["commit", "-qam", "two"]).unwrap();

        let err = push(&near, &As::default()).unwrap_err();
        let said = err.downcast_ref::<PushNameMismatch>().expect("the refusal is not recognised");
        assert_eq!((said.branch.as_str(), said.remote.as_str(), said.target.as_str()), ("feature", "origin", "main"));
        let text = err.to_string();
        assert!(text.contains("feature") && text.contains("origin/main"), "{text}");
        assert!(!text.contains("push.default"), "git's own page came through: {text}");
        let _ = std::fs::remove_dir_all(&near);
        let _ = std::fs::remove_dir_all(&far);
        let _ = std::fs::remove_dir_all(&seed);
    }

    /// A pull refused because it would write over uncommitted work names the
    /// files in the way, and nothing but them
    #[test]
    fn a_pull_in_the_way_of_uncommitted_work_names_the_files() {
        let Some(far) = scratch_repo("blocked-far") else { return };
        for f in ["a.txt", "b.txt"] {
            std::fs::write(far.join(f), "one\n").unwrap();
        }
        run(&far, &["add", "."]).unwrap();
        run(&far, &["commit", "-m", "one"]).unwrap();
        let near = std::env::temp_dir().join(format!("shikisha-git-{}-blocked-near", std::process::id()));
        let _ = std::fs::remove_dir_all(&near);
        run(&far, &["clone", "-q", &far.display().to_string(), &near.display().to_string()]).unwrap();

        // There: a.txt changes. Here: a.txt and b.txt are edited, not committed
        std::fs::write(far.join("a.txt"), "far\n").unwrap();
        run(&far, &["commit", "-am", "far"]).unwrap();
        std::fs::write(near.join("a.txt"), "near\n").unwrap();
        std::fs::write(near.join("b.txt"), "near\n").unwrap();

        let err = pull(&near, &As::default()).unwrap_err();
        let blocked = err.downcast_ref::<PullBlocked>().expect("the refusal is not recognised");
        assert_eq!(blocked.0, vec!["a.txt".to_string()], "only the file in the way is named");
        assert!(err.to_string().contains("a.txt"), "{err}");

        // Out of the way, the same pull goes through
        run(&near, &["checkout", "--", "a.txt"]).unwrap();
        pull(&near, &As::default()).expect("the pull still fails with nothing in the way");
        let _ = std::fs::remove_dir_all(&near);
        let _ = std::fs::remove_dir_all(&far);
    }

    /// A branch that follows nothing still pulls: what came in elsewhere is
    /// taken from the branch of its own name on the server.
    ///
    /// The screen offers the pull, because it counts against that branch. A
    /// bare `git pull` has nothing to read there and answers "there is no
    /// tracking information for the current branch", so where to pull from is
    /// said outright
    #[test]
    fn a_branch_that_follows_nothing_pulls_from_its_own_name() {
        let Some(far) = scratch_repo("byname-far") else { return };
        std::fs::write(far.join("a.txt"), "one
").unwrap();
        run(&far, &["add", "."]).unwrap();
        run(&far, &["commit", "-m", "one"]).unwrap();
        let near = std::env::temp_dir().join(format!("shikisha-git-{}-byname-near", std::process::id()));
        let _ = std::fs::remove_dir_all(&near);
        run(&far, &["clone", "-q", &far.display().to_string(), &near.display().to_string()]).unwrap();
        run(&near, &["config", "user.email", "test@example.invalid"]).unwrap();
        run(&near, &["config", "user.name", "test"]).unwrap();

        // A branch made here, sent from a terminal with no --set-upstream
        run(&near, &["checkout", "-q", "-b", "side"]).unwrap();
        run(&near, &["push", "-q", &far.display().to_string(), "side:side"]).unwrap();
        run(&near, &["fetch", "-q"]).unwrap();
        assert!(run(&near, &["rev-parse", "@{upstream}"]).is_err(), "it follows something after all");

        // A commit arrives on that branch over there
        run(&far, &["checkout", "-q", "side"]).unwrap();
        std::fs::write(far.join("b.txt"), "theirs
").unwrap();
        run(&far, &["add", "."]).unwrap();
        run(&far, &["commit", "-m", "theirs"]).unwrap();
        run(&near, &["fetch", "-q"]).unwrap();
        assert_eq!(
            upstream(&near).unwrap().map(|f| (f.name, f.ahead, f.behind)),
            Some(("origin/side".to_string(), 0, 1)),
            "the screen would not offer a pull at all"
        );

        pull(&near, &As::default()).expect("a branch that follows nothing cannot pull");
        assert!(near.join("b.txt").is_file(), "what was pulled did not arrive");
        let _ = std::fs::remove_dir_all(&near);
        let _ = std::fs::remove_dir_all(&far);
    }

    #[test]
    fn a_failure_is_not_buried_under_line_ending_notes() {
        let said = "warning: in the working copy of 'a.md', LF will be replaced by CRLF the next time Git touches it\n\
                    error: something real\n\
                    warning: in the working copy of 'b.rs', CRLF will be replaced by LF the next time Git touches it";
        assert_eq!(without_line_ending_notes(said), "error: something real");
    }

    /// A branch that follows another says how many commits each side has that
    /// the other does not; one that follows nothing is compared with the
    /// branch of its own name on the server, and says nothing only when there
    /// is no such branch either
    #[test]
    fn a_branch_counts_what_it_has_to_send_and_to_take() {
        let Some(far) = scratch_repo("upstream-far") else { return };
        std::fs::write(far.join("a.txt"), "one").unwrap();
        run(&far, &["add", "."]).unwrap();
        run(&far, &["commit", "-m", "one"]).unwrap();
        let near = std::env::temp_dir().join(format!("shikisha-git-{}-upstream-near", std::process::id()));
        let _ = std::fs::remove_dir_all(&near);
        run(&far, &["clone", "-q", &far.display().to_string(), &near.display().to_string()]).unwrap();
        run(&near, &["config", "user.email", "test@example.invalid"]).unwrap();
        run(&near, &["config", "user.name", "test"]).unwrap();
        let follows = |name: &str, ahead, behind, tracked| {
            Some(Follows { name: name.to_string(), ahead, behind, tracked })
        };
        assert_eq!(upstream(&near).unwrap(), follows("origin/main", 0, 0, true));

        // Two commits here, one there
        for n in ["two", "three"] {
            std::fs::write(near.join("a.txt"), n).unwrap();
            run(&near, &["commit", "-am", n]).unwrap();
        }
        std::fs::write(far.join("b.txt"), "far").unwrap();
        run(&far, &["add", "."]).unwrap();
        run(&far, &["commit", "-m", "far"]).unwrap();
        assert_eq!(upstream(&near).unwrap(), follows("origin/main", 2, 0, true), "not fetched yet");
        run(&near, &["fetch", "-q"]).unwrap();
        assert_eq!(upstream(&near).unwrap(), follows("origin/main", 2, 1, true));

        run(&near, &["checkout", "-q", "-b", "alone"]).unwrap();
        assert_eq!(upstream(&near).unwrap(), None, "a branch never pushed is compared with something");

        // Sent from elsewhere without being set to follow anything -- a push
        // from a terminal with no `--set-upstream`. The work is on the server,
        // and the screen that only asked what the branch follows said it had
        // never been pushed
        std::fs::write(near.join("c.txt"), "alone").unwrap();
        run(&near, &["add", "."]).unwrap();
        run(&near, &["commit", "-m", "alone"]).unwrap();
        run(&near, &["push", "-q", &far.display().to_string(), "alone:alone"]).unwrap();
        run(&near, &["fetch", "-q"]).unwrap();
        assert!(
            run(&near, &["rev-parse", "--abbrev-ref", "@{upstream}"]).is_err(),
            "the branch follows something after all, so this proves nothing"
        );
        assert_eq!(
            upstream(&near).unwrap(),
            follows("origin/alone", 0, 0, false),
            "the branch of its own name on the server was not found"
        );
        // ...and one commit later it is a push, not a first publish
        std::fs::write(near.join("c.txt"), "more").unwrap();
        run(&near, &["commit", "-am", "more"]).unwrap();
        assert_eq!(upstream(&near).unwrap(), follows("origin/alone", 1, 0, false));
        let _ = std::fs::remove_dir_all(&near);
        let _ = std::fs::remove_dir_all(&far);
    }

    #[test]
    fn a_token_account_answers_its_own_server_and_no_other() {
        let Some(dir) = scratch_repo("token") else { return };
        let who = As {
            auth: Auth::Token {
                host: "github.com".into(),
                login: "someone".into(),
                token: "tok$en 'with' \"quotes\"".into(),
            },
            ..Default::default()
        };
        let said = credential_for(&dir, "github.com", &who).expect("the token is handed to its server");
        assert!(said.contains("username=someone"), "{said}");
        assert!(said.contains("password=tok$en 'with' \"quotes\""), "the token arrives whole: {said}");
        // Another server gets nothing, and with nobody to ask, git gives up
        // rather than answering with the token or anything else
        assert!(
            credential_for(&dir, "gitlab.example", &who).is_none_or(|s| !s.contains("tok$en")),
            "the token went to a server it is not for"
        );
    }

    /// The PC's git told which account to be: the name reaches the credential
    /// helper for that server, and a helper asked about any other server is
    /// handed whatever git on this machine would hand it
    #[test]
    fn the_pcs_git_is_told_its_account_for_that_server_only() {
        let Some(dir) = scratch_repo("pcas") else { return };
        let who = As {
            auth: Auth::PcAs { host: "github.com".into(), login: "octo-cat".into() },
            ..Default::default()
        };
        let named = |url: &str| {
            run_as(&dir, &["config", "--get-urlmatch", "credential.username", url], "", LIMIT, &who)
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        assert_eq!(named("https://github.com/owner/repo.git"), "octo-cat");
        assert_ne!(named("https://gitlab.example/owner/repo.git"), "octo-cat");
    }

    /// Git's words for a prompt it could not show are said as what to do only
    /// when that is what they are about
    #[test]
    fn only_a_prompt_that_could_not_be_shown_is_said_again() {
        let prompt = "fatal: Cannot prompt because user interactivity has been disabled.";
        assert_eq!(pc_sign_in_said(&Auth::Own, "fatal: repository not found"), None);
        assert_eq!(pc_sign_in_said(&Auth::Sealed, prompt), None);
        let gone = Auth::PcAs { host: "github.com".into(), login: "nobody-has-this-name-here".into() };
        assert!(pc_sign_in_said(&gone, prompt).is_some_and(|s| s.contains("nobody-has-this-name-here")));
    }

    #[test]
    fn a_sealed_git_has_no_credentials_at_all() {
        let Some(dir) = scratch_repo("sealed") else { return };
        // Whatever this machine has set up -- a credential manager, a store --
        // is set aside, so nothing is handed out
        let said = credential_for(&dir, "github.com", &As::sealed());
        assert!(said.is_none_or(|s| !s.contains("password=")), "a credential came out of a sealed git");
    }

    #[test]
    fn an_account_signs_its_commits() {
        let Some(dir) = scratch_repo("identity") else { return };
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        let who = As {
            name: Some("Work Name".into()),
            email: Some("work@example.invalid".into()),
            ..As::sealed()
        };
        commit(&dir, "first", &[], false, false, &who).unwrap();
        let by = run(&dir, &["log", "-1", "--format=%an <%ae>"]).unwrap();
        assert_eq!(by.trim(), "Work Name <work@example.invalid>");
    }

    #[test]
    fn a_key_account_uses_that_key() {
        let Some(dir) = scratch_repo("sshkey") else { return };
        let key = dir.join("no such key");
        let who = As { auth: Auth::Ssh { key: key.clone() }, ..Default::default() };
        // Nothing listens on port 9 here, so it fails either way; what ssh
        // says on the way is which key it was told to use
        let err = run_as(&dir, &["ls-remote", "ssh://git@127.0.0.1:9/x.git"], "", LIMIT, &who)
            .unwrap_err()
            .to_string();
        if err.contains("not found") && !err.contains("no such key") && err.contains("ssh") {
            return; // no ssh on this machine
        }
        assert!(err.contains("no such key"), "ssh was not handed the key: {err}");
    }

    #[test]
    fn the_wrong_kind_of_account_is_refused_before_it_runs() {
        let Some(dir) = scratch_repo("fits") else { return };
        run(&dir, &["remote", "add", "origin", "git@github.com:someone/thing.git"]).unwrap();
        let token = As {
            auth: Auth::Token { host: "github.com".into(), login: "x".into(), token: "t".into() },
            account: Some("work".into()),
            ..Default::default()
        };
        let err = fetch(&dir, &token).unwrap_err().to_string();
        assert!(err.contains("work"), "the account is named: {err}");
        assert!(is_ssh_url("git@github.com:someone/thing.git"));
        assert!(is_ssh_url("ssh://git@host:22/x"));
        assert!(!is_ssh_url("https://github.com/someone/thing.git"));
        assert!(!is_ssh_url("C:/repos/thing"));
        assert!(!is_ssh_url("/srv/repos/thing"));
    }

    #[test]
    fn a_shared_branch_says_no_before_it_commits() {
        let Some(dir) = scratch_repo("protected") else { return };
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();

        // On main it refuses, and says which branch it is refusing about --
        // whoever catches this offers to make a branch instead
        let refused = commit(&dir, "first", &guarded(), false, false, &As::default()).unwrap_err().to_string();
        assert!(refused.contains("main"), "the reason for refusing includes the branch name: {refused}");
        // Nothing was committed by the refusal
        assert!(log(&dir, 1).map(|l| l.is_empty()).unwrap_or(true));

        // ...and it goes through for someone who says they meant it
        let hash = commit(&dir, "first", &guarded(), true, false, &As::default()).expect("it gets through when done knowingly");
        assert!(!hash.is_empty());
        assert_eq!(log(&dir, 1).unwrap()[0].subject, "first");

        // On a branch of its own, nothing is in the way
        run(&dir, &["checkout", "-q", "-b", "feature"]).unwrap();
        std::fs::write(dir.join("a.txt"), "hello again").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "second", &guarded(), false, false, &As::default()).expect("it does not stop on your own branch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_way_out_of_a_refusal_keeps_the_work() {
        let Some(dir) = scratch_repo("branchnew") else { return };
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "start", &guarded(), true, false, &As::default()).unwrap();

        // Something staged, on a branch that will not take it
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        assert!(commit(&dir, "next", &guarded(), false, false, &As::default()).is_err());

        // The offer: a branch, and the staged work still staged on it
        branch_create(&dir, "work/next").expect("a branch can be made");
        assert_eq!(branch(&dir).unwrap().as_deref(), Some("work/next"));
        let rows = status(&dir).unwrap();
        assert_eq!(rows.iter().find(|c| c.path == "a.txt").unwrap().index, 'M',
            "what is staged comes along when moving");
        commit(&dir, "next", &guarded(), false, false, &As::default()).expect("it gets through on the branch moved to");

        assert!(branch_create(&dir, "  ").is_err(), "an empty name is refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_staged_and_what_is_not_are_told_apart() {
        let Some(dir) = scratch_repo("status") else { return };
        std::fs::write(dir.join("kept.txt"), "one").unwrap();
        stage(&dir, &["kept.txt".to_string()]).unwrap();
        commit(&dir, "start", &guarded(), true, false, &As::default()).unwrap();

        std::fs::write(dir.join("kept.txt"), "two").unwrap();
        std::fs::write(dir.join("fresh.txt"), "new").unwrap();
        stage(&dir, &["kept.txt".to_string()]).unwrap();

        let rows = status(&dir).unwrap();
        let staged = rows.iter().find(|c| c.path == "kept.txt").expect("the changed line shows");
        assert_eq!(staged.index, 'M', "the staged side is modified");
        let untracked = rows.iter().find(|c| c.path == "fresh.txt").expect("new files show too");
        assert_eq!((untracked.index, untracked.work), ('?', '?'));

        // ...and taking it back out moves it to the other side
        unstage(&dir, &["kept.txt".to_string()]).unwrap();
        let after = status(&dir).unwrap();
        let back = after.iter().find(|c| c.path == "kept.txt").unwrap();
        assert_eq!(back.index, ' ', "it was unstaged");
        assert_eq!(back.work, 'M', "it is still in the working tree");

        // The diff has the words that changed in it
        let d = diff(&dir, Some("kept.txt"), false).unwrap();
        assert!(d.contains("+two"), "the diff has the changed line: {d}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_history_comes_back_drawn_and_in_pieces() {
        let Some(dir) = scratch_repo("history") else { return };
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "first", &guarded(), true, false, &As::default()).unwrap();
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        std::fs::write(dir.join("b.txt"), "new").unwrap();
        stage(&dir, &["a.txt".to_string(), "b.txt".to_string()]).unwrap();
        commit(&dir, "second\n\nwith a reason", &guarded(), true, false, &As::default()).unwrap();

        let rows = graph(&dir, false, false, 10, None).unwrap();
        let commits: Vec<&Line> = rows.iter().filter(|r| !r.hash.is_empty()).collect();
        assert_eq!(commits.len(), 2, "as many rows as commits");
        assert!(commits[0].graph.contains('*'), "the picture git draws comes along");
        assert_eq!(commits[0].subject, "second");
        assert!(commits[0].date.starts_with("20"), "the date and time are in a readable form: {}", commits[0].date);

        let d = detail(&dir, &commits[0].hash).unwrap();
        assert_eq!(d.subject, "second");
        assert_eq!(d.body, "with a reason");
        assert_eq!(d.parents, vec![commits[1].hash.clone()], "it has one parent");
        assert_eq!(d.files, vec!["a.txt".to_string(), "b.txt".to_string()]);
        assert!(!d.committer.is_empty() && !d.commit_date.is_empty());

        // What it did to one file, cut into pieces that can be walked back
        let patch = show_bytes(&dir, &commits[0].hash, "a.txt").unwrap();
        let hunks = split_hunks_bytes(&patch, None);
        assert_eq!(hunks.len(), 1);
        assert!(hunks[0].patch.contains("+two"));
        // ...and walking one back leaves the file as it was before that commit
        apply(&dir, &hunks[0].patch, hunks[0].encoding, false, true).expect("it can be undone");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap().trim(), "one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_branches_are_listed_with_the_one_you_are_on() {
        let Some(dir) = scratch_repo("branches") else { return };
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "start", &guarded(), true, false, &As::default()).unwrap();
        branch_create(&dir, "side").unwrap();

        let list = branches(&dir).unwrap();
        let names: Vec<&str> = list.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"main") && names.contains(&"side"), "{names:?}");
        assert_eq!(
            list.iter().filter(|(_, here)| *here).map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["side"],
            "only the current branch is marked"
        );

        checkout(&dir, "main").expect("it can go back");
        assert_eq!(branch(&dir).unwrap().as_deref(), Some("main"));
        assert!(checkout(&dir, "no-such-branch").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn amend_rewrites_rather_than_adds() {
        let Some(dir) = scratch_repo("amend") else { return };
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        stage(&dir, &["a.txt".to_string()]).unwrap();
        commit(&dir, "frist", &guarded(), true, false, &As::default()).unwrap();

        commit(&dir, "first", &guarded(), true, true, &As::default()).expect("it can be rewritten");
        let log = log(&dir, 5).unwrap();
        assert_eq!(log.len(), 1, "the commit count does not grow");
        assert_eq!(log[0].subject, "first", "the reworded one is kept");
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
        assert_eq!(hunks.len(), 2, "one per @@");
        assert_eq!(hunks[0].file, "f.txt");
        assert_eq!((hunks[0].start, hunks[0].end), (1, 4));
        assert_eq!((hunks[1].start, hunks[1].end), (21, 22));
        // Each piece carries the file's header, so it stands on its own as a
        // patch -- which is the whole point of cutting it this way
        assert!(hunks[1].patch.starts_with("diff --git a/f.txt b/f.txt\n"));
        assert!(hunks[1].patch.contains("+++ b/f.txt\n"));
        assert!(hunks[1].patch.contains("@@ -20,2 +21,2 @@\n-old\n+new\n"));
        assert!(!hunks[1].patch.contains("+two"), "neighboring hunks do not mix");
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
        commit(&dir, "start", &guarded(), true, false, &As::default()).unwrap();

        // Two changes, far enough apart to be two hunks
        let edited = format!(
            "{}\n",
            start.replace("line 1\n", "LINE ONE\n").replace("line 20", "LINE TWENTY")
        );
        std::fs::write(dir.join("f.txt"), edited).unwrap();
        let hunks = split_hunks(&diff(&dir, Some("f.txt"), false).unwrap());
        assert_eq!(hunks.len(), 2, "two places far apart are two hunks: {hunks:?}",
            hunks = hunks.iter().map(|h| h.header.clone()).collect::<Vec<_>>());

        // Stage the first one only
        apply(&dir, &hunks[0].patch, hunks[0].encoding, true, false).expect("a single hunk can be staged");
        let staged = diff(&dir, Some("f.txt"), true).unwrap();
        assert!(staged.contains("LINE ONE"), "the one chosen is in");
        assert!(!staged.contains("LINE TWENTY"), "the one not chosen is not in");
        // ...and the other is still waiting in the tree
        let left = diff(&dir, Some("f.txt"), false).unwrap();
        assert!(left.contains("LINE TWENTY") && !left.contains("LINE ONE"));

        // Taking it back out again
        apply(&dir, &hunks[0].patch, hunks[0].encoding, true, true).expect("it can be taken back");
        assert!(diff(&dir, Some("f.txt"), true).unwrap().trim().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A spreadsheet's CSV: Shift_JIS, with Windows line endings. What changed
    /// in it reads as the words it holds, and one piece of it can be staged
    /// and taken back without a byte of the file changing on the way
    #[test]
    fn a_shift_jis_csv_is_read_and_staged_as_itself() {
        let Some(dir) = scratch_repo("sjis") else { return };
        // The line endings kept as they are, as on any machine not set to
        // convert them: the carriage returns are then in the diff itself
        run(&dir, &["config", "core.autocrlf", "false"]).unwrap();
        let sjis = |text: &str| crate::charset::write_as(text, encoding_rs::SHIFT_JIS).unwrap();
        let rows = |changed: &str| {
            let mut out = String::from("氏名,住所\r\n");
            for n in 1..=20 {
                out.push_str(&format!("{},東京都{n}丁目\r\n", if n == 2 { changed } else { "山田" }));
            }
            out
        };
        std::fs::write(dir.join("名簿.csv"), sjis(&rows("山田"))).unwrap();
        stage(&dir, &["名簿.csv".to_string()]).unwrap();
        commit(&dir, "start", &[], true, false, &As::default()).unwrap();
        let changed = sjis(&rows("佐藤"));
        std::fs::write(dir.join("名簿.csv"), &changed).unwrap();

        let raw = diff_bytes(&dir, Some("名簿.csv"), false).unwrap();
        let hunks = split_hunks_bytes(&raw, None);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].encoding, encoding_rs::SHIFT_JIS);
        assert!(hunks[0].exact);
        assert!(hunks[0].patch.contains("+佐藤,東京都2丁目"), "{}", hunks[0].patch);
        assert!(diff(&dir, Some("名簿.csv"), false).unwrap().contains("-山田,東京都2丁目"));

        apply(&dir, &hunks[0].patch, hunks[0].encoding, true, false).expect("the piece can be staged");
        let staged = run_bytes(&dir, &["show", ":名簿.csv"], b"", LIMIT).unwrap();
        assert_eq!(staged, changed, "what is staged is the file, byte for byte");
        apply(&dir, &hunks[0].patch, hunks[0].encoding, true, true).expect("it can be taken back");
        assert!(diff_bytes(&dir, Some("名簿.csv"), true).unwrap().is_empty());

        // Read as the wrong encoding it can still be looked at, but it is not
        // exact, and handing it back is refused rather than written
        let wrong = split_hunks_bytes(&raw, Some(encoding_rs::UTF_8));
        assert!(!wrong[0].exact);
        assert!(apply(&dir, &wrong[0].patch, wrong[0].encoding, true, false).is_err());
        assert!(diff_bytes(&dir, Some("名簿.csv"), true).unwrap().is_empty(), "nothing was staged");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn each_file_in_a_diff_is_read_in_its_own_encoding() {
        let mut raw = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1 +1 @@\n-old\n+新しい\n"
            .as_bytes()
            .to_vec();
        raw.extend_from_slice(b"diff --git a/b.csv b/b.csv\n--- a/b.csv\n+++ b/b.csv\n@@ -1 +1 @@\n-old\n+");
        raw.extend_from_slice(&crate::charset::write_as("新しい行です", encoding_rs::SHIFT_JIS).unwrap());
        raw.push(b'\n');
        let hunks = split_hunks_bytes(&raw, None);
        assert_eq!(hunks.len(), 2);
        assert_eq!((hunks[0].file.as_str(), hunks[0].encoding), ("a.txt", encoding_rs::UTF_8));
        assert!(hunks[0].patch.ends_with("+新しい\n"));
        assert_eq!((hunks[1].file.as_str(), hunks[1].encoding), ("b.csv", encoding_rs::SHIFT_JIS));
        assert!(hunks[1].patch.ends_with("+新しい行です\n"));
        assert!(hunks.iter().all(|h| h.exact));
    }

    #[test]
    fn a_conflict_is_listed_as_one() {
        let Some(dir) = scratch_repo("conflict") else { return };
        std::fs::write(dir.join("c.txt"), "base").unwrap();
        stage(&dir, &["c.txt".to_string()]).unwrap();
        commit(&dir, "base", &guarded(), true, false, &As::default()).unwrap();

        run(&dir, &["checkout", "-q", "-b", "other"]).unwrap();
        std::fs::write(dir.join("c.txt"), "theirs").unwrap();
        stage(&dir, &["c.txt".to_string()]).unwrap();
        commit(&dir, "theirs", &guarded(), false, false, &As::default()).unwrap();

        run(&dir, &["checkout", "-q", "main"]).unwrap();
        std::fs::write(dir.join("c.txt"), "ours").unwrap();
        stage(&dir, &["c.txt".to_string()]).unwrap();
        commit(&dir, "ours", &guarded(), true, false, &As::default()).unwrap();

        // The merge fails, which is the point: what matters is that the file
        // can then be found by name rather than by reading git's message
        assert!(run(&dir, &["merge", "other"]).is_err());
        assert_eq!(conflicts(&dir).unwrap(), vec!["c.txt".to_string()]);
        let rows = status(&dir).unwrap();
        let row = rows.iter().find(|c| c.path == "c.txt").unwrap();
        assert!(
            row.index == 'U' || row.work == 'U' || (row.index == 'A' && row.work == 'A'),
            "it is marked as a conflict: {}{}",
            row.index,
            row.work
        );
        assert!(row.tangled, "both sides are still in it");
        assert_eq!(tangled(&dir).unwrap(), vec!["c.txt".to_string()]);

        // Sorted out by hand: git still calls it unmerged until it is staged,
        // and that is exactly the state that must not ask for help again
        std::fs::write(dir.join("c.txt"), "ours and theirs").unwrap();
        let after = status(&dir).unwrap();
        let row = after.iter().find(|c| c.path == "c.txt").unwrap();
        assert!(!row.tangled, "once the markers are gone, there is nothing left to resolve");
        assert!(conflicts(&dir).unwrap().contains(&"c.txt".to_string()), "git still treats it as unmerged");
        assert!(tangled(&dir).unwrap().is_empty(), "nothing is left to resolve");
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
            assert!(!c.path.is_empty(), "a row with no path shows");
            assert!(!c.index.is_whitespace() || !c.work.is_whitespace());
        }
        assert!(log(here, 3).expect("log reads").len() <= 3);
    }
}

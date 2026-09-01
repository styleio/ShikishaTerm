//! Whether a working folder is actually on this machine.
//!
//! Settings travel. The same `config.json` is carried between two PCs on a
//! sync folder or a USB stick, and it names working folders by absolute path —
//! so a folder that is right on one machine is simply not there on the other.
//! Until now nothing said so: a tab whose folder was missing launched anyway,
//! in the app's own folder, because the launch path quietly dropped a `cwd` it
//! could not find. An agent told to work on a project would happily work on
//! whatever it landed in instead, and the sidebar showed nothing unusual.
//!
//! So the question is asked out loud, and the answer is kept here.
//!
//! **Asked away from the drawing.** `is_dir()` on a disconnected network drive
//! blocks for as long as the operating system feels like — tens of seconds —
//! and a window that stops painting during that is a window everybody blames
//! us for. Every look happens on a thread of its own and the answer is
//! collected later, so a dead drive costs one stuck thread and a line on
//! screen saying which drive is not answering, rather than a frozen app.
//!
//! Nothing here runs `git`, in the spirit of [`crate::repo`]: what a folder is
//! and where it came from are file reads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

/// The one table, for the whole app.
///
/// Whether a drive is on this machine is a fact about the machine, not about
/// any one screen, and two tables would mean two answers and two sets of
/// threads asking the same disk the same question.
pub fn watch() -> &'static Watch {
    static WATCH: OnceLock<Watch> = OnceLock::new();
    WATCH.get_or_init(Watch::new)
}

/// How long the launch path waits for an answer before going ahead anyway.
pub const BEFORE_LAUNCH: Duration = Duration::from_millis(300);

/// How long an answer is trusted before the folder is looked at again.
///
/// A USB stick gets plugged in, a network drive comes back, a folder is made
/// by hand in Explorer. None of those tell us, so the only way to notice is to
/// keep asking — rarely enough to cost nothing, often enough that the person
/// who just plugged the stick in does not have to restart the app.
const FRESH: Duration = Duration::from_secs(3);

/// How long a look may take before it is worth mentioning.
///
/// A folder on a local disk answers in well under a millisecond, so a spinner
/// for it would be a flicker that says only that the app is busy. What this
/// delay leaves is the case actually worth showing: a drive that is not
/// answering.
const SLOW: Duration = Duration::from_millis(400);

/// What is true of a working folder right now.
///
/// Three separate answers rather than one "missing", because the person's next
/// move is different in each. A drive that is not on this machine is not
/// something to create — whoever left the USB stick at home wants to be told
/// that, not offered an empty folder in its place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(tag = "as", rename_all = "lowercase")]
pub enum Health {
    /// It is there
    #[default]
    Fine,
    /// Still being looked at, and long enough for that to be worth saying.
    /// `secs` is how long it has been waiting, so the line can count up
    Looking { drive: String, secs: u64 },
    /// The drive itself is not on this machine
    NoDrive { drive: String },
    /// The drive is here; the folder is not
    Missing,
}

impl Health {
    /// Whether this is a state a person has to do something about.
    pub fn wrong(&self) -> bool {
        matches!(self, Health::NoDrive { .. } | Health::Missing)
    }
}

/// The drive a path is on, as something to show a person (`D:`).
///
/// A UNC path (`\\server\share`) has no drive letter, so it has none to be
/// missing: it is either reachable or it is not, and there is nothing useful
/// to say about a letter that was never involved.
pub fn drive_of(p: &Path) -> Option<String> {
    let s = p.to_string_lossy();
    let mut chars = s.chars();
    let letter = chars.next()?;
    if !letter.is_ascii_alphabetic() || chars.next()? != ':' {
        return None;
    }
    Some(format!("{}:", letter.to_ascii_uppercase()))
}

/// One folder's last answer, and whether a new one is on its way.
struct Entry {
    health: Health,
    /// When the answer being held was arrived at. Absent while the very first
    /// look is still out, which is what makes a folder nobody has looked at
    /// yet different from one that answered
    settled: Option<Instant>,
    /// When the look that is out now started. Absent when none is out
    started: Option<Instant>,
}

/// Everything known about the working folders, kept up to date in the
/// background.
///
/// Shared by handing out clones: one table, however many places ask.
#[derive(Clone, Default)]
pub struct Watch {
    known: Arc<Mutex<HashMap<PathBuf, Entry>>>,
}

impl Watch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask about these folders, and answer with what is known so far.
    ///
    /// Called from the drawing, every frame. It never waits: whatever has come
    /// back is returned, and anything stale or never asked is sent off to a
    /// thread to be looked at. The first frame after a folder appears in the
    /// settings therefore says [`Health::Fine`] for a local folder (the look
    /// finishes in microseconds) and says nothing yet for a slow one, which is
    /// exactly the difference worth drawing.
    ///
    /// These are also the folders worth remembering: anything else is dropped,
    /// so switching workspace stops the old one's drives being polled forever.
    pub fn look(&self, paths: &[PathBuf]) -> HashMap<PathBuf, Health> {
        self.ask(paths, true)
    }

    /// What is true of one folder, waited for — briefly.
    ///
    /// For the one caller that cannot come back next frame: deciding whether to
    /// start a tab. A folder on a local disk answers in microseconds, so this
    /// is not a wait at all. A drive that is not answering runs out the
    /// deadline and is called fine, which means the tab is launched and fails
    /// on its own terms — an honest failure beats an app that will not open.
    pub fn settled(&self, p: &Path, within: Duration) -> Health {
        let one = [p.to_path_buf()];
        let until = Instant::now() + within;
        loop {
            // Never evicting: this asks about one folder, and the table
            // belongs to all of them
            self.ask(&one, false);
            if let Some(h) = self.answered(p) {
                return h;
            }
            if Instant::now() >= until {
                return Health::Fine;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// What came back about this folder, if anything has.
    fn answered(&self, p: &Path) -> Option<Health> {
        let known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        let e = known.get(p)?;
        e.settled.is_some().then(|| e.health.clone())
    }

    fn ask(&self, paths: &[PathBuf], evict: bool) -> HashMap<PathBuf, Health> {
        let mut out = HashMap::with_capacity(paths.len());
        let now = Instant::now();
        let mut send: Vec<PathBuf> = Vec::new();
        {
            let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
            if evict {
                known.retain(|k, _| paths.iter().any(|p| p == k));
            }
            for p in paths {
                let e = known.entry(p.clone()).or_insert(Entry {
                    health: Health::Fine,
                    settled: None,
                    started: None,
                });
                // Out for long enough to be worth mentioning, and only then:
                // a local folder answers before this ever shows
                if let Some(at) = e.started {
                    if now.duration_since(at) >= SLOW {
                        out.insert(
                            p.clone(),
                            Health::Looking {
                                drive: drive_of(p).unwrap_or_default(),
                                secs: now.duration_since(at).as_secs(),
                            },
                        );
                        continue;
                    }
                }
                // Nothing has come back yet and it has not been long enough to
                // say so. Say what was true last time, which on the very first
                // frame is "fine" — a folder is innocent until looked at, so a
                // warning never flashes up on startup and then vanishes
                if let Some(settled) = e.settled {
                    out.insert(p.clone(), e.health.clone());
                    if e.started.is_none() && now.duration_since(settled) >= FRESH {
                        e.started = Some(now);
                        send.push(p.clone());
                    }
                } else {
                    out.insert(p.clone(), Health::Fine);
                    if e.started.is_none() {
                        e.started = Some(now);
                        send.push(p.clone());
                    }
                }
            }
        }
        // Started outside the lock: spawning is cheap but the lock is held by
        // the drawing, and a thread that is born holding it would have the app
        // waiting on the very thing this exists to avoid
        for p in send {
            let known = Arc::clone(&self.known);
            std::thread::spawn(move || {
                let health = health_of(&p);
                if let Ok(mut map) = known.lock() {
                    // Only if it is still wanted: the answer to a question
                    // about a folder that has since been closed is not an
                    // answer about anything
                    if let Some(e) = map.get_mut(&p) {
                        e.health = health;
                        e.settled = Some(Instant::now());
                        e.started = None;
                    }
                }
            });
        }
        out
    }
}

/// The one look, on whichever thread is doing the waiting.
///
/// The drive is asked about first and separately. Both questions can block, but
/// only one of them has an answer worth its own words: "there is no D: drive on
/// this machine" is a different problem from "D: is here and that folder is
/// not", and asking about the folder alone cannot tell them apart.
fn health_of(p: &Path) -> Health {
    if p.is_dir() {
        return Health::Fine;
    }
    if let Some(drive) = drive_of(p) {
        if !Path::new(&format!("{drive}\\")).is_dir() {
            return Health::NoDrive { drive };
        }
    }
    Health::Missing
}

// ---------------------------------------------------------------------------
// Putting a folder back
// ---------------------------------------------------------------------------

/// One thing that has to happen for a folder to exist here.
///
/// Every one of them carries the command that will run, assembled by the same
/// code that runs it: a line built twice is a line that will differ once, and
/// the person is being asked to approve exactly this one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "do", rename_all = "lowercase")]
pub enum Step {
    /// Bring the project itself. Nothing of it is on this machine yet
    Clone { url: String, to: String, line: String },
    /// Give the branch its own folder, from the checkout that is here
    Expand { branch: String, to: String, line: String },
    /// Make an ordinary folder. Nothing git about it
    Make { to: String },
}

/// Why a folder cannot simply be put back.
///
/// Each of these is something the app must not decide on its own. Two of them
/// exist because `git clone` says only "already exists and is not an empty
/// directory" for three different situations, one of which is not a problem at
/// all — so what is actually there is worked out here, before anything runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "why", rename_all = "lowercase")]
pub enum Blocked {
    /// A different project is sitting where this one has to go
    OtherProject { at: String, found: String, wanted: String },
    /// Something that is not a repository is in the way. Windows hides
    /// `desktop.ini`, which a sync folder leaves everywhere, so an apparently
    /// empty folder is refused by git for a reason nobody can see — which is
    /// why what is in there is named rather than described
    NotEmpty { at: String, holds: Vec<String> },
    /// The branch is already open elsewhere in this same clone. Git allows one
    /// working folder per branch per clone; two machines holding the same
    /// branch is ordinary, two folders on one machine is not
    BranchTaken { branch: String, at: String },
    /// Nothing was written down about where this folder came from
    Unknown,
    /// The drive is not on this machine, so nothing can be put on it
    NoDrive { drive: String },
}

/// What is sitting where the checkout has to go.
enum Sitting {
    /// Nothing, or nothing that stops a clone
    Room,
    /// This very project, already here
    Ours,
    /// A repository, but not this one
    Theirs(String),
    /// Not a repository, and not empty
    Stuff(Vec<String>),
}

/// Everything that has to happen, or the one thing standing in the way.
///
/// `checkout` is where the project itself lives on this machine, when the
/// settings say. Left out, it is worked out from the folder's own path, which
/// is the shape this app gives a branch's folder in the first place.
pub fn plan(
    cwd: &Path,
    source: &crate::config::Source,
    checkout: Option<&Path>,
) -> Result<Vec<Step>, Blocked> {
    use crate::config::Source;
    if let Some(drive) = drive_of(cwd) {
        if !Path::new(&drive_root(&drive)).is_dir() {
            return Err(Blocked::NoDrive { drive });
        }
    }
    if cwd.is_dir() {
        return Ok(Vec::new());
    }
    let (origin, branch, base) = match source {
        Source::Plain => return Ok(vec![Step::Make { to: cwd.display().to_string() }]),
        Source::Unknown => return Err(Blocked::Unknown),
        Source::Worktree { origin, branch, base } => (origin, branch, base),
    };
    let at = match checkout {
        Some(p) => p.to_path_buf(),
        None => checkout_for(cwd, branch).ok_or(Blocked::Unknown)?,
    };
    let mut steps = Vec::new();
    // 1-4: the project itself. Decided before anything runs, because git's own
    // answer cannot tell "somebody else's project is here" from "yours already
    // is", and one of those two is not a problem
    match sitting_at(&at, origin) {
        Sitting::Room => {
            let argv = clone_argv(origin, &at);
            steps.push(Step::Clone {
                url: scrub(origin),
                to: at.display().to_string(),
                line: said(&argv),
            });
        }
        Sitting::Ours => {}
        Sitting::Theirs(found) => {
            return Err(Blocked::OtherProject {
                at: at.display().to_string(),
                found,
                wanted: scrub(origin),
            });
        }
        Sitting::Stuff(holds) => {
            return Err(Blocked::NotEmpty { at: at.display().to_string(), holds });
        }
    }
    // 5: the branch. Only askable of a clone that is already here — one that is
    // about to be made holds nothing yet, so nothing can be in the way
    if steps.is_empty() {
        if let Some(open) = branch_open_at(&at, branch) {
            return Err(Blocked::BranchTaken {
                branch: branch.clone(),
                at: open.display().to_string(),
            });
        }
    }
    let argv = expand_argv(&at, branch, base, cwd);
    steps.push(Step::Expand {
        branch: branch.clone(),
        to: cwd.display().to_string(),
        line: said(&argv),
    });
    Ok(steps)
}

/// Carries out one step. What runs is the same `argv` the plan showed.
pub fn take(step: &Step, source: &crate::config::Source) -> anyhow::Result<()> {
    use crate::config::Source;
    match step {
        Step::Make { to } => Ok(std::fs::create_dir_all(to)?),
        Step::Clone { url, to, .. } => crate::worktree::run(&clone_argv(url, Path::new(to))),
        Step::Expand { branch, to, .. } => {
            let Source::Worktree { base, .. } = source else {
                anyhow::bail!("nothing says which project this folder belongs to");
            };
            let at = checkout_for(Path::new(to), branch)
                .ok_or_else(|| anyhow::anyhow!("the project's own folder could not be found"))?;
            // git makes the folders on the way down, so nothing has to be made
            // first — and a folder made first would have to be empty anyway,
            // which is the one thing a half-finished attempt leaves it not
            crate::worktree::run(&expand_argv(&at, branch, base, Path::new(to)))
        }
    }
}

/// The root of a drive, spelled the way the operating system wants it.
fn drive_root(drive: &str) -> String {
    format!("{drive}{}", std::path::MAIN_SEPARATOR)
}

fn clone_argv(url: &str, to: &Path) -> Vec<String> {
    vec!["git".into(), "clone".into(), scrub(url), to.display().to_string()]
}

/// What expands a branch into its own folder.
///
/// The branch is named one of two ways because it may be in one of two states,
/// and only one command works for each: known to git already — here or on the
/// remote — or gone entirely and having to be started again from whatever it
/// grew from.
fn expand_argv(at: &Path, branch: &str, base: &str, to: &Path) -> Vec<String> {
    let mut v: Vec<String> = vec![
        "git".into(),
        "-C".into(),
        at.display().to_string(),
        "worktree".into(),
        "add".into(),
    ];
    if !branch_known(at, branch) {
        v.push("-b".into());
        v.push(branch.to_string());
        v.push(to.display().to_string());
        v.push(match base.trim().is_empty() {
            true => "HEAD".into(),
            false => base.to_string(),
        });
        return v;
    }
    v.push(to.display().to_string());
    v.push(branch.to_string());
    v
}

/// The command as a person reads it, quoted where a path has a space in it.
fn said(argv: &[String]) -> String {
    argv.iter()
        .map(|a| match a.contains(' ') {
            true => format!("\"{a}\""),
            false => a.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A remote URL with any credentials taken out of it.
///
/// These get written into settings that are carried between machines on a sync
/// folder, and shown on screen. A URL with `user:token@` in it would put a live
/// credential in both places, and a token that has been synced somewhere cannot
/// be taken back. Git has its own place to keep them, and that is where they
/// belong.
pub fn scrub(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    match rest.split_once('@') {
        Some((_, host)) => format!("{scheme}://{host}"),
        None => url.to_string(),
    }
}

/// Whether two remotes name the same repository.
///
/// Compared with the credentials off and the trailing `.git` and slash gone:
/// one machine's `origin` is written by whoever cloned it, and the same project
/// is reached by more than one spelling.
fn same_repo(a: &str, b: &str) -> bool {
    let tidy = |u: &str| {
        scrub(u)
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .to_lowercase()
    };
    !a.trim().is_empty() && tidy(a) == tidy(b)
}

/// What is in the place the checkout has to go.
fn sitting_at(at: &Path, origin: &str) -> Sitting {
    if !at.exists() {
        return Sitting::Room;
    }
    if let Some(found) = crate::repo::remote_url_of(at) {
        return match same_repo(&found, origin) {
            true => Sitting::Ours,
            false => Sitting::Theirs(scrub(&found)),
        };
    }
    // Every entry counts, hidden ones included: git refuses a folder holding
    // only `desktop.ini`, which a sync folder puts everywhere and Explorer does
    // not show. Naming what is in there is the difference between "this looks
    // like a bug in your app" and "ah, that file"
    let holds: Vec<String> = std::fs::read_dir(at)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .take(8)
        .collect();
    match holds.is_empty() {
        true => Sitting::Room,
        false => Sitting::Stuff(holds),
    }
}

/// Which working folder of this clone already holds the branch, if one does.
///
/// Read rather than asked of git, in the spirit of [`crate::repo`]: a clone
/// mid-rebase answers anyway, and it costs a couple of small files.
fn branch_open_at(at: &Path, branch: &str) -> Option<PathBuf> {
    let want = format!("ref: refs/heads/{branch}");
    let git = at.join(".git");
    if std::fs::read_to_string(git.join("HEAD")).is_ok_and(|h| h.trim() == want) {
        return Some(at.to_path_buf());
    }
    for entry in std::fs::read_dir(git.join("worktrees")).into_iter().flatten().flatten() {
        let held = entry.path();
        if !std::fs::read_to_string(held.join("HEAD")).is_ok_and(|h| h.trim() == want) {
            continue;
        }
        // `gitdir` names the `.git` file inside the working folder, so the
        // folder is its parent
        let Ok(named) = std::fs::read_to_string(held.join("gitdir")) else {
            continue;
        };
        return Some(
            PathBuf::from(named.trim())
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| held.clone()),
        );
    }
    None
}

/// Whether git already has this branch, here or on the remote.
fn branch_known(at: &Path, branch: &str) -> bool {
    let git = at.join(".git");
    if git.join("refs/heads").join(branch).exists()
        || git.join("refs/remotes/origin").join(branch).exists()
    {
        return true;
    }
    // Refs that have been packed away live in one file instead
    std::fs::read_to_string(git.join("packed-refs")).is_ok_and(|text| {
        let heads = format!(" refs/heads/{branch}");
        let remote = format!(" refs/remotes/origin/{branch}");
        text.lines().any(|l| l.ends_with(&heads) || l.ends_with(&remote))
    })
}

/// Where the project itself must be, worked out from a branch folder's path.
///
/// The reverse of the shape this app gives a branch's folder:
/// `<parent>/<name>.worktrees/<branch as folders>`. A branch with slashes in it
/// is that many folders deep, which is why the branch has to be known to walk
/// back up — and why the branch is written down rather than read off the
/// folder's label, which flattens the slashes and cannot be undone.
pub fn checkout_for(cwd: &Path, branch: &str) -> Option<PathBuf> {
    let deep = branch.split('/').filter(|s| !s.is_empty()).count().max(1);
    let mut at = cwd;
    for _ in 0..deep {
        at = at.parent()?;
    }
    let leaf = at.file_name()?.to_string_lossy().to_string();
    let name = leaf.strip_suffix(".worktrees")?;
    Some(at.parent()?.join(name))
}

// ---------------------------------------------------------------------------
// How far a folder is from the remote
// ---------------------------------------------------------------------------

/// How far this folder's branch has drifted from the remote.
///
/// **As of the last fetch, and nothing more.** Nothing here goes to the
/// network. Asking the remote would mean a connection every few seconds per
/// folder, and — worse — it would put this app's git in the way of the git the
/// person (or the agent in that tab) is running in the same folder. What is on
/// disk is enough to say "you are three behind what you last saw", which is the
/// thing worth knowing at a glance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Default)]
pub struct Drift {
    /// Commits the remote has that this folder does not
    pub behind: u32,
    /// Commits this folder has that the remote does not
    pub ahead: u32,
}

impl Drift {
    pub fn any(&self) -> bool {
        self.behind > 0 || self.ahead > 0
    }
}

/// One folder's last answer, and what it was an answer about.
struct Drifted {
    drift: Drift,
    /// The two commits it was worked out from. While these are unchanged the
    /// answer cannot have changed either, so nothing is recomputed
    of: (String, String),
    settled: bool,
}

/// How far each folder has drifted, kept up to date in the background.
#[derive(Clone, Default)]
pub struct Drifts {
    known: Arc<Mutex<HashMap<PathBuf, Drifted>>>,
}

/// The one table, for the whole app.
pub fn drifts() -> &'static Drifts {
    static DRIFTS: OnceLock<Drifts> = OnceLock::new();
    DRIFTS.get_or_init(Drifts::default)
}

impl Drifts {
    /// Ask about these folders, and answer with what is known so far.
    ///
    /// Which two commits to compare is read off the disk here — it is a couple
    /// of small files. Counting what lies between them is not: that is a walk
    /// of the history, so it happens on a thread of its own and only when the
    /// two commits have actually changed. In practice that is once per fetch,
    /// not once per frame.
    pub fn look(&self, paths: &[PathBuf]) -> HashMap<PathBuf, Drift> {
        let mut out = HashMap::with_capacity(paths.len());
        let mut send: Vec<(PathBuf, String, String)> = Vec::new();
        {
            let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
            known.retain(|k, _| paths.iter().any(|p| p == k));
            for p in paths {
                let Some(ends) = ends_of(p) else {
                    // Not in a repository, or on a branch the remote has never
                    // heard of. Neither is a state to draw a number for
                    known.remove(p);
                    continue;
                };
                match known.get(p) {
                    Some(e) if e.of == ends => {
                        if e.settled {
                            out.insert(p.clone(), e.drift.clone());
                        }
                    }
                    _ => {
                        known.insert(
                            p.clone(),
                            Drifted { drift: Drift::default(), of: ends.clone(), settled: false },
                        );
                        send.push((p.clone(), ends.0, ends.1));
                    }
                }
            }
        }
        for (p, mine, theirs) in send {
            let known = Arc::clone(&self.known);
            std::thread::spawn(move || {
                let drift = count_between(&p, &mine, &theirs);
                if let Ok(mut map) = known.lock() {
                    if let Some(e) = map.get_mut(&p) {
                        // Only if it is still an answer to the question that
                        // was asked: a fetch during the walk moves the ends
                        if e.of == (mine, theirs) {
                            e.drift = drift;
                            e.settled = true;
                        }
                    }
                }
            });
        }
        out
    }
}

/// The two commits to compare: where this folder is, and where the remote was
/// when it was last fetched.
///
/// Read the way git would read them, never by running git — the answer is
/// wanted several times a second, and a repository mid-rebase answers anyway.
fn ends_of(cwd: &Path) -> Option<(String, String)> {
    let branch = crate::repo::branch_of(cwd)?;
    let git = crate::repo::family_of(cwd)?;
    let mine = ref_at(&git, &format!("refs/heads/{branch}"))?;
    let theirs = ref_at(&git, &format!("refs/remotes/origin/{branch}"))?;
    Some((mine, theirs))
}

/// One ref, loose or packed.
fn ref_at(git: &Path, full: &str) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(git.join(full)) {
        let said = text.trim();
        if !said.is_empty() && !said.starts_with("ref:") {
            return Some(said.to_string());
        }
    }
    // Refs that have been packed away live in one file, one per line
    let packed = std::fs::read_to_string(git.join("packed-refs")).ok()?;
    packed.lines().find_map(|l| {
        let (sha, name) = l.split_once(' ')?;
        (name.trim() == full).then(|| sha.trim().to_string())
    })
}

/// How many commits lie on each side. The one place a git process is started.
///
/// `rev-list` only reads: it takes no lock and touches no working file, so it
/// cannot collide with the git the person -- or the agent in that tab -- is
/// running in the same folder. Run once per pair of commits, which is once per
/// fetch rather than once per frame.
fn count_between(cwd: &Path, mine: &str, theirs: &str) -> Drift {
    if mine == theirs {
        return Drift::default();
    }
    let mut asking = std::process::Command::new("git");
    asking
        .arg("-C")
        .arg(cwd)
        .args(["rev-list", "--left-right", "--count", &format!("{mine}...{theirs}")]);
    let Ok(out) = crate::detach_console(&mut asking).output() else {
        return Drift::default();
    };
    if !out.status.success() {
        return Drift::default();
    }
    let said = String::from_utf8_lossy(&out.stdout);
    let mut parts = said.split_whitespace();
    // Left is what only this folder has, right is what only the remote has
    let ahead = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let behind = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    Drift { behind, ahead }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drive_letter_is_read_off_the_front() {
        assert_eq!(drive_of(Path::new(r"D:\Simic")).as_deref(), Some("D:"));
        // Written in lower case by a person, shown the way drives are named
        assert_eq!(drive_of(Path::new(r"c:\x")).as_deref(), Some("C:"));
    }

    /// A share has no letter to be missing, so nothing is claimed about one.
    #[test]
    fn a_share_has_no_drive_letter() {
        assert_eq!(drive_of(Path::new(r"\\192.168.0.35\projects\x")), None);
        assert_eq!(drive_of(Path::new("/home/me/x")), None);
    }

    /// The folder this test is running in is there, whatever machine it is.
    #[test]
    fn a_folder_that_is_there_is_fine() {
        let here = std::env::temp_dir();
        assert_eq!(health_of(&here), Health::Fine);
    }

    /// Missing, not "no drive": the drive under the temp folder exists.
    #[test]
    fn a_folder_that_is_not_there_is_missing() {
        let gone = std::env::temp_dir().join("shikisha-not-here-9f3a1c");
        assert_eq!(health_of(&gone), Health::Missing);
    }

    /// Asking does not block, and the answer arrives.
    #[test]
    fn looking_answers_without_waiting() {
        let w = Watch::new();
        let gone = std::env::temp_dir().join("shikisha-not-here-2b7e");
        let paths = vec![gone.clone()];
        // The first frame says nothing is wrong: a folder is innocent until it
        // has actually been looked at, so no warning ever flashes on startup
        assert_eq!(w.look(&paths).get(&gone), Some(&Health::Fine));
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(10));
            if w.look(&paths).get(&gone) == Some(&Health::Missing) {
                return;
            }
        }
        panic!("the look never came back");
    }

    /// Folders that are no longer worked in stop being asked about.
    #[test]
    fn folders_nobody_works_in_are_forgotten() {
        let w = Watch::new();
        let a = std::env::temp_dir();
        let b = std::env::temp_dir().join("shikisha-gone-4d1");
        w.look(&[a.clone(), b.clone()]);
        w.look(&[a.clone()]);
        let held = w.known.lock().unwrap();
        assert!(held.contains_key(&a));
        assert!(!held.contains_key(&b));
    }
}

#[cfg(test)]
mod restore_tests {
    use super::*;
    use crate::config::Source;

    /// A real repository, because every one of these is about what git actually
    /// does. Two of the answers below were arrived at by running git and
    /// reading what it said, not by reasoning about what it ought to say.
    fn project(name: &str, at: &Path) -> String {
        std::fs::create_dir_all(at).unwrap();
        let git = |args: &[&str]| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(at).args(args);
            let out = crate::detach_console(&mut run).output().expect("git が要る");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        let url = format!("https://example.test/team/{name}.git");
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        git(&["remote", "add", "origin", &url]);
        std::fs::write(at.join("readme.md"), "hi\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        git(&["branch", "work-2"]);
        url
    }

    fn clear(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-restore-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cut(origin: &str, branch: &str) -> Source {
        Source::Worktree {
            origin: origin.into(),
            branch: branch.into(),
            base: "origin/main".into(),
        }
    }

    /// A folder that is already here needs nothing done to it.
    #[test]
    fn a_folder_that_is_here_needs_nothing() {
        let at = clear("here");
        assert_eq!(plan(&at, &Source::Plain, None), Ok(Vec::new()));
    }

    /// Said to be an ordinary folder, so an ordinary folder is what it gets.
    #[test]
    fn a_plain_folder_is_only_made() {
        let at = clear("plain").join("nope");
        assert_eq!(
            plan(&at, &Source::Plain, None),
            Ok(vec![Step::Make { to: at.display().to_string() }])
        );
    }

    /// The one case that has to ask a person: settings written by hand say
    /// where a folder is and nothing about what it held.
    #[test]
    fn nothing_written_down_has_to_ask() {
        let at = clear("nosource").join("gone");
        assert_eq!(plan(&at, &Source::Unknown, None), Err(Blocked::Unknown));
    }

    /// A machine that has never seen the project brings it, then expands the
    /// branch. Two steps, both shown before either runs.
    #[test]
    fn a_project_that_is_not_here_is_cloned_and_then_expanded() {
        let root = clear("fresh");
        let checkout = root.join("myproject");
        let want = root.join("myproject.worktrees").join("work-2");
        let steps = plan(&want, &cut("https://example.test/team/myproject.git", "work-2"), None)
            .expect("nothing is in the way");
        assert_eq!(steps.len(), 2, "{steps:?}");
        let Step::Clone { to, line, .. } = &steps[0] else { panic!("{steps:?}") };
        assert_eq!(Path::new(to), checkout, "the project goes where the settings say");
        assert!(line.starts_with("git clone https://example.test/team/myproject.git"), "{line}");
        let Step::Expand { line, .. } = &steps[1] else { panic!("{steps:?}") };
        assert!(line.contains("worktree add"), "{line}");
    }

    /// The project is already here, so only the branch is missing. This is the
    /// everyday case: the same layout on two machines, one of them yet to
    /// expand this branch.
    #[test]
    fn a_project_already_here_is_only_expanded() {
        let root = clear("already");
        let checkout = root.join("myproject");
        let url = project("myproject", &checkout);
        let want = root.join("myproject.worktrees").join("work-2");
        let steps = plan(&want, &cut(&url, "work-2"), None).expect("nothing is in the way");
        assert_eq!(steps.len(), 1, "no clone: it is already here — {steps:?}");
        let Step::Expand { line, .. } = &steps[0] else { panic!("{steps:?}") };
        // The branch exists, so it is checked out rather than started again
        assert!(line.ends_with("work-2"), "{line}");
        assert!(!line.contains(" -b "), "{line}");
    }

    /// Somebody else's project sitting in the place this one has to go. Git
    /// would say only "already exists and is not an empty directory", which is
    /// the same thing it says when the right project is already there.
    #[test]
    fn somebody_elses_project_in_the_way_stops_it() {
        let root = clear("other");
        let checkout = root.join("myproject");
        project("something-else", &checkout);
        let want = root.join("myproject.worktrees").join("work-2");
        let out = plan(&want, &cut("https://example.test/team/myproject.git", "work-2"), None);
        let Err(Blocked::OtherProject { found, wanted, .. }) = out else {
            panic!("a different project was not noticed: {out:?}");
        };
        assert!(found.contains("something-else"), "{found}");
        assert!(wanted.contains("myproject"), "{wanted}");
    }

    /// One hidden file is enough for git to refuse, and a sync folder leaves
    /// `desktop.ini` everywhere. Explorer does not show it, so the folder looks
    /// empty and the refusal looks like a bug in this app — unless what is in
    /// there is named.
    #[test]
    fn a_folder_holding_only_a_hidden_file_stops_it_and_names_the_file() {
        let root = clear("hidden");
        let checkout = root.join("myproject");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(checkout.join("desktop.ini"), "[.ShellClassInfo]\n").unwrap();
        let want = root.join("myproject.worktrees").join("work-2");
        let out = plan(&want, &cut("https://example.test/team/myproject.git", "work-2"), None);
        let Err(Blocked::NotEmpty { holds, .. }) = out else {
            panic!("a folder that is not empty was not noticed: {out:?}");
        };
        assert_eq!(holds, vec!["desktop.ini".to_string()]);
    }

    /// One branch, one working folder, per clone. Two machines holding the same
    /// branch is ordinary; two folders on one machine is what git refuses.
    #[test]
    fn a_branch_already_open_in_this_clone_stops_it() {
        let root = clear("taken");
        let checkout = root.join("myproject");
        let url = project("myproject", &checkout);
        // The checkout itself moves onto the branch
        let mut run = std::process::Command::new("git");
        run.arg("-C").arg(&checkout).args(["switch", "-q", "work-2"]);
        assert!(crate::detach_console(&mut run).output().unwrap().status.success());

        let want = root.join("myproject.worktrees").join("work-2");
        let out = plan(&want, &cut(&url, "work-2"), None);
        let Err(Blocked::BranchTaken { branch, at }) = out else {
            panic!("the branch being open here was not noticed: {out:?}");
        };
        assert_eq!(branch, "work-2");
        assert_eq!(Path::new(&at), checkout);
    }

    /// The checkout on another branch is not in the way at all. Stopping here
    /// too would stop nearly every time, since a checkout sitting on `main` is
    /// the rare case rather than the usual one.
    #[test]
    fn the_checkout_being_on_another_branch_is_not_in_the_way() {
        let root = clear("elsewhere");
        let checkout = root.join("myproject");
        let url = project("myproject", &checkout);
        let want = root.join("myproject.worktrees").join("work-2");
        assert!(plan(&want, &cut(&url, "work-2"), None).is_ok(), "main を開いていても関係ない");
    }

    /// A branch that is gone everywhere is started again from what it grew
    /// from — the last thing left that says where it belongs.
    #[test]
    fn a_branch_that_no_longer_exists_is_started_again_from_its_base() {
        let root = clear("gonebranch");
        let checkout = root.join("myproject");
        let url = project("myproject", &checkout);
        let want = root.join("myproject.worktrees").join("never-existed");
        let steps = plan(&want, &cut(&url, "never-existed"), None).expect("nothing in the way");
        let Step::Expand { line, .. } = &steps[0] else { panic!("{steps:?}") };
        assert!(line.contains(" -b never-existed "), "{line}");
        assert!(line.ends_with("origin/main"), "{line}");
    }

    /// Where the project is, worked out from the branch folder's own path.
    #[test]
    fn the_project_is_found_from_the_branch_folder() {
        let at = Path::new(r"D:\Simic.worktrees\work-2");
        assert_eq!(checkout_for(at, "work-2"), Some(PathBuf::from(r"D:\Simic")));
        // A branch with a slash in it is that many folders deep, which is why
        // the branch has to be known rather than read off the folder's label
        let deep = Path::new(r"D:\Simic.worktrees\feature\login");
        assert_eq!(checkout_for(deep, "feature/login"), Some(PathBuf::from(r"D:\Simic")));
        // Somewhere that is not one of ours has no project to point back at
        assert_eq!(checkout_for(Path::new(r"D:\just\a\folder"), "x"), None);
    }

/// The whole of it, carried out. Everything above stops at deciding; this
    /// one runs what was decided and looks at what is on the disk afterwards.
    #[test]
    fn a_folder_is_really_put_back() {
        let root = clear("real");
        let checkout = root.join("myproject");
        let url = project("myproject", &checkout);
        let want = root.join("myproject.worktrees").join("work-2");
        let source = cut(&url, "work-2");

        let steps = plan(&want, &source, None).expect("nothing is in the way");
        for s in &steps {
            take(s, &source).expect("the step runs");
        }
        assert!(want.is_dir(), "the folder was not made");
        assert!(want.join("readme.md").exists(), "it came up empty");
        assert_eq!(crate::repo::branch_of(&want).as_deref(), Some("work-2"));
        assert_eq!(
            crate::repo::family_of(&want),
            crate::repo::family_of(&checkout),
            "it belongs to the project it was cut from"
        );
        // And now that it is there, there is nothing left to do
        assert_eq!(plan(&want, &source, None), Ok(Vec::new()));
        // Asking for the same branch a second time is refused rather than
        // quietly making a second folder for it
        let twice = root.join("myproject.worktrees").join("again");
        assert!(matches!(
            plan(&twice, &cut(&url, "work-2"), Some(&checkout)),
            Err(Blocked::BranchTaken { .. })
        ));
    }

    /// An ordinary folder is made, and nothing git is attempted on it.
    #[test]
    fn a_plain_folder_is_really_made() {
        let root = clear("realplain");
        let want = root.join("notes");
        let steps = plan(&want, &crate::config::Source::Plain, None).unwrap();
        for s in &steps {
            take(s, &crate::config::Source::Plain).expect("the step runs");
        }
        assert!(want.is_dir());
        assert!(!want.join(".git").exists(), "a plain folder is not a repository");
    }

    /// A remote URL can hold a live token. These are written into settings that
    /// get carried between machines and shown on screen; neither may ever hold
    /// a credential.
    #[test]
    fn credentials_never_reach_the_settings_or_the_screen() {
        assert_eq!(
            scrub("https://user:ghp_secretsecret@github.com/team/x.git"),
            "https://github.com/team/x.git"
        );
        assert_eq!(scrub("https://oauth2:tok@gitlab.com/g/p.git"), "https://gitlab.com/g/p.git");
        // Nothing to take out, nothing changed
        assert_eq!(scrub("https://github.com/team/x.git"), "https://github.com/team/x.git");
        assert_eq!(scrub("git@github.com:team/x.git"), "git@github.com:team/x.git");
    }

    /// One project reached by two spellings is one project. Whoever cloned it
    /// on the other machine wrote the remote their own way.
    #[test]
    fn the_same_project_spelled_two_ways_is_one_project() {
        assert!(same_repo(
            "https://user:tok@github.com/Team/X.git",
            "https://github.com/team/x"
        ));
        assert!(!same_repo("https://github.com/team/x", "https://github.com/team/y"));
        // Nothing is not everything: a folder with no remote matches no project
        assert!(!same_repo("", "https://github.com/team/x"));
    }
}

//! Where a tab is: the branch it sits on, and the ports it opened.
//!
//! Both are things you would otherwise have to go and look at. Someone with
//! six agents running has six answers to "which branch is that one on" and
//! "which of these is serving on 3000", and every one of them costs a tab
//! switch and a command. They are cheap to know and expensive to ask for,
//! which is exactly the sort of thing a window should just say.
//!
//! Nothing here runs `git`. The branch is a line in a file, and reading it
//! directly means a tab in a huge repository costs the same as a tab in a
//! small one -- and that a repository mid-rebase, with a lock held, answers
//! anyway.
//!
//! The ports come from the machine's own table of listeners, matched against
//! the tab's process and everything it started. That last part matters: what
//! opens the port is almost never the shell we launched, it is the dev server
//! three processes further down.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What a tab can say about where it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Place {
    pub branch: Option<String>,
    /// Ports this tab's processes are listening on, low to high
    pub ports: Vec<u16>,
    /// The program holding each of those ports, by port: what a person would
    /// recognise it by (`node.exe`, `python`), for the ports panel. A port
    /// whose holder could not be named is simply not in here
    pub programs: std::collections::BTreeMap<u16, String>,
    /// The host to open a port at, for a port `localhost` does not reach
    /// (`open_host`). A port not in here opens at `localhost`
    pub hosts: std::collections::BTreeMap<u16, String>,
    /// `owner/name` on GitHub, when that is where this folder pushes to
    pub repo: Option<String>,
    /// The folder shared by this checkout and every branch cut from it. Two
    /// tabs holding the same one are working on the same project, which is
    /// what the list draws as one family
    pub family: Option<PathBuf>,
    /// Whether this folder is one of those cut branches rather than the
    /// original checkout
    pub linked: bool,
    /// The pull request this branch is on, already written out. Filled in from
    /// elsewhere: it is the one thing here that has to be asked over a network
    pub pr: Option<String>,
}

impl Place {
    /// Whether there is anything to say at all.
    pub fn known(&self) -> bool {
        self.branch.is_some() || self.pr.is_some() || !self.ports.is_empty()
    }
}

/// The branch a folder is on, or the commit if it is not on one.
///
/// Reads the same file git would read. A detached head has no branch to name,
/// so it says the commit instead -- shortened, because that is how people say
/// it to each other, and because the row is narrow
pub fn branch_of(cwd: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir(cwd)?.join("HEAD")).ok()?;
    let head = head.trim();
    match head.strip_prefix("ref: refs/heads/") {
        Some(name) if !name.is_empty() => Some(name.to_string()),
        _ => {
            let sha = head.trim();
            let looks_like_a_commit =
                sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit());
            looks_like_a_commit.then(|| sha[..7].to_string())
        }
    }
}

/// Where this folder pushes to, as `owner/name`, when that is GitHub.
///
/// Only GitHub, because the only thing this is for is asking GitHub about a
/// pull request. A repository that lives somewhere else is not a failure and
/// gets no line, which is the same answer as a folder that is not a
/// repository at all
pub fn origin_of(cwd: &Path) -> Option<String> {
    github_path(&remote_url_of(cwd)?)
}

/// Where this folder's repository pushes to, exactly as git has it written.
///
/// The whole URL rather than a GitHub name, because this is also what says
/// whether two folders on two machines are the same project — and plenty of
/// projects are not on GitHub. Whoever shows or stores it takes the
/// credentials off first (see `folders::scrub`): git keeps them in the URL when
/// it is told to, and this file is read by things that write elsewhere.
pub fn remote_url_of(cwd: &Path) -> Option<String> {
    // The shared folder, not this checkout's own: a worktree's git folder holds
    // its HEAD and little else, and `config` is one of the things it does not
    // have. Reading it there found nothing, so every worktree tab was missing
    // the one line that says which repository it belongs to
    let text = std::fs::read_to_string(family_of(cwd)?.join("config")).ok()?;
    // The url under [remote "origin"], and nothing under any other section
    let mut inside = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t.replace(char::is_whitespace, "") == "[remote\"origin\"]";
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(url) = t.strip_prefix("url")
            && let Some(v) = url.split_once('=') {
                return Some(v.1.trim().to_string());
            }
    }
    None
}

/// `owner/name` out of any of the ways a GitHub remote is written.
pub fn github_path(url: &str) -> Option<String> {
    let rest = ["https://github.com/", "http://github.com/", "ssh://git@github.com/",
                "git@github.com:", "github.com/", "git://github.com/"]
        .into_iter()
        .find_map(|p| url.strip_prefix(p))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let (owner, name) = rest.split_once('/')?;
    // A path with more in it is not a repository url; guessing would send a
    // token somewhere on the strength of a bad guess
    let plain = |s: &str| !s.is_empty() && !s.contains('/');
    (plain(owner) && plain(name)).then(|| format!("{owner}/{name}"))
}

/// The folder git keeps its own files in, for this working folder.
///
/// Walks up, because a tab is usually opened somewhere inside a repository
/// rather than at its root. A `.git` that is a *file* rather than a folder is
/// a worktree or a submodule: it names where the real one is, and that one has
/// its own HEAD -- which is the whole point of a worktree, and the case that
/// matters most here, since a worktree per branch is how several agents work
/// in one repository without treading on each other
fn git_dir(cwd: &Path) -> Option<PathBuf> {
    let mut at = Some(cwd);
    // Bounded rather than "until the root": a path that loops, or one mounted
    // somewhere very deep, should not turn a once-a-second check into a walk
    for _ in 0..40 {
        let here = at?;
        let dot = here.join(".git");
        if dot.is_dir() {
            return Some(dot);
        }
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).ok()?;
            let named = text.trim().strip_prefix("gitdir:")?.trim();
            let p = PathBuf::from(named);
            return Some(match p.is_absolute() {
                true => p,
                false => here.join(p),
            });
        }
        at = here.parent();
    }
    None
}

/// What two folders share when they are the same repository.
///
/// A checkout and every worktree cut from it keep separate working folders and
/// separate HEADs, but exactly one store of objects, refs and config. Git
/// writes the way back to it in `commondir`, so this is the same path for all
/// of them and different for anything else -- which is precisely the question
/// the tab list asks when it colours several branches of one project as one
/// family. Nothing here runs `git`: it is two file reads, cheap enough to ask
/// per tab, and a repository mid-rebase answers anyway
pub fn family_of(cwd: &Path) -> Option<PathBuf> {
    let dir = git_dir(cwd)?;
    // A plain clone has no `commondir` and is its own family. Spelled the same
    // way as the branches cut from it, or the checkout reached by a short path
    // would not recognise its own worktrees
    let Ok(text) = std::fs::read_to_string(dir.join("commondir")) else {
        return Some(real(dir));
    };
    let named = text.trim();
    if named.is_empty() {
        return Some(real(dir));
    }
    let p = PathBuf::from(named);
    // Written relative to the worktree's own git folder, and usually `../..`
    Some(real(match p.is_absolute() {
        true => p,
        false => dir.join(p),
    }))
}

/// The one spelling of a path, so that two of them can be compared.
///
/// Windows hands out the same folder under more than one name -- the short
/// `RUNNER~1` form of a long one, a different case, a drive mapped elsewhere --
/// and two tabs in one repository that arrived by different routes would then
/// look like two projects. Asked of the filesystem rather than worked out, and
/// the prefix it answers with is taken off again because this is also shown to
/// people. A folder that is not there cannot be asked about, so it is only
/// tidied
fn real(p: PathBuf) -> PathBuf {
    let Ok(full) = std::fs::canonicalize(&p) else {
        return tidy(p);
    };
    plain(full.to_string_lossy().to_string())
}

/// A path as Windows resolved it, written the way everyone else writes one.
///
/// A folder on another machine comes back as `\\?\UNC\<server>\<share>\...`:
/// the two slashes that start a network path have been swallowed by the
/// prefix, so taking off only `\\?\` leaves `UNC\<server>\...`, a name that
/// points nowhere. git was handed exactly that and said it could not change to
/// it, which left a repository opened across the network unable to have a
/// branch cut from it. The network form is put back together; everything else
/// only loses the prefix
fn plain(said: String) -> PathBuf {
    match said.strip_prefix(VERBATIM_UNC) {
        Some(rest) => PathBuf::from(format!("\\\\{rest}")),
        None => PathBuf::from(said.strip_prefix(VERBATIM).unwrap_or(&said)),
    }
}

/// What Windows puts in front of a path it has resolved in full.
const VERBATIM: &str = "\\\\?\\";

/// The same, for a folder on another machine: what follows it is
/// `<server>\<share>\...`, without the `\\` that says so.
const VERBATIM_UNC: &str = "\\\\?\\UNC\\";

/// The original checkout of whatever repository this folder belongs to.
///
/// Every branch cut from it points back at one shared git folder, and that
/// folder sits inside the original -- so the answer is the same from anywhere
/// in the family, which is what makes it the thing to hang a new branch off
pub fn main_checkout(cwd: &Path) -> Option<PathBuf> {
    let git = family_of(cwd)?;
    // `<checkout>/.git` -> `<checkout>`. A bare repository has no checkout to
    // name, and is not somewhere anyone is working anyway
    (git.file_name()? == ".git").then(|| git.parent().map(Path::to_path_buf))?
}

/// Whether this folder is a branch cut from a checkout, rather than the
/// checkout itself.
///
/// Both are working folders of one repository and both answer with their own
/// branch, so nothing about the branch tells them apart. What does is where
/// their git folder is: the original's *is* the shared one, and a cut branch's
/// sits inside it. Worth telling apart because the list marks a cut branch --
/// closing it is a different thing from closing the project
pub fn is_linked(cwd: &Path) -> bool {
    match (git_dir(cwd), family_of(cwd)) {
        // Both through the same spelling, or a checkout reached by a short path
        // would look like a branch of itself
        (Some(d), Some(shared)) => real(d) != shared,
        _ => false,
    }
}

/// The worktrees cut from a repository, wherever they are: each folder and the
/// branch it is on. Read from the notes git keeps in the shared git folder
/// (`worktrees/<name>/gitdir` and `HEAD`), not by starting git, and only the
/// ones whose folder is still there -- git keeps a note for a deleted
/// worktree until it is pruned, and a folder that is gone is nothing to show.
/// `family` is the shared git folder, as [`family_of`] names it
pub fn worktrees_of(family: &Path) -> Vec<(PathBuf, Option<String>)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(family.join("worktrees")) else {
        return out;
    };
    for entry in entries.flatten() {
        let note = entry.path();
        let Ok(gitdir) = std::fs::read_to_string(note.join("gitdir")) else { continue };
        // It names the `.git` file inside the working folder; the folder is
        // around it. Put back together from its parts, so it is spelled the
        // way the rest of the screen spells a path here
        let file: PathBuf = Path::new(gitdir.trim()).components().collect();
        let Some(folder) = file.parent().map(Path::to_path_buf) else { continue };
        if !folder.is_dir() {
            continue;
        }
        let branch = std::fs::read_to_string(note.join("HEAD"))
            .ok()
            .and_then(|h| h.trim().strip_prefix("ref: refs/heads/").map(str::to_string));
        out.push((folder, branch));
    }
    out.sort_by_key(|(f, _)| f.display().to_string().to_lowercase());
    out
}

/// Where an AI CLI keeps worktrees of its own inside a checkout: Claude Code
/// makes one per helper it runs apart (`agent-<id>`) and removes it after.
/// Only the CLIs this app works with; a folder of that name anywhere else in
/// a path is not one of these
const TOOL_SCRATCH: &[&[&str]] = &[&[".claude", "worktrees"]];

/// Whether `folder` is a worktree an AI CLI made for its own use inside
/// `checkout`, rather than one somebody made to work in.
///
/// Such a folder is the tool's, like its cache: it comes and goes with a
/// helper's run, and offering it to the person as a worktree of theirs is
/// offering them somebody else's scratch paper. Judged against the checkout
/// the worktree belongs to, not by a name found anywhere in the path -- a
/// project of somebody's own called `worktrees` under a `.claude` elsewhere is
/// theirs. `bases` are the places the project's settings say its worktrees go:
/// one of those that points inside the scratch place was chosen by a person,
/// and what is in it is theirs
pub fn tool_scratch(checkout: &Path, folder: &Path, bases: &[PathBuf]) -> bool {
    if scratch_by_name(checkout, folder, bases) {
        return true;
    }
    // The same folder can be written two ways -- `RUNNER~1` for a long name,
    // a path git wrote one way and the settings another -- and a comparison of
    // names takes them for two. Asked of the disk only for a folder whose
    // path has the scratch place's names in it: a question per worktree on
    // every drawing would be paid for by every project
    let named = TOOL_SCRATCH.iter().any(|parts| {
        let segs: Vec<String> = folder.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect();
        segs.windows(parts.len()).any(|w| w.iter().zip(parts.iter()).all(|(a, b)| a == b))
    });
    if !named {
        return false;
    }
    let bases: Vec<PathBuf> = bases.iter().map(|b| real(b.clone())).collect();
    scratch_by_name(&real(checkout.to_path_buf()), &real(folder.to_path_buf()), &bases)
}

fn scratch_by_name(checkout: &Path, folder: &Path, bases: &[PathBuf]) -> bool {
    TOOL_SCRATCH.iter().any(|parts| {
        let root = parts.iter().fold(checkout.to_path_buf(), |p, s| p.join(s));
        crate::worktree::inside_checkout(&root, folder)
            && !crate::uistate::same_folder(&root, folder)
            && !bases.iter().any(|b| {
                crate::worktree::inside_checkout(&root, b) && crate::worktree::inside_checkout(b, folder)
            })
    })
}

/// The same path written the one way, so that two of them can be compared.
///
/// `..` is resolved by reading the path rather than by asking the disk: the
/// answer is wanted several times a second, and a folder that has just been
/// removed should still be recognisable as the family it belonged to
pub(crate) fn tidy(path: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            // A `..` with nothing above it is kept: dropping it would turn a
            // path that points outside into one that points here
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Every listening port on this machine, by the process that opened it.
///
/// Asked once and shared out, not once per tab: this is a table of the whole
/// machine either way, and reading it several times a second per tab would be
/// paying for the same answer over and over
pub fn listeners() -> HashMap<u32, std::collections::BTreeMap<u16, Vec<std::net::IpAddr>>> {
    let mut out: HashMap<u32, std::collections::BTreeMap<u16, Vec<std::net::IpAddr>>> = HashMap::new();
    // Both families. A dev server that binds ::1 and one that binds 127.0.0.1
    // are the same thing to the person looking at the row; which addresses a
    // port was bound on is kept, because it decides how the port is reached
    for (pid, port, addr) in listening() {
        let at = out.entry(pid).or_default().entry(port).or_default();
        if !at.contains(&addr) {
            at.push(addr);
        }
    }
    out
}

/// The host to open a port at, when `localhost` would not reach it.
///
/// A program bound to every address (`0.0.0.0`, `::`) or to the loopback
/// answers at `localhost`, which is `None` here. One bound only to a
/// particular address -- the LAN's, say -- answers there alone: asking
/// `localhost` for it finds nothing listening. An IPv4 address is preferred
/// over an IPv6 one when a port is on both, and an IPv6 one comes in the
/// brackets an address in a URL needs
pub fn open_host(bound: &[std::net::IpAddr]) -> Option<String> {
    use std::net::IpAddr;
    if bound.is_empty() || bound.iter().any(|a| a.is_loopback() || a.is_unspecified()) {
        return None;
    }
    // An IPv6 address that only means something on one network card
    // (fe80::/10) is the last choice: a URL cannot say which card
    let link_local = |a: &IpAddr| matches!(a, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80);
    let pick = bound
        .iter()
        .find(|a| a.is_ipv4())
        .or_else(|| bound.iter().find(|a| !link_local(a)))
        .or_else(|| bound.first())?;
    Some(match pick {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => format!("[{v6}]"),
    })
}

/// The ports below one tab, and the program holding each.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Held {
    /// Low to high
    pub ports: Vec<u16>,
    /// By port, the program that holds it, when it could be named
    pub programs: std::collections::BTreeMap<u16, String>,
    /// By port, the host to open it at, for a port `localhost` does not
    /// reach (see `open_host`). A port missing here opens at `localhost`
    pub hosts: std::collections::BTreeMap<u16, String>,
}

/// Every tab's ports, given each tab's own process.
///
/// The tab's process is a shell; what listens is whatever it started, however
/// far down. So this walks the machine's process tree once and gives each tab
/// the ports of everything below it -- and names the program holding each,
/// asked only of the few processes that turned out to hold one
pub fn ports_below(roots: &[(usize, u32)]) -> HashMap<usize, Held> {
    let mut out = HashMap::new();
    if roots.is_empty() {
        return out;
    }
    let by_pid = listeners();
    if by_pid.is_empty() {
        return out;
    }
    let children = child_map();
    // The same process under two roots is named once
    let mut names: HashMap<u32, Option<String>> = HashMap::new();
    for (key, root) in roots {
        let mut held = Held::default();
        for pid in descendants(*root, &children) {
            let Some(p) = by_pid.get(&pid) else { continue };
            let name = names.entry(pid).or_insert_with(|| program_of(pid)).clone();
            for (port, bound) in p {
                if !held.ports.contains(port) {
                    held.ports.push(*port);
                    if let Some(n) = &name {
                        held.programs.insert(*port, n.clone());
                    }
                    if let Some(host) = open_host(bound) {
                        held.hosts.insert(*port, host);
                    }
                }
            }
        }
        held.ports.sort_unstable();
        if !held.ports.is_empty() {
            out.insert(*key, held);
        }
    }
    out
}

/// The name of the program a process runs: the file name of its executable
/// (`node.exe` on Windows), which is what a person would look for in a task
/// manager. A process that ended or cannot be asked about has none
pub fn program_of(pid: u32) -> Option<String> {
    let table = process_table(Some(&[pid]), sysinfo::ProcessRefreshKind::nothing());
    let name = table.process(sysinfo::Pid::from_u32(pid))?.name().to_string_lossy();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// A process and everything it started, however deep.
pub(crate) fn descendants(root: u32, children: &HashMap<u32, Vec<u32>>) -> Vec<u32> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut todo = vec![root];
    while let Some(pid) = todo.pop() {
        // A parent id can point back up if an id has been reused since. Without
        // this the walk never ends
        if !seen.insert(pid) {
            continue;
        }
        out.push(pid);
        if let Some(kids) = children.get(&pid) {
            todo.extend(kids.iter().copied());
        }
    }
    out
}

// ── The two things only the operating system knows ────────────────

// The process tree and the listening sockets are read through two libraries
// rather than by hand, on every system. Windows answers through its process
// snapshot and its TCP table, Linux keeps both in /proc (a port is tied to its
// process by the inode of the socket), and macOS keeps them behind its own
// process calls and each process's file descriptors. Written by hand, each was
// a different program to keep right; reading /proc directly answered only
// Linux, and on a Mac every one of these came back empty without a word.

/// A look at the machine's processes, with only what `kind` asks for filled
/// in -- all of them, or just `only`.
///
/// Threads are left out. Linux lists each thread as a process of its own, and
/// a thread is not something a tab started: counted, it would put the same
/// program into the tree many times over
pub(crate) fn process_table(only: Option<&[u32]>, kind: sysinfo::ProcessRefreshKind) -> sysinfo::System {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut table = System::new();
    let picked: Vec<Pid> = only.unwrap_or_default().iter().map(|p| Pid::from_u32(*p)).collect();
    let which = match only {
        Some(_) => ProcessesToUpdate::Some(&picked),
        None => ProcessesToUpdate::All,
    };
    table.refresh_processes_specifics(which, true, kind.without_tasks());
    table
}

/// Parent to children, for every process on the machine.
pub(crate) fn child_map() -> HashMap<u32, Vec<u32>> {
    let table = process_table(None, sysinfo::ProcessRefreshKind::nothing());
    let mut out: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, p) in table.processes() {
        if let Some(parent) = p.parent() {
            out.entry(parent.as_u32()).or_default().push(pid.as_u32());
        }
    }
    out
}

/// Every listening TCP socket, as (process, port, address bound).
///
/// Both families: a dev server that binds ::1 and one that binds 127.0.0.1
/// are the same thing to the person looking at the row. A socket owned by
/// another account is not listed: nothing is known of it
fn listening() -> Vec<(u32, u16, std::net::IpAddr)> {
    use ::listeners::{Protocol, SocketState};
    let Ok(all) = ::listeners::get_all() else { return Vec::new() };
    all.into_iter()
        .filter(|l| l.protocol == Protocol::TCP && l.state == SocketState::Listen && l.socket.port() != 0)
        .map(|l| (l.process.pid, l.socket.port(), l.socket.ip()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-repo-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A repository's worktrees are read from the notes in its git folder:
    /// each folder and its branch, and not a folder that has been deleted
    #[test]
    fn a_repositorys_worktrees_are_read_from_its_notes_and_a_deleted_one_is_left_out() {
        let root = tmp("worktrees");
        let git = root.join("repo").join(".git");
        let here = root.join("wt-here");
        std::fs::create_dir_all(&here).unwrap();
        for (name, folder, head) in [
            ("here", here.clone(), "ref: refs/heads/feature/login\n"),
            ("gone", root.join("wt-gone"), "ref: refs/heads/old\n"),
            ("detached", here.clone(), "0123456789abcdef\n"),
        ] {
            let note = git.join("worktrees").join(name);
            std::fs::create_dir_all(&note).unwrap();
            // git writes forward slashes on every system
            std::fs::write(note.join("gitdir"), format!("{}\n", folder.join(".git").display().to_string().replace('\\', "/"))).unwrap();
            std::fs::write(note.join("HEAD"), head).unwrap();
        }
        let found = worktrees_of(&git);
        assert_eq!(found.len(), 2, "a deleted worktree was listed: {found:?}");
        assert!(found.iter().any(|(f, b)| f == &here && b.as_deref() == Some("feature/login")), "the branch was not read: {found:?}");
        assert!(found.iter().any(|(f, b)| f == &here && b.is_none()), "a detached worktree was dropped: {found:?}");
        assert!(worktrees_of(&root.join("nowhere")).is_empty());
    }

    /// A helper's worktree inside the checkout is the tool's; the same names
    /// anywhere else, and a place the project itself chose, are the person's
    #[test]
    fn a_worktree_an_ai_tool_made_for_itself_is_told_apart() {
        let p = |s: &str| PathBuf::from(crate::local_path(s));
        let checkout = p("D:/work/app");
        let helper = checkout.join(".claude").join("worktrees").join("agent-a1b2c3");
        assert!(tool_scratch(&checkout, &helper, &[]));
        // Written with the other slash, and on Windows in other letters, the same folder
        let spelled = if cfg!(windows) { "d:/WORK/app/.claude/worktrees/agent-a1b2c3" } else { "D:/work/app/.claude/worktrees/agent-a1b2c3" };
        assert!(tool_scratch(&checkout, &p(spelled), &[]));
        // The scratch place itself, a sibling of it, and another checkout's are not
        assert!(!tool_scratch(&checkout, &checkout.join(".claude").join("worktrees"), &[]));
        assert!(!tool_scratch(&checkout, &checkout.join(".claude").join("skills").join("x"), &[]));
        assert!(!tool_scratch(&checkout, &p("D:/elsewhere/.claude/worktrees/agent-a1b2c3"), &[]));
        // An ordinary worktree beside the checkout
        assert!(!tool_scratch(&checkout, &p("D:/work/app-login"), &[]));
        // The project said its worktrees go in there: then they are the person's
        let chosen = checkout.join(".claude").join("worktrees");
        assert!(!tool_scratch(&checkout, &helper, std::slice::from_ref(&chosen)));
        // A base somewhere else does not change what is inside the scratch place
        assert!(tool_scratch(&checkout, &helper, &[p("D:/trees")]));
    }

    /// The checkout reached by the short form of its folder's name, and the
    /// helper's worktree by the long one (the way CI's runner folder comes
    /// out), are still one checkout and its scratch place. Skipped where the
    /// disk keeps no short names
    #[cfg(windows)]
    #[test]
    fn a_helpers_worktree_is_known_under_either_spelling_of_the_checkout() {
        let root = std::env::temp_dir().join(format!("shikisha-scratch-long-folder-name-{}", std::process::id()));
        let helper = root.join(".claude").join("worktrees").join("agent-a1b2c3");
        std::fs::create_dir_all(&helper).unwrap();
        // Handed to cmd as it is written: quoted again by the usual rules, the
        // path would reach it inside `\"`, which it does not read as quotes
        use std::os::windows::process::CommandExt as _;
        let out = std::process::Command::new("cmd")
            .raw_arg(format!("/c for %I in (\"{}\") do @echo %~sI", root.display()))
            .output()
            .unwrap();
        let short = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if short.as_os_str().is_empty() || !short.to_string_lossy().contains('~') {
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        assert!(tool_scratch(&short, &helper, &[]), "{} and {} were taken for two folders", short.display(), helper.display());
        assert!(!tool_scratch(&short, &root.join("elsewhere"), &[]));
        // A place the project chose, written the long way, still exempts it
        assert!(!tool_scratch(&short, &helper, &[root.join(".claude").join("worktrees")]));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// This process listens nowhere, but it is running: its own name comes
    /// back, and it is the file's name alone -- the row has room for
    /// `node.exe`, and that is what a task manager calls it too
    #[test]
    fn this_programs_own_name_can_be_read() {
        let name = program_of(std::process::id()).expect("no name for the running test");
        assert!(!name.is_empty() && !name.contains(['/', '\\']), "{name:?}");
    }

    #[test]
    fn the_branch_is_read_from_the_file_git_keeps_it_in() {
        let root = tmp("branch");
        let git = root.join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(branch_of(&root).as_deref(), Some("main"));
        // A tab is usually opened somewhere inside the repository, not at its root
        let deep = root.join("src").join("inner");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(branch_of(&deep).as_deref(), Some("main"));
        // Slashes in a branch name are part of the name
        std::fs::write(git.join("HEAD"), "ref: refs/heads/feature/keys\n").unwrap();
        assert_eq!(branch_of(&root).as_deref(), Some("feature/keys"));
    }

    #[test]
    fn a_detached_head_says_the_commit_rather_than_nothing() {
        let root = tmp("detached");
        let git = root.join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(git.join("HEAD"), "9f2c1ab7d4e5f60718293a4b5c6d7e8f90a1b2c3\n").unwrap();
        assert_eq!(branch_of(&root).as_deref(), Some("9f2c1ab"));
    }

    #[test]
    fn a_worktree_is_on_its_own_branch_not_the_main_one() {
        // The case that matters most: a worktree per branch is how several
        // agents work in one repository without treading on each other, and
        // reading the main repository's HEAD would show them all the same name
        let root = tmp("worktree");
        let main_git = root.join("main").join(".git");
        std::fs::create_dir_all(main_git.join("worktrees").join("side")).unwrap();
        std::fs::write(main_git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            main_git.join("worktrees").join("side").join("HEAD"),
            "ref: refs/heads/side-quest\n",
        )
        .unwrap();
        let side = root.join("side");
        std::fs::create_dir_all(&side).unwrap();
        std::fs::write(
            side.join(".git"),
            format!("gitdir: {}\n", main_git.join("worktrees").join("side").display()),
        )
        .unwrap();
        assert_eq!(branch_of(&side).as_deref(), Some("side-quest"));
        assert_eq!(branch_of(&root.join("main")).as_deref(), Some("main"));
    }

    /// Lays out a checkout with one worktree cut from it, the way git does.
    /// Returns the two working folders and the shared git folder
    fn a_repository_with_a_worktree(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = tmp(name);
        let main = root.join("main");
        let git = main.join(".git");
        let linked = git.join("worktrees").join("side");
        std::fs::create_dir_all(&linked).unwrap();
        std::fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(linked.join("HEAD"), "ref: refs/heads/side-quest\n").unwrap();
        // Git writes this relative, pointing back up at the shared folder
        std::fs::write(linked.join("commondir"), "../..\n").unwrap();
        let side = root.join("side");
        std::fs::create_dir_all(&side).unwrap();
        std::fs::write(side.join(".git"), format!("gitdir: {}\n", linked.display())).unwrap();
        (main, side, git)
    }

    #[test]
    fn a_worktree_and_the_checkout_it_came_from_are_one_family() {
        // What the tab list colours as one project. The two folders are not
        // nested in each other and have different branches, so the only thing
        // that can answer is the folder git keeps in common
        let (main, side, _git) = a_repository_with_a_worktree("family");
        assert_eq!(family_of(&side), family_of(&main));
        // The shared folder itself, however this machine spells the way there
        assert!(family_of(&main).is_some_and(|f| f.ends_with(".git")));
        // Deeper inside counts as the same family, as it is the same checkout
        let deep = side.join("src");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(family_of(&deep), family_of(&main));

        // A different repository is a different family, and no repository at
        // all has none
        let elsewhere = tmp("family-elsewhere");
        std::fs::create_dir_all(elsewhere.join(".git")).unwrap();
        assert_ne!(family_of(&elsewhere), family_of(&main));
        assert_eq!(family_of(&tmp("family-plain")), None);
    }

    /// The same folder, spelled two ways, is one project.
    ///
    /// Windows hands the same place out under more than one name: a different
    /// case, and the short `RUNNER~1` form of a long one -- which is what the
    /// build machine's temporary folder is called, and where this was found.
    /// Two tabs that arrived by different spellings looked like two projects,
    /// so they were drawn as strangers and a branch could not find the checkout
    /// it was cut from.
    #[test]
    #[cfg(windows)]
    fn one_folder_spelled_two_ways_is_one_project() {
        let (main, side, _) = a_repository_with_a_worktree("family-spelling");
        let shouted = PathBuf::from(main.display().to_string().to_uppercase());
        assert!(shouted.exists(), "upper case points at the same place");
        assert_ne!(shouted, main, "as spelling, they are different");

        assert_eq!(family_of(&shouted), family_of(&main), "they are not seen as the same family");
        assert_eq!(family_of(&shouted), family_of(&side));
        // And the checkout still knows it is not one of its own branches
        assert!(!is_linked(&shouted), "the main checkout looks like a branch");
        assert!(is_linked(&side));
    }

    /// The same promise where case is not what hands out a second name for one
    /// folder. A link is: someone keeps their projects under a short name that
    /// points at the long one, opens a tab through it, and the branches cut
    /// from that repository have to still be its branches.
    #[test]
    #[cfg(unix)]
    fn one_folder_reached_two_ways_is_one_project() {
        let (main, side, _) = a_repository_with_a_worktree("family-spelling");
        let other = main.with_file_name("by-another-name");
        let _ = std::fs::remove_file(&other);
        std::os::unix::fs::symlink(&main, &other).expect("the link cannot be made");
        assert!(other.exists(), "what the link points to does not exist");
        assert_ne!(other, main, "as spelling, they are different");

        assert_eq!(family_of(&other), family_of(&main), "they are not seen as the same family");
        assert_eq!(family_of(&other), family_of(&side));
        assert!(!is_linked(&other), "the main checkout looks like a branch");
        assert!(is_linked(&side));
    }

    #[test]
    fn a_cut_branch_knows_it_is_not_the_checkout_it_came_from() {
        // What the list marks with a sign of its own: closing a branch that was
        // cut for a piece of work is a different act from closing the project
        let (main, side, _) = a_repository_with_a_worktree("family-linked");
        assert!(is_linked(&side), "it is a branch");
        assert!(!is_linked(&main), "the main checkout is not a branch");
        assert!(!is_linked(&tmp("family-linked-plain")), "not a repository, so not a branch either");
    }

    #[test]
    fn a_worktree_says_which_repository_it_pushes_to() {
        // The linked folder holds a HEAD and not much else -- `config` lives
        // only in the shared one -- so asking the checkout's own folder left
        // every worktree tab without a repository and therefore without a PR
        let (main, side, git) = a_repository_with_a_worktree("family-origin");
        std::fs::write(
            git.join("config"),
            "[remote \"origin\"]\n\turl = https://github.com/styleio/ShikishaTerm.git\n",
        )
        .unwrap();
        assert_eq!(origin_of(&side).as_deref(), Some("styleio/ShikishaTerm"));
        assert_eq!(origin_of(&side), origin_of(&main));
    }

    /// A repository kept on another machine is opened through its network name,
    /// and Windows answers about it in a spelling nothing else accepts: taking
    /// off only `\\?\` left `UNC\192.168.0.35\...`, and cutting a branch from
    /// that project died with "cannot change to" (2026-09-18, a user's share).
    #[test]
    fn a_repository_on_another_machine_keeps_the_slashes_that_name_it() {
        assert_eq!(
            plain("\\\\?\\UNC\\192.168.0.35\\projects\\php7\\te0_main".to_string()),
            PathBuf::from("\\\\192.168.0.35\\projects\\php7\\te0_main"),
        );
        // A folder on a drive of this machine loses the prefix and nothing else
        assert_eq!(plain("\\\\?\\D:\\ShikishaTerm".to_string()), PathBuf::from("D:\\ShikishaTerm"));
        // And a path that was never answered about is left as it is
        assert_eq!(plain("D:\\ShikishaTerm".to_string()), PathBuf::from("D:\\ShikishaTerm"));
        assert_eq!(
            plain("\\\\192.168.0.35\\projects".to_string()),
            PathBuf::from("\\\\192.168.0.35\\projects"),
        );
    }

    #[test]
    fn a_path_that_walks_back_up_is_still_the_same_path() {
        let base = PathBuf::from("a").join("b").join("c");
        assert_eq!(tidy(base.join("..").join("..")), PathBuf::from("a"));
        assert_eq!(tidy(base.join(".").join("d")), base.join("d"));
        // Nothing to climb out of, so the climb is part of the answer
        assert_eq!(tidy(PathBuf::from("..").join("x")), PathBuf::from("..").join("x"));
    }

    #[test]
    fn a_folder_that_is_not_in_a_repository_says_nothing() {
        let root = tmp("plain");
        assert_eq!(branch_of(&root), None);
        assert!(!Place::default().known());
    }

    #[test]
    fn a_place_knows_whether_it_has_anything_to_say() {
        assert!(!Place::default().known());
        assert!(Place { branch: Some("main".into()), ..Default::default() }.known());
        assert!(Place { ports: vec![8080], ..Default::default() }.known());
        assert!(Place { pr: Some("#12".into()), ..Default::default() }.known());
    }

    #[test]
    fn a_remote_is_read_however_it_was_written() {
        for url in [
            "https://github.com/styleio/ShikishaTerm.git",
            "https://github.com/styleio/ShikishaTerm",
            "git@github.com:styleio/ShikishaTerm.git",
            "ssh://git@github.com/styleio/ShikishaTerm.git",
        ] {
            assert_eq!(github_path(url).as_deref(), Some("styleio/ShikishaTerm"), "{url}");
        }
        // Somewhere else is not a failure, it is simply not GitHub
        assert_eq!(github_path("https://gitlab.com/group/thing.git"), None);
        assert_eq!(github_path("https://github.com/onlyowner"), None);
        assert_eq!(github_path(""), None);
    }

    #[test]
    fn the_origin_is_taken_from_the_origin_section_and_no_other() {
        let root = tmp("origin");
        let git = root.join(".git");
        std::fs::create_dir_all(&git).unwrap();
        std::fs::write(
            git.join("config"),
            "[remote \"upstream\"]
	url = https://github.com/someone/else.git
             [remote \"origin\"]
	url = git@github.com:styleio/ShikishaTerm.git
",
        )
        .unwrap();
        assert_eq!(origin_of(&root).as_deref(), Some("styleio/ShikishaTerm"));
    }

    #[test]
    fn a_process_tree_that_points_back_at_itself_still_ends() {
        // Process ids get reused, so a parent id can point at something that
        // is now below it. Walking that without a guard never returns
        let children = HashMap::from([(1u32, vec![2u32]), (2, vec![1, 3])]);
        let mut seen = descendants(1, &children);
        seen.sort_unstable();
        assert_eq!(seen, vec![1, 2, 3]);
    }

    #[test]
    fn this_machine_is_listening_on_something_and_we_can_see_it() {
        // A listener of our own, so the test does not depend on what else the
        // machine happens to be running -- and so it means the same thing on a
        // system where other accounts' sockets are none of our business
        let held = std::net::TcpListener::bind("127.0.0.1:0").expect("listen");
        let ours = held.local_addr().expect("addr").port();

        let all = listeners();
        assert!(!all.is_empty(), "not a single listening port was read");
        let mine = std::process::id();
        let bound = all.get(&mine).and_then(|ports| ports.get(&ours));
        assert!(bound.is_some(), "the port {ours} opened here does not show in the table");
        // And with the address it was bound on, which is what says it opens
        // at localhost
        assert_eq!(bound.unwrap(), &vec![std::net::IpAddr::from([127, 0, 0, 1])]);
        assert_eq!(open_host(bound.unwrap()), None);
        for (pid, ports) in &all {
            assert!(*pid > 0 || !ports.is_empty());
            for p in ports.keys() {
                assert!(*p > 0, "port 0 is mixed in");
            }
        }
    }

    #[test]
    fn a_port_bound_to_one_address_is_opened_there_and_not_at_localhost() {
        use std::net::IpAddr;
        let lan: IpAddr = "192.168.1.20".parse().unwrap();
        let v6: IpAddr = "2001:db8::5".parse().unwrap();
        let link: IpAddr = "fe80::1".parse().unwrap();
        // Everywhere, or the loopback: localhost reaches it
        assert_eq!(open_host(&["0.0.0.0".parse().unwrap()]), None);
        assert_eq!(open_host(&["::".parse().unwrap()]), None);
        assert_eq!(open_host(&["::1".parse().unwrap()]), None);
        assert_eq!(open_host(&[lan, "127.0.0.1".parse().unwrap()]), None);
        // Only somewhere particular: there
        assert_eq!(open_host(&[lan]).as_deref(), Some("192.168.1.20"));
        assert_eq!(open_host(&[v6, lan]).as_deref(), Some("192.168.1.20"));
        assert_eq!(open_host(&[v6]).as_deref(), Some("[2001:db8::5]"));
        assert_eq!(open_host(&[link, v6]).as_deref(), Some("[2001:db8::5]"));
        assert_eq!(open_host(&[]), None);
    }

    #[test]
    fn our_own_process_is_somewhere_in_the_machine_tree() {
        let children = child_map();
        assert!(!children.is_empty(), "the process list was not read");
        let me = std::process::id();
        assert!(
            children.values().any(|kids| kids.contains(&me)),
            "this process does not show in the parent-child table"
        );
    }
}

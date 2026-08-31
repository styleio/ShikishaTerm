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
        if let Some(url) = t.strip_prefix("url") {
            if let Some(v) = url.split_once('=') {
                return github_path(v.1.trim());
            }
        }
    }
    None
}

/// `owner/name` out of any of the ways a GitHub remote is written.
fn github_path(url: &str) -> Option<String> {
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
    // A plain clone has no `commondir` and is its own family
    let Ok(text) = std::fs::read_to_string(dir.join("commondir")) else {
        return Some(tidy(dir));
    };
    let named = text.trim();
    if named.is_empty() {
        return Some(tidy(dir));
    }
    let p = PathBuf::from(named);
    // Written relative to the worktree's own git folder, and usually `../..`
    Some(tidy(match p.is_absolute() {
        true => p,
        false => dir.join(p),
    }))
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
        (Some(d), Some(shared)) => tidy(d) != shared,
        _ => false,
    }
}

/// The same path written the one way, so that two of them can be compared.
///
/// `..` is resolved by reading the path rather than by asking the disk: the
/// answer is wanted several times a second, and a folder that has just been
/// removed should still be recognisable as the family it belonged to
fn tidy(path: PathBuf) -> PathBuf {
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
pub fn listeners() -> HashMap<u32, Vec<u16>> {
    let mut out: HashMap<u32, Vec<u16>> = HashMap::new();
    // Both families. A dev server that binds ::1 and one that binds 127.0.0.1
    // are the same thing to the person looking at the row
    for family in [AF_INET, AF_INET6] {
        for (pid, port) in listening_on(family) {
            let slot = out.entry(pid).or_default();
            if !slot.contains(&port) {
                slot.push(port);
            }
        }
    }
    for v in out.values_mut() {
        v.sort_unstable();
    }
    out
}

/// Every tab's ports, given each tab's own process.
///
/// The tab's process is a shell; what listens is whatever it started, however
/// far down. So this walks the machine's process tree once and gives each tab
/// the ports of everything below it
pub fn ports_below(roots: &[(usize, u32)]) -> HashMap<usize, Vec<u16>> {
    let mut out = HashMap::new();
    if roots.is_empty() {
        return out;
    }
    let by_pid = listeners();
    if by_pid.is_empty() {
        return out;
    }
    let children = child_map();
    for (key, root) in roots {
        let mut ports: Vec<u16> = Vec::new();
        for pid in descendants(*root, &children) {
            if let Some(p) = by_pid.get(&pid) {
                for port in p {
                    if !ports.contains(port) {
                        ports.push(*port);
                    }
                }
            }
        }
        ports.sort_unstable();
        if !ports.is_empty() {
            out.insert(*key, ports);
        }
    }
    out
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

use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

/// Parent to children, for every process on the machine.
pub(crate) fn child_map() -> HashMap<u32, Vec<u32>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    let mut out: HashMap<u32, Vec<u32>> = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                out.entry(e.th32ParentProcessID)
                    .or_default()
                    .push(e.th32ProcessID);
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    out
}

/// The listening sockets of one address family, as (process, port).
fn listening_on(family: u16) -> Vec<(u32, u16)> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
        TCP_TABLE_OWNER_PID_LISTENER,
    };
    let mut out = Vec::new();
    unsafe {
        let mut size: u32 = 0;
        // First call asks how much room the table needs. It is expected to
        // fail; the answer is the size it wrote back
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            family as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if size == 0 {
            return out;
        }
        let mut buf: Vec<u8> = vec![0; size as usize];
        let rc = GetExtendedTcpTable(
            buf.as_mut_ptr().cast(),
            &mut size,
            0,
            family as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        if rc != 0 {
            return out;
        }
        // Both tables begin with the number of rows, then the rows themselves
        let count = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let (row_size, head) = match family == AF_INET {
            true => (std::mem::size_of::<MIB_TCPROW_OWNER_PID>(), 4),
            false => (std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>(), 4),
        };
        for i in 0..count {
            let at = head + i * row_size;
            if at + row_size > buf.len() {
                break;
            }
            let row = buf.as_ptr().add(at);
            let (port_at, pid_at) = match family == AF_INET {
                // state, localAddr, localPort, remoteAddr, remotePort, pid
                true => (8, std::mem::size_of::<MIB_TCPROW_OWNER_PID>() - 4),
                // localAddr[16], scope, localPort, ... , pid
                false => (20, std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>() - 4),
            };
            let port_raw = std::ptr::read_unaligned(row.add(port_at).cast::<u32>());
            let pid = std::ptr::read_unaligned(row.add(pid_at).cast::<u32>());
            // The port sits in the first two bytes, in network order
            let port = u16::from_be((port_raw & 0xffff) as u16);
            if port != 0 {
                out.push((pid, port));
            }
        }
    }
    out
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
        let (main, side, git) = a_repository_with_a_worktree("family");
        assert_eq!(family_of(&side), family_of(&main));
        assert_eq!(family_of(&main).as_deref(), Some(git.as_path()));
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

    #[test]
    fn a_cut_branch_knows_it_is_not_the_checkout_it_came_from() {
        // What the list marks with a sign of its own: closing a branch that was
        // cut for a piece of work is a different act from closing the project
        let (main, side, _) = a_repository_with_a_worktree("family-linked");
        assert!(is_linked(&side), "枝である");
        assert!(!is_linked(&main), "本体は枝ではない");
        assert!(!is_linked(&tmp("family-linked-plain")), "リポジトリでなければ枝でもない");
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
        // Not a fixed expectation about which ports: any Windows machine has
        // listeners, and the point of the test is that the table reads at all
        // rather than coming back empty because a field moved
        let all = listeners();
        assert!(!all.is_empty(), "LISTEN中のポートが1つも読めていない");
        for (pid, ports) in &all {
            assert!(*pid > 0 || !ports.is_empty());
            for p in ports {
                assert!(*p > 0, "ポート0が混ざっている");
            }
        }
    }

    #[test]
    fn our_own_process_is_somewhere_in_the_machine_tree() {
        let children = child_map();
        assert!(!children.is_empty(), "プロセス一覧が読めていない");
        let me = std::process::id();
        assert!(
            children.values().any(|kids| kids.contains(&me)),
            "自分自身が親子表に出てこない"
        );
    }
}

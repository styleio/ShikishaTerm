//! Working on several branches of one repository at once.
//!
//! Git can give a repository more than one working folder -- one per branch,
//! all sharing the same history -- which is the only way several agents can
//! work on one project without editing each other's files. Doing it by hand
//! means choosing a path, remembering it, and cleaning it up afterwards, so
//! this does those three things and leaves the branch name to the person.
//!
//! **What runs is shown before it runs.** Every caller that puts a line on
//! screen and the one that executes it read the same `argv`, because a command
//! assembled twice is a command that will differ once.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Everything decided about a branch that is about to get its own folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The checkout it is cut from
    pub main: PathBuf,
    pub branch: String,
    /// Where the files will be
    pub folder: PathBuf,
    /// What it starts from -- `origin/main` unless someone says otherwise
    pub base: String,
    /// Whether the branch is being made, or is one that already exists
    pub fresh: bool,
    /// The machine it happens on. None is this one.
    ///
    /// Carried on the plan rather than looked up again when it runs, because
    /// the line on screen and the line that runs come from one place -- and on
    /// another machine "one place" has to include which machine
    pub host: Option<crate::config::HostSpec>,
    /// Where the project can be fetched from, for a machine that has never
    /// seen it. Empty everywhere else, where the project is already there
    pub origin: String,
    /// What the project says its environment needs, when it says. A machine
    /// made a second ago holds the source and nothing else, so without this a
    /// clone is a folder nothing can be built in
    pub env: Option<crate::devcontainer::Env>,
}

impl Plan {
    /// Where this happens, as the person reads it.
    pub fn where_at(&self) -> String {
        match &self.host {
            Some(h) => h.name.clone(),
            None => String::new(),
        }
    }

    /// Everything that will run, in order.
    ///
    /// One command nearly always: a branch is cut from a checkout that is
    /// already there. A machine that is made fresh has no checkout, so the
    /// project is fetched first and the branch cut second -- two commands, and
    /// both of them on screen, because a person checking what will happen is
    /// owed all of it and not the first half.
    pub fn argvs(&self) -> Vec<Vec<String>> {
        let fresh_machine = self.host.as_ref().is_some_and(|h| h.is_made());
        if !fresh_machine {
            let mut steps = vec![self.argv()];
            steps.extend(self.getting_ready());
            return steps;
        }
        let at = self.folder.display().to_string();
        let mut steps = vec![
            vec![
                "git".into(),
                "clone".into(),
                "--branch".into(),
                // The branch a clone lands on is named as the remote has it,
                // without the remote's own prefix
                self.base.rsplit('/').next().unwrap_or(&self.base).to_string(),
                self.origin.clone(),
                at.clone(),
            ],
            vec![
                "git".into(),
                "-C".into(),
                at.clone(),
                "switch".into(),
                "-c".into(),
                self.branch.clone(),
            ],
        ];
        steps.extend(self.getting_ready());
        steps
    }

    /// What the project says it needs, after it is there to need it.
    ///
    /// The same wherever the folder was cut. A worktree on this machine is as
    /// bare as one in a sandbox -- the source and nothing installed -- so the
    /// project's own words about what to run apply to both. They come from the
    /// file the world already uses to hold them, which is why there is no
    /// second place to write them.
    ///
    /// Each is a line for a shell rather than a program and its arguments,
    /// because that is how that file writes them, and the whole line goes in
    /// as one argument so nothing in it is split a second time.
    fn getting_ready(&self) -> Vec<Vec<String>> {
        let at = self.folder.display().to_string();
        self.env
            .iter()
            .flat_map(|e| e.setup.iter())
            .map(|line| match self.host.is_none() && cfg!(windows) {
                // This machine, and this machine is Windows. A line written
                // for a container will not always survive that; it is on
                // screen before it runs, and git's own refusal follows if not
                true => vec!["cmd".into(), "/c".into(), format!("cd /d {at} && {line}")],
                false => vec!["sh".into(), "-lc".into(), format!("cd {at} && {line}")],
            })
            .collect()
    }

    /// Exactly what will run, in the words git will get. Shown to the person
    /// first, then handed to the process: one line, one source
    pub fn argv(&self) -> Vec<String> {
        let mut v = vec![
            "git".into(),
            "-C".into(),
            self.main.display().to_string(),
            "worktree".into(),
            "add".into(),
        ];
        if self.fresh {
            v.push("-b".into());
            v.push(self.branch.clone());
        }
        v.push(self.folder.display().to_string());
        // An existing branch is named as the thing to check out; a new one is
        // named above and needs the point it grows from instead
        v.push(match self.fresh {
            true => self.base.clone(),
            false => self.branch.clone(),
        });
        v
    }

    /// The same, as a person reads it. Quoted only where it has to be
    pub fn line(&self) -> String {
        self.argvs().iter().map(|a| said(a)).collect::<Vec<_>>().join("
")
    }
}

/// One command, as a person reads it.
fn said(argv: &[String]) -> String {
    argv.iter()
        .map(|a| match a.contains(' ') {
            true => format!("\"{a}\""),
            false => a.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Works out where a branch's folder goes and what will make it.
///
/// `base` is what a new branch grows from; leave it out for the sensible one.
pub fn plan(main: &Path, branch: &str, base: Option<&str>) -> Result<Plan> {
    plan_into(main, branch, base, None, None)
}

/// The same, put somewhere of somebody's choosing.
///
/// `at` empty means the place this app would pick. A person who says otherwise
/// is taken at their word and not corrected -- the path is on screen, and the
/// only thing that would be gained by overruling them is being wrong somewhere
/// they cannot see.
pub fn plan_into(
    main: &Path,
    branch: &str,
    base: Option<&str>,
    at: Option<&Path>,
    env: Option<crate::devcontainer::Env>,
) -> Result<Plan> {
    plan_for(main, None, branch, base, at, env)
}

/// The same, for a project that has been written down.
pub fn plan_for(
    main: &Path,
    project: Option<&str>,
    branch: &str,
    base: Option<&str>,
    at: Option<&Path>,
    env: Option<crate::devcontainer::Env>,
) -> Result<Plan> {
    let branch = branch.trim().to_string();
    if branch.is_empty() {
        bail!(crate::i18n::t("err.worktree.no_branch"));
    }
    if !name_is_usable(&branch) {
        bail!(crate::i18n::tp("err.worktree.bad_branch", &[("name", &branch)]));
    }
    let main = crate::repo::main_checkout(main)
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.worktree.not_a_repo")))?;
    let fresh = !branch_exists(&main, &branch);
    // A branch that is already open in a folder cannot be opened in a second
    // one. git refuses that too, but only once the button is pressed, and in
    // its own language -- the name is on screen while it is typed, so the
    // answer is as well
    if !fresh && let Some(open) = checked_out_at(&main, &branch) {
        bail!(crate::i18n::tp(
            "err.worktree.in_use",
            &[("branch", &branch), ("path", &open.display().to_string())]
        ));
    }
    let base = match base.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => b.to_string(),
        None => default_base(&main),
    };
    let folder = match at.filter(|p| !p.as_os_str().is_empty()) {
        Some(p) => p.to_path_buf(),
        None => folder_for_project(&main, project, &branch),
    };
    // Said now rather than when the button is pressed. Every branch of every
    // project shares one place, so the name that is free here can be a folder
    // somebody else's project is already standing in -- and a path that is on
    // screen looking fine until you press it is the worst way to find out
    free_to_make(&folder)?;
    Ok(Plan { folder, main, branch, base, fresh, host: None, origin: String::new(), env })
}

/// Whether a folder can be made here, in the words the person will read.
///
/// Asked twice on purpose: once while the name is being typed, and again by
/// the one that actually makes it -- between those two a folder can appear,
/// and the second asking is the one that is true
fn free_to_make(folder: &Path) -> Result<()> {
    match folder.exists() {
        true => bail!(crate::i18n::tp(
            "err.worktree.exists",
            &[("path", &folder.display().to_string())]
        )),
        false => Ok(()),
    }
}

/// Works out what making this branch on another machine would do.
///
/// Almost nothing the local planner does can be done from here without asking
/// the far end -- whether a branch exists, what the remote calls its default,
/// whether a folder can be written to -- and asking costs a round trip. The
/// dialog asks again on every keystroke, so this asks nothing: it assembles
/// the line, shows it, and lets git on the far side be the one that refuses.
/// A refusal from git arrives with git's own words, which is better than a
/// guess made here.
pub fn plan_on(
    host: &crate::config::HostSpec,
    branch: &str,
    base: Option<&str>,
    at: Option<&str>,
    origin: &str,
    env: Option<crate::devcontainer::Env>,
) -> Result<Plan> {
    let branch = branch.trim().to_string();
    if branch.is_empty() {
        bail!(crate::i18n::t("err.worktree.no_branch"));
    }
    if !name_is_usable(&branch) {
        bail!(crate::i18n::tp("err.worktree.bad_branch", &[("name", &branch)]));
    }
    // A machine that is made has no project on it yet, so what it needs is
    // somewhere to fetch one from; one that is already there needs the folder
    // the project is already in. Neither can stand in for the other
    let project = match host.is_made() {
        true => {
            if origin.trim().is_empty() {
                bail!(crate::i18n::tp("err.worktree.no_origin", &[("host", &host.name)]));
            }
            // Where a clone lands on a machine built from an image: its own
            // home, which is the one path every one of these images has
            host.project.as_deref().map(str::trim).filter(|p| !p.is_empty()).unwrap_or("/home/user")
        }
        false => {
            let p = host.project.as_deref().map(str::trim).unwrap_or_default();
            if p.is_empty() {
                bail!(crate::i18n::tp("err.worktree.no_project", &[("host", &host.name)]));
            }
            p
        }
    };
    let folder = match at.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => p.to_string(),
        None => match host.is_made() {
            // Nothing is there yet, so the only shape to keep is the branch's
            true => format!(
                "{}/{}",
                project.trim_end_matches('/'),
                branch.split('/').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/")
            ),
            false => remote_folder(host, project, &branch),
        },
    };
    Ok(Plan {
        main: PathBuf::from(project),
        branch,
        folder: PathBuf::from(folder),
        base: base.map(str::trim).filter(|b| !b.is_empty()).unwrap_or("origin/main").to_string(),
        // Git says otherwise if it is not, and says it in git's words
        fresh: true,
        host: Some(host.clone()),
        origin: origin.trim().to_string(),
        // Whatever kind of machine it is. A new worktree is bare wherever it
        // is cut: nothing is installed in it, here or on a server
        env,
    })
}

/// Where a branch goes on a machine this program has never looked at.
///
/// The same shape as here -- one place, a folder per project, the branch's own
/// shape kept -- with forward slashes, because the far end is a server and a
/// server is not Windows often enough to guess otherwise. Where that one place
/// is has to be told to us: nothing can be worked out about a machine from
/// here, and inventing `$HOME` for a server we have never seen is inventing
fn remote_folder(host: &crate::config::HostSpec, project: &str, branch: &str) -> String {
    let project = project.trim_end_matches('/');
    let leaf = project.rsplit('/').next().unwrap_or("repo");
    let root = match host.branches.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => b.trim_end_matches('/').to_string(),
        // Beside the checkout, which is the only place we can name without
        // being told -- and it is where a person who has not said would look
        None => format!("{}.branches", project),
    };
    let leafs: Vec<&str> = branch.split('/').filter(|s| !s.is_empty()).collect();
    match host.branches.is_some() {
        true => format!("{root}/{leaf}/{}", leafs.join("/")),
        false => format!("{root}/{}", leafs.join("/")),
    }
}

/// Makes the folder, and remembers what it was cut from.
///
/// The note goes into the repository's own settings rather than ours: what a
/// branch grew from is a fact about the branch, and one that outlives this app
/// being installed
pub fn create(plan: &Plan) -> Result<()> {
    // A folder on another machine is not ours to look at, and git over there
    // refuses in its own words if something is already standing in the way
    if plan.host.is_none() {
        free_to_make(&plan.folder)?;
        if let Some(parent) = plan.folder.parent() {
            std::fs::create_dir_all(parent)?;
        }
    }
    for argv in plan.argvs() {
        run_for(plan, &argv)?;
    }
    if plan.fresh {
        // Best effort: the folder is made and usable either way, and a missing
        // note only means a later diff has to guess its starting point
        let _ = run_for(
            plan,
            &[
                "git".into(),
                "-C".into(),
                plan.folder.display().to_string(),
                "config".into(),
                format!("branch.{}.shikishaBase", plan.branch),
                plan.base.clone(),
            ],
        );
    }
    Ok(())
}

/// What a new folder will not have, and cannot get from git.
///
/// A branch's folder arrives with everything git tracks and nothing it does
/// not: no `.env`, no `node_modules`, no build cache. The first thing anyone
/// does in it is fail to build, which makes "start another branch" a promise
/// the app does not keep. So the things git was told to ignore, that are
/// actually there, are offered to come along.
///
/// Asked of git rather than guessed from a list of names, because what counts
/// as ignored is the repository's own answer and it is written down already.
pub fn carryables(main: &Path) -> Vec<Carry> {
    let mut names: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(main) else {
        return Vec::new();
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // Never git's own folder: the new checkout has one of those already,
        // and it is the whole reason the two are separate
        if name == ".git" || name.is_empty() {
            continue;
        }
        names.push(name);
    }
    names.sort_by_key(|n| n.to_lowercase());
    let ignored = ignored_of(main, &names);
    names
        .into_iter()
        .filter(|n| ignored.contains(n))
        .map(|name| {
            let at = main.join(&name);
            Carry {
                folder: at.is_dir(),
                // A secret is not carried unless somebody says so. Copying one
                // is how a token ends up in three places nobody is watching
                on: !looks_secret(&name),
                name,
            }
        })
        .collect()
}

/// One thing a new folder can be given a copy of.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Carry {
    pub name: String,
    /// Whether it is a folder: those are linked rather than copied
    pub folder: bool,
    /// Whether it starts ticked
    pub on: bool,
}

/// Whether a name is the sort of thing that holds a live secret.
fn looks_secret(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with(".env")
        || n.contains("secret")
        || n.contains("credential")
        || n.ends_with(".pem")
        || n.ends_with(".key")
        || n.starts_with("id_rsa")
        || n.starts_with("id_ed25519")
}

/// Which of these names the repository ignores, according to the repository.
fn ignored_of(main: &Path, names: &[String]) -> std::collections::HashSet<String> {
    // Asked twice at most. Git answers 0 when some of these are ignored and 1
    // when none are; anything else is git not having answered at all -- it can
    // refuse for a moment while the index is still locked by the commit before
    // it, and coming back with "nothing is ignored" would quietly offer an
    // empty list instead of the truth
    for attempt in 0..2 {
        match ask_ignored(main, names) {
            Some(found) => return found,
            None => std::thread::sleep(std::time::Duration::from_millis(60 * (attempt + 1))),
        }
    }
    Default::default()
}

/// One asking. `None` means git did not answer, which is not the same as
/// answering that nothing is ignored
fn ask_ignored(main: &Path, names: &[String]) -> Option<std::collections::HashSet<String>> {
    use std::io::Write as _;
    let mut asking = std::process::Command::new("git");
    asking
        .arg("-C")
        .arg(main)
        .args(["check-ignore", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let mut child = crate::detach_console(&mut asking).spawn().ok()?;
    let mut wrote = true;
    if let Some(mut w) = child.stdin.take() {
        wrote = w.write_all(names.join("\n").as_bytes()).is_ok();
    }
    let out = child.wait_with_output().ok()?;
    // 0 = some are ignored, 1 = none are. Anything else is a refusal
    if !wrote || !matches!(out.status.code(), Some(0) | Some(1)) {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().trim_matches('"').to_string())
            .filter(|l| !l.is_empty())
            .collect(),
    )
}

/// Gives a new folder the things that were ticked.
///
/// A folder is linked rather than copied: `node_modules` is a gigabyte and
/// copying it per branch is a way to fill a disk. A file is copied, because
/// linking one means editing it in the branch edits it everywhere.
///
/// Every failure is left as a warning rather than undoing the branch: the
/// checkout is made and usable, and someone who wanted three of these and got
/// two would rather be told which one is missing than have the whole thing
/// taken away again.
pub fn carry_into(plan: &Plan, names: &[String]) -> Vec<String> {
    let mut trouble = Vec::new();
    for name in names {
        if name.contains('/') || name.contains('\\') || name == ".." {
            continue;
        }
        let from = plan.main.join(name);
        let to = plan.folder.join(name);
        if !from.exists() || to.exists() {
            continue;
        }
        let done = match from.is_dir() {
            true => link_folder(&from, &to),
            false => std::fs::copy(&from, &to).is_ok(),
        };
        if !done {
            trouble.push(name.clone());
        }
    }
    trouble
}

/// A second name for one folder, made the way this system lets anyone make one.
///
/// On Windows that is a junction: a symbolic link there needs rights most
/// people running this do not have. Everywhere else a symbolic link is the
/// ordinary thing and needs nothing.
fn link_folder(from: &Path, to: &Path) -> bool {
    #[cfg(windows)]
    {
        let mut link = std::process::Command::new("cmd");
        link.args(["/c", "mklink", "/J"])
            .arg(to)
            .arg(from)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::detach_console(&mut link)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to).is_ok()
    }
}

/// Gets rid of a branch's folder, once there is nothing in it to lose.
///
/// Refused while anything is uncommitted. A folder full of work that only
/// exists there is the one thing this must never take, and "are you sure" is
/// not a good enough answer when the app is the one that made the folder in
/// the first place. What it does not check is whether the branch was merged:
/// that is a judgement, and it belongs to the person.
pub fn discard(folder: &Path) -> Result<()> {
    if !folder.exists() {
        return Ok(());
    }
    // Already out of git's hands, and only the empty folder is left: Windows
    // keeps one alive while it is some process's working folder, and the tab
    // that was standing here has just been asked to leave. Nothing but an
    // empty folder is ever removed this way, so it can take nothing with it
    if !crate::repo::is_linked(folder) {
        let empty = std::fs::read_dir(folder).map(|d| d.count() == 0).unwrap_or(false);
        if empty {
            std::fs::remove_dir(folder)?;
            return Ok(());
        }
    }
    ready_to_discard(folder)?;
    let main = crate::repo::main_checkout(folder)
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.worktree.not_a_repo")))?;
    // Git's own removal, so the repository stops listing it too. Anything
    // linked into the folder is unhooked first: removing the folder with a
    // junction still in it walks through and takes what is on the other side
    for e in std::fs::read_dir(folder)?.flatten() {
        let at = e.path();
        let linked = std::fs::symlink_metadata(&at)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if linked && at.is_dir() {
            let _ = std::fs::remove_dir(&at);
        }
    }
    run(&[
        "git".into(),
        "-C".into(),
        main.display().to_string(),
        "worktree".into(),
        "remove".into(),
        folder.display().to_string(),
    ])?;
    // Git empties the folder but can leave the folder itself, because Windows
    // holds one open while it is some process's working folder -- and the tab
    // that just closed was standing in this one. Only ever removed empty, so
    // this can never take anything with it
    let _ = std::fs::remove_dir(folder);
    Ok(())
}

/// Removes a folder once whatever is standing in it has left.
///
/// Git will not remove a folder a process is standing in, and what stands in
/// this one is the tabs that were working there -- which are on their way out,
/// because taking the folder out of the settings is what ends them. So the
/// removal waits, off to one side, rather than failing on the first try.
///
/// Nothing here decides *whether* it should go: that was settled by
/// `ready_to_discard` before anything was closed, while saying no was still
/// free.
pub fn discard_soon(folder: std::path::PathBuf) {
    std::thread::spawn(move || {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            // Gone is the only thing that counts as done: git can let go of a
            // folder while Windows still holds the empty shell of it open
            if !folder.exists() {
                return;
            }
            let trouble = discard(&folder).err();
            if std::time::Instant::now() > until {
                if let Some(e) = trouble {
                    crate::append_hook_log(&format!("could not remove {}: {e:#}", folder.display()));
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
    });
}

/// A branch about to be called something else.
///
/// The folder stays where it is. It was named after the branch on the day it
/// was made, and the two go their own ways from here -- which is what git
/// itself does, since a folder's branch can be switched at any time and nobody
/// expects the folder to move. Moving it would take the tabs standing in it
/// with it, and a build's leftovers name their own folder in every file they
/// wrote, so a move is a rebuild too
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub folder: PathBuf,
    pub from: String,
    pub to: String,
    /// The remote branch this one follows, when it has been sent before
    pub sent_as: Option<String>,
}

impl Rename {
    /// What will run, in the words git will get
    pub fn argv(&self) -> Vec<String> {
        vec![
            "git".into(),
            "-C".into(),
            self.folder.display().to_string(),
            "branch".into(),
            "-m".into(),
            self.from.clone(),
            self.to.clone(),
        ]
    }

    /// The same line, as a person reads it
    pub fn line(&self) -> String {
        self.argv()
            .iter()
            .map(|a| match a.contains(' ') {
                true => format!("\"{a}\""),
                false => a.clone(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Works out what calling this folder's branch something else would do.
///
/// Only a folder this app cut for a branch: the project's own checkout is on
/// the branch the project is on, and renaming that from a settings screen is
/// not what anybody came here for.
pub fn rename_plan(folder: &Path, to: &str) -> Result<Rename> {
    let to = to.trim().to_string();
    if to.is_empty() {
        bail!(crate::i18n::t("err.worktree.no_branch"));
    }
    if !name_is_usable(&to) {
        bail!(crate::i18n::tp("err.worktree.bad_branch", &[("name", &to)]));
    }
    if !crate::repo::is_linked(folder) {
        bail!(crate::i18n::t("err.worktree.not_a_branch"));
    }
    let from = crate::repo::branch_of(folder)
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.worktree.not_a_branch")))?;
    Ok(Rename { folder: folder.to_path_buf(), from, to, sent_as: upstream_of(folder) })
}

/// Does it, and stops the new name from pushing under the old one.
///
/// Git moves `branch.<name>.*` across on its own, the note about where this
/// branch grew from included, but it leaves the branch following the remote
/// branch it was pushed to -- under the name it had then. Left alone, the next
/// push goes quietly to the old name and the new one never appears. Letting
/// the following go means the next push says it is setting one up, which is
/// the truth
pub fn rename(r: &Rename) -> Result<()> {
    run(&r.argv())?;
    if r.sent_as.is_some() {
        let _ = run(&[
            "git".into(),
            "-C".into(),
            r.folder.display().to_string(),
            "branch".into(),
            "--unset-upstream".into(),
        ]);
    }
    Ok(())
}

/// The remote branch this folder's branch follows, if it follows one.
fn upstream_of(folder: &Path) -> Option<String> {
    let mut asking = std::process::Command::new("git");
    asking
        .arg("-C")
        .arg(folder)
        .args(["rev-parse", "--abbrev-ref", "@{upstream}"]);
    let out = crate::detach_console(&mut asking).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let said = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!said.is_empty()).then_some(said)
}

/// Whether this folder can be thrown away at all -- asked before anything is
/// closed, so a refusal costs nothing.
pub fn ready_to_discard(folder: &Path) -> Result<()> {
    if !folder.exists() {
        return Ok(());
    }
    if !crate::repo::is_linked(folder) {
        bail!(crate::i18n::t("err.worktree.not_a_branch"));
    }
    let mut asking = std::process::Command::new("git");
    asking.arg("-C").arg(folder).args(["status", "--porcelain"]);
    let dirty = crate::detach_console(&mut asking).output()?;
    let said = String::from_utf8_lossy(&dirty.stdout);
    if !said.trim().is_empty() {
        bail!(crate::i18n::tp(
            "err.worktree.dirty",
            &[("count", &said.lines().count().to_string())]
        ));
    }
    Ok(())
}

/// Where a branch's folder goes, and why there.
///
/// Under the person's own folder, in one place that holds every branch of
/// every project. **The project's folder is left exactly as it was** -- which
/// is the point: beside the checkout, every project grew a second folder next
/// to it that nobody made and nobody asked for, and it turned up in the
/// editor's file tree, in backups, and in whatever the person had pointed at
/// the folder that project lives in.
///
/// Three things send it somewhere else instead: a home folder that cannot be
/// written to, one that is being synced to the cloud, where every branch would
/// be uploaded in full, and one long enough that what lands inside the folder
/// would not fit.
pub fn folder_for(main: &Path, branch: &str) -> PathBuf {
    folder_for_project(main, None, branch)
}

/// The same, for a project that has a name of its own.
///
/// The name is what keeps two projects apart. Without one there is only the
/// checkout's folder name, and two clones both sitting in a folder called
/// `api` are handed the same place -- refused rather than overwritten, but
/// refused is still a person stuck. A project that has been written down is
/// told apart by what it is called, which is unique because names are.
pub fn folder_for_project(main: &Path, project: Option<&str>, branch: &str) -> PathBuf {
    let name = project
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            main.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "repo".into())
        });
    // The branch's own shape is kept: `feature/login` is two folders, which is
    // what makes it impossible for two branches to want one folder
    let leaf: PathBuf = branch.split('/').filter(|s| !s.is_empty()).collect();
    let at = branches_root().join(&name).join(&leaf);
    match short_enough(&at) {
        true => at,
        // Ours, per machine, which is as short as this program can offer
        false => away_from_home().join(&name).join(&leaf),
    }
}

/// Whether a whole source tree will fit under this folder.
///
/// Windows stops most programs at 260 characters, and the number that matters
/// is not this folder's -- it is this folder plus the longest path inside it,
/// which nobody can know. So the folder is held well short, and what is left
/// is for the project's own files.
///
/// Measured, not guessed at: a home folder 186 characters long put this folder
/// at 225 and git could not make it, while the same branch under a short home
/// was fine. The limit below is the one this app has always used
/// The same guard, applied to whichever name was used
fn short_enough(at: &Path) -> bool {
    at.display().to_string().chars().count() < 180
}

/// The one place branches live, worked out once.
///
/// Once because the answer costs a folder made and removed in the person's
/// home, and the dialog asks for it on every keystroke -- and because whether
/// a home folder can be written to does not change while a program is running
fn branches_root() -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        // A test run never writes into the person's own folder. A run that
        // leaves folders in somebody's home has changed the machine it was
        // meant to be checking, and two runs at once would fight over the same
        // names -- so the process gets one of its own. What the real answer
        // would be is checked directly, by a test of its own
        if cfg!(test) {
            return std::env::temp_dir()
                .join(format!("shikisha-branches-{}", std::process::id()));
        }
        real_branches_root()
    })
    .clone()
}

/// The person's own folder, or ours when theirs cannot hold it.
fn real_branches_root() -> PathBuf {
    match home_dir().filter(|h| !synced(h) && writable(h)) {
        // Named for the program, then for what these are, so somebody who
        // finds this folder without being told can tell both
        Some(home) => home.join("SHIKISHA-TERM").join("branches"),
        None => away_from_home(),
    }
}

/// The person's own folder, as this system spells it.
fn home_dir() -> Option<PathBuf> {
    for key in ["USERPROFILE", "HOME"] {
        let Ok(said) = std::env::var(key) else { continue };
        let at = PathBuf::from(said.trim());
        if at.is_dir() {
            return Some(at);
        }
    }
    None
}

/// The place for branches that cannot sit in the person's own folder. Ours,
/// per machine, and never synced anywhere
fn away_from_home() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("SHIKISHA-TERM").join("worktrees")
}

/// Whether a folder is being copied to the cloud as it changes.
///
/// Only what can be known for certain is checked. Everything else is left to
/// the path being on screen and editable before anything is made: a guess that
/// moved someone's files somewhere they did not choose would be worse than
/// letting them see where they are going
fn synced(dir: &Path) -> bool {
    let here = dir.display().to_string().to_lowercase();
    for key in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
        if let Ok(root) = std::env::var(key)
            && !root.is_empty() && here.starts_with(&root.to_lowercase()) {
                return true;
            }
    }
    // Dropbox leaves this beside the folder it syncs
    let mut at = Some(dir);
    while let Some(d) = at {
        if d.join(".dropbox.device").exists() || d.join(".dropbox").exists() {
            return true;
        }
        at = d.parent();
    }
    false
}

/// Whether we could actually make a folder here. Asked by trying, because
/// permissions on Windows are not something to be reasoned about from a path
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".shikisha-probe-{}", crate::random_hex(6)));
    match std::fs::create_dir(&probe) {
        Ok(()) => {
            let _ = std::fs::remove_dir(&probe);
            true
        }
        Err(_) => false,
    }
}

/// What a new branch grows from, when nobody says.
///
/// The default branch as the remote sees it, because branches cut for parallel
/// work are meant to become pull requests. Growing them from whatever is
/// checked out right now would hand every one of them somebody's unfinished
/// experiment.
pub fn default_base(main: &Path) -> String {
    let Some(git) = crate::repo::family_of(main) else {
        return "HEAD".into();
    };
    // What `origin` calls its default, when it has been asked and written down
    if let Ok(text) = std::fs::read_to_string(git.join("refs/remotes/origin/HEAD"))
        && let Some(r) = text.trim().strip_prefix("ref: refs/remotes/")
            && !r.is_empty() {
                return r.to_string();
            }
    for name in ["origin/main", "origin/master"] {
        if ref_exists(&git, &format!("refs/remotes/{name}")) {
            return name.to_string();
        }
    }
    // Nothing to compare with, so it grows from where the checkout is. Said as
    // the branch's own name rather than as HEAD: the same commit either way,
    // and one of the two is a word people use
    crate::repo::branch_of(main).unwrap_or_else(|| "HEAD".into())
}

/// The branches a new one could grow from, best first.
///
/// What the remote calls its default, then the rest of what the remote has,
/// then this machine's own branches. Read out of the repository's files
/// rather than by running git, because this is asked while someone is typing.
pub fn bases(main: &Path) -> Vec<String> {
    let Some(git) = crate::repo::family_of(main) else {
        return vec!["HEAD".into()];
    };
    let mut out = vec![default_base(main)];
    let mut add = |name: String| {
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    };
    // Loose refs, then the ones packed away together
    for (under, prefix) in [("refs/remotes", ""), ("refs/heads", "")] {
        let root = git.join(under);
        let mut todo = vec![root.clone()];
        while let Some(dir) = todo.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                let path = e.path();
                if path.is_dir() {
                    todo.push(path);
                    continue;
                }
                if let Ok(rest) = path.strip_prefix(&root) {
                    let name = rest.to_string_lossy().replace('\\', "/");
                    // `origin/HEAD` is a pointer at another of these, not a
                    // branch anyone means to start from
                    if !name.ends_with("HEAD") {
                        add(format!("{prefix}{name}"));
                    }
                }
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(git.join("packed-refs")) {
        for line in text.lines() {
            let Some((_, r)) = line.split_once(' ') else { continue };
            let r = r.trim();
            for under in ["refs/remotes/", "refs/heads/"] {
                if let Some(name) = r.strip_prefix(under)
                    && !name.ends_with("HEAD") {
                        add(name.to_string());
                    }
            }
        }
    }
    out.truncate(200);
    out
}

/// Whether a name drawn for new work is still free: no branch of that name
/// here, and nothing standing where its folder would go
pub fn is_free(main: &Path, name: &str) -> bool {
    !branch_exists(main, name) && !folder_for(main, name).exists()
}

/// A name for the next branch, when nobody has one in mind.
///
/// Short and countable rather than unique-by-construction: this ends up on a
/// pull request, and `work-3` is something a person can say out loud, which
/// a timestamp and a handful of hex is not. Free names only -- the first one
/// that is not already a branch here.
pub fn suggest(main: &Path) -> String {
    // Two words rather than a number, because this name is what the branch is
    // called for as long as the work has no title, and a list of them has to be
    // readable: `work-2` and `work-3` sit next to each other and say the same
    // nothing, while `mighty-gannet` and `polite-marmot` tell themselves apart
    // across a room. An adjective and a noun -- never a real person's surname,
    // which a random adjective in front of it is one draw away from insulting
    //
    // A name has to clear two things, not one. The branch is this
    // repository's own business, but the folder is shared with every other
    // project -- two projects whose folders are both called `api` are handed
    // the same place, and a name free in one of them can be taken in the
    // other. Asking costs a look at the disk per draw, which is affordable
    // now that working out where a folder goes is arithmetic on a path and
    // no longer a probe
    let free = |name: &str| is_free(main, name);
    for _ in 0..20 {
        match petname::petname(2, "-") {
            Some(name) if free(&name) => return name,
            // Drawn again: two draws can land on one name, and the list is
            // large enough that they rarely do twice
            Some(_) => continue,
            // No word lists at all, which is not a reason to offer no name
            None => break,
        }
    }
    // From two, because the checkout itself is the first piece of work
    for n in 2..200 {
        let name = format!("work-{n}");
        if free(&name) {
            return name;
        }
    }
    String::new()
}

/// One folder per AI: the same name with the AI's own on the end, so the
/// branches say at a glance who is working on which. Every plan is made
/// before any runs, so what is shown is the whole of what will happen
pub fn fan(main: &Path, name: &str, base: Option<&str>, ais: &[String]) -> Vec<(String, Result<Plan>)> {
    ais.iter()
        .map(|ai| {
            let ai = ai.trim().to_string();
            let branch = format!("{}-{ai}", name.trim());
            (ai.clone(), plan(main, &branch, base))
        })
        .collect()
}

/// Where a branch is already open, if it is.
///
/// Read off the files git keeps that answer in, without starting git: the
/// checkout's own HEAD, then each linked worktree's HEAD beside the note of
/// where that worktree is. The dialog asks on every keystroke
fn checked_out_at(main: &Path, branch: &str) -> Option<PathBuf> {
    let git = crate::repo::family_of(main)?;
    let wanted = format!("ref: refs/heads/{branch}");
    let on = |head: &Path| std::fs::read_to_string(head).is_ok_and(|t| t.trim() == wanted);
    if on(&git.join("HEAD")) {
        return Some(main.to_path_buf());
    }
    for entry in std::fs::read_dir(git.join("worktrees")).ok()?.flatten() {
        let dir = entry.path();
        if on(&dir.join("HEAD")) {
            // `gitdir` holds the worktree's own .git file; the folder is the
            // one around it. Unreadable, the answer is still "open somewhere"
            return Some(match std::fs::read_to_string(dir.join("gitdir")) {
                // git writes it with forward slashes on every system; put
                // back together from its parts it is spelled the way the
                // rest of the screen spells a path here
                Ok(t) => {
                    let p: PathBuf = Path::new(t.trim()).components().collect();
                    p.parent().map(Path::to_path_buf).unwrap_or(p)
                }
                Err(_) => dir,
            });
        }
    }
    None
}

/// Whether a branch of this name is already in the repository.
fn branch_exists(main: &Path, branch: &str) -> bool {
    match crate::repo::family_of(main) {
        Some(git) => ref_exists(&git, &format!("refs/heads/{branch}")),
        None => false,
    }
}

/// A ref, whether it is kept as a file or packed away with the others.
fn ref_exists(git: &Path, full: &str) -> bool {
    if git.join(full).exists() {
        return true;
    }
    match std::fs::read_to_string(git.join("packed-refs")) {
        Ok(text) => text
            .lines()
            .any(|l| l.split_once(' ').is_some_and(|(_, r)| r.trim() == full)),
        Err(_) => false,
    }
}

/// Whether git would accept this as a branch name.
///
/// Only the rules that matter here: the ones that would otherwise turn into a
/// path we did not intend, or an error from git that says nothing useful
fn name_is_usable(branch: &str) -> bool {
    !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch.ends_with(".lock")
        && !branch.contains("//")
        && !branch.contains("..")
        && !branch.contains('\\')
        && !branch
            .chars()
            .any(|c| c.is_control() || " ~^:?*[".contains(c))
}

/// Runs one command and complains in the person's language when it fails.
/// The same line, run wherever this plan belongs.
///
/// One door, so nothing has to remember to ask "which machine" a second time.
/// A failure over there arrives with git's own words, the same as here
pub fn run_for(plan: &Plan, argv: &[String]) -> Result<()> {
    let Some(host) = plan.host.as_ref() else {
        return run(argv);
    };
    // A machine that is made is asked for once and then kept, so the two
    // commands of a clone land on the same machine. A second plan gets a
    // second machine, which is right: two folders are two machines
    if host.is_made() {
        let sandbox =
            crate::e2b::sandbox_for(host, plan.env.as_ref().and_then(|e| e.image.as_deref()))?;
        let line = for_a_shell(argv);
        let ran = crate::e2b::exec(&sandbox, &line, None)?;
        if ran.ok() {
            return Ok(());
        }
        bail!(crate::i18n::tp(
            "err.worktree.failed",
            &[("said", &ran.said()), ("command", &line)]
        ));
    }
    let spec = crate::config::host_spec(host)?;
    let line = for_a_shell(argv);
    let ran = crate::ssh::exec(&spec, &line, 60_000)?;
    if ran.ok() {
        return Ok(());
    }
    bail!(crate::i18n::tp(
        "err.worktree.failed",
        &[("said", &ran.said()), ("command", &line)]
    ))
}

/// One command, in the words a server's shell wants.
///
/// Not this machine's shell: the far end is a server, and single quotes are
/// what a server's shell takes literally. Only where they are needed, so an
/// ordinary path stays readable on screen and in the log
fn for_a_shell(argv: &[String]) -> String {
    argv.iter()
        .map(|a| match a.contains(' ') || a.contains('\'') || a.contains('"') {
            // A server's shell, not this one: single quotes, and a single
            // quote inside them closed and reopened the way sh wants
            true => format!("'{}'", a.replace('\'', "'\\''")),
            false => a.clone(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn run(argv: &[String]) -> Result<()> {
    let (head, rest) = argv.split_first().expect("空のコマンド");
    let mut running = std::process::Command::new(head);
    running.args(rest);
    // A clone of a private repository will ask for a password, and there is
    // nowhere for it to ask: this app has no console, so git would wait for an
    // answer that can never come and the whole thing would hang with nothing on
    // screen. Told not to ask, it fails in a second and says why — which the
    // person can act on
    running.env("GIT_TERMINAL_PROMPT", "0");
    // Nothing here may put a black window on somebody's screen: this app has no
    // console of its own, so every one of these would flash one open
    let out = crate::detach_console(&mut running).output()?;
    if out.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&out.stderr);
    let said = said.trim();
    bail!(crate::i18n::tp(
        "err.worktree.failed",
        &[("said", said), ("command", &argv.join(" "))]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder this run of the tests can have to itself.
    ///
    /// Two runs at once is the ordinary case here -- the suite runs in the
    /// worktree of one branch while another is still going -- and a name fixed
    /// in the temp folder means one run deleting the repository the other is in
    /// the middle of using. What comes back then is a git error about a missing
    /// object, which sends whoever reads it looking at git. Same shape as
    /// [`branches_root`] under test: the process gets its own
    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("shikisha-wt-{name}-{}", std::process::id()))
    }

    /// A project of this test's own. Named after the test, because where
    /// branches go is keyed by the project's name alone now -- two tests both
    /// calling their project "myproject" would be handed each other's folders
    fn repo(name: &str) -> PathBuf {
        let d = scratch(name).join(format!("proj-{name}"));
        let _ = std::fs::remove_dir_all(d.parent().unwrap());
        let _ = std::fs::remove_dir_all(branches_root().join(format!("proj-{name}")));
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        d
    }

    /// A branch already open somewhere is refused while its name is typed,
    /// naming where it is open -- the checkout itself, or a worktree
    #[test]
    fn a_branch_open_in_another_folder_is_said_before_the_button() {
        let main = repo("inuse");
        let git = main.join(".git");
        for b in ["main", "feature-x", "spare"] {
            std::fs::create_dir_all(git.join("refs/heads")).unwrap();
            std::fs::write(git.join("refs/heads").join(b), "3c06a89\n").unwrap();
        }
        let wt = scratch("inuse").join("wt").join("feature-x");
        std::fs::create_dir_all(git.join("worktrees/feature-x")).unwrap();
        std::fs::write(git.join("worktrees/feature-x/HEAD"), "ref: refs/heads/feature-x\n").unwrap();
        std::fs::write(git.join("worktrees/feature-x/gitdir"), format!("{}\n", wt.join(".git").display())).unwrap();

        let said = plan(&main, "feature-x", None).unwrap_err().to_string();
        assert!(said.contains(&wt.display().to_string()), "どのフォルダで開いているかを言わない: {said}");
        let said = plan(&main, "main", None).unwrap_err().to_string();
        assert!(said.contains(&main.display().to_string()), "{said}");
        // A branch that exists and is open nowhere is simply checked out
        let p = plan(&main, "spare", None).expect("空いているブランチは開ける");
        assert!(!p.fresh);
    }

    #[test]
    fn a_branch_gets_a_folder_of_its_own_away_from_the_project() {
        let main = repo("place");
        let at = folder_for(&main, "feature/login");
        // The project's folder is left alone: nothing of ours appears beside it
        assert!(!at.starts_with(main.parent().unwrap()), "本体の隣に置いている: {at:?}");
        // One place holds all of them, under the project they belong to
        assert!(at.starts_with(branches_root().join("proj-place")), "{at:?}");
        assert!(at.ends_with("feature/login"), "枝の名前がそのまま入れ子になる: {at:?}");
        // Two branches that differ only in shape never want the same folder
        assert_ne!(folder_for(&main, "feature/login"), folder_for(&main, "feature-login"));
        // Two projects of the same name in different places still collide here,
        // which is the price of one place; the folder is refused when it is
        // already there rather than written into
        assert_eq!(folder_for(&main, "x"), folder_for(Path::new("Z:/elsewhere/proj-place"), "x"));
    }

    /// Where the branches go does not depend on where the project is.
    ///
    /// A project on a drive that is not there, or one nobody may write to, used
    /// to send its branches somewhere else. Now there is nowhere else to send
    /// them: they were never going to sit beside it
    #[test]
    fn a_project_nobody_can_write_to_changes_nothing() {
        let at = folder_for(Path::new("Z:/nowhere/myproject"), "fix/crash");
        assert!(at.starts_with(branches_root()), "{at:?}");
        assert!(at.ends_with("fix/crash"));
    }

    /// A machine that is made fresh has no project on it, so the project is
    /// fetched before the branch is cut.
    ///
    /// Two commands, both on screen. A person checking what is about to happen
    /// is owed all of it: shown only the first half, they would be agreeing to
    /// a clone and getting a branch as well
    #[test]
    fn a_machine_made_from_nothing_is_given_the_project_first() {
        let host = crate::config::HostSpec {
            name: "sandbox".into(),
            kind: Some("e2b".into()),
            template: Some("base".into()),
            ..Default::default()
        };
        assert!(host.is_made(), "作る機械だと見なされていない");

        let p = plan_on(&host, "polite-marmot", Some("origin/master"), None, "https://example.test/p.git", None)
            .expect("計画できる");
        let steps = p.argvs();
        assert_eq!(steps.len(), 2, "2段になっていない: {steps:?}");
        // The project arrives first, on the branch the remote calls it, with
        // the remote's own prefix left off -- a clone has no remotes yet
        assert_eq!(
            steps[0],
            ["git", "clone", "--branch", "master", "https://example.test/p.git", "/home/user/polite-marmot"]
        );
        assert_eq!(steps[1], ["git", "-C", "/home/user/polite-marmot", "switch", "-c", "polite-marmot"]);
        // Both of them are what the person reads
        assert_eq!(p.line().lines().count(), 2, "片方しか見えていない: {}", p.line());

        // Nowhere to fetch from is a refusal, not a clone of nothing
        assert!(plan_on(&host, "polite-marmot", Some("origin/master"), None, "", None).is_err());

        // A machine that is already there is one command, from the checkout
        let there = crate::config::HostSpec {
            name: "bench".into(),
            at: "ssh://me@host:22".into(),
            project: Some("/srv/p".into()),
            ..Default::default()
        };
        assert!(!there.is_made());
        let q = plan_on(&there, "polite-marmot", Some("main"), None, "", None).expect("計画できる");
        assert_eq!(q.argvs().len(), 1);
        assert!(q.line().contains("worktree add"), "{}", q.line());
    }

    /// A project with a name of its own keeps its branches apart from another
    /// project whose folder happens to be called the same thing.
    ///
    /// Without a name there is only the folder's, and two clones both sitting
    /// in a folder called `api` are handed one place. It is refused rather
    /// than written into, but refused is still somebody stuck for a reason
    /// they did not cause
    #[test]
    fn two_projects_of_one_name_are_two_places_once_they_are_named() {
        let a = repo("named-a");
        let b = PathBuf::from("Z:/elsewhere/proj-named-a");
        // The same folder name, so the same place -- this is the collision
        assert_eq!(folder_for(&a, "work"), folder_for(&b, "work"));
        // Written down, they are two
        assert_ne!(
            folder_for_project(&a, Some("ours"), "work"),
            folder_for_project(&b, Some("theirs"), "work")
        );
        assert!(folder_for_project(&a, Some("ours"), "work").ends_with("ours/work"));
        // An empty name is the same as none: nothing was written down
        assert_eq!(folder_for_project(&a, Some("  "), "work"), folder_for(&a, "work"));
    }

    /// A project that cannot have a devcontainer says it in the settings, and
    /// it runs the same way.
    ///
    /// This repository is the example: a Windows executable with Win32 and
    /// WebView2 in it, which no Linux container builds. Writing a devcontainer
    /// for it would be putting a lie in the repository
    #[test]
    fn a_project_that_cannot_have_the_file_still_gets_its_setup() {
        let main = repo("plainsetup");
        let told = crate::devcontainer::told(&main, Some("cargo fetch
tools/conpty.ps1"));
        let env = told.expect("設定から拾えていない");
        assert_eq!(env.setup, ["cargo fetch", "tools/conpty.ps1"], "1行に1つ");
        assert!(!env.from.is_empty(), "どこから来たのか言えていない");

        let p = plan_for(&main, Some("ours"), "work", Some("main"), None, Some(env))
            .expect("計画できる");
        assert_eq!(p.argvs().len(), 3, "枝と2行: {:?}", p.argvs());
        assert!(p.argvs()[1].last().is_some_and(|l| l.contains("cargo fetch")));
        assert!(p.argvs()[2].last().is_some_and(|l| l.contains("conpty")));

        // Nothing said either way is nothing to run
        assert!(crate::devcontainer::told(&main, None).is_none());
        assert!(crate::devcontainer::told(&main, Some("   ")).is_none());
    }

    /// A project that says what its environment needs gets it, after the
    /// project is there to need it.
    ///
    /// Two things come from that file and nothing else does: what the machine
    /// is built from, and what to run once the source has landed. Both are on
    /// screen with everything else, because a person agreeing to this is
    /// agreeing to commands the project wrote, not ones this app did
    #[test]
    fn a_project_that_says_what_it_needs_is_listened_to() {
        let host = crate::config::HostSpec {
            name: "sandbox".into(),
            kind: Some("e2b".into()),
            template: Some("base".into()),
            ..Default::default()
        };
        let env = crate::devcontainer::read(
            r#"{"image":"node:22","onCreateCommand":"npm ci","postCreateCommand":["npm","run","build"]}"#,
        );
        let p = plan_on(&host, "work", Some("origin/main"), None, "https://example.test/p.git", env)
            .expect("計画できる");
        let steps = p.argvs();
        assert_eq!(steps.len(), 4, "取得・枝・2つの支度: {steps:?}");
        assert_eq!(steps[2], ["sh", "-lc", "cd /home/user/work && npm ci"]);
        assert_eq!(steps[3], ["sh", "-lc", "cd /home/user/work && npm run build"]);
        // The project's word about the image beats the machine's setting: a
        // repository that names one has said the thing that matters most
        let picked = |i| crate::e2b::template_for(&host, i);
        assert_eq!(picked(p.env.as_ref().and_then(|e| e.image.as_deref())), "node:22");
        assert_eq!(picked(None), "base", "何も言わなければ機械の設定");
        // Every one of them is read before any of them runs
        assert_eq!(p.line().lines().count(), 4, "{}", p.line());

        // A machine that is already there still gets a folder with nothing
        // installed in it. The preparation follows the folder, not the machine
        let there = crate::config::HostSpec {
            name: "bench".into(),
            at: "ssh://me@host:22".into(),
            project: Some("/srv/p".into()),
            ..Default::default()
        };
        let q = plan_on(&there, "work", Some("main"), None, "",
                        crate::devcontainer::read(r#"{"postCreateCommand":"npm ci"}"#))
            .expect("計画できる");
        assert_eq!(q.argvs().len(), 2, "既にある機械で支度をしていない: {:?}", q.argvs());
        assert!(q.argvs()[1].last().is_some_and(|l| l.contains("npm ci")), "{:?}", q.argvs()[1]);
        // Nothing is fetched there: the project is on that machine already
        assert!(q.argvs()[0].contains(&"worktree".to_string()));

        // And on this machine too, which is as bare as any of them
        let main = repo("prep");
        let here = plan_into(&main, "work", Some("main"), None,
                             crate::devcontainer::read(r#"{"postCreateCommand":"cargo fetch"}"#))
            .expect("計画できる");
        assert_eq!(here.argvs().len(), 2, "この PC で支度をしていない");
        assert!(here.argvs()[1].last().is_some_and(|l| l.contains("cargo fetch")));
        // Turned off, nothing of it is there
        let bare = plan_into(&main, "work", Some("main"), None, None).expect("計画できる");
        assert_eq!(bare.argvs().len(), 1);
    }

    /// Somebody who names a place gets that place, and is not corrected.
    ///
    /// The path is on screen before the button is pressed, so overruling it
    /// would only be wrong somewhere they cannot see. What still holds is the
    /// refusal: a folder already standing there is said while it is typed
    #[test]
    fn a_place_of_somebody_elses_choosing_is_the_place() {
        let main = repo("elsewhere");
        let mine = scratch("chosen").join("right here");
        let _ = std::fs::remove_dir_all(&mine);
        let p = plan_into(&main, "polite-marmot", Some("main"), Some(&mine), None).expect("計画できる");
        assert_eq!(p.folder, mine, "指定した場所が使われていない");
        assert_ne!(p.folder, folder_for(&main, "polite-marmot"), "既定に引き戻されている");
        // And it is the place the command names, not just the one on screen
        assert!(p.line().contains("\"") && p.line().contains("right here"), "{}", p.line());

        // Nothing named: the app's own answer stands
        let same = plan_into(&main, "polite-marmot", Some("main"), None, None).expect("計画できる");
        assert_eq!(same.folder, folder_for(&main, "polite-marmot"));
        // An empty name is the same as none
        let blank = plan_into(&main, "polite-marmot", Some("main"), Some(Path::new("")), None)
            .expect("計画できる");
        assert_eq!(blank.folder, same.folder);

        // A place already taken is refused here, not when the button is pressed
        std::fs::create_dir_all(&mine).unwrap();
        assert!(plan_into(&main, "polite-marmot", Some("main"), Some(&mine), None).is_err());
        let _ = std::fs::remove_dir_all(mine.parent().unwrap());
    }

    /// A folder deep enough to break things is sent where it is shortest.
    ///
    /// Windows stops at 260 and what lands inside this folder counts too, so
    /// the failure is not ours to report: git simply cannot make it. Measured
    /// on this machine, a folder 225 characters long already could not be made
    #[test]
    fn a_path_too_long_to_hold_a_project_goes_somewhere_shorter() {
        let main = repo("long");
        let deep = "feature/".repeat(24) + "end";
        let at = folder_for(&main, &deep);
        assert!(at.starts_with(away_from_home()), "長すぎるのに逃がしていない: {at:?}");
        // The short one is left where branches belong
        assert!(folder_for(&main, "polite-marmot").starts_with(branches_root()));
    }

    /// The place itself is the person's own folder, unless it cannot be.
    #[test]
    fn branches_live_in_the_persons_own_folder() {
        let root = real_branches_root();
        match home_dir().filter(|h| !synced(h) && writable(h)) {
            Some(home) => {
                assert!(root.starts_with(&home), "自分のフォルダの下に無い: {root:?}");
                // `Path::ends_with` matches whole path parts, not the end
                // of the text, so one spelling answers on every system
                assert!(root.ends_with("SHIKISHA-TERM/branches"), "{root:?}");
            }
            // No home to speak of: ours, per machine
            None => assert!(root.starts_with(away_from_home()), "{root:?}"),
        }
    }

    #[test]
    fn what_will_run_is_one_line_and_one_source() {
        let plan = Plan {
            main: PathBuf::from("D:/work/myproject"),
            branch: "feature/login".into(),
            folder: PathBuf::from("D:/work/myproject.worktrees/feature/login"),
            base: "origin/main".into(),
            fresh: true,
            host: None,
            origin: String::new(),
            env: None,
        };
        assert_eq!(
            plan.argv(),
            ["git", "-C", "D:/work/myproject", "worktree", "add", "-b", "feature/login",
             "D:/work/myproject.worktrees/feature/login", "origin/main"]
        );
        assert_eq!(plan.line(), plan.argv().join(" "), "見せる行と走る行が同じ");
        // A branch that already exists is checked out rather than made, and
        // then there is nothing for it to grow from
        let old = Plan { fresh: false, ..plan };
        assert_eq!(
            old.argv(),
            ["git", "-C", "D:/work/myproject", "worktree", "add",
             "D:/work/myproject.worktrees/feature/login", "feature/login"]
        );
    }

    #[test]
    fn a_name_git_would_refuse_is_refused_here_first() {
        for bad in ["", " ", "/leading", "trailing/", "two//slashes", "up..down",
                    "back\\slash", "with space", "star*", "colon:here", "x.lock"] {
            assert!(!name_is_usable(bad.trim()) || bad.trim().is_empty(), "通してはいけない: {bad:?}");
        }
        for good in ["main", "feature/login", "fix/crash-on-open", "work-2", "release/1.2.3"] {
            assert!(name_is_usable(good), "普通の名前が通らない: {good:?}");
        }
    }

    /// Work names itself once it is under way, and the folder does not follow.
    ///
    /// Against a real repository, because every interesting part of this is
    /// git's: that the note about where the branch grew from travels with the
    /// new name, and that the following of a remote branch does not.
    #[test]
    fn a_branch_can_be_called_something_else_later() {
        let Some(main) = real_repo("rename") else { return };
        let plan = plan(&main, "mighty-gannet", Some("main")).expect("計画できる");
        create(&plan).expect("作れる");
        let folder = plan.folder.clone();
        git(&main, &["config", &format!("branch.{}.shikishaBase", "mighty-gannet"), "main"]);

        // Read before it runs, and it is the line that runs
        let r = rename_plan(&folder, " fix/crash ").expect("改名を計画できる");
        assert_eq!(r.from, "mighty-gannet");
        assert_eq!(r.to, "fix/crash", "前後の空白は落とす");
        assert!(r.line().contains("branch -m mighty-gannet fix/crash"), "{}", r.line());
        rename(&r).expect("改名できる");

        assert_eq!(crate::repo::branch_of(&folder).as_deref(), Some("fix/crash"));
        // Where it grew from is a fact about the branch, so it comes along
        let note = std::process::Command::new("git")
            .arg("-C").arg(&folder)
            .args(["config", "--get", "branch.fix/crash.shikishaBase"])
            .output().expect("git が動く");
        assert_eq!(String::from_utf8_lossy(&note.stdout).trim(), "main", "生まれの記録が消えた");
        // The folder stays put: it was named on the first day and nothing moves
        assert!(folder.exists(), "フォルダが動いてしまった");

        // A name git would refuse never reaches git
        assert!(rename_plan(&folder, "two words").is_err());
        assert!(rename_plan(&folder, "").is_err());
        // The project's own folder is not a branch cut from it
        assert!(rename_plan(&main, "whatever").is_err(), "本体の枝を改名できてしまう");
    }

    /// A repository git itself made, or nothing. Skipped rather than failed
    /// where git is not installed: this is the only test here that needs it
    fn real_repo(name: &str) -> Option<PathBuf> {
        let at = scratch(&format!("rn-{name}"));
        let _ = std::fs::remove_dir_all(&at);
        let main = at.join(format!("proj-{name}"));
        let _ = std::fs::remove_dir_all(branches_root().join(format!("proj-{name}")));
        std::fs::create_dir_all(&main).ok()?;
        git(&main, &["init", "-q", "-b", "main", "."]);
        git(&main, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q",
                     "--allow-empty", "-m", "one"]);
        crate::repo::branch_of(&main).is_some().then_some(main)
    }

    fn git(at: &Path, args: &[&str]) {
        let mut run = std::process::Command::new("git");
        run.arg("-C").arg(at).args(args);
        let _ = crate::detach_console(&mut run).output();
    }

    /// A name for work that has none yet is one git will take, and one a
    /// person can tell from the next one.
    ///
    /// The second half is what a number could not do, and is why the words are
    /// here: twenty draws landing on one word twice would be a list nobody can
    /// read. Checked rather than assumed, because a word list can shrink
    #[test]
    fn work_with_no_name_is_offered_two_words() {
        let main = repo("suggest");
        let drawn: std::collections::HashSet<String> =
            (0..20).map(|_| suggest(&main)).collect();
        assert!(drawn.len() > 15, "20回引いて{}種類しか出ない", drawn.len());
        for name in &drawn {
            assert!(name_is_usable(name), "git が受け取らない名前: {name:?}");
            assert_eq!(name.matches('-').count(), 1, "2語でつながっていない: {name:?}");
            assert!(!name.starts_with("work-"), "数字の名前に落ちている: {name:?}");
        }
    }

    /// The whole of it, against a real repository.
    ///
    /// Everything else here is arithmetic on paths and strings; this is the one
    /// that proves the folder comes out on its own branch, belonging to the
    /// same project, with the note about where it came from actually written.
    /// One folder per AI: the same name, each with its AI on the end, and a
    /// plan for every one before any is made.
    #[test]
    fn a_name_fans_out_into_one_branch_per_ai() {
        let main = repo("fan");
        let ais = vec!["claude".to_string(), "codex".to_string(), " gemini ".to_string()];
        let out = fan(&main, "kanban", Some("main"), &ais);
        let names: Vec<String> = out.iter().map(|(_, p)| p.as_ref().unwrap().branch.clone()).collect();
        assert_eq!(names, ["kanban-claude", "kanban-codex", "kanban-gemini"]);
        assert_eq!(out[2].0, "gemini", "AI の名前は整えて持つ");
        // Each gets its own folder, and every line is the one that will run
        let folders: std::collections::HashSet<_> =
            out.iter().map(|(_, p)| p.as_ref().unwrap().folder.clone()).collect();
        assert_eq!(folders.len(), 3);
        assert!(out.iter().all(|(_, p)| p.as_ref().unwrap().line().contains("worktree add")));
        // A name git would refuse is refused per branch, and the others still plan
        let bad = fan(&main, "kan ban", Some("main"), &ais);
        assert!(bad.iter().all(|(_, p)| p.is_err()), "空白入りの名前が通った");
    }

    #[test]
    fn a_branch_really_gets_its_own_folder() {
        let main = scratch("real").join("proj-real");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(&main).args(args);
            let out = crate::detach_console(&mut run).output().expect("git が要る");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(main.join("readme.md"), "hi\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);

        let cut = plan(&main, "feature/login", None).unwrap();
        assert!(cut.fresh, "まだ無い枝");
        assert_eq!(cut.base, "main", "remote が無ければ今いる所から、その名前で");
        create(&cut).unwrap();

        let made = &cut.folder;
        assert!(made.join("readme.md").exists(), "中身が入っている");
        assert_eq!(crate::repo::branch_of(made).as_deref(), Some("feature/login"));
        assert_eq!(crate::repo::family_of(made), crate::repo::family_of(&main), "同じ家族");
        assert!(crate::repo::is_linked(made), "本体から切った枝である");
        // Both asked the same way. Comparing against the path this test wrote
        // would be comparing an answer with a spelling, and a machine whose
        // temporary folder is handed out short (`RUNNER~1`) has two of those
        assert_eq!(
            crate::repo::main_checkout(made),
            crate::repo::main_checkout(&main),
            "枝から本体に戻れていない"
        );

        // Where it grew from, written into the repository itself
        let mut ask = std::process::Command::new("git");
        ask.arg("-C").arg(made).args(["config", "--get", "branch.feature/login.shikishaBase"]);
        let out = crate::detach_console(&mut ask).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "main");

        // Asking for the same branch twice does not quietly make a second one
        assert!(create(&cut).is_err(), "同じ場所に二度作らない");
        // Nor does it get as far as the button: a folder already standing there
        // is said while the name is still being typed
        assert!(plan(&main, "feature/login", None).is_err(), "押すまで分からない");
        // With the folder gone, the branch that now exists is checked out
        // rather than made again
        crate::worktree::discard(made).expect("片付く");
        let again = plan(&main, "feature/login", None).unwrap();
        assert!(!again.fresh, "既にある枝は作り直さない");

        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    /// What a fresh folder is missing, and getting it there.
    #[test]
    fn what_git_does_not_carry_can_be_brought_along() {
        let main = scratch("carry").join("proj-carry");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(&main).args(args);
            let out = crate::detach_console(&mut run).output().expect("git が要る");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(main.join(".gitignore"), "node_modules/\n.env\nbuild/\n").unwrap();
        std::fs::create_dir_all(main.join("node_modules").join("left-pad")).unwrap();
        std::fs::write(main.join("node_modules").join("left-pad").join("index.js"), "x").unwrap();
        std::fs::write(main.join(".env"), "TOKEN=live\n").unwrap();
        std::fs::write(main.join("readme.md"), "hi\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);

        let offered = carryables(&main);
        let named = |n: &str| offered.iter().find(|c| c.name == n);
        // Only what git was told to ignore, and only what is really there
        assert!(named("readme.md").is_none(), "追跡されているものは出さない");
        assert!(named(".gitignore").is_none());
        assert!(named("build").is_none(), "無いものは出さない");
        let modules = named("node_modules").expect("node_modules が出ていない");
        assert!(modules.folder && modules.on, "重いものは既定で持って行く");
        let env = named(".env").expect(".env が出ていない");
        assert!(!env.folder && !env.on, "生きた鍵は既定では持って行かない");

        // Bringing them: a folder is linked, a file is copied
        let cut = plan(&main, "feature/login", None).unwrap();
        create(&cut).unwrap();
        let missed = carry_into(&cut, &["node_modules".to_string(), ".env".to_string()]);
        assert!(missed.is_empty(), "持って行けなかったもの: {missed:?}");
        assert!(
            cut.folder.join("node_modules").join("left-pad").join("index.js").exists(),
            "リンクの向こうが見えていない"
        );
        assert_eq!(std::fs::read_to_string(cut.folder.join(".env")).unwrap(), "TOKEN=live\n");
        // The copy is a copy: editing it in the branch leaves the original be
        std::fs::write(cut.folder.join(".env"), "TOKEN=other\n").unwrap();
        assert_eq!(std::fs::read_to_string(main.join(".env")).unwrap(), "TOKEN=live\n");

        // Nothing is reached outside the folder it came from
        assert!(carry_into(&cut, &["../secrets".to_string()]).is_empty());
        assert!(!cut.folder.join("..").join("secrets").exists());

        // The junction has to go before the folder does, or removing the tree
        // would walk into it and take the original's contents with it
        let mut unhook = std::process::Command::new("cmd");
        unhook.args(["/c", "rmdir"]).arg(cut.folder.join("node_modules"));
        let _ = crate::detach_console(&mut unhook).status();
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    /// Throwing a branch's folder away, and refusing to.
    #[test]
    fn a_folder_with_work_in_it_is_not_thrown_away() {
        let main = scratch("discard").join("proj-discard");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(&main).args(args);
            let out = crate::detach_console(&mut run).output().expect("git が要る");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(main.join("readme.md"), "hi\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);

        let cut = plan(&main, "feature/gone", None).unwrap();
        create(&cut).unwrap();

        // The project's own folder is not a branch and is never thrown away,
        // however it is asked
        assert!(discard(&main).is_err(), "本体を消してはいけない");
        assert!(main.join("readme.md").exists());

        // Work that only exists here is the one thing this must not take
        std::fs::write(cut.folder.join("notes.md"), "half an idea\n").unwrap();
        assert!(discard(&cut.folder).is_err(), "未コミットがあるのに消した");
        assert!(cut.folder.join("notes.md").exists(), "消えてしまった");

        // Once there is nothing to lose, it goes -- and git stops listing it
        std::fs::remove_file(cut.folder.join("notes.md")).unwrap();
        discard(&cut.folder).unwrap();
        assert!(!cut.folder.exists(), "フォルダが残っている");
        let mut ask = std::process::Command::new("git");
        ask.arg("-C").arg(&main).args(["worktree", "list", "--porcelain"]);
        let listed = crate::detach_console(&mut ask).output().unwrap();
        assert!(
            !String::from_utf8_lossy(&listed.stdout).contains("feature/gone"),
            "git がまだ持っている"
        );
        // Asking again is not an error: it is already how it was asked to be
        discard(&cut.folder).unwrap();

        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    #[test]
    fn the_branches_it_could_grow_from_are_the_ones_there_are() {
        let main = repo("bases");
        let git = main.join(".git");
        std::fs::create_dir_all(git.join("refs/heads/feature")).unwrap();
        std::fs::write(git.join("refs/heads/main"), "0
").unwrap();
        std::fs::write(git.join("refs/heads/feature/login"), "0
").unwrap();
        std::fs::create_dir_all(git.join("refs/remotes/origin")).unwrap();
        std::fs::write(git.join("refs/remotes/origin/main"), "0
").unwrap();
        std::fs::write(git.join("refs/remotes/origin/HEAD"), "ref: refs/remotes/origin/main
").unwrap();
        std::fs::write(git.join("packed-refs"), "0000 refs/heads/old-thing
").unwrap();

        let found = bases(&main);
        assert_eq!(found.first().map(String::as_str), Some("origin/main"), "既定が先頭: {found:?}");
        for want in ["origin/main", "main", "feature/login", "old-thing"] {
            assert!(found.iter().any(|b| b == want), "{want} が無い: {found:?}");
        }
        // The pointer at another branch is not a branch anyone starts from
        assert!(!found.iter().any(|b| b.ends_with("HEAD")), "HEAD を出している: {found:?}");
        // Said once each
        let mut sorted = found.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), found.len(), "重複がある: {found:?}");
    }

    #[test]
    fn a_new_branch_grows_from_what_the_remote_calls_its_default() {
        let main = repo("base");
        let git = main.join(".git");
        // Nothing known yet: it grows from where the checkout is standing,
        // said as the branch's own name
        assert_eq!(default_base(&main), "main");
        // A remote branch, found whether it is a file or packed away
        std::fs::create_dir_all(git.join("refs/remotes/origin")).unwrap();
        std::fs::write(git.join("refs/remotes/origin/main"), "0\n").unwrap();
        assert_eq!(default_base(&main), "origin/main");
        std::fs::write(git.join("packed-refs"), "0000 refs/remotes/origin/trunk\n").unwrap();
        // What origin itself says beats any guess
        std::fs::write(git.join("refs/remotes/origin/HEAD"), "ref: refs/remotes/origin/trunk\n").unwrap();
        assert_eq!(default_base(&main), "origin/trunk");
    }
}

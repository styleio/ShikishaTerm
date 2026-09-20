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

    /// The folder on this machine the new one is written down beside.
    ///
    /// Here, the checkout it is cut from, whose tabs it takes. On another
    /// machine `main` is a path over there, which means nothing to this one,
    /// so it is the folder here the ask came from
    pub fn like<'a>(&'a self, asked_from: &'a Path) -> &'a Path {
        match self.host {
            Some(_) => asked_from,
            None => &self.main,
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
        let mut steps = self.cutting();
        steps.extend(self.getting_ready());
        steps
    }

    /// The commands that make the folder, before anything is run inside it
    fn cutting(&self) -> Vec<Vec<String>> {
        let fresh_machine = self.host.as_ref().is_some_and(|h| h.is_made());
        if !fresh_machine {
            return vec![self.argv()];
        }
        let at = self.folder.display().to_string();
        vec![
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
                // Following nothing, for the same reason as a worktree's
                // branch (`argv`): grown from the clone's own branch, a
                // setting of "always" would have it follow that one
                "--no-track".into(),
                "-c".into(),
                self.branch.clone(),
            ],
        ]
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
            // Following nothing. Grown from a remote branch, git would have it
            // follow that one (`origin/main`), and a branch that follows a
            // branch of another name is one a plain push refuses to send. With
            // nothing followed, the first push sets one up under its own name
            v.push("--no-track".into());
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

/// A branch already open in a folder, which git will not open in a second one.
///
/// Told apart from the other reasons a plan fails, because this one has two
/// answers a person can give in a word: use that folder, or take another name
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InUse {
    pub branch: String,
    pub folder: PathBuf,
}

impl std::fmt::Display for InUse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::i18n::tp(
            "err.worktree.in_use",
            &[("branch", &self.branch), ("path", &self.folder.display().to_string())],
        ))
    }
}

impl std::error::Error for InUse {}

/// The first name after `name` -- `name-2`, `name-3` -- that is a new branch
/// with a free folder, for when `name` is taken
pub fn next_free(main: &Path, name: &str) -> String {
    (2..100)
        .map(|n| format!("{name}-{n}"))
        .find(|candidate| is_free(main, candidate))
        .unwrap_or_else(|| format!("{name}-{}", crate::random_hex(3)))
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
        bail!(InUse { branch: branch.clone(), folder: open });
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
    make(plan, &|_| {}, &|| false)
}

/// How far a folder being made has got, in the order it gets there
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Looking at where it goes
    Preparing = 0,
    /// Git cutting the folder, or a fresh machine fetching the project
    Creating = 1,
    /// What the project says it needs, and what comes along from the checkout
    SettingUp = 2,
    /// Asked to stop, and taking back what was made so far
    Stopping = 3,
}

impl Stage {
    /// Its name on the page
    pub fn key(self) -> &'static str {
        match self {
            Stage::Preparing => "preparing",
            Stage::Creating => "creating",
            Stage::SettingUp => "setting_up",
            Stage::Stopping => "stopping",
        }
    }
    fn of(n: u8) -> Stage {
        match n {
            1 => Stage::Creating,
            2 => Stage::SettingUp,
            3 => Stage::Stopping,
            _ => Stage::Preparing,
        }
    }
}

/// Why a making ended without a folder, when nobody failed: it was stopped
#[derive(Debug)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped")
    }
}

impl std::error::Error for Stopped {}

/// The same, told how far it has got and asked between commands whether to
/// go on. Stopped or failed once anything was made, what was made is taken
/// back: a folder half made and written down nowhere is one nobody will find
/// again, and it would stand in the way of trying the same name once more
fn make(plan: &Plan, at_stage: &dyn Fn(Stage), stop: &dyn Fn() -> bool) -> Result<()> {
    at_stage(Stage::Preparing);
    // A folder on another machine is not ours to look at, and git over there
    // refuses in its own words if something is already standing in the way
    if plan.host.is_none() {
        free_to_make(&plan.folder)?;
        if let Some(parent) = plan.folder.parent() {
            std::fs::create_dir_all(parent)?;
        }
    }
    if stop() {
        return Err(Stopped.into());
    }
    at_stage(Stage::Creating);
    let steps = plan.cutting().into_iter().map(|a| (Stage::Creating, a))
        .chain(plan.getting_ready().into_iter().map(|a| (Stage::SettingUp, a)));
    for (stage, argv) in steps {
        at_stage(stage);
        let ran = run_for(plan, &argv);
        let stopped = stop();
        if ran.is_err() || stopped {
            if stopped {
                at_stage(Stage::Stopping);
            }
            take_back(plan);
            return match ran {
                Err(e) if !stopped => Err(e),
                _ => Err(Stopped.into()),
            };
        }
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

/// Takes back a folder that was being made. Only what this making made: the
/// folder was not there before it began (`free_to_make`), and a new branch is
/// deleted only while it still points where it grew from, so nothing anybody
/// committed is ever lost to it
fn take_back(plan: &Plan) {
    // A made machine is thrown away whole; nothing in it outlives the making
    if plan.host.as_ref().is_some_and(|h| h.is_made()) {
        return;
    }
    let folder = plan.folder.display().to_string();
    let main = plan.main.display().to_string();
    // Whatever was already linked in when the making stopped is unhooked
    // before git or the filesystem is asked to take the folder away
    if plan.host.is_none() {
        unhook_links(&plan.folder);
    }
    let _ = run_for(
        plan,
        &["git".into(), "-C".into(), main.clone(), "-c".into(), LONG_PATHS.into(), "worktree".into(), "remove".into(), "--force".into(), folder],
    );
    if plan.host.is_some() {
        return;
    }
    // Git may have stopped before it wrote the folder down anywhere
    if plan.folder.exists() {
        let _ = std::fs::remove_dir_all(&plan.folder);
    }
    let _ = run(&["git".into(), "-C".into(), main.clone(), "worktree".into(), "prune".into()]);
    if plan.fresh {
        let tip = |name: &str| {
            let mut asking = std::process::Command::new("git");
            asking.arg("-C").arg(&plan.main).args(["rev-parse", "--verify", "--quiet", &format!("{name}^{{commit}}")]);
            crate::detach_console(&mut asking)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        };
        if let (Some(branch), Some(base)) = (tip(&format!("refs/heads/{}", plan.branch)), tip(&plan.base))
            && branch == base
        {
            let _ = run(&["git".into(), "-C".into(), main, "branch".into(), "-D".into(), plan.branch.clone()]);
        }
    }
}

/// A folder being made on a thread of its own.
///
/// Making one takes seconds on a large project and minutes on a machine that
/// fetches it first, and the window cannot wait that long. The page shows how
/// far it has got from `stage`, a press on its ✕ asks it to `stop`, and the
/// loop takes the `outcome` once there is one
pub struct Making {
    pub plan: Plan,
    stage: std::sync::Arc<std::sync::atomic::AtomicU8>,
    stopping: std::sync::Arc<std::sync::atomic::AtomicBool>,
    outcome: std::sync::Arc<std::sync::Mutex<Option<Result<Brought, String>>>>,
}

impl Making {
    /// Starts making `plan`, bringing `carry` along once it is there
    pub fn start(plan: Plan, carry: Vec<Carry>) -> Making {
        use std::sync::atomic::Ordering;
        let making = Making {
            plan: plan.clone(),
            stage: Default::default(),
            stopping: Default::default(),
            outcome: Default::default(),
        };
        let (stage, stopping, outcome) = (making.stage.clone(), making.stopping.clone(), making.outcome.clone());
        std::thread::spawn(move || {
            let at_stage = |s: Stage| {
                // Once stopping, it says so until it has stopped
                if stage.load(Ordering::Relaxed) != Stage::Stopping as u8 {
                    stage.store(s as u8, Ordering::Relaxed);
                }
            };
            let stop = || stopping.load(Ordering::Relaxed);
            let made = make(&plan, &at_stage, &stop).map(|()| {
                at_stage(Stage::SettingUp);
                carry_into(&plan, &carry)
            });
            // Stopped after the last command: taken back all the same, since
            // nobody is waiting for the folder any more
            let made = match made {
                Ok(_) if stop() => {
                    take_back(&plan);
                    Err(Stopped.into())
                }
                other => other,
            };
            let said = made.map_err(|e: anyhow::Error| match e.downcast_ref::<Stopped>() {
                Some(_) => String::new(),
                None => format!("{e:#}"),
            });
            *outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(said);
        });
        making
    }

    /// The same thread, for a folder that is already there: only what comes
    /// into it is done.
    ///
    /// What this is for is the answer to a question the first run asked --
    /// copying in a folder that could not be linked. Copying a folder off a
    /// share takes as long as making the branch did, so it happens here rather
    /// than in the middle of a frame, and it is read back the same way
    pub fn carrying(plan: Plan, carry: Vec<Carry>) -> Making {
        use std::sync::atomic::Ordering;
        let making = Making {
            plan: plan.clone(),
            stage: Default::default(),
            stopping: Default::default(),
            outcome: Default::default(),
        };
        making.stage.store(Stage::SettingUp as u8, Ordering::Relaxed);
        let outcome = making.outcome.clone();
        std::thread::spawn(move || {
            let said = carry_into(&plan, &carry);
            *outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(Ok(said));
        });
        making
    }

    pub fn stage(&self) -> Stage {
        Stage::of(self.stage.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Asks it to stop at the next command, and to take back what it made
    pub fn stop(&self) {
        self.stopping.store(true, std::sync::atomic::Ordering::Relaxed);
        self.stage.store(Stage::Stopping as u8, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn stopped(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// What became of it, once: what came along, or why not. An empty reason
    /// is a making that was stopped
    pub fn outcome(&self) -> Option<Result<Brought, String>> {
        self.outcome.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

/// What a new folder will not have, and cannot get from git.
///
/// A branch's folder arrives with everything git tracks and nothing it does
/// not: no `.env`, no `node_modules`, no build cache. The first thing anyone
/// does in it is fail to build, which makes "start another branch" a promise
/// the app does not keep. So the things git ignores, that are actually there,
/// are offered to come along -- each the way the project said for the line
/// that ignores it -- together with anything the project brings from elsewhere.
///
/// Asked of git rather than guessed from a list of names, because what counts
/// as ignored is the repository's own answer and it is written down already.
pub fn carryables(main: &Path, rules: &[crate::config::BringRule]) -> Vec<Carry> {
    let found = ignored(main);
    let mut out: Vec<Carry> = found
        .iter()
        .map(|i| {
            let rule = rules.iter().find(|r| r.is_for(&i.source, &i.pattern));
            Carry {
                name: i.path.trim_end_matches('/').to_string(),
                folder: i.folder,
                how: rule
                    .map(|r| r.how.clone())
                    .filter(|h| HOWS.contains(&h.as_str()))
                    .unwrap_or_else(|| default_how(&found, &i.source, &i.pattern).to_string()),
                from: None,
                replace: rule.map(|r| r.replace.clone()).unwrap_or_default(),
                line: Some(CarryLineKey { source: i.source.clone(), pattern: i.pattern.clone() }),
            }
        })
        .collect();
    // From anywhere else, where the project named a place to put it
    for r in rules.iter().filter(|r| r.pattern.is_none()) {
        let (Some(from), Some(to)) = (r.from.as_deref(), r.to.as_deref()) else { continue };
        let (from, to) = (from.trim(), clean_inside(to));
        if from.is_empty() || to.is_empty() || out.iter().any(|c| c.name == to) {
            continue;
        }
        out.push(Carry {
            folder: Path::new(from).is_dir(),
            name: to,
            how: match HOWS.contains(&r.how.as_str()) {
                true => r.how.clone(),
                false => "copy".into(),
            },
            from: Some(from.to_string()),
            replace: r.replace.clone(),
            line: None,
        });
    }
    out
}

/// The lines of the ignore files, each once, with how everything it matches
/// comes along -- what the worktree dialog offers to choose by line.
///
/// Read off the things offered, before anything was changed for one folder,
/// so a line says what the project says: its rule, or the answer when there is
/// none. In the order its first match is offered
pub fn carry_lines(offered: &[Carry]) -> Vec<CarryLine> {
    let mut lines: Vec<CarryLine> = Vec::new();
    for c in offered {
        let Some(key) = &c.line else { continue };
        match lines.iter_mut().find(|l| l.source == key.source && l.pattern == key.pattern) {
            Some(l) => {
                l.count += 1;
                l.folders &= c.folder;
            }
            None => lines.push(CarryLine {
                source: key.source.clone(),
                pattern: key.pattern.clone(),
                how: c.how.clone(),
                count: 1,
                folders: c.folder,
            }),
        }
    }
    lines
}

/// Which line of which ignore file makes git ignore something
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CarryLineKey {
    /// The ignore file, as git names it (`.gitignore` is the project's own)
    pub source: String,
    /// The line as written
    pub pattern: String,
}

/// One line of an ignore file, as the worktree dialog shows it
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CarryLine {
    pub source: String,
    pub pattern: String,
    /// How what it matches comes along, as the project says
    pub how: String,
    /// How many of the things offered it matches
    pub count: usize,
    /// Whether every one of them is a folder (a folder has no text to replace in)
    pub folders: bool,
}

/// The ways something can reach a new folder, in the order they are offered
pub const HOWS: [&str; 4] = ["copy", "replace", "link", "skip"];

/// One thing a new folder can be given.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Carry {
    /// Where it goes inside the new folder, `/` between the parts
    pub name: String,
    /// Whether it is a folder
    pub folder: bool,
    /// `copy`, `replace`, `link` or `skip`
    pub how: String,
    /// Where it comes from, when that is not the same place in the checkout
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// For `replace`: what is written differently in the copy
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub replace: Vec<crate::config::Replace>,
    /// The ignore line that decides it; absent for what comes from elsewhere
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<CarryLineKey>,
}

/// One thing git ignores in a checkout, and the line that makes it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Ignored {
    /// Relative to the checkout, `/` between parts; a folder ends in `/`
    pub path: String,
    pub folder: bool,
    /// The file the deciding line is in, as git names it: `.gitignore`,
    /// `web/.gitignore`, `.git/info/exclude`, or a path outside the repository
    pub source: String,
    /// Its line number there, from 1
    pub line: usize,
    /// The line as written
    pub pattern: String,
    /// Whether the name is the sort that holds a live secret
    pub secret: bool,
}

/// How much of this is listed. A repository that ignores more than this many
/// separate things has a line matching thousands of generated files, and the
/// first thousands say what that line is
const IGNORED_MOST: usize = 2000;

/// Everything git ignores that is actually in the checkout, each with the line
/// that decides it. A folder git ignores whole is one entry, not its contents.
pub fn ignored(main: &Path) -> Vec<Ignored> {
    let listed = git_z(main, &["ls-files", "-z", "--others", "--ignored", "--exclude-standard", "--directory"], "");
    let Some(listed) = listed else { return Vec::new() };
    let paths: Vec<&str> = listed.split('\0').filter(|p| !p.is_empty()).take(IGNORED_MOST).collect();
    if paths.is_empty() {
        return Vec::new();
    }
    // Which line decides each: <source> NUL <line> NUL <pattern> NUL <path> NUL
    //
    // Asked without the slash that marks a folder. With it, `www/tmp/` is
    // matched by `www/tmp/*` -- the star matching nothing after the slash --
    // so the folder was offered beside the very things inside it that the
    // line ignores, although git itself does not ignore the folder. Without
    // it git still knows a folder from the disk, so `node_modules/` goes on
    // matching `node_modules`
    let asked: Vec<&str> = paths.iter().map(|p| p.trim_end_matches('/')).collect();
    let Some(why) = git_z(main, &["check-ignore", "-z", "-v", "--stdin"], &asked.join("\0")) else {
        return Vec::new();
    };
    let parts: Vec<&str> = why.split('\0').collect();
    parts
        .chunks(4)
        .filter(|c| c.len() == 4 && !c[3].is_empty() && !c[2].starts_with('!'))
        .map(|c| {
            // Given back the way it was listed: a folder ends in /
            let path = paths
                .iter()
                .find(|p| p.trim_end_matches('/') == c[3])
                .map(|p| p.to_string())
                .unwrap_or_else(|| c[3].to_string());
            let leaf = path.trim_end_matches('/').rsplit('/').next().unwrap_or_default().to_string();
            Ignored {
                folder: path.ends_with('/') || main.join(&path).is_dir(),
                secret: looks_secret(&leaf),
                source: c[0].replace('\\', "/"),
                line: c[1].parse().unwrap_or(0),
                pattern: c[2].to_string(),
                path,
            }
        })
        .collect()
}

/// What a line of an ignore file does when the project has not said: nothing
/// if it matches anything that holds a secret or matches nothing at all, a
/// link if everything it matches is a folder, and a copy otherwise.
///
/// The same answer for the settings screen, which shows it beside the line,
/// and for the dialog that makes a folder, so the two never disagree
pub fn default_how(found: &[Ignored], source: &str, pattern: &str) -> &'static str {
    let matched: Vec<&Ignored> = found.iter().filter(|i| i.source == source && i.pattern == pattern).collect();
    match () {
        _ if matched.is_empty() || matched.iter().any(|i| i.secret) => "skip",
        _ if matched.iter().all(|i| i.folder) => "link",
        _ => "copy",
    }
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

/// A git whose output is split by NULs, fed `input`. None when git did not
/// answer: exit 1 from `check-ignore` means "none of these", which is an answer
fn git_z(main: &Path, args: &[&str], input: &str) -> Option<String> {
    // Asked twice at most: git can refuse for a moment while the index is still
    // locked by the commit before it, and "nothing is ignored" would then
    // quietly offer an empty list instead of the truth
    for attempt in 0..2 {
        let mut asking = std::process::Command::new("git");
        asking
            .arg("-C")
            .arg(main)
            .args(args)
            .stdin(if input.is_empty() { std::process::Stdio::null() } else { std::process::Stdio::piped() })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let Ok(mut child) = crate::detach_console(&mut asking).spawn() else { return None };
        // Written from a thread of its own: git answers while it reads, and a
        // long list written here while its answer fills the other pipe would
        // leave both sides waiting on each other
        let feeding = child.stdin.take().map(|mut w| {
            let input = input.to_string();
            std::thread::spawn(move || {
                use std::io::Write as _;
                let _ = w.write_all(input.as_bytes());
            })
        });
        let Ok(out) = child.wait_with_output() else { return None };
        if let Some(f) = feeding {
            let _ = f.join();
        }
        if matches!(out.status.code(), Some(0) | Some(1)) {
            return Some(String::from_utf8_lossy(&out.stdout).to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(60 * (attempt + 1)));
    }
    None
}

/// A place inside the new folder, as written: `/` between parts, no leading
/// slash, and nothing that climbs out. Empty when there is nothing left
fn clean_inside(to: &str) -> String {
    let parts: Vec<&str> = to
        .split(['/', '\\'])
        .map(str::trim)
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.iter().any(|p| *p == ".." || p.contains(':')) {
        return String::new();
    }
    parts.join("/")
}

/// What happened to what was asked to come along.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Brought {
    /// What could not be put there at all
    pub missed: Vec<String>,
    /// Files asked to be linked that were copied instead, because this machine
    /// does not let a file be linked without rights nobody gave
    pub copied: Vec<String>,
    /// Folders asked for as a second name that could not have one, and are
    /// waiting to be told what to do about it.
    ///
    /// Neither brought nor lost. A folder on another machine's share cannot
    /// have the kind of second name this needs no rights for, and copying it
    /// instead is a different thing from sharing it -- the branch would get
    /// its own copy of something that may be very large, and writing in it
    /// would no longer reach the project's own. That is the person's call, so
    /// it is carried back to be asked rather than decided here
    pub unlinked: Vec<String>,
    /// Replacements that could not be made, each with why: the file is there,
    /// copied as it was
    pub unreplaced: Vec<String>,
}

/// Gives a new folder what the dialog left on: copied, linked, or left out.
///
/// Every failure is left as a warning rather than undoing the branch: the
/// checkout is made and usable, and someone who wanted three of these and got
/// two would rather be told which one is missing than have the whole thing
/// taken away again.
pub fn carry_into(plan: &Plan, items: &[Carry]) -> Brought {
    let mut said = Brought::default();
    let wanted: Vec<&Carry> = items.iter().filter(|c| c.how != "skip").collect();
    // Files are put between folders on this machine. A folder on another one
    // is a path there, and writing to it here would make a stray folder on
    // this machine and call it done, so every one of them is said as not brought
    if plan.host.is_some() {
        said.missed = wanted.iter().map(|c| c.name.clone()).collect();
        return said;
    }
    for c in wanted {
        let inside = clean_inside(&c.name);
        if inside.is_empty() {
            continue;
        }
        let from = match &c.from {
            Some(f) => PathBuf::from(f),
            None => plan.main.join(&inside),
        };
        let to = plan.folder.join(&inside);
        if !from.exists() || to.exists() {
            if !from.exists() {
                said.missed.push(c.name.clone());
            }
            continue;
        }
        if let Some(parent) = to.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let done = match (c.how.as_str(), from.is_dir()) {
            // A folder that cannot be given a second name is carried back as a
            // question rather than counted as lost: see [`Brought::unlinked`]
            ("link", true) => match link_folder(&from, &to) {
                true => true,
                false => {
                    said.unlinked.push(c.name.clone());
                    continue;
                }
            },
            ("link", false) => {
                link_file(&from, &to) || {
                    let copied = std::fs::copy(&from, &to).is_ok();
                    if copied {
                        said.copied.push(c.name.clone());
                    }
                    copied
                }
            }
            (_, true) => copy_folder(&from, &to).is_ok(),
            (how, false) => {
                let copied = std::fs::copy(&from, &to).is_ok();
                if copied && how == "replace" && !c.replace.is_empty() {
                    let written = std::fs::read_to_string(&to)
                        .map_err(|e| e.to_string())
                        .and_then(|text| apply_replaces(&text, &c.replace))
                        .and_then(|(text, unmatched)| {
                            std::fs::write(&to, text).map_err(|e| e.to_string()).map(|()| unmatched)
                        });
                    match written {
                        Err(why) => said.unreplaced.push(format!("{} ({why})", c.name)),
                        Ok(unmatched) => said.unreplaced.extend(unmatched.iter().map(|find| {
                            format!("{} ({})", c.name, crate::i18n::tp("err.replace.nomatch", &[("find", find)]))
                        })),
                    }
                }
                copied
            }
        };
        if !done {
            said.missed.push(c.name.clone());
        }
    }
    said
}

/// A copy of a whole folder. What is linked inside it is copied as the thing it
/// points at would be skipped: following a link out of the folder could copy
/// something nobody meant to bring
fn copy_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let kind = std::fs::symlink_metadata(e.path())?.file_type();
        let there = to.join(e.file_name());
        if kind.is_symlink() {
            continue;
        } else if kind.is_dir() {
            copy_folder(&e.path(), &there)?;
        } else {
            std::fs::copy(e.path(), &there)?;
        }
    }
    Ok(())
}

/// A second name for one file. Windows asks for rights to make one that most
/// people running this do not have, so this is allowed to fail
fn link_file(from: &Path, to: &Path) -> bool {
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(from, to).is_ok()
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to).is_ok()
    }
}

/// A copied file's text with every replacement made, in order.
///
/// A plain `find` is replaced wherever it appears. A regular expression is
/// read with `^` and `$` at each line's start and end, so `^PORT=.*$` is one
/// line, and `with` may use its groups as `$1`. One that does not compile is
/// said, naming the one that did not
///
/// Also answers which of them found nothing to replace: a template that no
/// longer has the line a replacement was written for is copied unchanged, and
/// that is worth saying rather than finding out when the app will not start.
/// A file written with Windows line endings is read the same way -- `$` is
/// the end of the line, before the `\r`
pub fn apply_replaces(
    text: &str,
    replaces: &[crate::config::Replace],
) -> std::result::Result<(String, Vec<String>), String> {
    let mut out = text.to_string();
    let mut unmatched = Vec::new();
    for r in replaces.iter().filter(|r| !r.find.is_empty()) {
        let (found, next) = match r.regex {
            false => (out.contains(&r.find), out.replace(&r.find, &r.with)),
            true => {
                let re = regex::RegexBuilder::new(&r.find)
                    .multi_line(true)
                    .crlf(true)
                    .build()
                    .map_err(|_| crate::i18n::tp("err.replace.regex", &[("find", &r.find)]))?;
                (re.is_match(&out), re.replace_all(&out, r.with.as_str()).into_owned())
            }
        };
        if !found {
            unmatched.push(r.find.clone());
        }
        out = next;
    }
    Ok((out, unmatched))
}

/// The lines of the project's own `.gitignore`, as written. Empty when there is none
pub fn gitignore_lines(main: &Path) -> Vec<String> {
    std::fs::read_to_string(main.join(".gitignore"))
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Add a line to the project's own `.gitignore`. A line already there is not
/// added twice, and the file keeps the line endings it had
pub fn gitignore_add(main: &Path, line: &str) -> Result<()> {
    let line = line.trim();
    if line.is_empty() || line.contains('\n') {
        bail!(crate::i18n::t("err.gitignore.empty"));
    }
    let at = main.join(".gitignore");
    let text = std::fs::read_to_string(&at).unwrap_or_default();
    if text.lines().any(|l| l.trim() == line) {
        bail!(crate::i18n::tp("err.gitignore.already", &[("line", line)]));
    }
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = text.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(nl);
    }
    out.push_str(line);
    out.push_str(nl);
    std::fs::write(&at, out)?;
    Ok(())
}

/// Take line `n` (from 1) out of the project's own `.gitignore`, if it still
/// reads `line` -- a file changed since it was read is not edited blind
pub fn gitignore_remove(main: &Path, n: usize, line: &str) -> Result<()> {
    let at = main.join(".gitignore");
    let text = std::fs::read_to_string(&at)?;
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<&str> = text.lines().collect();
    if n == 0 || lines.get(n - 1).map(|l| l.trim()) != Some(line.trim()) {
        bail!(crate::i18n::t("err.gitignore.changed"));
    }
    lines.remove(n - 1);
    let mut out = lines.join(nl);
    if !out.is_empty() {
        out.push_str(nl);
    }
    std::fs::write(&at, out)?;
    Ok(())
}

/// Files git still tracks although an ignore line now matches them. Adding a
/// line does not stop git following a file it already follows, which is the
/// first thing anybody adding one runs into
pub fn tracked_but_ignored(main: &Path) -> Vec<String> {
    git_z(main, &["ls-files", "-z", "--cached", "--ignored", "--exclude-standard"], "")
        .map(|s| s.split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Stop git following these files. The files themselves stay where they are;
/// the change shows in the git column like any other, to be committed
pub fn untrack(main: &Path, paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let mut args: Vec<&str> = vec!["rm", "-r", "-q", "--cached", "--"];
    args.extend(paths.iter().map(String::as_str));
    crate::git::run(main, &args).map(|_| ())
}

/// A second name for one folder, made the way this system lets anyone make one.
///
/// On Windows that is a junction, because a symbolic link there needs rights
/// most people running this do not have. A junction, though, is a name for a
/// place on this machine and nothing else: asked for a folder on another
/// machine's share it answers "local volumes are required", and it answers
/// that however the share was reached, a mapped drive letter included. So a
/// folder it refuses is asked for again as a symbolic link, which is the one
/// kind that can point at another machine -- and where nobody turned that
/// right on, both fail and the caller copies the folder instead.
///
/// Everywhere else a symbolic link is the ordinary thing and needs nothing.
fn link_folder(from: &Path, to: &Path) -> bool {
    #[cfg(windows)]
    {
        // Every part written with a `/` is made a `\` first: cmd reads a
        // forward slash as the start of a switch, and a name like
        // `web/node_modules` would be a folder it refuses rather than a path
        let told = |p: &Path| p.display().to_string().replace('/', "\\");
        let made = |kind: &str| {
            let mut link = std::process::Command::new("cmd");
            link.args(["/c", "mklink", kind])
                .arg(told(to))
                .arg(told(from))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            crate::detach_console(&mut link)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        made("/J") || made("/D")
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(from, to).is_ok()
    }
}

/// Unhooks every link inside a folder, however deep, before anything removes
/// the folder itself.
///
/// A junction left in place is walked into by whatever deletes the tree, and
/// what it takes is the original's contents rather than the second name for
/// them. One `.env` linked in and forgotten is the whole checkout's `.env`.
///
/// What is asked is the folder, never the settings. A line set to Link when
/// the folder was made can say Copy by the time it goes, and a junction from
/// a version of this app that no longer exists answers to no line at all: the
/// only account of what is really there is the folder itself. For the same
/// reason a link is unhooked and not descended into -- walking through one is
/// the very thing this is here to stop.
pub fn unhook_links(folder: &Path) {
    let Ok(here) = std::fs::read_dir(folder) else { return };
    for e in here.flatten() {
        let at = e.path();
        let Ok(kind) = std::fs::symlink_metadata(&at).map(|m| m.file_type()) else { continue };
        if kind.is_symlink() {
            // Asked of the link rather than of what it points at, which may be
            // a folder, a file, or gone: a junction and a folder symlink go
            // with the first call, a file symlink with the second, and neither
            // call opens the other side
            if std::fs::remove_dir(&at).is_err() {
                let _ = std::fs::remove_file(&at);
            }
        } else if kind.is_dir() {
            unhook_links(&at);
        }
    }
}

/// Said to git on every removal. Without it git on Windows cannot delete a
/// path longer than 260 characters, and a build or a browser profile under
/// `target` has thousands of them. Git does not stop cleanly when it meets one:
/// it has already deleted `.git` and forgotten the worktree, and it leaves
/// everything after that file on disk
const LONG_PATHS: &str = "core.longpaths=true";

/// Gets rid of a branch's folder, once there is nothing in it to lose.
///
/// Refused while anything is uncommitted. A folder full of work that only
/// exists there is the one thing this must never take, and "are you sure" is
/// not a good enough answer when the app is the one that made the folder in
/// the first place. What it does not check is whether the branch was merged:
/// that is a judgement, and it belongs to the person.
pub fn discard(folder: &Path) -> Result<()> {
    discard_step(folder, &mut false)
}

/// One try at [`discard`]. `released` is whether git has let go of this
/// folder during this removal. What is left after that is still this
/// worktree's, and it is cleared here even though git no longer calls the
/// folder a worktree. Without that record, a folder git half-deleted looks
/// the same as a folder that was never a worktree, and it could never be
/// finished.
fn discard_step(folder: &Path, released: &mut bool) -> Result<()> {
    if !folder.exists() {
        return Ok(());
    }
    if crate::repo::is_linked(folder) {
        ready_to_discard(folder)?;
        let main = crate::repo::main_checkout(folder)
            .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.worktree.not_a_repo")))?;
        // Git's own removal, so the repository stops listing it too. Anything
        // linked into the folder is unhooked first: removing the folder with a
        // junction still in it walks through and takes what is on the other side
        unhook_links(folder);
        let removed = run(&[
            "git".into(),
            "-C".into(),
            main.display().to_string(),
            "-c".into(),
            LONG_PATHS.into(),
            "worktree".into(),
            "remove".into(),
            folder.display().to_string(),
        ]);
        // Git deletes `.git` before anything else in the folder. If it is still
        // there, git refused before it touched anything, and git's reason is the
        // answer. If it is gone, git got part of the way, and the rest is ours
        if let Err(e) = removed {
            if folder.join(".git").exists() {
                return Err(e);
            }
        }
        *released = true;
        let _ = run(&[
            "git".into(),
            "-C".into(),
            main.display().to_string(),
            "worktree".into(),
            "prune".into(),
        ]);
    }
    if !folder.exists() {
        return Ok(());
    }
    if *released {
        return clear_remains(folder);
    }
    // Not a worktree, and not one this removal saw: only an empty folder is
    // taken, so a folder that was never ours can lose nothing
    let empty = std::fs::read_dir(folder).map(|d| d.count() == 0).unwrap_or(false);
    if !empty {
        bail!(crate::i18n::t("err.worktree.not_a_branch"));
    }
    std::fs::remove_dir(folder)?;
    Ok(())
}

/// Deletes what is left in a worktree's folder once git has let go of it, and
/// then the folder.
///
/// A link is removed and never entered, for the reason given at
/// [`unhook_links`]. When something will not go, everything else is still
/// deleted, and the first thing that stayed is named: which file, and what
/// Windows said about it. That is what somebody needs in order to close the
/// program holding it.
fn clear_remains(folder: &Path) -> Result<()> {
    let mut stuck: Option<(PathBuf, std::io::Error)> = None;
    scrub(folder, &mut stuck);
    let last = match std::fs::remove_dir(folder) {
        Ok(()) => return Ok(()),
        Err(_) if !folder.exists() => return Ok(()),
        Err(e) => e,
    };
    let (at, why) = stuck.unwrap_or((folder.to_path_buf(), last));
    let shown = at.strip_prefix(folder).ok().filter(|p| !p.as_os_str().is_empty()).unwrap_or(&at);
    bail!(crate::i18n::tp(
        "err.worktree.left",
        &[("path", &shown.display().to_string()), ("why", &why.to_string())]
    ))
}

/// Empties `dir`, keeping the first thing that would not go
fn scrub(dir: &Path, stuck: &mut Option<(PathBuf, std::io::Error)>) {
    fn keep(stuck: &mut Option<(PathBuf, std::io::Error)>, at: &Path, e: std::io::Error) {
        if stuck.is_none() {
            *stuck = Some((at.to_path_buf(), e));
        }
    }
    let here = match std::fs::read_dir(dir) {
        Ok(here) => here,
        Err(e) => return keep(stuck, dir, e),
    };
    for entry in here.flatten() {
        let at = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&at) else { continue };
        let kind = meta.file_type();
        let gone = if kind.is_symlink() {
            // The same two calls as `unhook_links`: neither opens the other side
            std::fs::remove_dir(&at).or_else(|_| std::fs::remove_file(&at))
        } else if kind.is_dir() {
            scrub(&at, stuck);
            // A folder that kept something already has its reason recorded
            if stuck.is_some() && std::fs::read_dir(&at).is_ok_and(|mut d| d.next().is_some()) {
                continue;
            }
            std::fs::remove_dir(&at)
        } else {
            std::fs::remove_file(&at).or_else(|e| {
                // A read-only file is deleted all the same. It is inside a
                // folder that is being deleted as a whole
                if !meta.permissions().readonly() {
                    return Err(e);
                }
                let mut writable = meta.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                writable.set_readonly(false);
                std::fs::set_permissions(&at, writable)?;
                std::fs::remove_file(&at)
            })
        };
        if let Err(e) = gone {
            keep(stuck, &at, e);
        }
    }
}

/// How long a removal waits for the tabs that were in the folder to leave
const REMOVAL_WAIT: std::time::Duration = std::time::Duration::from_secs(15);

/// Deletes a folder once whatever is standing in it has left.
///
/// Git will not remove a folder a process is standing in, and what stands in
/// this one is the tabs that were working there. They are on their way out,
/// because taking the folder out of the settings is what ends them. So the
/// removal tries again for a while rather than failing on the first try, and
/// returns the last reason if the folder is still there when time is up.
///
/// Nothing here decides *whether* it should go: that was settled by
/// `ready_to_discard` before anything was closed, while saying no was still
/// free.
pub fn discard_waiting(folder: &Path) -> Result<()> {
    let until = std::time::Instant::now() + REMOVAL_WAIT;
    let mut released = false;
    loop {
        let tried = discard_step(folder, &mut released);
        // Gone is the only thing that counts as done: git can let go of a
        // folder while Windows still holds the empty shell of it open
        if !folder.exists() {
            return Ok(());
        }
        if std::time::Instant::now() > until {
            tried?;
            bail!(crate::i18n::tp("err.worktree.still_there", &[("path", &folder.display().to_string())]));
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

/// A worktree's folder being deleted on a thread of its own.
///
/// A big folder takes seconds to delete, and the tabs standing in it take a
/// moment to leave. The window cannot wait for either. The loop takes the
/// `outcome` once there is one, and a folder that stayed is said, not only
/// written to a log.
pub struct Removal {
    pub folder: PathBuf,
    outcome: std::sync::Arc<std::sync::Mutex<Option<Result<(), String>>>>,
}

impl Removal {
    pub fn start(folder: PathBuf) -> Removal {
        let removal = Removal { folder: folder.clone(), outcome: Default::default() };
        let outcome = removal.outcome.clone();
        std::thread::spawn(move || {
            let said = discard_waiting(&folder).map_err(|e| format!("{e:#}"));
            if let Err(why) = &said {
                crate::append_hook_log(&format!("could not remove {}: {why}", folder.display()));
            }
            *outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(said);
        });
        removal
    }

    /// What became of it, once: gone, or why it is still there
    pub fn outcome(&self) -> Option<Result<(), String>> {
        self.outcome.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
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
    // The letters a new branch is held to, for the same reasons. A rename
    // draws no name of its own -- there is a branch here already, and putting
    // a drawn name on it is not what anybody opened this to do -- so a name
    // with nothing usable left is refused, as it always was. What will run is
    // under the box while it is typed, so the tidied name is read before it
    // is pressed
    let Some(to) = tidy(&to) else {
        bail!(crate::i18n::tp("err.worktree.bad_branch", &[("name", &to)]));
    };
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
    // Every untracked file named on its own, so a link can be told apart from
    // the folder it stands in; -z because a path may contain anything
    asking
        .arg("-C")
        .arg(folder)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
    let dirty = crate::detach_console(&mut asking).output()?;
    let count = unsaved_work(&String::from_utf8_lossy(&dirty.stdout), &|p| {
        std::fs::symlink_metadata(folder.join(p)).is_ok_and(|m| m.file_type().is_symlink())
    });
    if count > 0 {
        bail!(crate::i18n::tp("err.worktree.dirty", &[("count", &count.to_string())]));
    }
    Ok(())
}

/// How many of the changes `git status -z` lists are work that exists only in
/// this folder.
///
/// An untracked link is not: it holds nothing, and removing the folder unhooks
/// it and leaves what it points at alone. It is also what carrying a folder in
/// as a link leaves behind, and an ignore line written for folders --
/// `node_modules/` -- does not match a symbolic link, so on anything but
/// Windows every such folder would be refused for good
fn unsaved_work(status_z: &str, is_link: &dyn Fn(&str) -> bool) -> usize {
    let mut fields = status_z.split('\0').filter(|f| !f.is_empty());
    let mut count = 0;
    while let Some(record) = fields.next() {
        let code = record.get(..2).unwrap_or_default();
        let path = record.get(3..).unwrap_or_default();
        // A rename or copy is followed by where it came from, as its own field
        if code.contains(['R', 'C']) {
            fields.next();
        }
        if code == "??" && is_link(path.trim_end_matches('/')) {
            continue;
        }
        count += 1;
    }
    count
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
pub(crate) fn branches_root() -> PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(chosen_branches_root).clone()
}

// A test run never writes into the person's own folder. A run that leaves
// folders in somebody's home has changed the machine it was meant to be
// checking, and two runs at once would fight over the same names -- so the
// process gets one of its own, emptied on the way in (`test_temp`: a process id
// comes round again, and what the last run left under it stands exactly where
// this run is about to cut a branch, which is refused). What the real answer
// would be is checked directly, by a test of its own
#[cfg(test)]
fn chosen_branches_root() -> PathBuf {
    crate::test_temp("branches")
}
#[cfg(not(test))]
fn chosen_branches_root() -> PathBuf {
    real_branches_root()
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

/// The same, on another machine: one folder there per AI.
pub fn fan_on(
    host: &crate::config::HostSpec,
    name: &str,
    base: Option<&str>,
    ais: &[String],
    origin: &str,
    env: Option<crate::devcontainer::Env>,
) -> Vec<(String, Result<Plan>)> {
    ais.iter()
        .map(|ai| {
            let ai = ai.trim().to_string();
            let branch = format!("{}-{ai}", name.trim());
            (ai.clone(), plan_on(host, &branch, base, None, origin, env.clone()))
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

/// How much of a typed name has to survive before it is worth keeping.
///
/// Three. Two lets `API連携` keep `API` and `図1` keep `1`, and a branch
/// called `1` says nothing at all; three keeps the first and draws a name for
/// the second
const ENOUGH: usize = 3;

/// Whether a character can be in a folder name on every machine and a branch
/// name in every git.
///
/// POSIX calls these the Portable Filename Character Set. Everything else --
/// Japanese, accents, emoji, brackets, spaces -- goes, because the name ends
/// up as a folder that other programs are handed and as a branch that is
/// pushed to a server
fn portable(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'
}

/// Names Windows keeps for devices, which no folder can be called.
///
/// With or without an ending: `NUL.txt` is the same refusal as `NUL`
fn reserved_on_windows(part: &str) -> bool {
    const DEVICES: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
        "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = part.split('.').next().unwrap_or_default().to_ascii_uppercase();
    DEVICES.contains(&stem.as_str())
}

/// One piece of a name -- what stands between two slashes -- made safe.
fn tidy_part(part: &str) -> String {
    let mut out = String::new();
    for c in part.chars().filter(|c| portable(*c)) {
        // Two dots in a row are a step up out of the folder this app chose,
        // and a name git refuses as well. The dot is in the portable set, so
        // dropping the letters we will not keep does not drop these
        if c == '.' && out.ends_with('.') {
            continue;
        }
        out.push(c);
    }
    // git takes no piece that starts or ends with a dot, and Windows drops a
    // trailing dot from a folder name without saying it did
    let mut out = out.trim_matches(['.', '-']).to_string();
    // A loose ref is written beside a file of the same name with this on the
    // end, so git keeps the ending for itself. Twice over, for `a.lock.lock`
    while let Some(shorter) = out.strip_suffix(".lock") {
        out = shorter.trim_matches(['.', '-']).to_string();
    }
    match reserved_on_windows(&out) {
        true => String::new(),
        false => out,
    }
}

/// What a name somebody typed will really be, as a branch and as a folder.
///
/// `None` when nothing worth keeping is left -- a name written entirely in
/// Japanese, or nothing typed at all -- and the caller draws one with
/// `suggest` instead. What was typed is not lost: it stays on the folder's
/// card, which is this app's own label and goes nowhere near git or the disk.
///
/// `/` survives, because a branch is allowed to carry it and this app writes
/// it out as folders (`feature/login` is `feature` with `login` inside), which
/// is what stops two branches from wanting one folder. Dropping it would turn
/// `feature/login` into `featurelogin` -- a different branch, and one nobody
/// asked for. It cannot lead anywhere else on its own: a piece can hold no
/// `\`, no `:` and no `..` by the time this is done with it
pub fn tidy(typed: &str) -> Option<String> {
    let name = typed
        .split('/')
        .map(tidy_part)
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    // Counted without the separators: `a/b/c` is three names of one letter,
    // not one name of five
    match name.chars().filter(|c| *c != '/').count() >= ENOUGH {
        true => Some(name),
        false => None,
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
    let (head, rest) = argv.split_first().expect("a command with nothing in it");
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
    // What it said about why: its errors, or else the end of what it printed
    // (a project's own setup line often writes only there), or else how it
    // ended. Never nothing, which reads as a message cut off
    let errors = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let said = match (errors.is_empty(), printed.is_empty()) {
        (false, _) => errors,
        (true, false) => printed.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"),
        (true, true) => crate::i18n::tp(
            "err.worktree.exit",
            &[("code", &out.status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into()))],
        ),
    };
    bail!(crate::i18n::tp(
        "err.worktree.failed",
        &[("said", &said), ("command", &argv.join(" "))]
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
        crate::test_temp(&format!("wt-{name}"))
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
        assert!(said.contains(&wt.display().to_string()), "it does not say which folder it is opening in: {said}");
        let said = plan(&main, "main", None).unwrap_err().to_string();
        // The checkout is named the way the planner names it: a temp folder can
        // be spelled with a short name (RUNNER~1) that the planner writes long
        let checkout = crate::repo::main_checkout(&main).unwrap();
        assert!(said.contains(&checkout.display().to_string()), "{said}");
        // A branch that exists and is open nowhere is simply checked out
        let p = plan(&main, "spare", None).expect("a free branch can be opened");
        assert!(!p.fresh);
    }

    #[test]
    fn a_branch_gets_a_folder_of_its_own_away_from_the_project() {
        let main = repo("place");
        let at = folder_for(&main, "feature/login");
        // The project's folder is left alone: nothing of ours appears beside it
        assert!(!at.starts_with(main.parent().unwrap()), "it is placed beside the main checkout: {at:?}");
        // One place holds all of them, under the project they belong to
        assert!(at.starts_with(branches_root().join("proj-place")), "{at:?}");
        assert!(at.ends_with("feature/login"), "the branch name nests as it is: {at:?}");
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
        assert!(host.is_made(), "it is not treated as a machine that gets made");

        let p = plan_on(&host, "polite-marmot", Some("origin/master"), None, "https://example.test/p.git", None)
            .expect("it can be planned");
        let steps = p.argvs();
        assert_eq!(steps.len(), 2, "not two steps: {steps:?}");
        // The project arrives first, on the branch the remote calls it, with
        // the remote's own prefix left off -- a clone has no remotes yet
        assert_eq!(
            steps[0],
            ["git", "clone", "--branch", "master", "https://example.test/p.git", "/home/user/polite-marmot"]
        );
        assert_eq!(steps[1], ["git", "-C", "/home/user/polite-marmot", "switch", "--no-track", "-c", "polite-marmot"]);
        // Both of them are what the person reads
        assert_eq!(p.line().lines().count(), 2, "only one of them is visible: {}", p.line());

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
        let q = plan_on(&there, "polite-marmot", Some("main"), None, "", None).expect("it can be planned");
        assert_eq!(q.argvs().len(), 1);
        assert!(q.line().contains("worktree add"), "{}", q.line());
    }

    /// One per AI on another machine is made on that machine, and nothing of
    /// this machine's is copied into a path that only exists there.
    ///
    /// The fan planned every folder here whichever machine was chosen, and the
    /// files that come along were copied to a folder of that name on this one
    #[test]
    fn work_for_another_machine_stays_on_that_machine() {
        let there = crate::config::HostSpec {
            name: "bench".into(),
            at: "ssh://me@host:22".into(),
            project: Some("/srv/p".into()),
            ..Default::default()
        };
        let fanned = fan_on(&there, "login", Some("main"), &["claude".into(), "codex".into()], "", None);
        assert_eq!(fanned.len(), 2);
        for (ai, plan) in &fanned {
            let plan = plan.as_ref().expect("it can be planned");
            assert_eq!(plan.where_at(), "bench", "{ai} was planned on this machine");
            assert_eq!(plan.branch, format!("login-{ai}"));
            assert!(plan.folder.to_string_lossy().starts_with("/srv/p"), "{}", plan.folder.display());
        }

        let local = repo("carry-there");
        std::fs::write(local.join(".env"), "SECRET=1").unwrap();
        let mut plan = fanned[0].1.as_ref().unwrap().clone();
        let stray = local.join("stray");
        plan.main = local.clone();
        plan.folder = stray.clone();
        let one = Carry { name: ".env".into(), folder: false, how: "copy".into(), from: None, replace: Vec::new(), line: None };
        let missed = carry_into(&plan, &[one]).missed;
        assert_eq!(missed, [".env"], "it could not carry it, but counts as carried");
        assert!(!stray.exists(), "a folder was made on this machine under the far path's name");
        assert_eq!(plan.like(&local), local.as_path(), "the basis for placing it is the far path");
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
        let env = told.expect("it was not picked up from the settings");
        assert_eq!(env.setup, ["cargo fetch", "tools/conpty.ps1"], "one per line");
        assert!(!env.from.is_empty(), "it cannot say where it came from");

        let p = plan_for(&main, Some("ours"), "work", Some("main"), None, Some(env))
            .expect("it can be planned");
        assert_eq!(p.argvs().len(), 3, "the branch and two lines: {:?}", p.argvs());
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
            .expect("it can be planned");
        let steps = p.argvs();
        assert_eq!(steps.len(), 4, "fetch, branch and two preparation steps: {steps:?}");
        assert_eq!(steps[2], ["sh", "-lc", "cd /home/user/work && npm ci"]);
        assert_eq!(steps[3], ["sh", "-lc", "cd /home/user/work && npm run build"]);
        // The project's word about the image beats the machine's setting: a
        // repository that names one has said the thing that matters most
        let picked = |i| crate::e2b::template_for(&host, i);
        assert_eq!(picked(p.env.as_ref().and_then(|e| e.image.as_deref())), "node:22");
        assert_eq!(picked(None), "base", "say nothing and the machine's setting applies");
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
            .expect("it can be planned");
        assert_eq!(q.argvs().len(), 2, "no preparation on a machine that already exists: {:?}", q.argvs());
        assert!(q.argvs()[1].last().is_some_and(|l| l.contains("npm ci")), "{:?}", q.argvs()[1]);
        // Nothing is fetched there: the project is on that machine already
        assert!(q.argvs()[0].contains(&"worktree".to_string()));

        // And on this machine too, which is as bare as any of them
        let main = repo("prep");
        let here = plan_into(&main, "work", Some("main"), None,
                             crate::devcontainer::read(r#"{"postCreateCommand":"cargo fetch"}"#))
            .expect("it can be planned");
        assert_eq!(here.argvs().len(), 2, "no preparation on this PC");
        assert!(here.argvs()[1].last().is_some_and(|l| l.contains("cargo fetch")));
        // Turned off, nothing of it is there
        let bare = plan_into(&main, "work", Some("main"), None, None).expect("it can be planned");
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
        let p = plan_into(&main, "polite-marmot", Some("main"), Some(&mine), None).expect("it can be planned");
        assert_eq!(p.folder, mine, "the place given is not used");
        assert_ne!(p.folder, folder_for(&main, "polite-marmot"), "it was pulled back to the default");
        // And it is the place the command names, not just the one on screen
        assert!(p.line().contains("\"") && p.line().contains("right here"), "{}", p.line());

        // Nothing named: the app's own answer stands
        let same = plan_into(&main, "polite-marmot", Some("main"), None, None).expect("it can be planned");
        assert_eq!(same.folder, folder_for(&main, "polite-marmot"));
        // An empty name is the same as none
        let blank = plan_into(&main, "polite-marmot", Some("main"), Some(Path::new("")), None)
            .expect("it can be planned");
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
        assert!(at.starts_with(away_from_home()), "too long, but not moved somewhere shorter: {at:?}");
        // The short one is left where branches belong
        assert!(folder_for(&main, "polite-marmot").starts_with(branches_root()));
    }

    /// The place itself is the person's own folder, unless it cannot be.
    #[test]
    fn branches_live_in_the_persons_own_folder() {
        let root = real_branches_root();
        match home_dir().filter(|h| !synced(h) && writable(h)) {
            Some(home) => {
                assert!(root.starts_with(&home), "not under the person's own folder: {root:?}");
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
            ["git", "-C", "D:/work/myproject", "worktree", "add", "--no-track", "-b", "feature/login",
             "D:/work/myproject.worktrees/feature/login", "origin/main"]
        );
        assert_eq!(plan.line(), plan.argv().join(" "), "the line shown and the line run are the same");
        // A branch that already exists is checked out rather than made, and
        // then there is nothing for it to grow from
        let old = Plan { fresh: false, ..plan };
        assert_eq!(
            old.argv(),
            ["git", "-C", "D:/work/myproject", "worktree", "add",
             "D:/work/myproject.worktrees/feature/login", "feature/login"]
        );
    }

    /// What somebody types is not what git and the disk are handed. A name
    /// made of letters they can both hold stands; anything else is dropped,
    /// and a name with too little left draws one instead (`None` here)
    #[test]
    fn a_typed_name_is_cut_down_to_what_a_branch_and_a_folder_can_both_hold() {
        // Nothing to do: already the letters both will take
        for same in ["main", "feature/login", "fix/crash-on-open", "work-2", "release/1.2.3"] {
            assert_eq!(tidy(same).as_deref(), Some(same), "an ordinary name was changed: {same:?}");
        }
        // The slash stays, because the folders this app writes out come from it
        assert_eq!(tidy("機能/ログイン").as_deref(), None, "nothing was left to keep");
        assert_eq!(tidy("機能/login-page").as_deref(), Some("login-page"));
        assert_eq!(tidy("feature/ログイン").as_deref(), Some("feature"));
        // Mixed: what is left is kept when there is enough of it
        assert_eq!(tidy("API連携").as_deref(), Some("API"));
        assert_eq!(tidy("ログイン画面 login").as_deref(), Some("login"));
        // Too little left to mean anything, so nothing is kept
        assert_eq!(tidy("図1").as_deref(), None);
        assert_eq!(tidy("ロ2").as_deref(), None);
        assert_eq!(tidy("").as_deref(), None);
        assert_eq!(tidy("   ").as_deref(), None);
        // The separators do not count toward the three: `a/b` is two letters
        // with a slash in the middle, not a name of three
        assert_eq!(tidy("あa/いb").as_deref(), None);
        // A step up out of the folder this app chose, which survives dropping
        // the letters we will not keep because the dot is one we do keep
        assert_eq!(tidy("日..本..語").as_deref(), None, "it kept a way out of the folder");
        assert_eq!(tidy("../../windows/system32").as_deref(), Some("windows/system32"));
        assert_eq!(tidy("up..down").as_deref(), Some("up.down"));
        // The endings git and Windows keep for themselves
        assert_eq!(tidy("branchname.lock").as_deref(), Some("branchname"));
        assert_eq!(tidy(".hidden-branch").as_deref(), Some("hidden-branch"));
        assert_eq!(tidy("trailing.").as_deref(), Some("trailing"));
        assert_eq!(tidy("NUL").as_deref(), None);
        assert_eq!(tidy("logs/nul.txt").as_deref(), Some("logs"));
        // Whatever comes out, git and this app both take it
        for typed in ["機能/ログイン", "API連携", "../../windows", "日..本", "a b\tc",
                      "star*name", "colon:here", "back\\slash", "x.lock", "~^?[", "CON.txt",
                      "//leading", "trailing//"] {
            let Some(name) = tidy(typed) else { continue };
            assert!(name_is_usable(&name), "{typed:?} became a name git refuses: {name:?}");
        }
    }

    #[test]
    fn a_name_git_would_refuse_is_refused_here_first() {
        for bad in ["", " ", "/leading", "trailing/", "two//slashes", "up..down",
                    "back\\slash", "with space", "star*", "colon:here", "x.lock"] {
            assert!(!name_is_usable(bad.trim()) || bad.trim().is_empty(), "must not be let through: {bad:?}");
        }
        for good in ["main", "feature/login", "fix/crash-on-open", "work-2", "release/1.2.3"] {
            assert!(name_is_usable(good), "an ordinary name does not get through: {good:?}");
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
        let plan = plan(&main, "mighty-gannet", Some("main")).expect("it can be planned");
        create(&plan).expect("it can be made");
        let folder = plan.folder.clone();
        git(&main, &["config", &format!("branch.{}.shikishaBase", "mighty-gannet"), "main"]);

        // Read before it runs, and it is the line that runs
        let r = rename_plan(&folder, " fix/crash ").expect("a rename can be planned");
        assert_eq!(r.from, "mighty-gannet");
        assert_eq!(r.to, "fix/crash", "surrounding spaces are dropped");
        assert!(r.line().contains("branch -m mighty-gannet fix/crash"), "{}", r.line());
        rename(&r).expect("it can be renamed");

        assert_eq!(crate::repo::branch_of(&folder).as_deref(), Some("fix/crash"));
        // Where it grew from is a fact about the branch, so it comes along
        let note = std::process::Command::new("git")
            .arg("-C").arg(&folder)
            .args(["config", "--get", "branch.fix/crash.shikishaBase"])
            .output().expect("git runs");
        assert_eq!(String::from_utf8_lossy(&note.stdout).trim(), "main", "the record of where it came from is gone");
        // The folder stays put: it was named on the first day and nothing moves
        assert!(folder.exists(), "the folder moved");

        // A name git would refuse never reaches git: what a folder and a
        // branch can both hold is kept, and the rest goes
        assert_eq!(
            rename_plan(&folder, "two words").expect("it is tidied, not refused").to,
            "twowords"
        );
        // Nothing left to rename it to, and a rename has no name of its own
        // to draw -- unlike making one, there is already a branch here
        assert!(rename_plan(&folder, "ログイン").is_err());
        assert!(rename_plan(&folder, "").is_err());
        // The project's own folder is not a branch cut from it
        assert!(rename_plan(&main, "whatever").is_err(), "the main checkout's branch can be renamed");
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
        assert!(drawn.len() > 15, "20 draws gave only {} different names", drawn.len());
        for name in &drawn {
            assert!(name_is_usable(name), "a name git will not take: {name:?}");
            assert_eq!(name.matches('-').count(), 1, "not two words joined: {name:?}");
            assert!(!name.starts_with("work-"), "it fell back to a numbered name: {name:?}");
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
        assert_eq!(out[2].0, "gemini", "the AI's name is kept tidied");
        // Each gets its own folder, and every line is the one that will run
        let folders: std::collections::HashSet<_> =
            out.iter().map(|(_, p)| p.as_ref().unwrap().folder.clone()).collect();
        assert_eq!(folders.len(), 3);
        assert!(out.iter().all(|(_, p)| p.as_ref().unwrap().line().contains("worktree add")));
        // A name git would refuse is refused per branch, and the others still plan
        let bad = fan(&main, "kan ban", Some("main"), &ais);
        assert!(bad.iter().all(|(_, p)| p.is_err()), "a name with a space got through");
    }

    #[test]
    fn a_branch_really_gets_its_own_folder() {
        let main = scratch("real").join("proj-real");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(&main).args(args);
            let out = crate::detach_console(&mut run).output().expect("git is needed");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(main.join("readme.md"), "hi\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);

        let cut = plan(&main, "feature/login", None).unwrap();
        assert!(cut.fresh, "a branch that does not exist yet");
        assert_eq!(cut.base, "main", "with no remote, from where it is now, under that name");
        create(&cut).unwrap();

        let made = &cut.folder;
        assert!(made.join("readme.md").exists(), "the contents are there");
        assert_eq!(crate::repo::branch_of(made).as_deref(), Some("feature/login"));
        assert_eq!(crate::repo::family_of(made), crate::repo::family_of(&main), "the same family");
        assert!(crate::repo::is_linked(made), "it is a branch cut from the main checkout");
        // Both asked the same way. Comparing against the path this test wrote
        // would be comparing an answer with a spelling, and a machine whose
        // temporary folder is handed out short (`RUNNER~1`) has two of those
        assert_eq!(
            crate::repo::main_checkout(made),
            crate::repo::main_checkout(&main),
            "it cannot get back to the main checkout from the branch"
        );

        // Where it grew from, written into the repository itself
        let mut ask = std::process::Command::new("git");
        ask.arg("-C").arg(made).args(["config", "--get", "branch.feature/login.shikishaBase"]);
        let out = crate::detach_console(&mut ask).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "main");

        // Asking for the same branch twice does not quietly make a second one
        assert!(create(&cut).is_err(), "it does not make it twice in the same place");
        // Nor does it get as far as the button: a folder already standing there
        // is said while the name is still being typed
        assert!(plan(&main, "feature/login", None).is_err(), "it is not known until pressed");
        // With the folder gone, the branch that now exists is checked out
        // rather than made again
        crate::worktree::discard(made).expect("cleaned up");
        let again = plan(&main, "feature/login", None).unwrap();
        assert!(!again.fresh, "an existing branch is not made again");

        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    /// A branch cut from `origin/main` follows nothing, so the git column's
    /// push sends it under its own name. It used to follow `origin/main`,
    /// and a plain push refused to choose between that and its own name.
    /// A branch somebody sets to follow another name on purpose is still told
    /// so in plain words.
    #[test]
    fn a_branch_cut_from_the_remote_pushes_under_its_own_name() {
        let root = scratch("tracks");
        let _ = std::fs::remove_dir_all(&root);
        let seed = root.join("seed");
        let far = root.join("far.git");
        let main = root.join("proj");
        std::fs::create_dir_all(&seed).unwrap();
        let git = |at: &Path, args: &[&str]| -> String {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(at).args(args);
            let out = crate::detach_console(&mut run).output().expect("git is needed");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        // Whether git has a value, without failing the test when there is none
        let has = |at: &Path, key: &str| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(at).args(["config", "--get", key]);
            crate::detach_console(&mut run).output().unwrap().status.success()
        };
        git(&seed, &["init", "-q", "-b", "main"]);
        git(&seed, &["config", "user.email", "t@example.com"]);
        git(&seed, &["config", "user.name", "t"]);
        std::fs::write(seed.join("readme.md"), "hi\n").unwrap();
        git(&seed, &["add", "-A"]);
        git(&seed, &["commit", "-qm", "first"]);
        git(&seed, &["clone", "-q", "--bare", &seed.display().to_string(), &far.display().to_string()]);
        git(&seed, &["clone", "-q", &far.display().to_string(), &main.display().to_string()]);
        git(&main, &["config", "user.email", "t@example.com"]);
        git(&main, &["config", "user.name", "t"]);
        // What a plain push does is decided here rather than by the settings
        // of the machine the tests run on
        git(&main, &["config", "push.default", "simple"]);
        let main_before = git(&far, &["rev-parse", "refs/heads/main"]);

        // a. Cut the way the app cuts it: from origin/main, following nothing
        let branch = "issue-3-tell-production-from-staging";
        let cut = plan(&main, branch, Some("origin/main")).unwrap();
        assert!(cut.fresh);
        assert!(cut.argv().contains(&"--no-track".to_string()), "{:?}", cut.argv());
        create(&cut).unwrap();
        let made = cut.folder.clone();
        assert!(!has(&made, &format!("branch.{branch}.merge")), "the new branch follows origin/main");
        assert!(!has(&made, &format!("branch.{branch}.remote")), "the new branch follows a remote");
        assert_eq!(git(&made, &["config", "--get", &format!("branch.{branch}.shikishaBase")]), "origin/main");

        // b. A commit, and the git column's push: a branch of the same name
        // appears on the remote and is followed from then on, and main stays put
        std::fs::write(made.join("staging.md"), "which is which\n").unwrap();
        git(&made, &["add", "-A"]);
        git(&made, &["commit", "-qm", "say which is which"]);
        crate::git::push(&made, &crate::git::As::default()).expect("the first push of a new branch goes through");
        assert_eq!(git(&far, &["rev-parse", &format!("refs/heads/{branch}")]), git(&made, &["rev-parse", "HEAD"]));
        assert_eq!(git(&made, &["rev-parse", "--abbrev-ref", "@{u}"]), format!("origin/{branch}"));
        assert_eq!(git(&far, &["rev-parse", "refs/heads/main"]), main_before, "the remote's main moved");

        // c. Following a branch of another name on purpose is still refused,
        // in the words that say which and how
        git(&made, &["branch", "--set-upstream-to", "origin/main"]);
        std::fs::write(made.join("staging.md"), "which is which, again\n").unwrap();
        git(&made, &["commit", "-qam", "again"]);
        let err = crate::git::push(&made, &crate::git::As::default()).unwrap_err();
        let said = err.downcast_ref::<crate::git::PushNameMismatch>().expect("the refusal is not recognised");
        assert_eq!((said.branch.as_str(), said.target.as_str()), (branch, "main"));
        assert_eq!(git(&far, &["rev-parse", "refs/heads/main"]), main_before, "the refused push moved main");

        let _ = discard(&made);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Made on a thread, a folder says how far it has got and ends in what
    /// came along. One that fails once git has made it, or is stopped, is taken
    /// back whole -- folder and new branch -- so the same name can be tried again
    #[test]
    fn a_folder_made_in_the_background_ends_made_or_taken_back() {
        let root = scratch("making");
        let main = root.join("proj-making");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(&main).args(args);
            let out = crate::detach_console(&mut run).output().expect("git is needed");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(main.join("readme.md"), "hi\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        let wait = |m: &Making| {
            for _ in 0..600 {
                if let Some(o) = m.outcome() {
                    return o;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            panic!("the making never ended");
        };

        let good = plan_into(&main, "bg/good", None, Some(&root.join("good")), None).unwrap();
        let made = Making::start(good.clone(), Vec::new());
        assert!(wait(&made).is_ok(), "the folder was not made");
        assert!(good.folder.join("readme.md").exists(), "the contents are not there");

        // The project's setup fails once the folder is there: all of it goes
        let env = crate::devcontainer::Env { setup: vec!["exit 3".into()], ..Default::default() };
        let bad = plan_into(&main, "bg/bad", None, Some(&root.join("bad")), Some(env)).unwrap();
        let failed = wait(&Making::start(bad.clone(), Vec::new()));
        assert!(matches!(&failed, Err(why) if !why.is_empty()), "a failure says nothing: {failed:?}");
        assert!(!bad.folder.exists(), "the half-made folder is left behind");
        assert!(!git(&["branch", "--list", "bg/bad"]).contains("bg/bad"), "the new branch is left behind");
        assert!(plan_into(&main, "bg/bad", None, Some(&root.join("bad")), None).unwrap().fresh, "the name cannot be tried again");

        // Stopped: nothing is left, and it is not called a failure
        let stopping = plan_into(&main, "bg/stop", None, Some(&root.join("stop")), None).unwrap();
        let m = Making::start(stopping.clone(), Vec::new());
        m.stop();
        assert_eq!(m.stage(), Stage::Stopping);
        assert!(matches!(wait(&m), Err(why) if why.is_empty()), "a stop is said as a failure");
        assert!(!stopping.folder.exists(), "a stopped folder is left behind");
        assert!(!git(&["branch", "--list", "bg/stop"]).contains("bg/stop"), "a stopped branch is left behind");

        // A branch somebody has committed to is never deleted by taking back
        git(&["branch", "kept", "main"]);
        let kept = Plan { fresh: true, branch: "kept".into(), ..plan_into(&main, "kept2", None, Some(&root.join("kept")), None).unwrap() };
        std::fs::write(main.join("more.md"), "x\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "second"]);
        take_back(&kept);
        assert!(git(&["branch", "--list", "kept"]).contains("kept"), "a branch not at its base was deleted");

        let _ = crate::worktree::discard(&good.folder);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What a fresh folder is missing, and getting it there.
    #[test]
    fn what_git_does_not_carry_can_be_brought_along() {
        use crate::config::{BringRule, Replace};
        let main = scratch("carry").join("proj-carry");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(&main).args(args);
            let out = crate::detach_console(&mut run).output().expect("git is needed");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(main.join(".gitignore"), "node_modules/\n.env\nbuild/\n*.local\n").unwrap();
        std::fs::create_dir_all(main.join("node_modules").join("left-pad")).unwrap();
        std::fs::write(main.join("node_modules").join("left-pad").join("index.js"), "x").unwrap();
        std::fs::write(main.join(".env"), "TOKEN=live\nPORT=3000\n").unwrap();
        std::fs::create_dir_all(main.join("web")).unwrap();
        std::fs::write(main.join("web").join("config.local"), "name=main\n").unwrap();
        std::fs::write(main.join("readme.md"), "hi\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);

        // Nothing chosen: each line shows what it would do by itself
        let offered = carryables(&main, &[]);
        let named = |list: &[Carry], n: &str| list.iter().find(|c| c.name == n).cloned();
        // Only what git was told to ignore, and only what is really there --
        // including deeper in the tree, where a pattern with a star reaches
        assert!(named(&offered, "readme.md").is_none(), "tracked files are not offered");
        assert!(named(&offered, "build").is_none(), "missing things are not offered");
        assert_eq!(named(&offered, "node_modules").map(|c| (c.folder, c.how)), Some((true, "link".into())));
        assert_eq!(named(&offered, ".env").map(|c| c.how), Some("skip".into()), "live keys are left out unless chosen");
        assert_eq!(named(&offered, "web/config.local").map(|c| c.how), Some("copy".into()));

        // The project's choices, per line, and a file from somewhere else
        let outside = main.parent().unwrap().join("templates");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("app.env"), "MADE=from-template\n").unwrap();
        let rules = vec![
            BringRule {
                pattern: Some(".env".into()),
                how: "replace".into(),
                replace: vec![
                    Replace { find: "live".into(), with: "branch".into(), regex: false },
                    Replace { find: r"^PORT=\d+$".into(), with: "PORT=3001".into(), regex: true },
                ],
                ..Default::default()
            },
            BringRule { pattern: Some("*.local".into()), how: "skip".into(), ..Default::default() },
            BringRule {
                from: Some(outside.join("app.env").display().to_string()),
                to: Some("config/app.env".into()),
                how: "copy".into(),
                ..Default::default()
            },
            // Nothing climbs out of the new folder, whatever is written
            BringRule { from: Some(outside.join("app.env").display().to_string()), to: Some("../escape".into()), how: "copy".into(), ..Default::default() },
        ];
        let chosen = carryables(&main, &rules);
        assert_eq!(named(&chosen, ".env").map(|c| c.how), Some("replace".into()));
        assert_eq!(named(&chosen, "web/config.local").map(|c| c.how), Some("skip".into()));
        assert!(named(&chosen, "config/app.env").is_some_and(|c| c.from.is_some()));
        assert!(!chosen.iter().any(|c| c.name.contains("..") || c.name.contains("escape")), "{chosen:?}");

        let cut = plan(&main, "feature/login", None).unwrap();
        create(&cut).unwrap();
        let brought = carry_into(&cut, &chosen);
        assert!(brought.missed.is_empty() && brought.unreplaced.is_empty(), "{brought:?}");
        assert!(
            cut.folder.join("node_modules").join("left-pad").join("index.js").exists(),
            "what the link points to cannot be seen"
        );
        assert_eq!(std::fs::read_to_string(cut.folder.join(".env")).unwrap(), "TOKEN=branch\nPORT=3001\n");
        assert_eq!(std::fs::read_to_string(main.join(".env")).unwrap(), "TOKEN=live\nPORT=3000\n", "the original was touched");
        assert!(!cut.folder.join("web").join("config.local").exists(), "a line left out came along");
        assert_eq!(std::fs::read_to_string(cut.folder.join("config").join("app.env")).unwrap(), "MADE=from-template\n");

        // The junction has to go before the folder does, or removing the tree
        // would walk into it and take the original's contents with it
        let mut unhook = std::process::Command::new("cmd");
        unhook.args(["/c", "rmdir"]).arg(cut.folder.join("node_modules"));
        let _ = crate::detach_console(&mut unhook).status();
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    /// A folder that cannot be given a second name is a question, not a loss,
    /// and the answer copies it in.
    ///
    /// The case this is for is a project on another machine's share: the kind
    /// of link Windows makes without asking anyone for rights is a name for a
    /// place on this machine only, so every folder a branch was to link came
    /// out as "could not be brought" and nothing arrived (2026-09-18, a
    /// project on a mapped drive). Held here with a destination no folder can
    /// be written to, which is the one way to refuse a link on any machine
    #[test]
    fn a_folder_that_cannot_be_linked_is_asked_about_and_then_copied() {
        let main = repo("carry-nolink");
        let there = main.join("vendor").join("left-pad");
        std::fs::create_dir_all(&there).unwrap();
        std::fs::write(there.join("index.js"), "x").unwrap();
        let cut = plan(&main, "feature/nolink", None).unwrap();
        std::fs::create_dir_all(&cut.folder).unwrap();
        // Nothing can be made under a name a file already holds, so this is a
        // link no machine can make
        std::fs::write(cut.folder.join("gone"), "in the way").unwrap();
        let carry = |name: &str, how: &str| Carry {
            name: name.into(),
            folder: true,
            how: how.into(),
            from: Some(main.join("vendor").display().to_string()),
            replace: Vec::new(),
            line: None,
        };
        let said = carry_into(&cut, &[carry("gone/vendor", "link")]);
        assert_eq!(said.unlinked, ["gone/vendor"], "a folder with no second name was not asked about");
        assert!(said.missed.is_empty(), "a question was counted as a loss: {said:?}");
        assert!(said.copied.is_empty(), "it was copied without being asked: {said:?}");
        // Answering yes is the same carrying, asked for as a copy -- and one
        // that cannot be written is a loss like any other
        let said = carry_into(&cut, &[carry("gone/vendor", "copy")]);
        assert_eq!(said.missed, ["gone/vendor"], "a copy that failed was not said to be missing");
        assert!(said.unlinked.is_empty(), "a copy was asked about as if it were a link: {said:?}");

        // Where it can be written, a link is made and nothing is asked
        let said = carry_into(&cut, &[carry("vendor", "link")]);
        assert!(said.missed.is_empty() && said.unlinked.is_empty(), "the folder did not arrive: {said:?}");
        assert!(
            cut.folder.join("vendor").join("left-pad").join("index.js").exists(),
            "what came along cannot be read",
        );
        let mut unhook = std::process::Command::new("cmd");
        unhook.args(["/c", "rmdir"]).arg(cut.folder.join("vendor"));
        let _ = crate::detach_console(&mut unhook).status();
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    /// A link carried in is not counted as work, whatever git makes of it; a
    /// file nobody committed still is, and a rename is one change, not two
    #[test]
    fn only_work_stops_a_folder_being_thrown_away() {
        let link = |p: &str| p == "web/node_modules" || p == ".env";
        assert_eq!(unsaved_work("", &link), 0);
        assert_eq!(unsaved_work("?? web/node_modules\0?? .env\0", &link), 0);
        assert_eq!(unsaved_work("?? web/node_modules\0?? notes.txt\0", &link), 1);
        // A link git follows is a change to it like any other
        assert_eq!(unsaved_work(" M .env\0", &link), 1);
        assert_eq!(unsaved_work("R  new.rs\0old.rs\0 M a.rs\0", &link), 2);
    }

    /// A link deeper in the tree is unhooked before the folder goes, whatever
    /// the settings say by then.
    #[test]
    fn links_are_unhooked_wherever_they_are() {
        let main = scratch("unhook").join("proj-unhook");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        let _ = std::fs::remove_dir_all(branches_root().join("proj-unhook"));
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(&main).args(args);
            let out = crate::detach_console(&mut run).output().expect("git is needed");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        // The one that matters: ignored, and not at the top of the tree
        std::fs::write(main.join(".gitignore"), "node_modules/
").unwrap();
        let keep = main.join("web").join("node_modules").join("left-pad");
        std::fs::create_dir_all(&keep).unwrap();
        std::fs::write(keep.join("index.js"), "x").unwrap();
        std::fs::write(main.join("readme.md"), "hi
").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);

        let offered = carryables(&main, &[]);
        assert_eq!(
            offered.iter().find(|c| c.name == "web/node_modules").map(|c| c.how.clone()),
            Some("link".into()),
            "{offered:?}"
        );
        let cut = plan(&main, "feature/deep", None).unwrap();
        create(&cut).unwrap();
        let brought = carry_into(&cut, &offered);
        assert!(brought.missed.is_empty(), "{brought:?}");
        let linked = cut.folder.join("web").join("node_modules");
        assert!(linked.join("left-pad").join("index.js").exists(), "the link was not made");

        // Set to Copy since, so nothing but the folder itself can say a link
        // is standing there
        let _ = crate::worktree::discard(&cut.folder);
        assert!(!cut.folder.exists(), "the folder is still there: {:?}", cut.folder);
        assert!(keep.join("index.js").exists(), "removing the folder took the original's contents");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    /// Replacements are made in order, plainly or as regular expressions with
    /// lines as their unit, and a broken expression is said rather than skipped
    #[test]
    fn a_copy_is_replaced_in_as_asked() {
        use crate::config::Replace;
        let r = |find: &str, with: &str, regex: bool| Replace { find: find.into(), with: with.into(), regex };
        let done = |text: &str, rs: &[Replace]| apply_replaces(text, rs).unwrap().0;
        let text = "A=1\r\nURL=http://a.b/c\r\nA=1\r\n";
        assert_eq!(done(text, &[r("A=1", "A=2", false)]), "A=2\r\nURL=http://a.b/c\r\nA=2\r\n");
        // Plain text is not a pattern: the dots are dots
        assert_eq!(done("a.b axb", &[r("a.b", "Z", false)]), "Z axb");
        assert_eq!(done("a.b axb", &[r("a.b", "Z", true)]), "Z Z");
        assert_eq!(done("PORT=3000\nX=1\n", &[r("^PORT=(\\d+)$", "PORT=4$1", true)]), "PORT=43000\nX=1\n");
        // Windows line endings: $ is still the end of the line
        assert_eq!(done("PORT=3000\r\nX=1\r\n", &[r("^PORT=\\d+$", "PORT=3001", true)]), "PORT=3001\r\nX=1\r\n");
        assert_eq!(done("ab", &[r("a", "b", false), r("bb", "c", false)]), "c", "not in order");
        assert!(apply_replaces("x", &[r("(", "y", true)]).is_err());
        // What found nothing is said
        assert_eq!(apply_replaces("A=1\n", &[r("A=1", "A=2", false), r("^B=", "B=", true)]).unwrap().1, ["^B="]);
    }

    /// A line that ignores what is inside a folder offers those things, not
    /// the folder: git does not ignore `www/tmp` for `www/tmp/*`, and offering
    /// it put a link to the whole folder beside a link to each thing in it.
    /// A line naming a folder still offers that folder
    #[test]
    fn a_folder_is_offered_only_when_a_line_ignores_the_folder_itself() {
        let main = scratch("ignoredfolders").join("proj-ignoredfolders");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        std::fs::create_dir_all(&main).unwrap();
        crate::git::run(&main, &["init", "-q", "-b", "main"]).unwrap();
        std::fs::write(main.join(".gitignore"), "www/tmp/*\nnode_modules/\nbuild\n*.log\n").unwrap();
        for dir in ["www/tmp/a", "www/tmp/b", "node_modules/x", "build/y", "logs"] {
            std::fs::create_dir_all(main.join(dir)).unwrap();
        }
        for file in ["www/tmp/a/f", "node_modules/x/f", "build/y/f", "logs/a.log"] {
            std::fs::write(main.join(file), "x").unwrap();
        }
        let found: Vec<(String, String, bool)> =
            ignored(&main).into_iter().map(|i| (i.path, i.pattern, i.folder)).collect();
        let row = |p: &str, pattern: &str, folder: bool| (p.to_string(), pattern.to_string(), folder);
        assert_eq!(found, [
            row("build/", "build", true),
            row("logs/a.log", "*.log", false),
            row("node_modules/", "node_modules/", true),
            row("www/tmp/a/", "www/tmp/*", true),
            row("www/tmp/b/", "www/tmp/*", true),
        ]);
    }

    /// The project's .gitignore, changed a line at a time, and the files git
    /// keeps following although a line now matches them
    #[test]
    fn gitignore_lines_are_added_and_taken_out() {
        let main = scratch("ignorelines").join("proj-ignorelines");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        std::fs::create_dir_all(&main).unwrap();
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@example.com"], &["config", "user.name", "t"]] {
            crate::git::run(&main, args).unwrap();
        }
        std::fs::write(main.join("secret.txt"), "x").unwrap();
        crate::git::run(&main, &["add", "secret.txt"]).unwrap();
        crate::git::run(&main, &["commit", "-qm", "with a secret"]).unwrap();

        gitignore_add(&main, "secret.txt").unwrap();
        gitignore_add(&main, "*.log").unwrap();
        assert!(gitignore_add(&main, " *.log ").is_err(), "a line was added twice");
        assert!(gitignore_add(&main, "  ").is_err());
        let lines = gitignore_lines(&main);
        let at = |l: &str| lines.iter().position(|x| x == l).map(|i| i + 1).unwrap();
        // Committed before it was ignored: git still follows it
        assert_eq!(tracked_but_ignored(&main), ["secret.txt"]);
        untrack(&main, &["secret.txt".to_string()]).unwrap();
        assert!(tracked_but_ignored(&main).is_empty());
        assert!(main.join("secret.txt").exists(), "the file itself went");

        // Taken out by number, only while that line still says the same thing
        assert!(gitignore_remove(&main, at("*.log"), "secret.txt").is_err());
        gitignore_remove(&main, at("*.log"), "*.log").unwrap();
        assert!(!gitignore_lines(&main).iter().any(|l| l == "*.log"));
        assert!(gitignore_lines(&main).iter().any(|l| l == "secret.txt"));
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
            let out = crate::detach_console(&mut run).output().expect("git is needed");
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
        assert!(discard(&main).is_err(), "the main checkout must not be deleted");
        assert!(main.join("readme.md").exists());

        // Work that only exists here is the one thing this must not take
        std::fs::write(cut.folder.join("notes.md"), "half an idea\n").unwrap();
        assert!(discard(&cut.folder).is_err(), "it deleted with uncommitted changes");
        assert!(cut.folder.join("notes.md").exists(), "it is gone");

        // Once there is nothing to lose, it goes -- and git stops listing it
        std::fs::remove_file(cut.folder.join("notes.md")).unwrap();
        discard(&cut.folder).unwrap();
        assert!(!cut.folder.exists(), "the folder is still there");
        let mut ask = std::process::Command::new("git");
        ask.arg("-C").arg(&main).args(["worktree", "list", "--porcelain"]);
        let listed = crate::detach_console(&mut ask).output().unwrap();
        assert!(
            !String::from_utf8_lossy(&listed.stdout).contains("feature/gone"),
            "git still holds it"
        );
        // Asking again is not an error: it is already how it was asked to be
        discard(&cut.folder).unwrap();

        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    /// A real project with a git repository and one worktree cut from it, for
    /// the removal tests
    fn cut_for_removal(name: &str) -> (PathBuf, Plan) {
        let main = scratch(name).join(format!("proj-{name}"));
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        let _ = std::fs::remove_dir_all(branches_root().join(format!("proj-{name}")));
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let mut run = std::process::Command::new("git");
            run.arg("-C").arg(&main).args(args);
            let out = crate::detach_console(&mut run).output().expect("git is needed");
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(main.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(main.join("readme.md"), "hi\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "first"]);
        let cut = plan(&main, "feature/leaving", None).unwrap();
        create(&cut).unwrap();
        (main, cut)
    }

    fn git_lists(main: &Path, folder: &Path) -> bool {
        let mut ask = std::process::Command::new("git");
        ask.arg("-C").arg(main).args(["worktree", "list", "--porcelain"]);
        let listed = crate::detach_console(&mut ask).output().unwrap();
        let name = folder.file_name().unwrap().to_string_lossy().into_owned();
        String::from_utf8_lossy(&listed.stdout).contains(&name)
    }

    /// A build leaves paths longer than Windows' 260 characters under an
    /// ignored folder. Without long paths, git deleted `.git` and forgot the
    /// worktree, then stopped at the first long path. Everything after that
    /// stayed on disk, and the next try refused the folder as "not a worktree"
    #[test]
    fn a_worktree_with_paths_longer_than_windows_allows_is_deleted_whole() {
        let (main, cut) = cut_for_removal("longpath");
        let mut deep = cut.folder.join("target").join("aa");
        while deep.as_os_str().len() < 300 {
            deep = deep.join("d".repeat(40));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("f.txt"), "x").unwrap();
        // Read only, as some tools leave what they write
        let stiff = cut.folder.join("target").join("zz").join("stiff.txt");
        std::fs::create_dir_all(stiff.parent().unwrap()).unwrap();
        std::fs::write(&stiff, "x").unwrap();
        let mut ro = std::fs::metadata(&stiff).unwrap().permissions();
        ro.set_readonly(true);
        std::fs::set_permissions(&stiff, ro).unwrap();

        discard_waiting(&cut.folder).unwrap();
        assert!(!cut.folder.exists(), "the folder is still there");
        assert!(!git_lists(&main, &cut.folder), "git still holds it");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    /// Something that will not delete is named, with everything else already
    /// gone. Once it is let go, the same removal finishes, even though git has
    /// forgotten the worktree by then
    #[cfg(windows)]
    #[test]
    fn a_file_held_open_is_named_and_the_removal_finishes_once_it_is_let_go() {
        use std::os::windows::fs::OpenOptionsExt;
        let (main, cut) = cut_for_removal("heldopen");
        let target = cut.folder.join("target");
        std::fs::create_dir_all(target.join("a")).unwrap();
        std::fs::write(target.join("a").join("free.txt"), "x").unwrap();
        let held_at = target.join("held.bin");
        std::fs::write(&held_at, "x").unwrap();
        std::fs::create_dir_all(target.join("z")).unwrap();
        std::fs::write(target.join("z").join("after.txt"), "x").unwrap();
        // Nobody else may open it, delete it or move it while this is open
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&held_at).unwrap();

        let mut released = false;
        let said = discard_step(&cut.folder, &mut released).unwrap_err().to_string();
        assert!(said.contains("held.bin"), "it does not say which file stayed: {said}");
        assert!(held_at.exists());
        assert!(!target.join("a").exists() && !target.join("z").exists(), "what could go did not");
        assert!(!cut.folder.join("readme.md").exists(), "what git tracks did not go");
        assert!(released, "git let go of it, and the removal does not know");
        assert!(!git_lists(&main, &cut.folder), "git still lists a worktree it has deleted .git from");

        drop(held);
        discard_step(&cut.folder, &mut released).unwrap();
        assert!(!cut.folder.exists(), "the rest was not finished once the file was let go");
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
        assert_eq!(found.first().map(String::as_str), Some("origin/main"), "the default comes first: {found:?}");
        for want in ["origin/main", "main", "feature/login", "old-thing"] {
            assert!(found.iter().any(|b| b == want), "{want} is missing: {found:?}");
        }
        // The pointer at another branch is not a branch anyone starts from
        assert!(!found.iter().any(|b| b.ends_with("HEAD")), "it offers HEAD: {found:?}");
        // Said once each
        let mut sorted = found.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), found.len(), "there are duplicates: {found:?}");
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

    /// One row per line of an ignore file, however many things it matches,
    /// in the order they are offered, saying what the project says
    #[test]
    fn the_things_offered_are_counted_by_their_line() {
        let item = |name: &str, folder: bool, how: &str, source: &str, pattern: &str| Carry {
            name: name.into(),
            folder,
            how: how.into(),
            from: None,
            replace: Vec::new(),
            line: Some(CarryLineKey { source: source.into(), pattern: pattern.into() }),
        };
        let offered = vec![
            item("www/tmp/a", true, "link", ".gitignore", "www/tmp/*"),
            item(".env", false, "skip", ".gitignore", ".env"),
            item("www/tmp/b", true, "link", ".gitignore", "www/tmp/*"),
            item("www/tmp/c.txt", false, "link", ".gitignore", "www/tmp/*"),
            item("web/cache", true, "link", "web/.gitignore", "www/tmp/*"),
            // From elsewhere: no line to choose it by
            Carry { name: "keys".into(), folder: true, how: "copy".into(), from: Some("D:/keys".into()), replace: Vec::new(), line: None },
        ];
        let lines = carry_lines(&offered);
        let said: Vec<_> = lines.iter().map(|l| (l.source.as_str(), l.pattern.as_str(), l.how.as_str(), l.count, l.folders)).collect();
        assert_eq!(said, [
            (".gitignore", "www/tmp/*", "link", 3, false),
            (".gitignore", ".env", "skip", 1, false),
            ("web/.gitignore", "www/tmp/*", "link", 1, true),
        ]);
    }
}


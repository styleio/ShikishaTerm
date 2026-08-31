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
}

impl Plan {
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

    /// The same line, as a person reads it. Quoted only where it has to be
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

/// Works out where a branch's folder goes and what will make it.
///
/// `base` is what a new branch grows from; leave it out for the sensible one.
pub fn plan(main: &Path, branch: &str, base: Option<&str>) -> Result<Plan> {
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
    let base = match base.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => b.to_string(),
        None => default_base(&main),
    };
    Ok(Plan { folder: folder_for(&main, &branch), main, branch, base, fresh })
}

/// Makes the folder, and remembers what it was cut from.
///
/// The note goes into the repository's own settings rather than ours: what a
/// branch grew from is a fact about the branch, and one that outlives this app
/// being installed
pub fn create(plan: &Plan) -> Result<()> {
    if plan.folder.exists() {
        bail!(crate::i18n::tp(
            "err.worktree.exists",
            &[("path", &plan.folder.display().to_string())]
        ));
    }
    if let Some(parent) = plan.folder.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let argv = plan.argv();
    run(&argv)?;
    if plan.fresh {
        // Best effort: the folder is made and usable either way, and a missing
        // note only means a later diff has to guess its starting point
        let _ = run(&[
            "git".into(),
            "-C".into(),
            plan.folder.display().to_string(),
            "config".into(),
            format!("branch.{}.shikishaBase", plan.branch),
            plan.base.clone(),
        ]);
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
    use std::io::Write as _;
    let mut child = match std::process::Command::new("git")
        .arg("-C")
        .arg(main)
        .args(["check-ignore", "--stdin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Default::default(),
    };
    if let Some(mut w) = child.stdin.take() {
        let _ = w.write_all(names.join("\n").as_bytes());
    }
    let Ok(out) = child.wait_with_output() else {
        return Default::default();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().trim_matches('"').to_string())
        .filter(|l| !l.is_empty())
        .collect()
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
            // A junction, which Windows lets anyone make -- a symbolic link
            // needs rights that most people running this do not have
            true => std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&to)
                .arg(&from)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false),
            false => std::fs::copy(&from, &to).is_ok(),
        };
        if !done {
            trouble.push(name.clone());
        }
    }
    trouble
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
    if !crate::repo::is_linked(folder) {
        bail!(crate::i18n::t("err.worktree.not_a_branch"));
    }
    let dirty = std::process::Command::new("git")
        .arg("-C")
        .arg(folder)
        .args(["status", "--porcelain"])
        .output()?;
    let said = String::from_utf8_lossy(&dirty.stdout);
    if !said.trim().is_empty() {
        bail!(crate::i18n::tp(
            "err.worktree.dirty",
            &[("count", &said.lines().count().to_string())]
        ));
    }
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
    Ok(())
}

/// Where a branch's folder goes, and why there.
///
/// Beside the checkout, in one folder that holds all of them: it is easy to
/// find, short enough for Windows, and easy to be rid of. Three things send it
/// somewhere else instead -- a parent that cannot be written to, a path long
/// enough to start breaking tools, and a folder that is being synced to the
/// cloud, where every branch would be uploaded in full.
pub fn folder_for(main: &Path, branch: &str) -> PathBuf {
    let name = main
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".into());
    // The branch's own shape is kept: `feature/login` is two folders, which is
    // what makes it impossible for two branches to want one folder
    let leaf: PathBuf = branch.split('/').filter(|s| !s.is_empty()).collect();
    if let Some(parent) = main.parent() {
        let beside = parent.join(format!("{name}.worktrees")).join(&leaf);
        if !synced(parent) && writable(parent) && beside.display().to_string().len() < 180 {
            return beside;
        }
    }
    away_from_home().join(&name).join(&leaf)
}

/// The place for folders that cannot sit beside their checkout. Ours, per
/// machine, and never synced anywhere
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
        if let Ok(root) = std::env::var(key) {
            if !root.is_empty() && here.starts_with(&root.to_lowercase()) {
                return true;
            }
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
    if let Ok(text) = std::fs::read_to_string(git.join("refs/remotes/origin/HEAD")) {
        if let Some(r) = text.trim().strip_prefix("ref: refs/remotes/") {
            if !r.is_empty() {
                return r.to_string();
            }
        }
    }
    for name in ["origin/main", "origin/master"] {
        if ref_exists(&git, &format!("refs/remotes/{name}")) {
            return name.to_string();
        }
    }
    "HEAD".into()
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
fn run(argv: &[String]) -> Result<()> {
    let (head, rest) = argv.split_first().expect("空のコマンド");
    let out = std::process::Command::new(head).args(rest).output()?;
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

    fn repo(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-wt-{name}")).join("myproject");
        let _ = std::fs::remove_dir_all(d.parent().unwrap());
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::write(d.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        d
    }

    #[test]
    fn a_branch_gets_a_folder_beside_the_checkout() {
        let main = repo("place");
        let at = folder_for(&main, "feature/login");
        // One folder holds all of them, so there is one thing to find and one
        // thing to delete
        assert_eq!(at.parent().unwrap().parent().unwrap().file_name().unwrap(), "myproject.worktrees");
        assert!(at.ends_with("feature/login"), "枝の名前がそのまま入れ子になる: {at:?}");
        assert!(at.starts_with(main.parent().unwrap()), "本体の隣に置く");
        // Two branches that differ only in shape never want the same folder
        assert_ne!(folder_for(&main, "feature/login"), folder_for(&main, "feature-login"));
    }

    #[test]
    fn a_folder_that_cannot_be_written_sends_it_somewhere_it_can() {
        let main = PathBuf::from("Z:/nowhere/myproject");
        let at = folder_for(&main, "fix/crash");
        assert!(at.starts_with(away_from_home()), "書けない場所の隣には置かない: {at:?}");
        assert!(at.ends_with("fix/crash"));
    }

    #[test]
    fn what_will_run_is_one_line_and_one_source() {
        let plan = Plan {
            main: PathBuf::from("D:/work/myproject"),
            branch: "feature/login".into(),
            folder: PathBuf::from("D:/work/myproject.worktrees/feature/login"),
            base: "origin/main".into(),
            fresh: true,
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

    /// The whole of it, against a real repository.
    ///
    /// Everything else here is arithmetic on paths and strings; this is the one
    /// that proves the folder comes out on its own branch, belonging to the
    /// same project, with the note about where it came from actually written.
    #[test]
    fn a_branch_really_gets_its_own_folder() {
        let main = std::env::temp_dir().join("shikisha-wt-real").join("myproject");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&main)
                .args(args)
                .output()
                .expect("git が要る");
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
        assert_eq!(cut.base, "HEAD", "remote が無ければ今いる所から");
        create(&cut).unwrap();

        let made = &cut.folder;
        assert!(made.join("readme.md").exists(), "中身が入っている");
        assert_eq!(crate::repo::branch_of(made).as_deref(), Some("feature/login"));
        assert_eq!(crate::repo::family_of(made), crate::repo::family_of(&main), "同じ家族");
        assert!(crate::repo::is_linked(made), "本体から切った枝である");
        assert_eq!(crate::repo::main_checkout(made).as_deref(), Some(main.as_path()));

        // Where it grew from, written into the repository itself
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(made)
            .args(["config", "--get", "branch.feature/login.shikishaBase"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "HEAD");

        // Asking for the same branch twice does not quietly make a second one
        assert!(create(&cut).is_err(), "同じ場所に二度作らない");
        // And a branch that now exists is checked out rather than created
        let again = plan(&main, "feature/login", None).unwrap();
        assert!(!again.fresh, "既にある枝は作り直さない");

        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    /// What a fresh folder is missing, and getting it there.
    #[test]
    fn what_git_does_not_carry_can_be_brought_along() {
        let main = std::env::temp_dir().join("shikisha-wt-carry").join("myproject");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&main)
                .args(args)
                .output()
                .expect("git が要る");
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
        let _ = std::process::Command::new("cmd")
            .args(["/c", "rmdir"])
            .arg(cut.folder.join("node_modules"))
            .status();
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    /// Throwing a branch's folder away, and refusing to.
    #[test]
    fn a_folder_with_work_in_it_is_not_thrown_away() {
        let main = std::env::temp_dir().join("shikisha-wt-discard").join("myproject");
        let _ = std::fs::remove_dir_all(main.parent().unwrap());
        std::fs::create_dir_all(&main).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&main)
                .args(args)
                .output()
                .expect("git が要る");
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
        let listed = std::process::Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&listed.stdout).contains("feature/gone"),
            "git がまだ持っている"
        );
        // Asking again is not an error: it is already how it was asked to be
        discard(&cut.folder).unwrap();

        let _ = std::fs::remove_dir_all(main.parent().unwrap());
    }

    #[test]
    fn a_new_branch_grows_from_what_the_remote_calls_its_default() {
        let main = repo("base");
        let git = main.join(".git");
        // Nothing known yet: whatever is checked out is all there is
        assert_eq!(default_base(&main), "HEAD");
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

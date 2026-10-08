//! The contents a person is about to lose when removing a working folder.
//! A review belongs to one machine, folder and version of its changes.

use anyhow::{Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Serialize)]
pub struct File {
    pub path: String,
    pub untracked: bool,
    pub staged: bool,
    pub work: bool,
}

#[derive(Serialize)]
pub struct Review {
    pub files: Vec<File>,
    pub branch: String,
    pub review: String,
    pub keeps_branch: bool,
    pub blocked: String,
}

pub(super) fn changes(folder: &Path, local: bool) -> Result<Vec<crate::git::Change>> {
    // No conflict-marker reads are needed to decide whether a file is lost.
    // Without optional locks: status may otherwise rewrite the index, and an
    // AI running git in the same folder meets index.lock
    let status = crate::git::run(folder, &["--no-optional-locks", "status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
    Ok(crate::git::read_status(&status).into_iter().filter(|c| {
        !(local && c.index == '?' && std::fs::symlink_metadata(folder.join(&c.path))
            .is_ok_and(|m| m.file_type().is_symlink()))
    }).collect())
}

pub fn inspect(folder: &Path, host: Option<&crate::config::HostSpec>) -> Result<Review> {
    let remote = host.map(crate::elsewhere::Elsewhere::of).transpose()?;
    crate::git::there(folder, remote.as_ref());
    let machine = host.is_some_and(|h| h.is_made());
    if !machine {
        let linked = if host.is_some() {
            let own = crate::git::run(folder, &["rev-parse", "--path-format=absolute", "--git-dir"])?;
            let common = crate::git::run(folder, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
            own.trim() != common.trim()
        } else {
            crate::repo::is_linked(folder)
        };
        if !linked { bail!(crate::i18n::t("err.worktree.not_a_branch")); }
    }
    let changes = changes(folder, host.is_none())?;
    let branch = crate::git::branch(folder)?.unwrap_or_default();
    let mut hash = Sha256::new();
    hash.update(crate::uistate::place_key(host.map(|h| h.name.as_str()), folder));
    hash.update(host.and_then(|h| h.instance.as_deref()).unwrap_or_default());
    hash.update(crate::git::run(folder, &["rev-parse", "HEAD"])?);
    hash.update(&branch);
    for c in &changes {
        hash.update(serde_json::to_vec(&(c.index, c.work, &c.path, &c.from))?);
    }
    if !changes.is_empty() {
        // Include both sides of the index: staging must not change what a
        // previous confirmation means, nor hide newly edited file contents.
        hash.update(crate::git::run_bytes(folder, &["diff", "--no-ext-diff", "--no-textconv", "--binary", "HEAD", "--"], b"", crate::git::LIMIT)?);
        hash.update(crate::git::run_bytes(folder, &["diff", "--no-ext-diff", "--no-textconv", "--binary", "--cached", "--"], b"", crate::git::LIMIT)?);
        let paths: Vec<_> = changes.iter().filter(|c| c.index == '?').map(|c| serde_json::to_string(&c.path)).collect::<Result<_, _>>()?;
        if !paths.is_empty() {
            // Git's quoted stdin paths preserve spaces, quotes and newlines.
            // hash-object without -w reads files without creating objects.
            hash.update(crate::git::run_stdin(folder, &["hash-object", "--no-filters", "--stdin-paths"], &(paths.join("\n") + "\n"), crate::git::LIMIT)?);
        }
    }
    let blocked = if machine {
        // A whole machine also loses its repository. Dirty-file consent must
        // never double as permission to lose commits that exist only there.
        let count = crate::git::run(folder, &["rev-list", "--count", "@{u}..HEAD"])
            .or_else(|_| crate::git::run(folder, &["rev-list", "--count", "HEAD", "--not", "--remotes"]))?;
        let count: usize = count.trim().parse()?;
        if count > 0 { crate::i18n::tp("err.worktree.unpushed", &[("count", &count.to_string())]) } else { String::new() }
    } else { String::new() };
    Ok(Review {
        files: changes.into_iter().map(|c| File { untracked: c.index == '?', staged: !matches!(c.index, ' ' | '?'), work: c.work != ' ', path: c.path }).collect(),
        keeps_branch: !machine && !branch.is_empty(), branch,
        review: hash.finalize().iter().map(|b| format!("{b:02x}")).collect(), blocked,
    })
}

/// Returns whether this exact set of changes was explicitly approved.
pub fn check(folder: &Path, host: Option<&crate::config::HostSpec>, approved: Option<&str>) -> Result<bool> {
    let current = inspect(folder, host)?;
    if !current.blocked.is_empty() { return Err(super::Refused(current.blocked).into()); }
    if let Some(approved) = approved {
        if approved != current.review { return Err(super::Refused(crate::i18n::t("err.worktree.review_changed")).into()); }
        return Ok(!current.files.is_empty());
    }
    if !current.files.is_empty() {
        return Err(super::Refused(crate::i18n::tp("err.worktree.dirty", &[("count", &current.files.len().to_string())])).into());
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Repo { root: PathBuf, main: PathBuf, work: PathBuf }
    impl Repo {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("shikisha-remove-{}", crate::random_hex(12)));
            std::fs::create_dir(&root).unwrap();
            let main = root.join("main");
            std::fs::create_dir(&main).unwrap();
            for args in [vec!["init", "-q", "-b", "main"], vec!["config", "user.name", "Removal test"], vec!["config", "user.email", "removal@example.invalid"]] {
                crate::git::run(&main, &args).unwrap();
            }
            std::fs::write(main.join("kept.txt"), "first\n").unwrap();
            crate::git::run(&main, &["add", "."]).unwrap();
            crate::git::run(&main, &["commit", "-qm", "initial"]).unwrap();
            let work = root.join("work");
            crate::git::run(&main, &["worktree", "add", "-b", "topic", work.to_str().unwrap()]).unwrap();
            crate::git::record_base(&work, "topic", "main").unwrap();
            Self { root, main, work }
        }
        fn write(&self, path: &str, body: &str) { std::fs::write(self.work.join(path), body).unwrap(); }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            assert!(self.root.starts_with(std::env::temp_dir()));
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn reviewed_removal_discards_files_and_keeps_unpublished_history() {
        let r = Repo::new();
        r.write("kept.txt", "committed only on topic\n");
        crate::git::run(&r.work, &["commit", "-qam", "only here"]).unwrap();
        let head = crate::git::run(&r.work, &["rev-parse", "HEAD"]).unwrap();
        r.write("kept.txt", "staged\n");
        crate::git::run(&r.work, &["add", "kept.txt"]).unwrap();
        r.write("kept.txt", "working\n");
        r.write("新しい file.txt", "untracked\n");
        let review = inspect(&r.work, None).unwrap();
        assert_eq!(review.files.len(), 2);
        assert!(review.keeps_branch);
        assert!(review.files.iter().any(|f| f.staged && f.work));
        assert!(super::super::discard(&r.work).is_err());
        assert!(r.work.join("新しい file.txt").exists());
        super::super::discard_step(&r.work, &mut false, Some(&review.review)).unwrap();
        assert!(!r.work.exists());
        assert_eq!(crate::git::run(&r.main, &["rev-parse", "topic"]).unwrap(), head);
        assert!(r.main.join("kept.txt").exists());
    }

    #[test]
    fn reviewed_removal_rejects_new_work_and_reused_approvals() {
        let r = Repo::new();
        let clean = inspect(&r.work, None).unwrap();
        r.write("new.txt", "one\n");
        assert!(check(&r.work, None, Some(&clean.review)).is_err());
        let first = inspect(&r.work, None).unwrap();
        r.write("new.txt", "two\n");
        assert!(check(&r.work, None, Some(&first.review)).is_err(), "same path, different contents");
        let latest = inspect(&r.work, None).unwrap();
        assert!(check(&r.work, None, Some(&latest.review)).unwrap());
        assert!(check(&r.main, None, Some(&latest.review)).is_err(), "main is never a removal target");
        let second = r.root.join("second");
        crate::git::run(&r.main, &["worktree", "add", "-b", "second", second.to_str().unwrap()]).unwrap();
        std::fs::write(second.join("new.txt"), "two\n").unwrap();
        assert!(check(&second, None, Some(&latest.review)).is_err(), "review belongs to another folder");
    }

    #[test]
    fn reviewed_removal_rechecks_staged_and_working_contents() {
        let r = Repo::new();
        r.write("kept.txt", "staged\n");
        crate::git::run(&r.work, &["add", "kept.txt"]).unwrap();
        r.write("kept.txt", "working\n");
        let first = inspect(&r.work, None).unwrap();
        r.write("kept.txt", "changed after question\n");
        assert!(check(&r.work, None, Some(&first.review)).is_err());
        r.write("kept.txt", "working\n");
        crate::git::run(&r.work, &["add", "kept.txt"]).unwrap();
        assert!(check(&r.work, None, Some(&first.review)).is_err(), "staging altered the reviewed index");
    }

    #[test]
    fn integration_follows_current_head_even_after_squash_merge() {
        let r = Repo::new();
        assert_eq!(crate::git::integrated_into(&r.work, "topic").as_deref(), Some("main"));
        r.write("kept.txt", "feature\n");
        crate::git::run(&r.work, &["commit", "-qam", "feature"]).unwrap();
        assert_eq!(crate::git::integrated_into(&r.work, "topic"), None);
        crate::git::run(&r.main, &["merge", "--squash", "topic"]).unwrap();
        crate::git::run(&r.main, &["commit", "-qm", "integrate feature"]).unwrap();
        assert_eq!(crate::git::integrated_into(&r.work, "topic").as_deref(), Some("main"));
        r.write("later.txt", "new work\n");
        crate::git::run(&r.work, &["add", "."]).unwrap();
        crate::git::run(&r.work, &["commit", "-qm", "after merge"]).unwrap();
        assert_eq!(crate::git::integrated_into(&r.work, "topic"), None);
    }
}

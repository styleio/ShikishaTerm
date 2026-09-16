//! What a command about a file on another machine means -- once.
//!
//! Two callers ask for one: a script, through `shikisha.sftp_*`, and the file
//! panel. They have to mean the same thing by it, and the only way to be sure
//! of that is for there to be one of it. They did not, and the difference was
//! not a detail: the panel kept a transfer inside the folder its tab works in,
//! and a script could name any path on this machine -- so `sftp_put` followed
//! by `sftp_read` read a file that `read_path` exists to keep an AI away from.
//!
//! So the fence, the permission and the meaning of each job live here, and
//! both callers come through. What stays with each of them is their own
//! vocabulary: the panel turns a pressed button into a job, a script turns a
//! written command into one. The job itself means one thing.

use crate::grants::Subject;
use crate::ssh::{FileAnswer, FileJob};
use anyhow::{Result, bail};
use std::path::PathBuf;

/// How far a command may reach, on each machine.
///
/// A fence that is not set is not a fence: a panel with no working folder, or
/// a server nobody gave a starting folder, is reached the way it always was.
/// Saying "none" rather than guessing a root keeps the promise honest -- what
/// is fenced is what somebody wrote down
#[derive(Debug, Clone, Default)]
pub struct Fences {
    /// The folder on this machine a command may touch
    pub here: Option<PathBuf>,
    /// The folder on the far end it may touch. Empty means wherever signing in
    /// puts you, which is not a place this program can fence
    pub there: String,
}

/// The name a job asks permission under.
///
/// Read off the job rather than passed beside it. Told twice, the two would one
/// day disagree, and a job running under somebody else's permission is the one
/// mistake this table exists to prevent
pub fn grant_of(job: &FileJob) -> &'static str {
    match job {
        FileJob::List { .. } => "sftp_ls",
        FileJob::Stat { .. } => "sftp_stat",
        FileJob::Get { .. } => "sftp_get",
        FileJob::Read { .. } => "sftp_read",
        FileJob::Put { .. } => "sftp_put",
        FileJob::MakeDir { .. } => "sftp_mkdir",
        FileJob::Rename { .. } => "sftp_rename",
        FileJob::Remove { .. } => "sftp_rm",
    }
}

/// The same path on the far end, refused if it is not inside the fence.
///
/// Worked out from the text, because the far end is not here to ask. `..` is
/// taken out the way a shell would take it out, and what is left has to still
/// be under the root -- so a path cannot climb out of the folder its tab was
/// given by writing enough of them
pub fn under_remote(root: &str, path: &str) -> Option<String> {
    let root = root.trim().trim_end_matches('/');
    // Nothing was written down, so there is nothing to be outside of
    if root.is_empty() || root == "." {
        return Some(path.to_string());
    }
    let tidy = |p: &str| {
        let absolute = p.starts_with('/');
        let mut out: Vec<&str> = Vec::new();
        for part in p.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    out.pop();
                }
                one => out.push(one),
            }
        }
        format!("{}{}", if absolute { "/" } else { "" }, out.join("/"))
    };
    let base = tidy(root);
    let full = tidy(&if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{root}/{path}")
    });
    (full == base || full.starts_with(&format!("{base}/"))).then_some(full)
}

/// The same job, with every path it names settled against the fences.
///
/// Returned rather than merely checked: what comes back is what runs, so there
/// is no second reading of the path between the checking and the doing
pub fn inside(job: FileJob, fences: &Fences) -> Result<FileJob> {
    let outside = || anyhow::anyhow!(crate::i18n::t("err.sftp.outside"));
    let there = |p: &str| under_remote(&fences.there, p).ok_or_else(outside);
    let here = |p: PathBuf| match &fences.here {
        None => Ok(p),
        Some(root) => {
            crate::runtime::local_under(root, &p.display().to_string()).ok_or_else(outside)
        }
    };
    Ok(match job {
        FileJob::List { path } => FileJob::List { path: there(&path)? },
        FileJob::Stat { path } => FileJob::Stat { path: there(&path)? },
        FileJob::Read { path } => FileJob::Read { path: there(&path)? },
        FileJob::MakeDir { path } => FileJob::MakeDir { path: there(&path)? },
        FileJob::Remove { path } => FileJob::Remove { path: there(&path)? },
        FileJob::Rename { from, to } => {
            FileJob::Rename { from: there(&from)?, to: there(&to)? }
        }
        FileJob::Get { from, to, overwrite } => {
            FileJob::Get { from: there(&from)?, to: here(to)?, overwrite }
        }
        FileJob::Put { from, to, overwrite } => {
            FileJob::Put { from: here(from)?, to: there(&to)?, overwrite }
        }
    })
}

/// Settled against the fences and allowed, and so ready to be run -- here, or
/// on a thread that will not hold up the window.
///
/// Both refusals happen before anything is asked of a machine, because a
/// refusal that costs nothing should arrive before one that costs a connection
pub fn ready(
    job: FileJob,
    fences: &Fences,
    caps: &crate::caps::Capabilities,
    who: Subject,
) -> Result<FileJob> {
    let name = grant_of(&job);
    let job = inside(job, fences)?;
    if !caps.allows(name, who) {
        bail!(crate::i18n::tp(
            "err.hooks.not_permitted",
            &[
                ("name", name),
                (
                    "who",
                    &crate::i18n::t(match who {
                        Subject::Ai => "grant.who.ai",
                        _ => "grant.who.human",
                    })
                )
            ],
        ));
    }
    Ok(job)
}

/// ...and run, for a caller that can wait where it stands. A caller that
/// cannot takes `ready` and carries what it gets to its own thread -- the same
/// job, settled the same way, because there is only one of this
pub fn run(
    at: &crate::elsewhere::Elsewhere,
    job: FileJob,
    fences: &Fences,
    caps: &crate::caps::Capabilities,
    who: Subject,
    wait_ms: u64,
) -> Result<FileAnswer> {
    crate::elsewhere::files(at, ready(job, fences, caps, who)?, wait_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fences(there: &str) -> Fences {
        Fences { here: None, there: there.into() }
    }

    #[test]
    fn a_job_asks_under_its_own_name() {
        assert_eq!(grant_of(&FileJob::Remove { path: "x".into() }), "sftp_rm");
        assert_eq!(grant_of(&FileJob::Read { path: "x".into() }), "sftp_read");
        assert_eq!(
            grant_of(&FileJob::Put { from: "a".into(), to: "b".into(), overwrite: false }),
            "sftp_put"
        );
    }

    #[test]
    fn a_path_under_the_root_is_settled_against_it() {
        assert_eq!(under_remote("/var/www/site", "public/a.txt").as_deref(),
                   Some("/var/www/site/public/a.txt"));
        assert_eq!(under_remote("/var/www/site", "/var/www/site/a.txt").as_deref(),
                   Some("/var/www/site/a.txt"));
        assert_eq!(under_remote("/var/www/site", "").as_deref(), Some("/var/www/site"));
    }

    #[test]
    fn a_path_that_climbs_out_is_refused() {
        assert_eq!(under_remote("/var/www/site", "../../../etc/passwd"), None);
        assert_eq!(under_remote("/var/www/site", "/etc/passwd"), None);
        // The name next door starts with the same letters and is still next door
        assert_eq!(under_remote("/var/www/site", "/var/www/site2/a.txt"), None);
        // Enough of them to land back inside is still inside
        assert_eq!(under_remote("/var/www/site", "a/../b.txt").as_deref(),
                   Some("/var/www/site/b.txt"));
    }

    /// A server nobody gave a starting folder is reached the way it always was
    #[test]
    fn no_root_written_down_is_no_fence() {
        assert_eq!(under_remote("", "/etc/passwd").as_deref(), Some("/etc/passwd"));
        assert_eq!(under_remote(".", "/etc/passwd").as_deref(), Some("/etc/passwd"));
    }

    #[test]
    fn a_fenced_job_carries_the_settled_paths() {
        let job = FileJob::Rename { from: "a.txt".into(), to: "b/c.txt".into() };
        match inside(job, &fences("/srv/app")).unwrap() {
            FileJob::Rename { from, to } => {
                assert_eq!(from, "/srv/app/a.txt");
                assert_eq!(to, "/srv/app/b/c.txt");
            }
            other => panic!("a rename stayed a rename: {other:?}"),
        }
        assert!(inside(FileJob::Remove { path: "../../etc/x".into() }, &fences("/srv/app")).is_err());
    }

    /// The half that was the hole: a script naming a file this tab was never
    /// given, to send somewhere it can then read it back from
    #[test]
    fn a_file_outside_this_machines_folder_is_refused() {
        let root = std::env::temp_dir().join("shikisha-fence");
        let fences = Fences { here: Some(root.clone()), there: String::new() };
        let ok = FileJob::Put {
            from: root.join("dist/a.txt"),
            to: "public/a.txt".into(),
            overwrite: false,
        };
        assert!(inside(ok, &fences).is_ok());
        let out = FileJob::Put {
            from: PathBuf::from("C:/Users/someone/.ssh/id_rsa"),
            to: "tmp/x".into(),
            overwrite: false,
        };
        assert!(inside(out, &fences).is_err(), "a path outside the folder is refused");
        // ...and the same coming back, which would write over it instead
        let back = FileJob::Get {
            from: "tmp/x".into(),
            to: PathBuf::from("C:/Windows/System32/drivers/etc/hosts"),
            overwrite: true,
        };
        assert!(inside(back, &fences).is_err());
    }
}

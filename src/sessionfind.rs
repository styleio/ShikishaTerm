//! Reading back which conversation a CLI started, from the CLI's own records.
//!
//! The last resort of the three ways to know. A CLI that takes a conversation
//! id at launch is told ours, and one that can report its own is asked to —
//! both leave nothing to work out. This is for the rest: the CLI picked a
//! conversation, wrote it down somewhere, and never said which.
//!
//! What is actually being solved is attribution, not reading. Every one of
//! these records says which *folder* it belongs to, and none says which *tab*:
//! two agents of the same kind in one folder produce two records that look
//! alike. So the rule is narrow on purpose:
//!
//!   - the record has to have appeared **after that tab was launched**, and
//!   - **no other tab could have written it** — same program, same folder
//!
//! When both hold, the newest match is that tab's conversation. When they do
//! not, nothing is claimed. A tab that comes back with somebody else's
//! conversation is a worse outcome than one that comes back empty.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::profile::RecordSpec;

/// The conversation a CLI most recently started in `cwd`, if it can only have
/// been the one we are asking about.
///
/// `since` is when the tab was launched; records older than that belong to
/// earlier runs, whoever made them.
pub fn find(spec: &RecordSpec, cwd: Option<&Path>, since: SystemTime) -> Option<String> {
    let dir = expand(&spec.dir);
    let cwd = cwd?;
    let mut best: Option<(SystemTime, String)> = None;
    for file in walk(&dir, 0, since) {
        let Ok(meta) = std::fs::metadata(&file) else {
            continue;
        };
        let Ok(made) = meta.modified() else { continue };
        if made < since {
            continue;
        }
        if best.as_ref().is_some_and(|(t, _)| *t >= made) {
            continue;
        }
        let Some((id, at)) = first_line_fields(&file, spec) else {
            continue;
        };
        // Same folder, written the way that CLI writes it — compared as paths
        // so that D:\Test and D:/Test are the one folder they plainly are
        if Path::new(&at) != cwd {
            continue;
        }
        best = Some((made, id));
    }
    best.map(|(_, id)| id)
}

/// The two fields we need, from the first line of a record.
///
/// The first line is where these CLIs put what the conversation IS, before any
/// of what was said in it. Reading one line also means a record of any size
/// costs the same to inspect
fn first_line_fields(file: &Path, spec: &RecordSpec) -> Option<(String, String)> {
    use std::io::BufRead as _;
    let f = std::fs::File::open(file).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(f).read_line(&mut line).ok()?;
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    let id = dotted(&v, &spec.id)?.as_str()?.to_string();
    let cwd = dotted(&v, &spec.cwd)?.as_str()?.to_string();
    (!id.is_empty()).then_some((id, cwd))
}

/// `payload.session_id` — a path a profile can write without knowing JSON
/// pointer syntax
fn dotted<'a>(v: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut at = v;
    for step in path.split('.') {
        at = at.get(step)?;
    }
    Some(at)
}

/// Every file under a folder, and only the files new enough to be worth a look.
///
/// Depth is bounded because following a tree forever is how a folder that turns
/// out to be a link to somewhere enormous hangs the app.
///
/// **Folders are not skipped by their age**, tempting as it is: adding a file
/// to `2026/08/25` updates that folder's time and none of its parents, so
/// `2026/08` still says "last touched when the 25th was created" and pruning on
/// that would skip today's records entirely. Listing a folder is cheap; it is
/// opening and parsing files that is not, and the age test on files spares that.
fn walk(dir: &Path, depth: usize, since: SystemTime) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if depth > 6 {
        return out;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => out.extend(walk(&p, depth + 1, since)),
            Ok(t) if t.is_file() => {
                let fresh = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .map(|m| m >= since)
                    .unwrap_or(true);
                if fresh {
                    out.push(p);
                }
            }
            _ => {}
        }
    }
    out
}

fn expand(path: &str) -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    PathBuf::from(path.replace("{home}", &home))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn spec(dir: &Path) -> RecordSpec {
        RecordSpec {
            dir: dir.display().to_string(),
            id: "payload.session_id".into(),
            cwd: "payload.cwd".into(),
        }
    }

    fn record(dir: &Path, name: &str, id: &str, cwd: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        let line = serde_json::json!({
            "type": "session_meta",
            "payload": { "session_id": id, "cwd": cwd }
        });
        std::fs::write(&p, format!("{line}\n{{\"more\":\"said later\"}}\n")).unwrap();
        p
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("shikisha-find-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_newest_record_for_this_folder_is_the_one() {
        let root = tmp("newest");
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let since = SystemTime::now() - Duration::from_secs(60);
        // Filed by date, the way these CLIs file them
        record(&root.join("2026/08/24"), "a.jsonl", "old-one", &work.display().to_string());
        std::thread::sleep(Duration::from_millis(20));
        record(&root.join("2026/08/25"), "b.jsonl", "the-one", &work.display().to_string());
        assert_eq!(find(&spec(&root), Some(&work), since).as_deref(), Some("the-one"));
    }

    #[test]
    fn a_new_record_inside_an_old_folder_tree_is_still_found() {
        // The trap this walk exists to avoid: adding today's record updates
        // today's folder and none of its parents, so a tree pruned by folder
        // age would skip the very record being looked for
        let root = tmp("prune");
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let day = root.join("2026/08/25");
        record(&day, "old.jsonl", "yesterday", &work.display().to_string());
        // Make every folder in the tree look untouched for a while
        let long_ago = SystemTime::now() + Duration::from_secs(3600);
        std::thread::sleep(Duration::from_millis(20));
        record(&day, "new.jsonl", "todays", &work.display().to_string());
        // A tab that started a moment ago finds the new one...
        let since = SystemTime::now() - Duration::from_millis(15);
        assert_eq!(find(&spec(&root), Some(&work), since).as_deref(), Some("todays"));
        // ...and one that starts in an hour finds neither
        assert_eq!(find(&spec(&root), Some(&work), long_ago), None);
    }

    #[test]
    fn a_record_from_another_folder_is_not_ours() {
        let root = tmp("elsewhere");
        let mine = root.join("mine");
        std::fs::create_dir_all(&mine).unwrap();
        record(&root.join("d"), "x.jsonl", "theirs", &root.join("theirs").display().to_string());
        let since = SystemTime::now() - Duration::from_secs(60);
        assert_eq!(find(&spec(&root), Some(&mine), since), None);
    }

    #[test]
    fn a_record_written_before_the_tab_started_is_a_previous_run() {
        let root = tmp("older");
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        record(&root.join("d"), "x.jsonl", "yesterday", &work.display().to_string());
        // The tab starts now; everything already on disk belongs to before
        let since = SystemTime::now() + Duration::from_secs(5);
        assert_eq!(find(&spec(&root), Some(&work), since), None);
    }

    #[test]
    fn the_same_folder_written_the_other_way_still_matches() {
        // A CLI writes what it was given; a config may say D:/Test where
        // Windows would say D:\Test. They are one folder and must compare so
        let root = tmp("slashes");
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let other_way = work.display().to_string().replace('\\', "/");
        record(&root.join("d"), "x.jsonl", "same-place", &other_way);
        let since = SystemTime::now() - Duration::from_secs(60);
        assert_eq!(find(&spec(&root), Some(&work), since).as_deref(), Some("same-place"));
    }
}

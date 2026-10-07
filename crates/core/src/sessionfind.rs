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
        // Same folder, written the way that CLI writes it
        if !same_folder(Path::new(&at), cwd) {
            continue;
        }
        best = Some((made, id));
    }
    best.map(|(_, id)| id)
}

/// The folders the fresh records claim, newest first — for saying out loud why
/// none of them was this tab's.
///
/// What fails here is attribution, and it fails without a sound: the app reads
/// real records, rejects every one, and starts the tab on a fresh conversation
/// as though it had never looked. The one fact that settles it is what folder
/// those records say they belong to, next to the folder the tab is in.
pub fn folders_seen(spec: &RecordSpec, since: SystemTime, most: usize) -> Vec<String> {
    let mut files: Vec<(SystemTime, PathBuf)> = walk(&expand(&spec.dir), 0, since)
        .into_iter()
        .filter_map(|f| {
            let made = std::fs::metadata(&f).ok()?.modified().ok()?;
            (made >= since).then_some((made, f))
        })
        .collect();
    files.sort_by_key(|(made, _)| std::cmp::Reverse(*made));
    let mut out: Vec<String> = Vec::new();
    for (_, f) in files {
        if out.len() >= most {
            break;
        }
        let Some((_, at)) = first_line_fields(&f, spec) else {
            continue;
        };
        if !out.contains(&at) {
            out.push(at);
        }
    }
    out
}

pub use crate::same_folder;

/// Whether the record of one conversation is still there.
///
/// Asked before resuming, because a CLI handed an id it has never heard of
/// says so in its own words, in its own place, and the person is left staring
/// at a red line with no idea that the app could have told them plainly.
pub fn exists(pattern: &str, id: &str) -> bool {
    locate(pattern, id).is_some()
}

/// A record's pattern with one conversation's id put in.
///
/// `{id}` is the whole id. `{id:8}` is its first eight characters: a CLI that
/// names the file by the head of the id (`session-…-e1e8d040.jsonl`) still
/// names one conversation, and the pattern says how much of the id to look for
/// rather than every reader of it knowing that CLI's habit
pub fn fill_id(pattern: &str, id: &str) -> String {
    fill(pattern, |n| match n {
        Some(n) => id.chars().take(n).collect(),
        None => id.to_string(),
    })
}

/// The same pattern with any id in it: `*`, for listing every record it names
pub fn any_id(pattern: &str) -> String {
    fill(pattern, |_| "*".to_string())
}

/// Every `{id}` and `{id:N}` in `pattern`, replaced by what `with` makes of N
fn fill(pattern: &str, with: impl Fn(Option<usize>) -> String) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(at) = rest.find("{id") {
        let after = &rest[at + 3..];
        let taken = match after.find('}') {
            Some(0) => Some((None, 1)),
            Some(end) => after[..end]
                .strip_prefix(':')
                .and_then(|n| n.parse::<usize>().ok())
                .map(|n| (Some(n), end + 1)),
            None => None,
        };
        match taken {
            Some((n, len)) => {
                out.push_str(&rest[..at]);
                out.push_str(&with(n));
                rest = &after[len..];
            }
            // Not a placeholder, only a name that happens to start the same way
            None => {
                out.push_str(&rest[..at + 3]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Where that record actually is.
///
/// `pattern` is a path with `{id}` (or `{id:N}`, see [`fill_id`]) in it and
/// `*` standing for any run of characters within one name —
/// `{home}/.claude/projects/*/{id}.jsonl`, or `…/rollout-*-{id}.jsonl` where
/// the id is only part of the file name.
///
/// The walk that answers "is it still there" already held the path and threw
/// it away. It is handed back now because reading a conversation means opening
/// that file, and a second walk written beside this one would be a second
/// place for the pattern rules to drift.
pub fn locate(pattern: &str, id: &str) -> Option<PathBuf> {
    if id.is_empty() {
        return None;
    }
    // A profile's pattern is written with whichever separator its author had
    // in mind. Windows takes either, so both are folded to one there; anywhere
    // else a backslash is an ordinary character in a name and folding it would
    // cut the path in the wrong place
    let sep = std::path::MAIN_SEPARATOR;
    let named = expand(&fill_id(pattern, id)).display().to_string();
    let full = match cfg!(windows) {
        true => named.replace('/', "\\"),
        false => named,
    };
    // Everything up to the first wildcard is a plain path and can be joined in
    // one step; only from there does anything have to be listed
    let (fixed, rest) = match full.find('*') {
        None => {
            let one = PathBuf::from(&full);
            return one.exists().then_some(one);
        }
        Some(at) => {
            let cut = full[..at].rfind(sep).map(|i| i + 1).unwrap_or(0);
            (full[..cut].to_string(), full[cut..].to_string())
        }
    };
    let steps: Vec<String> = rest.split(sep).map(str::to_string).collect();
    walk_glob(PathBuf::from(fixed), &steps)
}

fn walk_glob(at: PathBuf, steps: &[String]) -> Option<PathBuf> {
    let Some(step) = steps.first() else {
        return at.exists().then_some(at);
    };
    if !step.contains('*') {
        return walk_glob(at.join(step), &steps[1..]);
    }
    let entries = std::fs::read_dir(&at).ok()?;
    entries.flatten().find_map(|e| {
        let name = e.file_name().to_string_lossy().to_string();
        name_matches(step, &name)
            .then(|| walk_glob(e.path(), &steps[1..]))
            .flatten()
    })
}

/// One name against one pattern. `*` is any run of characters, and the pieces
/// between the stars have to appear in order — enough for "a file whose name
/// ends with this id", which is the only thing this is asked
fn name_matches(pattern: &str, name: &str) -> bool {
    let pieces: Vec<&str> = pattern.split('*').collect();
    if pieces.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (pieces[0], pieces[pieces.len() - 1]);
    if !name.starts_with(first) || !name.ends_with(last) || name.len() < first.len() + last.len() {
        return false;
    }
    let mut at = first.len();
    for piece in &pieces[1..pieces.len() - 1] {
        match name[at..].find(piece) {
            Some(found) => at += found + piece.len(),
            None => return false,
        }
    }
    at <= name.len() - last.len()
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

use crate::with_home as expand;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn spec(dir: &Path) -> RecordSpec {
        RecordSpec {
            dir: dir.display().to_string(),
            id: "payload.session_id".into(),
            cwd: "payload.cwd".into(),
            turns: false,
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

    /// The same folder, spelled the several ways Windows allows.
    ///
    /// Compared as written, a tab whose folder was typed `D:/Simic2` never
    /// matched the `D:\Simic2` its CLI wrote down the moment the disk held a
    /// different case, and the conversation was silently never found.
    #[test]
    #[cfg(windows)]
    fn the_same_folder_spelled_differently_is_the_same_folder() {
        let same = |a: &str, b: &str| same_folder(Path::new(a), Path::new(b));
        assert!(same(r"D:\Simic2", "D:/Simic2"), "the direction of the slashes does not matter");
        assert!(same(r"D:\simic2", "D:/Simic2"), "case does not matter");
        assert!(same(r"d:\Simic2", "D:/Simic2"), "the drive letter is the same too");
        assert!(same(r"D:\Simic2\", "D:/Simic2"), "a trailing separator does not matter");
        assert!(!same(r"D:\Simic2", "D:/Simic"), "a different folder is a different folder");
        assert!(!same(r"D:\Simic2", r"C:\Simic2"), "a different drive means different");
        assert!(!same(r"D:\a\Simic2", r"D:\Simic2"), "a different depth means different");
    }

    /// The same promise where the filesystem means what it says: a redundant
    /// spelling is the same folder, and a different case is not.
    #[test]
    #[cfg(unix)]
    fn the_same_folder_spelled_differently_is_the_same_folder_on_unix() {
        let same = |a: &str, b: &str| same_folder(Path::new(a), Path::new(b));
        assert!(same("/home/dev/simic2/", "/home/dev/simic2"), "a trailing separator does not matter");
        assert!(same("/home/dev/./simic2", "/home/dev/simic2"), "putting 'here' in between is the same");
        // Case is the one thing the system decides: a Mac opens either spelling
        assert_eq!(same("/home/dev/Simic2", "/home/dev/simic2"), cfg!(target_os = "macos"), "case");
        assert!(!same(r"/home/dev\simic2", "/home/dev/simic2"), "a backslash is part of a name here");
        assert!(!same("/home/dev/simic2", "/home/dev/simic"), "a different folder is a different folder");
        assert!(!same("/home/dev/a/simic2", "/home/dev/simic2"), "a different depth means different");
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
    fn a_conversation_is_looked_for_where_that_cli_files_it() {
        let root = tmp("verify");
        let day = root.join("2026/08/25");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("rollout-2026-08-25T10-00-00-abc123.jsonl"), "{}").unwrap();
        let by_folder = root.join("proj");
        std::fs::create_dir_all(&by_folder).unwrap();
        std::fs::write(by_folder.join("def456.jsonl"), "{}").unwrap();

        // Named by its file, in a folder we do not know the name of
        let one = format!("{}/*/{{id}}.jsonl", root.display());
        assert!(exists(&one, "def456"));
        assert!(!exists(&one, "not-there"));

        // Named inside a longer file name, several folders down
        let two = format!("{}/*/*/*/rollout-*-{{id}}.jsonl", root.display());
        assert!(exists(&two, "abc123"));
        assert!(!exists(&two, "abc999"));

        // Named by the head of the id only
        let chats = root.join("proj/chats");
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(chats.join("session-2026-09-28T02-10-e1e8d040.jsonl"), "{}").unwrap();
        let three = format!("{}/*/chats/session-*-{{id:8}}.jsonl", root.display());
        assert!(exists(&three, "e1e8d040-0e00-4caf-b275-ab21c423b4cf"));
        assert!(!exists(&three, "e1e8d041-0e00-4caf-b275-ab21c423b4cf"));
    }

    #[test]
    fn an_id_goes_in_whole_or_by_its_head() {
        let id = "e1e8d040-0e00-4caf";
        assert_eq!(fill_id("a/{id}.jsonl", id), "a/e1e8d040-0e00-4caf.jsonl");
        assert_eq!(fill_id("s-*-{id:8}.jsonl", id), "s-*-e1e8d040.jsonl");
        assert_eq!(fill_id("{id:99}", id), id, "a head longer than the id is the id");
        assert_eq!(any_id("x/{id}/s-{id:8}.jsonl"), "x/*/s-*.jsonl");
        // Only the placeholder is replaced, not what merely looks like one
        assert_eq!(fill_id("{idea}/{id:x}/{id", id), "{idea}/{id:x}/{id");
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

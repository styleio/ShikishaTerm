//! What of a pull request has already been handed to an AI tab: the review
//! comments, by GitHub's own id for each, and the failed checks, by the
//! commit they ran on and their names.
//!
//! "Hand the review to the AI" and "fix CI with the AI" are pressed again
//! after the AI has done its part and pushed, when the reviewer says more or
//! a check fails anew. What it was given the first time is not what it is to
//! work on the second: a thread nobody marked resolved on GitHub is still
//! open there, though the AI answered it. Kept here, so only what is new goes
//! the second time -- and kept in a file, so a restart in between does not
//! hand the first round over again.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const FILE: &str = "handed.json";
const VERSION: u32 = 1;

/// How many pull requests are remembered: the ones handed most recently.
/// Far more than anybody has open at once, and few enough that the file
/// stays a few kilobytes
const KEPT: usize = 200;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    prs: BTreeMap<String, Handed>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Handed {
    /// The review comments handed, by GitHub's id
    #[serde(default)]
    comments: Vec<String>,
    /// The commit the failed checks handed ran on, and their names
    #[serde(default)]
    ci_sha: String,
    #[serde(default)]
    ci_checks: Vec<String>,
    /// When it was last written, to keep the newest
    #[serde(default)]
    at: u64,
}

pub fn path() -> PathBuf {
    crate::config::root_dir().join("data").join(FILE)
}

/// What a pull request is known by here: its project and number, or the
/// branch when checks are fixed before there is a pull request
pub fn key(project: &str, number: u64, head: &str) -> String {
    match number {
        0 => format!("{project}@{head}"),
        n => format!("{project}#{n}"),
    }
}

fn read(file: &Path) -> Store {
    std::fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<Store>(t.trim_start_matches('\u{feff}')).ok())
        .filter(|s| s.version <= VERSION)
        .unwrap_or_default()
}

fn write(file: &Path, mut store: Store) {
    if store.prs.len() > KEPT {
        let mut by_age: Vec<(u64, String)> = store.prs.iter().map(|(k, h)| (h.at, k.clone())).collect();
        by_age.sort();
        for (_, k) in by_age.iter().take(store.prs.len() - KEPT) {
            store.prs.remove(k);
        }
    }
    store.version = VERSION;
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&store) {
        let _ = crate::crypto::write_atomic(file, &text);
    }
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The ids of a thread's comments, as [`crate::github::review_threads`] gives them
fn ids_of(thread: &Value) -> Vec<String> {
    thread
        .get("ids")
        .and_then(|i| i.as_array())
        .into_iter()
        .flatten()
        .filter_map(|i| i.as_str().map(str::to_string))
        .collect()
}

/// The open review threads with something in them not handed before -- each
/// whole, so what is new is read with what it answers, its new comments
/// marked `"new": true` -- and how many open threads were all handed before.
/// `queued` is what is on its way to a tab and not yet in: handed already, as
/// far as a second press goes
pub fn new_threads(file: &Path, key: &str, threads: &[Value], queued: &[&Note]) -> (Vec<Value>, usize) {
    let store = read(file);
    let mut known = store.prs.get(key).map(|h| h.comments.clone()).unwrap_or_default();
    for n in queued {
        if let Note::Review { key: k, threads } = n
            && k == key
        {
            known.extend(threads.iter().flat_map(ids_of));
        }
    }
    let mut fresh = Vec::new();
    let mut earlier = 0;
    for t in threads {
        let ids = ids_of(t);
        if ids.iter().all(|i| known.contains(i)) && !ids.is_empty() {
            earlier += 1;
            continue;
        }
        let mut t = t.clone();
        // Marked only where some of the thread was seen: a thread all new
        // needs no marks
        if ids.iter().any(|i| known.contains(i))
            && let Some(said) = t.get_mut("said").and_then(|s| s.as_array_mut())
        {
            for (c, id) in said.iter_mut().zip(&ids) {
                if !known.contains(id) {
                    c["new"] = json!(true);
                }
            }
        }
        fresh.push(t);
    }
    (fresh, earlier)
}

/// Remember the comments of these threads as handed
pub fn note_threads(file: &Path, key: &str, threads: &[Value]) {
    let mut store = read(file);
    let h = store.prs.entry(key.to_string()).or_default();
    for id in threads.iter().flat_map(ids_of) {
        if !h.comments.contains(&id) {
            h.comments.push(id);
        }
    }
    h.at = now();
    write(file, store);
}

fn check_names(failed: &[Value]) -> Vec<String> {
    failed.iter().filter_map(|c| c.get("name").and_then(|n| n.as_str()).map(str::to_string)).collect()
}

/// Whether these failed checks of commit `sha` are other than the ones
/// handed last, or on their way (`queued`): a new commit, or a check that
/// failed since
pub fn ci_is_new(file: &Path, key: &str, sha: &str, failed: &[Value], queued: &[&Note]) -> bool {
    let store = read(file);
    let mut known: Vec<String> = match store.prs.get(key) {
        Some(h) if !sha.is_empty() && h.ci_sha == sha => h.ci_checks.clone(),
        _ => Vec::new(),
    };
    for n in queued {
        if let Note::Ci { key: k, sha: s, checks } = n
            && k == key
            && s == sha
        {
            known.extend(check_names(checks));
        }
    }
    check_names(failed).iter().any(|n| !known.contains(n)) || failed.is_empty()
}

/// Remember these failed checks of commit `sha` as handed
pub fn note_ci(file: &Path, key: &str, sha: &str, failed: &[Value]) {
    let mut store = read(file);
    let h = store.prs.entry(key.to_string()).or_default();
    if h.ci_sha != sha {
        h.ci_sha = sha.to_string();
        h.ci_checks.clear();
    }
    for n in check_names(failed) {
        if !h.ci_checks.contains(&n) {
            h.ci_checks.push(n);
        }
    }
    h.at = now();
    write(file, store);
}

/// What is to be remembered as handed once the words carrying it have gone
/// into their tab: written then, and not when they were queued, so words
/// lost on the way (the app closed while the tab was still busy) are not
/// counted as given
#[derive(Debug, Clone)]
pub enum Note {
    Review { key: String, threads: Vec<Value> },
    Ci { key: String, sha: String, checks: Vec<Value> },
}

impl Note {
    /// The pull request it is about, as [`key`] names it
    pub fn key(&self) -> &str {
        match self {
            Note::Review { key, .. } | Note::Ci { key, .. } => key,
        }
    }

    pub fn record(&self) {
        let file = path();
        match self {
            Note::Review { key, threads } => note_threads(&file, key, threads),
            Note::Ci { key, sha, checks } => note_ci(&file, key, sha, checks),
        }
    }
}

/// The threads as the prompt shows them: without the ids, which mean nothing
/// to the AI
pub fn for_prompt(threads: &[Value]) -> Vec<Value> {
    threads
        .iter()
        .map(|t| {
            let mut t = t.clone();
            if let Some(o) = t.as_object_mut() {
                o.remove("ids");
            }
            t
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread(path: &str, ids: &[&str]) -> Value {
        json!({
            "path": path, "line": 3,
            "said": ids.iter().map(|i| json!({"who": "rev", "said": format!("comment {i}")})).collect::<Vec<_>>(),
            "ids": ids,
        })
    }

    fn book(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shikisha-handed-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(FILE)
    }

    #[test]
    fn only_what_was_not_handed_goes_the_second_time() {
        let file = book("review");
        let k = key("shop", 12, "fix");
        let first = vec![thread("a.rs", &["c1"]), thread("b.rs", &["c2"])];
        let (fresh, earlier) = new_threads(&file, &k, &first, &[]);
        assert_eq!((fresh.len(), earlier), (2, 0));
        // On its way to the tab and not in yet: not handed a second time
        let queued = Note::Review { key: k.clone(), threads: fresh.clone() };
        assert_eq!(new_threads(&file, &k, &first, &[&queued]).0.len(), 0);
        note_threads(&file, &k, &fresh);
        // The reviewer answered on one thread and opened another; neither
        // of the first two was marked resolved
        let second = vec![thread("a.rs", &["c1", "c3"]), thread("b.rs", &["c2"]), thread("c.rs", &["c4"])];
        let (fresh, earlier) = new_threads(&file, &k, &second, &[]);
        assert_eq!(earlier, 1);
        assert_eq!(fresh.len(), 2);
        assert_eq!(fresh[0]["path"], "a.rs");
        // The answer is marked; what it answers is there to read, unmarked
        assert!(fresh[0]["said"][0].get("new").is_none());
        assert_eq!(fresh[0]["said"][1]["new"], true);
        assert!(fresh[1]["said"][0].get("new").is_none());
        assert!(for_prompt(&fresh)[0].get("ids").is_none());
        // Another pull request knows nothing of these
        assert_eq!(new_threads(&file, &key("shop", 13, "other"), &second, &[]).0.len(), 3);
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }

    #[test]
    fn a_check_is_new_on_a_new_commit_or_when_another_fails() {
        let file = book("ci");
        let k = key("shop", 0, "fix");
        assert_eq!(k, "shop@fix");
        let lint = vec![json!({"name": "lint"})];
        assert!(ci_is_new(&file, &k, "aaa", &lint, &[]));
        let queued = Note::Ci { key: k.clone(), sha: "aaa".into(), checks: lint.clone() };
        assert!(!ci_is_new(&file, &k, "aaa", &lint, &[&queued]));
        note_ci(&file, &k, "aaa", &lint);
        assert!(!ci_is_new(&file, &k, "aaa", &lint, &[]));
        assert!(ci_is_new(&file, &k, "aaa", &[json!({"name": "lint"}), json!({"name": "test"})], &[]));
        assert!(ci_is_new(&file, &k, "bbb", &lint, &[]));
        let _ = std::fs::remove_dir_all(file.parent().unwrap());
    }
}

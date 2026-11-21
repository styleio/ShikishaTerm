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
    crate::config::state_path(FILE)
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

/// The most of the review comments or failed checks put into one prompt, in
/// characters. A pull request a bot reviewed every push of holds hundreds of
/// threads, and every one of them was pasted into the AI's terminal; what is
/// past this is named by how many, for the AI to read on GitHub
pub const PROMPT_ROOM: usize = 40_000;

/// The most of one comment shown, in characters
const SAID_MOST: usize = 4_000;

/// How many comments of one thread are shown: its first, which says what
/// the thread is about, and the latest after it, which say where it stands
const SAID_KEPT: usize = 5;

/// What goes into the prompt in place of `{comments}` or `{checks}`: the
/// items as one line of JSON each, review threads shortened ([`for_prompt`],
/// then [`shortened`]), as many as [`PROMPT_ROOM`] holds -- and how many
/// were left out for want of room
pub fn prompt_items(items: &[Value], review: bool) -> (String, usize) {
    let shown: Vec<Value> = match review {
        true => for_prompt(items).iter().map(shortened).collect(),
        false => items.to_vec(),
    };
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0;
    for item in &shown {
        let line = serde_json::to_string(item).unwrap_or_default();
        // The first is always shown, cut or not: a prompt that says only
        // that everything was left out gives the AI nothing to start on
        if !lines.is_empty() && used + line.len() > PROMPT_ROOM {
            break;
        }
        used += line.len() + 1;
        lines.push(line);
    }
    let left_out = shown.len() - lines.len();
    (format!("[\n{}\n]", lines.join(",\n")), left_out)
}

/// One review thread with its long comments cut, and only its first and
/// latest comments when it has many: `left_out` says how many in between
/// are not shown
fn shortened(thread: &Value) -> Value {
    let mut t = thread.clone();
    let Some(said) = t.get_mut("said").and_then(Value::as_array_mut) else { return t };
    let between = said.len().saturating_sub(SAID_KEPT);
    if between > 0 {
        said.drain(1..1 + between);
    }
    for s in said.iter_mut() {
        if let Some(text) = s.get("said").and_then(Value::as_str)
            && text.chars().count() > SAID_MOST
        {
            let cut: String = text.chars().take(SAID_MOST).collect();
            s["said"] = Value::String(format!("{cut} [...]"));
        }
    }
    if between > 0
        && let Some(o) = t.as_object_mut()
    {
        o.insert("left_out".into(), json!(between));
    }
    t
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

#[cfg(test)]
mod prompt_tests {
    use super::*;

    fn thread(n: usize, said: usize, long: usize) -> Value {
        json!({
            "path": format!("src/f{n}.rs"), "line": 3, "ids": ["x"],
            "said": (0..said).map(|i| json!({"who": "rev", "said": format!("{i}:{}", "w".repeat(long))})).collect::<Vec<_>>(),
        })
    }

    /// A long thread keeps its first comment and its latest, says how many
    /// in between it leaves out, and a long comment is cut
    #[test]
    fn a_long_thread_keeps_its_first_and_latest_comments() {
        let (text, left_out) = prompt_items(&[thread(1, 12, SAID_MOST + 50)], true);
        assert_eq!(left_out, 0);
        let shown: Vec<Value> = serde_json::from_str(&text).unwrap();
        let said = shown[0]["said"].as_array().unwrap();
        assert_eq!(said.len(), SAID_KEPT);
        assert!(said[0]["said"].as_str().unwrap().starts_with("0:"));
        assert!(said[SAID_KEPT - 1]["said"].as_str().unwrap().starts_with("11:"));
        assert!(said[0]["said"].as_str().unwrap().ends_with("[...]"));
        assert_eq!(shown[0]["left_out"], json!(12 - SAID_KEPT));
        assert!(shown[0].get("ids").is_none(), "ids mean nothing to the AI");
    }

    /// What does not fit is left out and counted; the first always goes in
    #[test]
    fn what_does_not_fit_is_counted_and_the_first_always_goes() {
        let many: Vec<Value> = (0..400).map(|n| thread(n, 2, 300)).collect();
        let (text, left_out) = prompt_items(&many, true);
        assert!(text.len() <= PROMPT_ROOM + 1000, "{}", text.len());
        assert!(left_out > 0 && left_out < 400);
        let shown: Vec<Value> = serde_json::from_str(&text).unwrap();
        assert_eq!(shown.len() + left_out, 400);
        assert_eq!(shown[0]["path"], "src/f0.rs", "in the order given");
        let huge = vec![json!({"name": "build", "log_tail": "x".repeat(PROMPT_ROOM * 2)})];
        let (text, left_out) = prompt_items(&huge, false);
        assert_eq!(left_out, 0);
        assert!(text.contains("build"));
    }
}

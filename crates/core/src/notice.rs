//! Telling an AI what this app changed under it.
//!
//! An AI CLI reads the folder it works in when its conversation starts -- the
//! branch it is on among it -- and goes on believing what it read. When this
//! app then changes something there by itself, the AI is the one party that
//! was not told, and the next time it looks it finds a world that moved for no
//! reason it can see. Measured on 2026-10-11: a session whose drawn branch was
//! renamed after its first answer saw the new name, took the branch to have
//! been switched by somebody else, and went off to work in a folder of its own.
//!
//! So each change is written down here, and each tab in that folder is told of
//! it once, at the start of its next turn (the hook `agenthook::NOTICE_ARG`).
//! Once, because a line repeated every turn is context spent on every turn for
//! something that was news exactly one time. Each tab, because every
//! conversation in the folder read the old name. Kept in memory: a notice is
//! for the conversations running now, and a restart of this app is a restart
//! of the tabs it tells -- they start out reading the folder as it is.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The call a hook makes for its notice
pub const METHOD: &str = "take_notice";

/// One change, and which tabs have been told of it
struct Notice {
    /// The folder, as a place key ([`crate::uistate::place_key`])
    place: PathBuf,
    /// The branch's name as the AI last read it, and its name now
    from: String,
    to: String,
    /// The tabs that have been told, by uid
    told: HashSet<String>,
}

impl Notice {
    fn says(&self) -> String {
        let (from, to) = (&self.from, &self.to);
        format!(
            "SHIKISHA-TERM renamed the git branch of this folder from `{from}` to `{to}`. \
             Only the name changed: the commits and the working tree are the same, \
             and nobody switched branches. Use `{to}` from now on."
        )
    }
}

static NOTICES: Mutex<Vec<Notice>> = Mutex::new(Vec::new());

/// The branch in a folder was renamed by this app.
///
/// A folder renamed again replaces what it had: two notices about one branch
/// would tell an AI a name that is already gone. The one that stands starts
/// from the name the conversations not yet told last read
pub fn branch_renamed(place: &Path, from: &str, to: &str) {
    let mut notices = NOTICES.lock().unwrap_or_else(|e| e.into_inner());
    let from = match notices.iter().position(|n| n.place == place) {
        Some(i) => notices.remove(i).from,
        None => from.to_string(),
    };
    notices.push(Notice { place: place.to_path_buf(), from, to: to.to_string(), told: HashSet::new() });
}

/// What a tab is to be told at the start of this turn, if anything: each
/// notice about its folder that it has not been told yet, marked told in the
/// same breath, so that asking twice can never tell it twice
pub fn take(tab: &str, cwd: &Path, host: Option<&str>) -> Option<String> {
    let mut notices = NOTICES.lock().unwrap_or_else(|e| e.into_inner());
    let says: Vec<String> = notices
        .iter_mut()
        .filter(|n| crate::uistate::is_place(cwd, host, &n.place))
        .filter_map(|n| n.told.insert(tab.to_string()).then(|| n.says()))
        .collect();
    (!says.is_empty()).then(|| says.join("\n"))
}

/// What a hook prints for the CLI: the notice as context for the turn about to
/// start, or `{}` -- which says nothing, and which a CLI that wants JSON from
/// every hook still reads
pub fn hook_output(event: &str, says: Option<&str>) -> serde_json::Value {
    match says.map(str::trim).filter(|s| !s.is_empty()) {
        Some(says) => serde_json::json!({
            "hookSpecificOutput": { "hookEventName": event, "additionalContext": says }
        }),
        None => serde_json::json!({}),
    }
}

/// The hook's half, run by a CLI as the tab whose key it holds: ask the app,
/// a little less long than the CLI waits for the hook (3 s), and print the
/// answer the way the CLI reads one. Anything that goes wrong prints `{}`: a
/// notice is worth one line in a conversation, never a turn held up
pub fn ask_and_print(event: &str) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(crate::api::ApiClient::from_env().and_then(|mut c| c.call(METHOD, Vec::new())));
    });
    let says = match rx.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(Ok(a)) if a["ok"] == serde_json::json!(true) => a["result"]["says"].as_str().map(str::to_string),
        Ok(Ok(a)) => {
            crate::append_hook_log(&format!("hook notice refused: {a}"));
            None
        }
        Ok(Err(e)) => {
            crate::append_hook_log(&format!("hook notice could not ask: {e}"));
            None
        }
        Err(_) => {
            crate::append_hook_log("hook notice: the app did not answer in time");
            None
        }
    };
    println!("{}", hook_output(event, says.as_deref()));
}

#[cfg(test)]
mod tests {
    use super::*;

    // One store for the whole test run, so each test has a folder of its own
    fn place(name: &str) -> PathBuf {
        PathBuf::from(crate::local_path(&format!("D:/notice-test/{name}")))
    }

    #[test]
    fn each_tab_is_told_once() {
        let at = place("once");
        branch_renamed(&at, "mighty-gannet", "fix/login");
        let first = take("tab-a", &at, None).expect("the first turn after the rename is told");
        assert!(first.contains("`mighty-gannet` to `fix/login`"), "{first}");
        assert_eq!(take("tab-a", &at, None), None, "the second turn is told again");
        assert_eq!(take("tab-a", &at, None), None, "the third turn is told again");
        assert!(take("tab-b", &at, None).is_some(), "another tab in the folder read the old name too");
        assert_eq!(take("tab-b", &at, None), None);
    }

    #[test]
    fn a_tab_elsewhere_is_never_told() {
        let at = place("here");
        branch_renamed(&at, "brave-otter", "feat/x");
        assert_eq!(take("tab-c", &place("there"), None), None);
        assert!(take("tab-c", &at, None).is_some());
        let far = PathBuf::from(crate::uistate::place_key(Some("server"), &at));
        branch_renamed(&far, "brave-otter", "feat/y");
        assert_eq!(take("tab-e", &at, None), Some(take_line("brave-otter", "feat/x")), "a folder on another machine is another folder");
    }

    fn take_line(from: &str, to: &str) -> String {
        Notice { place: PathBuf::new(), from: from.into(), to: to.into(), told: HashSet::new() }.says()
    }

    #[test]
    fn a_second_rename_starts_from_the_name_last_read() {
        let at = place("twice");
        branch_renamed(&at, "calm-heron", "feat/a");
        branch_renamed(&at, "feat/a", "feat/b");
        let said = take("tab-d", &at, None).expect("told");
        assert!(said.contains("`calm-heron` to `feat/b`"), "{said}");
        assert_eq!(said.lines().count(), 1, "one notice per folder, not one per rename: {said}");
    }

    #[test]
    fn a_hook_with_nothing_to_say_prints_empty_json() {
        assert_eq!(hook_output("UserPromptSubmit", None), serde_json::json!({}));
        assert_eq!(hook_output("UserPromptSubmit", Some("  ")), serde_json::json!({}));
        let v = hook_output("UserPromptSubmit", Some("renamed"));
        assert_eq!(v["hookSpecificOutput"]["additionalContext"], "renamed");
        assert_eq!(v["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    }
}

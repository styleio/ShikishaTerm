//! A new worktree, trusted by an AI CLI as the checkout it is cut from is.
//!
//! Some AI CLIs ask, the first time they start in a folder, whether to trust
//! it -- and keep the answer by the folder. A worktree is a new folder, so
//! such a CLI asks again in every one, about a project the person already
//! said yes to. Where the CLI keeps its answers in a file of its own, the
//! same yes can be written there for the worktree, as the CLI itself writes
//! it when the person says yes on its screen.
//!
//! Only a yes the person gave is carried, never made up: a worktree whose
//! checkout the CLI does not trust is left for the CLI to ask about. And the
//! file is the CLI's, so nothing is written into it until the person has
//! agreed to that, once for each CLI ([`crate::config::Config::worktree_trust`]):
//! asked on the board the first time it would matter, and changed in the
//! settings after.
//!
//! Which CLIs: those that ask again in a worktree. Claude Code and Codex CLI
//! do not -- each takes a worktree of a project it trusts as trusted (checked
//! with the CLIs themselves) -- so nothing is written for them. Gemini CLI
//! asks, and keeps its answers in `~/.gemini/trustedFolders.json`.
//!
//! On a MicroVM the machine is the app's own, made for the project, and its
//! AIs are carried over without asking ([`crate::microvm::trust_as_checkout`]).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A CLI that keeps whether it trusts a folder in a file of its own
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keeper {
    /// Its name, as the profiles and every other setting about it say it
    pub name: &'static str,
}

/// Gemini CLI
pub const GEMINI: Keeper = Keeper { name: "Gemini CLI" };

/// The CLIs this is done for
pub const KEEPERS: [Keeper; 1] = [GEMINI];

/// The answers [`crate::config::Config::worktree_trust`] holds
pub const ON: &str = "on";
pub const OFF: &str = "off";

/// Where a folder is: this PC, or a machine reached over SSH
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum At {
    Here,
    Far(crate::config::HostSpec),
}

impl At {
    fn shown(&self) -> String {
        match self {
            At::Here => String::new(),
            At::Far(h) => h.name.clone(),
        }
    }
}

// ── Gemini CLI's file ────────────────────────────────────────────────────────

/// What Gemini CLI writes for a folder it trusts
const TRUST_FOLDER: &str = "TRUST_FOLDER";
const TRUST_PARENT: &str = "TRUST_PARENT";
const DO_NOT_TRUST: &str = "DO_NOT_TRUST";

/// Gemini CLI's file of trusted folders on this PC: where the CLI is told
/// to keep it, else in its folder in the home (`GEMINI_CLI_HOME` standing
/// in for the home, as the CLI reads it)
fn gemini_file_here() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GEMINI_CLI_TRUSTED_FOLDERS_PATH").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    gemini_dir_here().map(|d| d.join("trustedFolders.json"))
}

fn gemini_dir_here() -> Option<PathBuf> {
    let home = std::env::var_os("GEMINI_CLI_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).or_else(crate::home_dir)?;
    Some(home.join(".gemini"))
}

/// A folder as Gemini CLI compares folders: `/` between the parts, nothing
/// at the end, and -- where the system does not tell letters' case apart,
/// on Windows and on a Mac -- in small letters
fn gemini_key(path: &str, fold: bool) -> String {
    let mut p = path.replace('\\', "/");
    while p.len() > 1 && p.ends_with('/') && !p.ends_with(":/") {
        p.pop();
    }
    if fold { p.to_lowercase() } else { p }
}

/// Whether `child` is `parent` or inside it, both as [`gemini_key`] gives them
fn within(parent: &str, child: &str) -> bool {
    child == parent || child.strip_prefix(parent).is_some_and(|rest| parent.ends_with('/') || rest.starts_with('/'))
}

/// Whether Gemini CLI trusts `folder`, by its file's rules the way the CLI
/// reads them: of the rules whose folder holds this one, the one written
/// with the longest path decides (a parent's rule holds the parent's
/// folder). `None` when no rule says, or the file cannot be read as one
fn gemini_trusts(text: &str, folder: &str, fold: bool) -> Option<bool> {
    let doc: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let rules = doc.as_object()?;
    let at = gemini_key(folder, fold);
    let mut best: Option<(usize, &str)> = None;
    for (path, level) in rules {
        let level = level.as_str()?;
        let held = match level {
            TRUST_PARENT => match gemini_key(path, fold).rsplit_once('/') {
                Some(("", _)) => "/".to_string(),
                Some((parent, _)) if parent.ends_with(':') => format!("{parent}/"),
                Some((parent, _)) => parent.to_string(),
                None => continue,
            },
            TRUST_FOLDER | DO_NOT_TRUST => gemini_key(path, fold),
            // A word the CLI would refuse the whole file for
            _ => return None,
        };
        if within(&held, &at) && best.is_none_or(|(n, _)| path.len() > n) {
            best = Some((path.len(), level));
        }
    }
    best.map(|(_, level)| level != DO_NOT_TRUST)
}

/// Gemini CLI's file with `folder` trusted, as the CLI writes it when the
/// person says yes: the folder, as it compares folders, said to be trusted.
/// `None` when the file is not one the CLI could read -- it is the person's
/// to put right, and is not written over
fn gemini_with(text: &str, folder: &str, fold: bool) -> Option<String> {
    let mut doc: serde_json::Value =
        if text.trim().is_empty() { serde_json::json!({}) } else { serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()? };
    doc.as_object_mut()?.insert(gemini_key(folder, fold), serde_json::json!(TRUST_FOLDER));
    serde_json::to_string_pretty(&doc).ok()
}

/// Whether Gemini CLI asks about folders at all: it does unless its settings
/// turn that off (`security.folderTrust.enabled`)
fn gemini_asks(settings: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(settings.trim_start_matches('\u{feff}'))
        .ok()
        .and_then(|s| s.pointer("/security/folderTrust/enabled").and_then(|v| v.as_bool()))
        .unwrap_or(true)
}

// ── Reading and writing, here and on a machine afar ─────────────────────────

/// The folder as the CLI sees it: links followed, and without the prefix
/// Windows puts on a path given back that way
fn real(folder: &Path) -> String {
    let p = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    let s = p.display().to_string();
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

/// Whether the system the CLI runs on tells letters' case apart in paths
fn folds_here() -> bool {
    cfg!(windows) || cfg!(target_os = "macos")
}

/// A server's paths: case is told apart there, unless it writes them the way
/// Windows does
fn folds_far(folder: &str) -> bool {
    folder.as_bytes().get(1) == Some(&b':')
}

/// Read a file of the CLI's, here or on the machine; `None` when it is not
/// there or cannot be read
fn read(at: &At, here: &Path, far: &str) -> Option<String> {
    match at {
        At::Here => std::fs::read_to_string(here).ok(),
        At::Far(host) => {
            let there = crate::elsewhere::Elsewhere::of(host).ok()?;
            match crate::elsewhere::files(&there, crate::ssh::FileJob::Read { path: far.into() }, 30_000) {
                Ok(crate::ssh::FileAnswer::Bytes(b)) => Some(String::from_utf8_lossy(&b).into_owned()),
                _ => None,
            }
        }
    }
}

/// The CLI's own lock on its file, taken the way it takes it (a folder named
/// for the file, made only if it is not there), so a yes the CLI is writing
/// at that moment is not written over. Waited for a second; one older than
/// the CLI's own limit for a lock was left by a CLI that stopped
struct Locked(PathBuf);

impl Locked {
    fn take(file: &Path) -> Option<Locked> {
        let lock = PathBuf::from(format!("{}.lock", file.display()));
        for _ in 0..20 {
            match std::fs::create_dir(&lock) {
                Ok(()) => return Some(Locked(lock)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&lock)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > std::time::Duration::from_secs(10));
                    if stale {
                        let _ = std::fs::remove_dir(&lock);
                        continue;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// Where Gemini CLI keeps its trusted folders on a server: from the home,
/// the way the file commands there take a path
const GEMINI_FAR_FILE: &str = ".gemini/trustedFolders.json";
const GEMINI_FAR_SETTINGS: &str = ".gemini/settings.json";

/// How a CLI stands on a worktree being made
#[derive(Debug, PartialEq, Eq)]
enum Stand {
    /// Nothing to do: the CLI is not on that machine, does not ask, does not
    /// trust the checkout, or already trusts the worktree
    Nothing,
    /// It trusts the checkout and would ask about the worktree
    WouldAsk,
}

fn stand(keeper: Keeper, at: &At, checkout: &str, worktree: &str) -> Stand {
    debug_assert_eq!(keeper, GEMINI);
    let fold = match at {
        At::Here => folds_here(),
        At::Far(_) => folds_far(worktree),
    };
    let (file, settings) = match at {
        At::Here => {
            let (Some(file), Some(dir)) = (gemini_file_here(), gemini_dir_here()) else { return Stand::Nothing };
            (read(at, &file, ""), read(at, &dir.join("settings.json"), ""))
        }
        At::Far(_) => (read(at, Path::new(""), GEMINI_FAR_FILE), read(at, Path::new(""), GEMINI_FAR_SETTINGS)),
    };
    let Some(file) = file else { return Stand::Nothing };
    if !settings.as_deref().is_none_or(gemini_asks) {
        return Stand::Nothing;
    }
    match (gemini_trusts(&file, checkout, fold), gemini_trusts(&file, worktree, fold)) {
        (Some(true), None) => Stand::WouldAsk,
        _ => Stand::Nothing,
    }
}

/// Write the yes for `worktree` into the CLI's file. Read again just before,
/// under the CLI's own lock here, so whatever the CLI wrote since is kept
fn write(keeper: Keeper, at: &At, worktree: &str) -> Result<(), String> {
    debug_assert_eq!(keeper, GEMINI);
    match at {
        At::Here => {
            let file = gemini_file_here().ok_or("there is no home folder")?;
            let _lock = Locked::take(&file).ok_or("Gemini CLI is writing its file")?;
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            if gemini_trusts(&text, worktree, folds_here()) == Some(true) {
                return Ok(());
            }
            let out = gemini_with(&text, worktree, folds_here()).ok_or("Gemini CLI's file is not one it could read")?;
            crate::crypto::write_atomic(&file, &out).map_err(|e| e.to_string())
        }
        At::Far(host) => {
            let there = crate::elsewhere::Elsewhere::of(host).map_err(|e| format!("{e:#}"))?;
            let text = read(at, Path::new(""), GEMINI_FAR_FILE).unwrap_or_default();
            let fold = folds_far(worktree);
            if gemini_trusts(&text, worktree, fold) == Some(true) {
                return Ok(());
            }
            let out = gemini_with(&text, worktree, fold).ok_or("Gemini CLI's file is not one it could read")?;
            crate::elsewhere::files(
                &there,
                crate::ssh::FileJob::Write { to: GEMINI_FAR_FILE.into(), bytes: out.into_bytes() },
                30_000,
            )
            .map(|_| ())
            .map_err(|e| format!("{e:#}"))
        }
    }
}

// ── The question ────────────────────────────────────────────────────────────

/// A worktree waiting on the person's answer about a CLI
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Asking {
    /// The CLI's name
    pub ai: String,
    /// The worktree's folder, as the person reads it
    pub folder: String,
    /// The checkout the CLI trusts
    pub checkout: String,
    /// The machine, when it is not this PC
    #[serde(skip_serializing_if = "String::is_empty")]
    pub machine: String,
    /// How many more worktrees wait on the same answer
    #[serde(skip_serializing_if = "is_zero")]
    pub more: usize,
    #[serde(skip)]
    waiting: Vec<(At, String)>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

static ASKING: Mutex<Vec<Asking>> = Mutex::new(Vec::new());

/// The questions up, one for each CLI
pub fn asking() -> Vec<Asking> {
    ASKING.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn ask(keeper: Keeper, at: At, checkout: &str, worktree: &str) {
    let mut all = ASKING.lock().unwrap_or_else(|e| e.into_inner());
    match all.iter_mut().find(|a| a.ai == keeper.name) {
        Some(a) => {
            if !a.waiting.iter().any(|(w_at, w)| *w_at == at && w == worktree) {
                a.waiting.push((at, worktree.to_string()));
                a.more = a.waiting.len() - 1;
            }
        }
        None => all.push(Asking {
            ai: keeper.name.into(),
            folder: worktree.into(),
            checkout: checkout.into(),
            machine: at.shown(),
            more: 0,
            waiting: vec![(at, worktree.to_string())],
        }),
    }
}

/// What could not be done about an answer, for the board to say once
static SAID: Mutex<Option<String>> = Mutex::new(None);

/// What is to be said, since this was last asked
pub fn take_said() -> Option<String> {
    SAID.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The person's answer about `ai`: yes or no, kept, and done for every
/// worktree that waited on it; or put away (`None`), and asked again with
/// the next worktree. What could not be done is said on the board
pub fn answered(ai: &str, yes: Option<bool>) {
    if let Err(e) = answer(ai, yes) {
        *SAID.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
    }
}

fn answer(ai: &str, yes: Option<bool>) -> Result<(), String> {
    let Some(asked) = ({
        let mut all = ASKING.lock().unwrap_or_else(|e| e.into_inner());
        let i = all.iter().position(|a| a.ai == ai);
        i.map(|i| all.remove(i))
    }) else {
        return Ok(());
    };
    let Some(yes) = yes else {
        crate::append_hook_log(&format!("worktree trust: the question about {ai} was put away"));
        return Ok(());
    };
    let Some(keeper) = KEEPERS.into_iter().find(|k| k.name == ai) else { return Ok(()) };
    if !crate::config::save_worktree_trust(ai, yes) {
        return Err(crate::i18n::t("msg.trust.not_saved"));
    }
    crate::append_hook_log(&format!("worktree trust: the person said {} for {ai}", if yes { ON } else { OFF }));
    if !yes {
        return Ok(());
    }
    let mut failed = Vec::new();
    for (at, worktree) in asked.waiting {
        match write(keeper, &at, &worktree) {
            Ok(()) => crate::append_hook_log(&format!("worktree trust: {worktree} is trusted by {ai} as its checkout is")),
            Err(e) => {
                crate::append_hook_log(&format!("worktree trust: {worktree} could not be written into {ai}'s file: {e}"));
                failed.push(worktree);
            }
        }
    }
    match failed.is_empty() {
        true => Ok(()),
        false => Err(crate::i18n::tp("msg.trust.not_written", &[("ai", ai), ("folder", &failed.join(", "))])),
    }
}

/// A worktree was just made from `checkout`: each CLI that trusts the
/// checkout and would ask about the worktree is told the same yes, if the
/// person agreed to that; asked whether to, if they have not been; and left
/// to ask on its own screen if they said no. Called on the thread making
/// the worktree, before its tabs are started, so a CLI started in it reads
/// the yes already there
pub fn worktree_made(at: At, checkout: &Path, worktree: &Path) {
    let (checkout, worktree) = match at {
        At::Here => (real(checkout), real(worktree)),
        At::Far(_) => (checkout.display().to_string().replace('\\', "/"), worktree.display().to_string().replace('\\', "/")),
    };
    let answers = crate::config::load().map(|c| c.worktree_trust).unwrap_or_default();
    for keeper in KEEPERS {
        if stand(keeper, &at, &checkout, &worktree) != Stand::WouldAsk {
            continue;
        }
        match answers.get(keeper.name).map(String::as_str) {
            Some(ON) => match write(keeper, &at, &worktree) {
                Ok(()) => crate::append_hook_log(&format!("worktree trust: {worktree} is trusted by {} as {checkout} is", keeper.name)),
                Err(e) => crate::append_hook_log(&format!("worktree trust: {worktree} could not be written into {}'s file: {e}", keeper.name)),
            },
            Some(OFF) => {}
            _ => ask(keeper, at.clone(), &checkout, &worktree),
        }
    }
}

/// The answer changed in the settings: kept, and -- when the question about
/// that CLI is up -- that question answered with it, so the worktrees that
/// waited on it are done too
pub fn set(ai: &str, on: bool) -> Result<(), String> {
    if !KEEPERS.iter().any(|k| k.name == ai) {
        return Err(format!("{ai} keeps no list of trusted folders this app writes"));
    }
    let up = ASKING.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|a| a.ai == ai);
    if up {
        return answer(ai, Some(on));
    }
    match crate::config::save_worktree_trust(ai, on) {
        true => Ok(()),
        false => Err(crate::i18n::t("msg.trust.not_saved")),
    }
}

/// How each CLI stands, for the settings: its name, the answer kept
/// ([`ON`], [`OFF`], or empty: not asked yet), and the file the yes goes in
pub fn rows() -> Vec<(Keeper, String, String)> {
    let answers = crate::config::load().map(|c| c.worktree_trust).unwrap_or_default();
    KEEPERS
        .into_iter()
        .map(|k| {
            let file = gemini_file_here().map(|f| f.display().to_string()).unwrap_or_default();
            (k, answers.get(k.name).cloned().unwrap_or_default(), file)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_reads_as_gemini_compares_it() {
        assert_eq!(gemini_key(r"C:\Users\Me\Proj\", true), "c:/users/me/proj");
        assert_eq!(gemini_key("C:/", true), "c:/");
        assert_eq!(gemini_key("/home/Me/proj/", false), "/home/Me/proj");
        assert_eq!(gemini_key(r"\\nas\share\x", true), "//nas/share/x");
    }

    #[test]
    fn the_longest_rule_that_holds_the_folder_decides() {
        let text = r#"{
  "c:/users/me": "TRUST_FOLDER",
  "c:/users/me/secret": "DO_NOT_TRUST",
  "d:/work/site/child": "TRUST_PARENT"
}"#;
        assert_eq!(gemini_trusts(text, r"C:\Users\Me\proj", true), Some(true), "under a trusted folder");
        assert_eq!(gemini_trusts(text, r"C:\Users\Me\secret\x", true), Some(false), "a longer rule says no");
        assert_eq!(gemini_trusts(text, r"D:\work\site", true), Some(true), "a parent's rule holds the parent");
        assert_eq!(gemini_trusts(text, r"D:\work\site-feature", true), None, "a sibling with a longer name is not inside");
        assert_eq!(gemini_trusts(text, r"E:\x", true), None);
        assert_eq!(gemini_trusts(r#"{"c:/a": "MAYBE"}"#, r"c:\a", true), None, "a file the CLI refuses says nothing");
        assert_eq!(gemini_trusts("not json", r"c:\a", true), None);
    }

    #[test]
    fn case_counts_only_where_the_system_counts_it() {
        let text = r#"{"/home/me/Proj": "TRUST_FOLDER"}"#;
        assert_eq!(gemini_trusts(text, "/home/me/Proj", false), Some(true));
        assert_eq!(gemini_trusts(text, "/home/me/proj", false), None);
        assert_eq!(gemini_trusts(text, "/home/me/proj", true), Some(true));
    }

    #[test]
    fn the_yes_is_written_as_the_cli_writes_it_and_nothing_else_changes() {
        let text = "{\n  \"d:/other\": \"DO_NOT_TRUST\",\n  \"c:/users/me/proj\": \"TRUST_FOLDER\"\n}";
        let out = gemini_with(text, r"C:\Users\Me\proj-feature", true).expect("written");
        assert_eq!(
            out,
            "{\n  \"d:/other\": \"DO_NOT_TRUST\",\n  \"c:/users/me/proj\": \"TRUST_FOLDER\",\n  \"c:/users/me/proj-feature\": \"TRUST_FOLDER\"\n}"
        );
        assert_eq!(gemini_trusts(&out, r"C:\Users\Me\proj-feature", true), Some(true));
        assert_eq!(gemini_with("", "/srv/x", false).as_deref(), Some("{\n  \"/srv/x\": \"TRUST_FOLDER\"\n}"), "no file yet");
        assert_eq!(gemini_with("{ broken", "/srv/x", false), None, "a file it could not read is not written over");
        assert_eq!(gemini_with("[1]", "/srv/x", false), None);
    }

    #[test]
    fn folder_trust_turned_off_asks_nothing() {
        assert!(gemini_asks("{}"));
        assert!(gemini_asks("not json"), "settings it cannot read leave it asking");
        assert!(gemini_asks(r#"{"security": {"folderTrust": {"enabled": true}}}"#));
        assert!(!gemini_asks(r#"{"security": {"folderTrust": {"enabled": false}}}"#));
    }

    #[test]
    fn a_question_waits_once_for_each_cli_and_counts_the_worktrees() {
        ASKING.lock().unwrap().clear();
        ask(GEMINI, At::Here, "c:/p", "c:/p-a");
        ask(GEMINI, At::Here, "c:/p", "c:/p-b");
        ask(GEMINI, At::Here, "c:/p", "c:/p-a");
        let up = asking();
        assert_eq!(up.len(), 1);
        assert_eq!((up[0].folder.as_str(), up[0].more), ("c:/p-a", 1));
        assert!(answer(GEMINI.name, None).is_ok());
        assert!(asking().is_empty(), "put away");
    }

    #[test]
    fn a_lock_is_taken_and_given_back() {
        let dir = std::env::temp_dir().join(format!("sk-aitrust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("trustedFolders.json");
        {
            let held = Locked::take(&file).expect("taken");
            assert!(dir.join("trustedFolders.json.lock").is_dir());
            drop(held);
        }
        assert!(!dir.join("trustedFolders.json.lock").exists(), "given back");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

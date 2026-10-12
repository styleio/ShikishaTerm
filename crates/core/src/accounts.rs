//! More than one account for an AI's CLI.
//!
//! An AI's CLI is installed once. Which account it is signed in as is not in
//! the program but in the folder it keeps its settings in -- `~/.claude` for
//! Claude Code, `~/.codex` for Codex -- and each lets an environment variable
//! name another folder (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`). So an account here
//! is a folder of its own (`data/ai-accounts/<id>/home`), and a tab started
//! with that account is started with the variable naming it. Nothing is
//! installed twice, and the sign-in itself is the CLI's own: this app never
//! reads or writes a password, a token or a credentials file.
//!
//! A folder of its own on its own would cut the account off from everything
//! the app and the person keep in the usual folder. So an account's folder is
//! [`prepare`]d each time a tab starts with it:
//!
//! - **what is said is kept in one place.** The folders the CLI writes its
//!   conversations into are links into the usual folder's (a junction on
//!   Windows, which needs no administrator). A conversation had under either
//!   account is found, resumed, searched and read in the conversation panel
//!   exactly as before -- every reader of a record keeps looking where it
//!   always did;
//! - **the person's tools are the same tools**: skills (this app's among
//!   them), commands, agents and plugins are links too;
//! - **settings start from the usual ones**: copied in once, when the account
//!   is made, without anything that belongs to the account they came from.
//!   After that they are the account's own, as a CLI's `/config` writes them;
//! - **what an account is signed in as stays its own**: its credentials, and
//!   the CLI's registry of the processes running in it.
//!
//! Codex is the one exception about conversations: its records are not linked
//! together. A fresh folder shown every conversation ever had sets Codex to
//! index them all as it starts, and it gives up on its own start while doing
//! so. Its records stay in the account's folder, and the places a record is
//! looked for are the usual folder and every account's ([`patterns`]).
//!
//! Only on this PC. A tab on a server or a MicroVM runs as that machine's CLI
//! is signed in; an account is not something this app carries elsewhere.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// An account's folder was made since this was last asked
static FRESH: AtomicBool = AtomicBool::new(false);

/// Whether an account's folder was made since this was last asked: its hooks
/// are then to be kept right, as its CLI's were agreed to
pub fn take_fresh() -> bool {
    FRESH.swap(false, Ordering::Relaxed)
}

/// One CLI whose account can be chosen, and how its folder is made up
pub struct Kind {
    /// The command it is started with, which the settings name an account's
    /// CLI by
    pub cli: &'static str,
    /// Its profile's name, which its hooks are agreed to by
    pub profile: &'static str,
    /// The variable it finds its folder by
    pub env: &'static str,
    /// Its usual folder, under the home folder
    pub dir: &'static str,
    /// Folders in the usual folder an account's folder links to: one place
    /// for each of them, whichever account wrote into it
    pub share: &'static [&'static str],
    /// Files in the usual folder an account's folder links to, or copies when
    /// a link to a file cannot be made (Windows without developer mode)
    pub share_files: &'static [&'static str],
    /// Files copied from the usual folder into a new account's, once
    pub copy: &'static [&'static str],
}

/// Claude Code. What it writes conversations and their pieces into is
/// shared; its process registry (`sessions`) and its credentials are not
pub const CLAUDE: Kind = Kind {
    cli: "claude",
    profile: "Claude Code",
    env: "CLAUDE_CONFIG_DIR",
    dir: ".claude",
    share: &[
        "projects",
        "session-env",
        "file-history",
        "shell-snapshots",
        "todos",
        "paste-cache",
        "tasks",
        "plans",
        "transcripts",
        "skills",
        "commands",
        "agents",
        "plugins",
        "output-styles",
    ],
    share_files: &[],
    copy: &["settings.json", "keybindings.json"],
};

/// Codex. Its records stay in each account's folder (see the module's notes)
pub const CODEX: Kind = Kind {
    cli: "codex",
    profile: "Codex CLI",
    env: "CODEX_HOME",
    dir: ".codex",
    share: &["skills", "plugins", "prompts", "themes"],
    share_files: &["AGENTS.md"],
    // Its hooks file with the rest: this app's hooks in it are approved for
    // the new folder by Codex itself once the folder is made (`take_fresh`)
    copy: &["config.toml", "hooks.json"],
};

pub const KINDS: [&Kind; 2] = [&CLAUDE, &CODEX];

/// The kind a CLI's command names
pub fn kind(cli: &str) -> Option<&'static Kind> {
    KINDS.into_iter().find(|k| k.cli == cli)
}

/// The kind a profile is
pub fn kind_of_profile(name: &str) -> Option<&'static Kind> {
    KINDS.into_iter().find(|k| k.profile == name)
}

/// The CLI's own folder, as it finds it when no account is chosen: the
/// variable when the person set it for everything, the usual folder in the
/// home folder otherwise. A variable this app set for one of its own tabs is
/// not the person's, and is not inherited by the app itself
pub fn usual_dir(k: &Kind) -> Option<PathBuf> {
    usual_dir_in(k, |name| std::env::var_os(name))
}

fn usual_dir_in(k: &Kind, env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(set) = env(k.env).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(set));
    }
    crate::home_in(&env).map(|h| h.join(k.dir))
}

/// The accounts the settings hold, as they were last read. Kept here because
/// a tab is started in many places, and each would otherwise read the whole
/// settings file to find out where one account's folder is
static KNOWN: RwLock<Vec<crate::config::AiAccountSpec>> = RwLock::new(Vec::new());

/// What the settings hold now; called each time they are read
pub fn set_known(list: &[crate::config::AiAccountSpec]) {
    if let Ok(mut known) = KNOWN.write() {
        *known = list.iter().filter(|a| valid_id(&a.id) && kind(&a.cli).is_some()).cloned().collect();
    }
}

pub fn known() -> Vec<crate::config::AiAccountSpec> {
    KNOWN.read().map(|k| k.clone()).unwrap_or_default()
}

/// The account a tab names, by id
pub fn find(id: &str) -> Option<crate::config::AiAccountSpec> {
    known().into_iter().find(|a| a.id == id)
}

/// An id is a folder's name: letters, digits and dashes, and short
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 40 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Where every account's folder is
pub fn root() -> PathBuf {
    crate::config::state_path("ai-accounts")
}

/// The folder an account's CLI is pointed at
pub fn dir_of(id: &str) -> PathBuf {
    root().join(id).join("home")
}

/// What tells a folder under [`root`] is an account's, written beside its
/// `home` once that is made up. Nothing is removed from a folder without it
const MARK: &str = "account.json";

/// The variable and the folder a tab started with account `id` is given,
/// with the folder made up for it. `profile` is the profile of what the tab
/// runs: an account of another CLI is refused rather than handed to a
/// program that would not read it
pub fn prepare(id: &str, profile: &str) -> Result<(&'static str, PathBuf), String> {
    let account = find(id).ok_or_else(|| crate::i18n::tp("msg.account.gone", &[("id", id)]))?;
    let k = kind(&account.cli).ok_or_else(|| crate::i18n::tp("msg.account.gone", &[("id", id)]))?;
    if kind_of_profile(profile).map(|t| t.cli) != Some(k.cli) {
        return Err(crate::i18n::tp("msg.account.other_cli", &[("name", &account.shown()), ("cli", profile)]));
    }
    let dir = dir_of(&account.id);
    let usual = usual_dir(k).ok_or_else(|| crate::i18n::t("msg.account.no_home"))?;
    make_up(k, &usual, &dir, &account).map_err(|e| {
        crate::append_hook_log(&format!("account {}: {e:#}", account.id));
        crate::i18n::tp("msg.account.not_made", &[("name", &account.shown()), ("why", &format!("{e:#}"))])
    })?;
    Ok((k.env, dir))
}

/// An account's folder, made up as the module's notes say. Each step is one
/// that may be taken again: a folder made up before is checked and left as
/// it is, and whatever the person put in its place is never replaced
fn make_up(k: &Kind, usual: &Path, dir: &Path, account: &crate::config::AiAccountSpec) -> Result<()> {
    let first = !dir.is_dir();
    std::fs::create_dir_all(dir).with_context(|| format!("make {}", dir.display()))?;
    if let Some(above) = dir.parent()
        && !above.join(MARK).is_file()
    {
        let mark = serde_json::json!({"id": account.id, "cli": k.cli});
        crate::crypto::write_atomic(&above.join(MARK), &mark.to_string())?;
    }
    for name in k.share {
        let from = usual.join(name);
        // Made in the usual folder when it is not there yet, so the first
        // conversation either account has goes into the one place
        if !from.exists() {
            std::fs::create_dir_all(&from).with_context(|| format!("make {}", from.display()))?;
        }
        link_dir(&from, &dir.join(name))?;
    }
    for name in k.share_files {
        let from = usual.join(name);
        if from.is_file() {
            link_file(&from, &dir.join(name))?;
        }
    }
    if first {
        for name in k.copy {
            let (from, to) = (usual.join(name), dir.join(name));
            if from.is_file() && !to.exists() {
                std::fs::copy(&from, &to).with_context(|| format!("copy {}", from.display()))?;
            }
        }
    }
    if k.cli == CLAUDE.cli {
        claude_extras(usual, dir)?;
    }
    if first {
        FRESH.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// What only Claude Code needs. Its memory file is read from its folder, so
/// an account's carries one line that reads the usual one in (a copy would
/// be a second memory that drifts). And its state file -- which folders are
/// trusted, what it has shown -- is in the folder when the variable is set,
/// beside it in the home folder when not: copied in once, without what
/// belongs to the account it came from
fn claude_extras(usual: &Path, dir: &Path) -> Result<()> {
    let memory = dir.join("CLAUDE.md");
    if usual.join("CLAUDE.md").is_file() && !memory.exists() {
        std::fs::write(&memory, format!("@{}\n", usual.join("CLAUDE.md").display()))?;
    }
    let state = dir.join(".claude.json");
    if !state.exists() {
        let usual_state = match std::env::var_os(CLAUDE.env).filter(|v| !v.is_empty()) {
            Some(_) => usual.join(".claude.json"),
            None => usual.parent().map(|h| h.join(".claude.json")).unwrap_or_default(),
        };
        if let Ok(text) = std::fs::read_to_string(&usual_state)
            && let Ok(serde_json::Value::Object(mut doc)) = serde_json::from_str::<serde_json::Value>(&text)
        {
            doc.retain(|key, _| !belongs_to_the_account(key));
            let written = serde_json::to_string_pretty(&serde_json::Value::Object(doc))?;
            crate::crypto::write_atomic(&state, &written)?;
        }
    }
    Ok(())
}

/// The keys of Claude Code's state file that are about the account signed
/// in, or are its readings of that account: carried into another account,
/// they would say it is someone else, or outrank its own sign-in
fn belongs_to_the_account(key: &str) -> bool {
    matches!(key, "oauthAccount" | "userID" | "machineID" | "primaryApiKey" | "customApiKeyResponses")
        || key.contains("Cache")
        || key.starts_with("cached")
        || key.starts_with("passes")
        || key.starts_with("s1m")
}

/// `link` made to point at the folder `target`, unless something is there
/// already: a link to the same place is left, and anything else -- a real
/// folder with something in it, a link somewhere else -- is the person's and
/// is left too. An empty folder is taken for one the CLI made before the
/// link was, and replaced
fn link_dir(target: &Path, link: &Path) -> Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(link) {
        if meta.file_type().is_symlink() || is_junction(link) {
            return Ok(());
        }
        if meta.is_dir() && std::fs::read_dir(link).map(|mut d| d.next().is_none()).unwrap_or(false) {
            std::fs::remove_dir(link).with_context(|| format!("replace {}", link.display()))?;
        } else {
            crate::append_hook_log(&format!("account folder: {} is somebody's own, left as it is", link.display()));
            return Ok(());
        }
    }
    #[cfg(windows)]
    junction::create(target, link).with_context(|| format!("link {} to {}", link.display(), target.display()))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).with_context(|| format!("link {} to {}", link.display(), target.display()))?;
    Ok(())
}

/// A file linked where that can be done, copied where it cannot (a link to
/// a file on Windows wants developer mode). A copy is refreshed when the
/// usual one is newer
fn link_file(target: &Path, link: &Path) -> Result<()> {
    if std::fs::symlink_metadata(link).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(());
    }
    if !link.exists() {
        #[cfg(unix)]
        if std::os::unix::fs::symlink(target, link).is_ok() {
            return Ok(());
        }
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(target, link).is_ok() {
            return Ok(());
        }
    }
    let newer = |a: &Path, b: &Path| {
        let at = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        match (at(a), at(b)) {
            (Some(a), Some(b)) => a > b,
            (Some(_), None) => true,
            _ => false,
        }
    };
    if newer(target, link) {
        std::fs::copy(target, link).with_context(|| format!("copy {}", target.display()))?;
    }
    Ok(())
}

#[cfg(windows)]
fn is_junction(p: &Path) -> bool {
    junction::exists(p).unwrap_or(false)
}

#[cfg(not(windows))]
fn is_junction(_: &Path) -> bool {
    false
}

/// The accounts a person asked to sign in, from the settings, waiting for
/// the board's loop to open a tab of each
static LOGINS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Ask for a tab to be opened as account `id`, for its CLI to sign it in
pub fn want_login(id: &str) {
    if let Ok(mut w) = LOGINS.lock()
        && !w.iter().any(|x| x == id)
    {
        w.push(id.to_string());
    }
}

/// The accounts asked to sign in, taken
pub fn take_logins() -> Vec<String> {
    LOGINS.lock().map(|mut w| std::mem::take(&mut *w)).unwrap_or_default()
}

/// Who an account is signed in as, read off what the CLI wrote: Claude
/// Code's state file, Codex's sign-in (the email in its identity token, not
/// the token). `None` while it has not been signed in
pub fn signed_in_as(k: &Kind, dir: &Path) -> Option<String> {
    if k.cli == CLAUDE.cli {
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join(".claude.json")).ok()?).ok()?;
        let account = doc.get("oauthAccount")?;
        return account.get("emailAddress").and_then(|e| e.as_str()).map(str::to_string);
    }
    let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("auth.json")).ok()?).ok()?;
    let token = doc.get("tokens")?.get("id_token")?.as_str()?;
    let claims = token_claims(token)?;
    claims.get("email").and_then(|e| e.as_str()).map(str::to_string).or_else(|| {
        // Signed in with an API key: there is no one to name, only a key
        doc.get("OPENAI_API_KEY").and_then(|k| k.as_str()).filter(|k| !k.is_empty()).map(|_| "API key".to_string())
    })
}

fn token_claims(token: &str) -> Option<serde_json::Value> {
    use base64::Engine as _;
    let middle = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(middle.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// An account's folder taken away, with what it was signed in as. The links
/// go first: a removal that walked into one would take the usual folder's
/// conversations with it. A folder without the mark is not removed
pub fn forget(id: &str) -> Result<()> {
    anyhow::ensure!(valid_id(id), "not an account's id: {id}");
    let above = root().join(id);
    if !above.exists() {
        return Ok(());
    }
    anyhow::ensure!(above.join(MARK).is_file(), "{} is not an account's folder", above.display());
    crate::worktree::unhook_links(&above);
    std::fs::remove_dir_all(&above).with_context(|| format!("remove {}", above.display()))
}

/// Every place a record a profile names with `pattern` may be: the CLI's own
/// folder as it finds it, then each account's of that CLI. The usual folder
/// comes first, so what was always found is found the same way. A pattern
/// that is not under a CLI's folder is only itself, and one under a folder
/// the accounts link to the usual one (`share`) is only the usual one: the
/// same records seen twice would be every conversation listed twice
pub fn patterns(pattern: &str) -> Vec<String> {
    let mut out = vec![];
    for k in KINDS {
        let Some(rest) = under(pattern, k) else { continue };
        if let Some(usual) = usual_dir(k) {
            out.push(format!("{}{rest}", usual.display()));
        }
        let first = rest.trim_start_matches(['/', '\\']).split(['/', '\\']).next().unwrap_or("");
        if k.share.contains(&first) {
            return out;
        }
        for a in known().into_iter().filter(|a| a.cli == k.cli) {
            out.push(format!("{}{rest}", dir_of(&a.id).display()));
        }
        return out;
    }
    vec![pattern.to_string()]
}

/// The part of `pattern` after a CLI's usual folder (`{home}/.claude`), with
/// its separator, when it is under one
fn under<'a>(pattern: &'a str, k: &Kind) -> Option<&'a str> {
    let rest = pattern.strip_prefix("{home}")?;
    let rest = rest.strip_prefix(['/', '\\'])?;
    let rest = rest.strip_prefix(k.dir)?;
    (rest.is_empty() || rest.starts_with(['/', '\\'])).then_some(rest)
}

/// `pattern` as it is for one account's folder, when it is under that CLI's
pub fn in_account(pattern: &str, k: &Kind, dir: &Path) -> Option<String> {
    under(pattern, k).map(|rest| format!("{}{rest}", dir.display()))
}

/// `pattern` as it is for the CLI's own folder as it finds it, which is the
/// person's own variable when they set one
pub fn in_usual(pattern: &str) -> String {
    patterns(pattern).into_iter().next().unwrap_or_else(|| pattern.to_string())
}

impl crate::config::AiAccountSpec {
    /// What it is called on screen: its name, else its id
    pub fn shown(&self) -> String {
        match self.name.trim() {
            "" => self.id.clone(),
            n => n.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(name: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!("sk-accounts-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Scratch(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            crate::worktree::unhook_links(&self.0);
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn account(id: &str, cli: &str) -> crate::config::AiAccountSpec {
        crate::config::AiAccountSpec { id: id.into(), cli: cli.into(), name: String::new() }
    }

    #[test]
    fn the_usual_folder_is_the_persons_variable_when_they_set_one() {
        let home = |k: &str| match k {
            "USERPROFILE" | "HOME" => Some("/home/me".into()),
            _ => None,
        };
        assert_eq!(usual_dir_in(&CLAUDE, home), Some(PathBuf::from("/home/me").join(".claude")));
        let set = |k: &str| match k {
            "CLAUDE_CONFIG_DIR" => Some("/elsewhere/claude".into()),
            "USERPROFILE" | "HOME" => Some("/home/me".into()),
            _ => None,
        };
        assert_eq!(usual_dir_in(&CLAUDE, set), Some(PathBuf::from("/elsewhere/claude")));
        let empty = |k: &str| match k {
            "CODEX_HOME" => Some("".into()),
            "USERPROFILE" | "HOME" => Some("/home/me".into()),
            _ => None,
        };
        assert_eq!(usual_dir_in(&CODEX, empty), Some(PathBuf::from("/home/me").join(".codex")), "an empty variable is no folder");
    }

    #[test]
    fn a_pattern_under_a_clis_folder_is_looked_for_in_every_account_of_it() {
        set_known(&[account("work", "codex"), account("home", "claude"), account("bad id!", "codex")]);
        let codex = patterns("{home}/.codex/sessions/*/*/*/rollout-*-{id}.jsonl");
        assert_eq!(codex.len(), 2, "{codex:?}");
        assert!(codex[1].starts_with(&dir_of("work").display().to_string()));
        assert!(codex[1].ends_with("/sessions/*/*/*/rollout-*-{id}.jsonl"));
        // Another CLI's folder, a name that only starts the same, and a path
        // under no CLI's folder are each only themselves or their own CLI's
        assert_eq!(patterns("{home}/.claude/projects/*/{id}.jsonl").len(), 1, "a shared folder was looked in twice");
        assert_eq!(patterns("{home}/.claude/settings.json").len(), 2);
        assert_eq!(patterns("{home}/.codexes/x"), vec!["{home}/.codexes/x".to_string()]);
        assert_eq!(patterns("{home}/.gemini/tmp/*/{id}.json"), vec!["{home}/.gemini/tmp/*/{id}.json".to_string()]);
        set_known(&[]);
    }

    #[test]
    fn an_account_is_made_up_from_the_usual_folder_and_keeps_what_is_its_own() {
        let s = Scratch::new("make");
        let usual = s.0.join("usual");
        std::fs::create_dir_all(usual.join("projects").join("C--work")).unwrap();
        std::fs::write(usual.join("projects").join("C--work").join("a.jsonl"), "{}").unwrap();
        std::fs::create_dir_all(usual.join("skills").join("shikisha")).unwrap();
        std::fs::write(usual.join("settings.json"), r#"{"model":"opus"}"#).unwrap();
        std::fs::write(usual.join(".credentials.json"), "secret").unwrap();
        std::fs::write(usual.join("CLAUDE.md"), "be kind").unwrap();
        let dir = s.0.join("acct").join("home");
        let spec = account("acct", "claude");
        make_up(&CLAUDE, &usual, &dir, &spec).unwrap();
        // The conversations and the skills are the usual folder's
        assert!(dir.join("projects").join("C--work").join("a.jsonl").is_file(), "the conversation is not seen from the account");
        assert!(dir.join("skills").join("shikisha").is_dir());
        // Written into through the account, it lands in the one place
        std::fs::write(dir.join("projects").join("C--work").join("b.jsonl"), "{}").unwrap();
        assert!(usual.join("projects").join("C--work").join("b.jsonl").is_file());
        // A folder the usual one did not have yet is made there and shared
        assert!(usual.join("todos").is_dir() && dir.join("todos").exists());
        // Settings start from the usual ones; the sign-in is not carried
        assert_eq!(std::fs::read_to_string(dir.join("settings.json")).unwrap(), r#"{"model":"opus"}"#);
        assert!(!dir.join(".credentials.json").exists(), "the sign-in was carried into another account");
        assert!(std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap().starts_with('@'));
        assert!(dir.parent().unwrap().join(MARK).is_file());
        // Made up again, nothing changes and nothing breaks
        std::fs::write(dir.join("settings.json"), r#"{"model":"sonnet"}"#).unwrap();
        make_up(&CLAUDE, &usual, &dir, &spec).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("settings.json")).unwrap(), r#"{"model":"sonnet"}"#, "the account's own settings were overwritten");
    }

    #[test]
    fn a_folder_the_person_filled_is_left_where_a_link_would_go() {
        let s = Scratch::new("own");
        let usual = s.0.join("usual");
        std::fs::create_dir_all(&usual).unwrap();
        let dir = s.0.join("acct").join("home");
        std::fs::create_dir_all(dir.join("commands")).unwrap();
        std::fs::write(dir.join("commands").join("mine.md"), "mine").unwrap();
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        make_up(&CLAUDE, &usual, &dir, &account("acct", "claude")).unwrap();
        assert!(dir.join("commands").join("mine.md").is_file(), "the person's folder was replaced");
        // An empty one is the CLI's, made before the link was
        let linked = std::fs::symlink_metadata(dir.join("agents")).map(|m| m.file_type().is_symlink()).unwrap_or(false) || is_junction(&dir.join("agents"));
        assert!(linked, "an empty folder was not linked");
    }

    #[test]
    fn the_state_file_comes_without_the_account_it_was_read_from() {
        assert!(belongs_to_the_account("oauthAccount"));
        assert!(belongs_to_the_account("primaryApiKey"));
        assert!(belongs_to_the_account("cachedStatsigGates"));
        assert!(belongs_to_the_account("passesEligibilityCache"));
        assert!(!belongs_to_the_account("projects"));
        assert!(!belongs_to_the_account("hasCompletedOnboarding"));
    }

    #[test]
    fn forgetting_an_account_never_reaches_through_a_link() {
        let s = Scratch::new("forget");
        let usual = s.0.join("usual");
        std::fs::create_dir_all(usual.join("projects")).unwrap();
        std::fs::write(usual.join("projects").join("kept.jsonl"), "{}").unwrap();
        let above = s.0.join("acct");
        let dir = above.join("home");
        make_up(&CLAUDE, &usual, &dir, &account("acct", "claude")).unwrap();
        crate::worktree::unhook_links(&above);
        std::fs::remove_dir_all(&above).unwrap();
        assert!(usual.join("projects").join("kept.jsonl").is_file(), "the usual folder's conversations went with the account");
    }

    #[test]
    fn who_codex_is_signed_in_as_is_the_email_in_its_identity() {
        use base64::Engine as _;
        let s = Scratch::new("who");
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"email":"a@example.invalid"}"#);
        std::fs::write(s.0.join("auth.json"), format!(r#"{{"tokens":{{"id_token":"x.{claims}.y"}}}}"#)).unwrap();
        assert_eq!(signed_in_as(&CODEX, &s.0).as_deref(), Some("a@example.invalid"));
        std::fs::write(s.0.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"b@example.invalid"}}"#).unwrap();
        assert_eq!(signed_in_as(&CLAUDE, &s.0).as_deref(), Some("b@example.invalid"));
        let none = Scratch::new("none");
        assert_eq!(signed_in_as(&CLAUDE, &none.0), None);
    }

    #[test]
    fn ids_are_folder_names() {
        assert!(valid_id("a1b2-c3"));
        for bad in ["", "../x", "a b", "a/b", &"x".repeat(41)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }
}

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
    /// Variables that sign the CLI in by themselves, ahead of anything in
    /// its folder: taken out of a tab running as an account, or every
    /// account would quietly be the one the variable names
    pub strip: &'static [&'static str],
    /// What the account's own sign-in is kept in, and is never copied, in
    /// the folder it is about: the conversations it kept for itself,
    /// carried into the usual folder when the account is taken away
    pub own_records: &'static [&'static str],
    /// The CLI's own command that signs it in and ends, which a tab opened
    /// to sign an account in runs
    pub login: &'static str,
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
    strip: &["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN", "AWS_BEARER_TOKEN_BEDROCK", "ANTHROPIC_CUSTOM_HEADERS"],
    own_records: &[],
    login: "claude auth login",
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
    strip: &["OPENAI_API_KEY", "CODEX_API_KEY"],
    own_records: &["sessions", "archived_sessions"],
    login: "codex login",
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

/// Where every account's folder is: this machine's own data folder, which
/// nothing syncs. Never the app's folder: a copy of the app kept in a synced
/// folder would carry every account's sign-in to the cloud and to every PC
/// that syncs it -- and a sign-in is this PC's, made on it
pub fn root() -> PathBuf {
    match crate::machine_data_dir() {
        Some(d) => d.join("ShikishaTerm").join("ai-accounts"),
        None => crate::config::state_path("ai-accounts"),
    }
}

/// The folder an account's CLI is pointed at
pub fn dir_of(id: &str) -> PathBuf {
    root().join(id).join("home")
}

/// What tells a folder under [`root`] is an account's, written beside its
/// `home` once that is made up. Nothing is removed from a folder without it
const MARK: &str = "account.json";

/// Written beside `home` once it has been made up whole, the last thing:
/// a making that stopped halfway is made again, copies and all, rather than
/// taken for one that was finished
const MADE: &str = "made";

/// What a tab started as an account is given: the variable and the folder
/// it names, made up for it, and the variables to take out of its
/// environment
pub struct Prepared {
    pub var: &'static str,
    pub dir: PathBuf,
    pub strip: &'static [&'static str],
}

/// The program a command runs: its first word without folder or extension,
/// or the word after `env` and the variables `env` is given
pub fn program_of(argv: &[String]) -> String {
    let stem = |w: &str| {
        Path::new(w).file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
    };
    let mut words = argv.iter().map(String::as_str);
    let first = words.next().unwrap_or_default();
    if stem(first) != "env" {
        return stem(first);
    }
    words.find(|w| !w.starts_with('-') && !w.contains('=')).map(stem).unwrap_or_default()
}

/// What a tab started with account `id` is given, with the folder made up
/// for it. `argv` is what the tab runs: an account of another CLI is
/// refused rather than handed to a program that would not read it
pub fn prepare(id: &str, argv: &[String]) -> Result<Prepared, String> {
    let account = find(id).ok_or_else(|| crate::i18n::tp("msg.account.gone", &[("id", id)]))?;
    let k = kind(&account.cli).ok_or_else(|| crate::i18n::tp("msg.account.gone", &[("id", id)]))?;
    let program = program_of(argv);
    if program != k.cli {
        return Err(crate::i18n::tp("msg.account.other_cli", &[("name", &account.shown()), ("cli", &program)]));
    }
    let dir = dir_of(&account.id);
    let usual = usual_dir(k).ok_or_else(|| crate::i18n::t("msg.account.no_home"))?;
    make_up(k, &usual, &dir, &account).map_err(|e| {
        crate::append_hook_log(&format!("account {}: {e:#}", account.id));
        crate::i18n::tp("msg.account.not_made", &[("name", &account.shown()), ("why", &format!("{e:#}"))])
    })?;
    Ok(Prepared { var: k.env, dir, strip: k.strip })
}

/// An account's folder, made up as the module's notes say. Each step is one
/// that may be taken again: a folder made up before is checked and left as
/// it is, and whatever the person put in its place is never replaced
fn make_up(k: &Kind, usual: &Path, dir: &Path, account: &crate::config::AiAccountSpec) -> Result<()> {
    let made = dir.parent().map(|above| above.join(MADE)).unwrap_or_default();
    let first = !made.is_file();
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
        crate::crypto::write_atomic(&made, "")?;
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
    // The usual folder in the home folder is read in by the one spelling an
    // import surely takes; anywhere else, a path with a drive letter or a
    // space in it may not be, and the memory is copied, kept up when the
    // usual one is newer
    let memory = dir.join("CLAUDE.md");
    let in_home = crate::home_dir().is_some_and(|h| crate::same_folder(&h.join(CLAUDE.dir), usual));
    if usual.join("CLAUDE.md").is_file() {
        match in_home {
            true if !memory.exists() => std::fs::write(&memory, "@~/.claude/CLAUDE.md\n")?,
            true => {}
            false => link_file(&usual.join("CLAUDE.md"), &memory)?,
        }
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
    matches!(
        key,
        "oauthAccount"
            | "userID"
            | "machineID"
            | "primaryApiKey"
            | "customApiKeyResponses"
            | "hasAvailableSubscription"
            | "subscriptionNoticeCount"
            | "claudeCodeFirstTokenDate"
            | "firstStartTime"
            | "lastSeenOrgDefaultUpdatedAt"
            | "artifactRosterDenied"
            | "additionalModelOptionsAnsweredAt"
            | "startupPrefetchedAt"
    )
        || key.contains("Cache")
        || key.starts_with("cached")
        || key.starts_with("passes")
        || key.starts_with("s1m")
}

/// `link` made to point at the folder `target`. A link already there is
/// left when it points there and pointed again when it does not; a real
/// folder with something in it is the person's and is left as it is. An
/// empty one is taken for one the CLI made before the link was, and replaced
fn link_dir(target: &Path, link: &Path) -> Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(link) {
        if meta.file_type().is_symlink() || is_junction(link) {
            // Ours, made when the usual folder was elsewhere (the person has
            // set the CLI's variable since): pointed at where it is now
            let same = match (std::fs::canonicalize(link), std::fs::canonicalize(target)) {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            };
            if same {
                return Ok(());
            }
            if std::fs::remove_dir(link).is_err() {
                std::fs::remove_file(link).with_context(|| format!("re-point {}", link.display()))?;
            }
            crate::append_hook_log(&format!("account folder: {} pointed again at {}", link.display(), target.display()));
        } else if meta.is_dir() && std::fs::read_dir(link).map(|mut d| d.next().is_none()).unwrap_or(false) {
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

/// An account's folder taken away, with what it was signed in as. Its own
/// records go to the usual folder first; then the links, before anything is
/// removed -- a removal that walked into one would take the usual folder's
/// conversations with it. A folder without the mark is not removed
pub fn forget(id: &str) -> Result<()> {
    anyhow::ensure!(valid_id(id), "not an account's id: {id}");
    let above = root().join(id);
    if !above.exists() {
        return Ok(());
    }
    anyhow::ensure!(above.join(MARK).is_file(), "{} is not an account's folder", above.display());
    // Which CLI it is an account of, as the mark wrote it when it was made
    let mark: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(above.join(MARK))?).unwrap_or_default();
    let cli = mark.get("cli").and_then(|c| c.as_str()).unwrap_or_default().to_string();
    let cli = cli.as_str();
    let home = above.join("home");
    // What the account kept for itself goes on being found: its records and
    // its typed-prompt history carried into the usual folder first
    if let Some(k) = kind(cli)
        && let Some(usual) = usual_dir(k)
    {
        carry_records(k, &home, &usual)?;
    }
    crate::worktree::unhook_links(&above);
    std::fs::remove_dir_all(&home).with_context(|| format!("remove {}", home.display()))?;
    // The mark goes last: a removal that stopped halfway leaves a folder
    // that is still known for an account's, to be removed again
    for e in std::fs::read_dir(&above).into_iter().flatten().flatten() {
        if e.file_name() != MARK {
            let p = e.path();
            let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        }
    }
    std::fs::remove_dir_all(&above).with_context(|| format!("remove {}", above.display()))
}

/// The records an account kept in its own folder, moved into the usual
/// folder (a file already there of the same name is left, and so is the
/// account's), and its prompt history added to the end of the usual one
fn carry_records(k: &Kind, home: &Path, usual: &Path) -> Result<()> {
    fn move_tree(from: &Path, to: &Path) -> Result<()> {
        if std::fs::symlink_metadata(from).is_ok_and(|m| m.file_type().is_symlink()) || is_junction(from) {
            return Ok(());
        }
        let Ok(entries) = std::fs::read_dir(from) else { return Ok(()) };
        std::fs::create_dir_all(to)?;
        for e in entries.flatten() {
            let (src, dst) = (e.path(), to.join(e.file_name()));
            if src.is_dir() {
                move_tree(&src, &dst)?;
            } else if !dst.exists() && std::fs::rename(&src, &dst).is_err() {
                std::fs::copy(&src, &dst).with_context(|| format!("carry {}", src.display()))?;
            }
        }
        Ok(())
    }
    for name in k.own_records {
        move_tree(&home.join(name), &usual.join(name))?;
    }
    let history = home.join("history.jsonl");
    if let Ok(text) = std::fs::read_to_string(&history)
        && !text.trim().is_empty()
    {
        use std::io::Write as _;
        let mut out = std::fs::OpenOptions::new().create(true).append(true).open(usual.join("history.jsonl"))?;
        out.write_all(text.as_bytes())?;
        if !text.ends_with('\n') {
            out.write_all(b"\n")?;
        }
    }
    Ok(())
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

/// Where a tab running as `account` -- or, with none, as the CLI is signed
/// in -- can resume a record `pattern` names from: the one folder the CLI
/// is pointed at, which is the only one it looks in. A record in another
/// account's folder is that account's to resume, not this tab's. Records
/// the accounts share are in the usual folder whoever had them
pub fn patterns_for(pattern: &str, account: Option<&str>) -> Vec<String> {
    for k in KINDS {
        let Some(rest) = under(pattern, k) else { continue };
        let first = rest.trim_start_matches(['/', '\\']).split(['/', '\\']).next().unwrap_or("");
        let own =account.and_then(find).filter(|a| a.cli == k.cli && !k.share.contains(&first));
        return match own {
            Some(a) => vec![format!("{}{rest}", dir_of(&a.id).display())],
            None => usual_dir(k).map(|u| vec![format!("{}{rest}", u.display())]).unwrap_or_default(),
        };
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

    /// The accounts known are one list for the whole program: a test that
    /// sets it holds this while it does
    static KNOWN_HELD: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _held = KNOWN_HELD.lock().unwrap_or_else(|e| e.into_inner());
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
        // Outside the home folder the memory is copied, not imported by a
        // path an import may not take
        assert_eq!(std::fs::read_to_string(dir.join("CLAUDE.md")).unwrap(), "be kind");
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
    fn the_program_a_command_runs_is_read_through_env() {
        let argv = |s: &str| s.split(' ').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(program_of(&argv("claude --resume x")), "claude");
        assert_eq!(program_of(&argv("/usr/local/bin/codex")), "codex");
        #[cfg(windows)]
        assert_eq!(program_of(&argv("C:\\tools\\codex.cmd")), "codex");
        assert_eq!(program_of(&argv("env claude auth login")), "claude");
        assert_eq!(program_of(&argv("env -i FOO=1 codex login")), "codex");
        assert_eq!(program_of(&argv("npx claude")), "npx");
    }

    #[test]
    fn a_tab_resumes_only_from_the_folder_its_cli_looks_in() {
        let _held = KNOWN_HELD.lock().unwrap_or_else(|e| e.into_inner());
        set_known(&[account("work", "codex"), account("home", "claude")]);
        let codex = "{home}/.codex/sessions/*/*/*/rollout-*-{id}.jsonl";
        let own = patterns_for(codex, Some("work"));
        assert_eq!(own.len(), 1);
        assert!(own[0].starts_with(&dir_of("work").display().to_string()), "{own:?}");
        let usual = patterns_for(codex, None);
        assert_eq!(usual.len(), 1);
        assert!(!usual[0].starts_with(&dir_of("work").display().to_string()));
        // Shared records are in the usual folder, whichever account
        let claude = patterns_for("{home}/.claude/projects/*/{id}.jsonl", Some("home"));
        assert!(!claude[0].starts_with(&dir_of("home").display().to_string()));
        // An account of another CLI is not this tab's
        assert_eq!(patterns_for(codex, Some("home")), usual);
        set_known(&[]);
    }

    #[test]
    fn a_making_that_stopped_halfway_is_made_again_whole() {
        let s = Scratch::new("half");
        let usual = s.0.join("usual");
        std::fs::create_dir_all(&usual).unwrap();
        std::fs::write(usual.join("settings.json"), "{}").unwrap();
        let dir = s.0.join("acct").join("home");
        // A folder from a making that stopped before its copies
        std::fs::create_dir_all(&dir).unwrap();
        make_up(&CLAUDE, &usual, &dir, &account("acct", "claude")).unwrap();
        assert!(dir.join("settings.json").is_file(), "the copies of a half-made folder were never made");
        assert!(dir.parent().unwrap().join(MADE).is_file());
    }

    #[test]
    fn records_an_account_kept_go_to_the_usual_folder_before_it_goes() {
        let s = Scratch::new("carry");
        let usual = s.0.join("usual");
        let home = s.0.join("acct").join("home");
        std::fs::create_dir_all(home.join("sessions").join("2026").join("10")).unwrap();
        std::fs::write(home.join("sessions").join("2026").join("10").join("rollout-a.jsonl"), "{}").unwrap();
        std::fs::write(home.join("history.jsonl"), "{\"t\":1}\n").unwrap();
        std::fs::create_dir_all(usual.join("sessions").join("2026").join("10")).unwrap();
        std::fs::write(usual.join("sessions").join("2026").join("10").join("rollout-a.jsonl"), "usual").unwrap();
        std::fs::write(usual.join("history.jsonl"), "{\"t\":0}\n").unwrap();
        carry_records(&CODEX, &home, &usual).unwrap();
        assert_eq!(std::fs::read_to_string(usual.join("history.jsonl")).unwrap(), "{\"t\":0}\n{\"t\":1}\n");
        assert_eq!(
            std::fs::read_to_string(usual.join("sessions").join("2026").join("10").join("rollout-a.jsonl")).unwrap(),
            "usual",
            "a record already there was overwritten"
        );
        std::fs::write(home.join("sessions").join("2026").join("10").join("rollout-b.jsonl"), "{}").unwrap();
        carry_records(&CODEX, &home, &usual).unwrap();
        assert!(usual.join("sessions").join("2026").join("10").join("rollout-b.jsonl").is_file());
    }

    #[test]
    fn ids_are_folder_names() {
        assert!(valid_id("a1b2-c3"));
        for bad in ["", "../x", "a b", "a/b", &"x".repeat(41)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }
}

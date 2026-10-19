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
//! checkout the CLI does not trust -- or that the CLI was told not to trust --
//! is left for the CLI to ask about, and that is looked at again, under the
//! CLI's own lock, at the moment of writing. The file is the CLI's, so nothing
//! is written into it until the person has agreed to that, once for each CLI
//! ([`crate::config::Config::worktree_trust`]): asked on the board the first
//! time it would matter, and changed in the settings after. A file that cannot
//! be read is never taken for an empty one.
//!
//! Which CLIs: those that ask again in a worktree. Claude Code and Codex CLI
//! do not -- each takes a worktree of a project it trusts as trusted (checked
//! with the CLIs themselves) -- so nothing is written for them. Gemini CLI
//! asks, and keeps its answers in `~/.gemini/trustedFolders.json`; how it
//! reads that file is followed here (its rules, links followed, its lock).
//!
//! On a MicroVM the machine is the app's own, made for the project, and its
//! AIs are carried over without asking ([`crate::microvm::trust_as_checkout`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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

/// How long a worktree waits, the first time, for the person to answer the
/// question about a CLI before its tabs start: a CLI started first would
/// already have asked on its own screen. Once answered it never waits again
const WAIT_FOR_ANSWER: Duration = Duration::from_secs(90);

/// How long anything asked of a server may take
const FAR_WAIT_MS: u64 = 20_000;

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

// ── Gemini CLI's rules ──────────────────────────────────────────────────────

const TRUST_FOLDER: &str = "TRUST_FOLDER";
const TRUST_PARENT: &str = "TRUST_PARENT";
const DO_NOT_TRUST: &str = "DO_NOT_TRUST";

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

/// The folder above, as Gemini CLI takes it (`path.dirname`)
fn parent_of(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = p.trim_end_matches('/');
    match p.rsplit_once('/') {
        Some(("", _)) => "/".to_string(),
        Some((up, _)) if up.ends_with(':') => format!("{up}/"),
        Some((up, _)) => up.to_string(),
        None => p.to_string(),
    }
}

/// The text of a file Gemini CLI reads, without the comments it allows in
/// it (`//` and `/* */`, outside strings), so it reads as JSON
fn without_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    out
}

/// The rules in Gemini CLI's file: each written path, its word, and the
/// folder it holds (a parent's rule holds the parent). `None` when the file
/// is not one the CLI would read -- the CLI refuses all of it then
fn gemini_rules(text: &str) -> Option<Vec<(String, String, String)>> {
    if text.trim().is_empty() {
        return Some(Vec::new());
    }
    let doc: serde_json::Value = serde_json::from_str(&without_comments(text)).ok()?;
    let mut out = Vec::new();
    for (path, level) in doc.as_object()? {
        let level = level.as_str()?;
        let held = match level {
            TRUST_PARENT => parent_of(path),
            TRUST_FOLDER | DO_NOT_TRUST => path.clone(),
            _ => return None,
        };
        out.push((path.clone(), level.to_string(), held));
    }
    Some(out)
}

/// Whether Gemini CLI trusts `folder`, read the way it reads its rules: each
/// rule's folder taken as it really is (`real`, links followed), and of the
/// rules whose folder holds this one, the one written with the longest path
/// decides. `None` when no rule says, or the file is not one it would read
fn gemini_trusts(text: &str, folder: &str, fold: bool, real: &dyn Fn(&str) -> String) -> Option<bool> {
    let at = gemini_key(&real(folder), fold);
    let mut best: Option<(usize, String)> = None;
    for (path, level, held) in gemini_rules(text)? {
        let held = gemini_key(&real(&held), fold);
        if within(&held, &at) && best.as_ref().is_none_or(|(n, _)| path.len() > *n) {
            best = Some((path.len(), level));
        }
    }
    best.map(|(_, level)| level != DO_NOT_TRUST)
}

/// Gemini CLI's file with `folder` trusted, as the CLI writes it when the
/// person says yes: the folder, as it compares folders, said to be trusted,
/// and the file written out as the CLI writes it. `None` when the file is
/// not one the CLI could read -- it is the person's to put right
fn gemini_with(text: &str, folder: &str, fold: bool) -> Option<String> {
    let mut doc: serde_json::Value =
        if text.trim().is_empty() { serde_json::json!({}) } else { serde_json::from_str(&without_comments(text)).ok()? };
    doc.as_object_mut()?.insert(gemini_key(folder, fold), serde_json::json!(TRUST_FOLDER));
    serde_json::to_string_pretty(&doc).ok()
}

/// Whether Gemini CLI asks about folders at all: it does unless its settings
/// turn that off (`security.folderTrust.enabled`)
fn gemini_asks(settings: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(&without_comments(settings))
        .ok()
        .and_then(|s| s.pointer("/security/folderTrust/enabled").and_then(|v| v.as_bool()))
        .unwrap_or(true)
}

// ── Where the file is, and the folders as the machine has them ──────────────

/// Gemini CLI's files on one machine, and how folders are told apart there
struct Place {
    at: At,
    /// The file of trusted folders, as it really is (a link followed)
    file: String,
    /// Whether the file is there at all
    exists: bool,
    settings: String,
    fold: bool,
}

/// Where the CLI keeps its file on this PC: where it is told to keep it,
/// else in its folder in the home (`GEMINI_CLI_HOME` standing in for the
/// home, as the CLI reads it)
fn gemini_dir_here() -> Option<PathBuf> {
    let home = std::env::var_os("GEMINI_CLI_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).or_else(crate::home_dir)?;
    Some(home.join(".gemini"))
}

fn gemini_file_here() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("GEMINI_CLI_TRUSTED_FOLDERS_PATH").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    gemini_dir_here().map(|d| d.join("trustedFolders.json"))
}

/// A path as it really is on this PC, links followed, and without the prefix
/// Windows puts on a path given back that way; as given when it is not there
fn real_here(path: &str) -> String {
    let Ok(p) = std::fs::canonicalize(path) else { return path.to_string() };
    let s = p.display().to_string();
    if let Some(unc) = s.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

fn there(host: &crate::config::HostSpec) -> Result<crate::elsewhere::Elsewhere, String> {
    crate::elsewhere::Elsewhere::of(host).map_err(|e| format!("{e:#}"))
}

/// Run a script on the server, with these words after it as `$1`, `$2`...
fn far_run(host: &crate::config::HostSpec, script: &str, args: &[&str]) -> Result<String, String> {
    let quoted: Vec<String> = args.iter().map(|a| crate::ssh::sh_quote(a)).collect();
    let command = format!("sh -c {} sh {}", crate::ssh::sh_quote(script), quoted.join(" "));
    let ran = crate::elsewhere::exec(&there(host)?, &command, FAR_WAIT_MS).map_err(|e| format!("{e:#}"))?;
    match ran.ok() {
        true => Ok(ran.out),
        false => Err(ran.said()),
    }
}

/// Each path the way the server has it: `~` as the home, made whole, and
/// links followed where the path is there (as the CLI follows them); one
/// line each, in order
const FAR_REAL: &str = r#"for p in "$@"; do
  case "$p" in "~") p="$HOME";; "~/"*) p="$HOME/${p#\~/}";; esac
  case "$p" in /*) ;; *) p="$HOME/$p";; esac
  if [ -e "$p" ]; then r=$(cd "$p" 2>/dev/null && pwd -P) || r=$(readlink -f -- "$p" 2>/dev/null) || r="$p"; else r="$p"; fi
  printf '%s\n' "$r"
done"#;

/// Where Gemini CLI keeps its files on the server, in the environment a
/// command there runs in: the file of trusted folders (a link followed),
/// whether it is there, and the settings
const FAR_PLACE: &str = r#"home="${GEMINI_CLI_HOME:-$HOME}"
f="${GEMINI_CLI_TRUSTED_FOLDERS_PATH:-$home/.gemini/trustedFolders.json}"
if [ -e "$f" ]; then f=$(readlink -f -- "$f" 2>/dev/null || printf '%s' "$f"); e=1; else e=0; fi
printf '%s\n%s\n%s\n' "$f" "$e" "$home/.gemini/settings.json""#;

/// Where the CLI's files are, on the machine the worktree is on
fn place(at: &At) -> Result<Place, String> {
    match at {
        At::Here => {
            let file = gemini_file_here().ok_or("there is no home folder")?;
            let settings = gemini_dir_here().map(|d| d.join("settings.json")).unwrap_or_default();
            let exists = file.exists();
            let file = if exists { real_here(&file.display().to_string()) } else { file.display().to_string() };
            Ok(Place {
                at: at.clone(),
                file,
                exists,
                settings: settings.display().to_string(),
                fold: cfg!(windows) || cfg!(target_os = "macos"),
            })
        }
        At::Far(host) => {
            let out = far_run(host, FAR_PLACE, &[])?;
            let mut lines = out.lines();
            let (Some(file), Some(e), Some(settings)) = (lines.next(), lines.next(), lines.next()) else {
                return Err(format!("the server said {out:?} about where Gemini CLI's files are"));
            };
            Ok(Place { at: at.clone(), file: file.into(), exists: e == "1", settings: settings.into(), fold: false })
        }
    }
}

impl Place {
    /// Read a file of the CLI's. `None` when it is not there; an error when
    /// it is there and could not be read, which is never taken for empty
    fn read(&self, path: &str, exists: bool) -> Result<Option<String>, String> {
        match &self.at {
            At::Here => match std::fs::read(path) {
                Ok(b) => Ok(Some(String::from_utf8_lossy(&b).into_owned())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(format!("{path}: {e}")),
            },
            At::Far(host) => {
                if !exists {
                    return Ok(None);
                }
                match crate::elsewhere::files(&there(host)?, crate::ssh::FileJob::Read { path: path.into() }, FAR_WAIT_MS) {
                    Ok(crate::ssh::FileAnswer::Bytes(b)) => Ok(Some(String::from_utf8_lossy(&b).into_owned())),
                    Ok(_) => Err(format!("{path}: not a file")),
                    Err(e) => Err(format!("{path}: {e:#}")),
                }
            }
        }
    }

    fn settings_text(&self) -> Option<String> {
        match &self.at {
            At::Here => std::fs::read_to_string(&self.settings).ok(),
            At::Far(_) => self.read(&self.settings, true).ok().flatten(),
        }
    }

    /// The folders, as they really are on the machine: every one asked at
    /// once, so a server is asked one question
    fn reals(&self, paths: &[String]) -> Result<HashMap<String, String>, String> {
        let mut out = HashMap::new();
        match &self.at {
            At::Here => {
                for p in paths {
                    out.insert(p.clone(), real_here(p));
                }
            }
            At::Far(host) => {
                let args: Vec<&str> = paths.iter().map(String::as_str).collect();
                let said = far_run(host, FAR_REAL, &args)?;
                let lines: Vec<&str> = said.lines().collect();
                if lines.len() != paths.len() {
                    return Err(format!("the server gave {} folders for {}", lines.len(), paths.len()));
                }
                for (p, r) in paths.iter().zip(lines) {
                    out.insert(p.clone(), r.to_string());
                }
            }
        }
        Ok(out)
    }

    /// How the CLI stands on `worktree`, by the file as it is now: whether
    /// it trusts the checkout and says nothing about the worktree -- the
    /// only case a yes is carried in
    fn would_ask(&self, text: &str, checkout: &str, worktree: &str) -> Result<bool, String> {
        let Some(rules) = gemini_rules(text) else { return Ok(false) };
        let mut paths: Vec<String> = rules.into_iter().map(|(_, _, held)| held).collect();
        paths.push(checkout.to_string());
        paths.push(worktree.to_string());
        let reals = self.reals(&paths)?;
        let real = |p: &str| reals.get(p).cloned().unwrap_or_else(|| p.to_string());
        Ok(gemini_trusts(text, checkout, self.fold, &real) == Some(true)
            && gemini_trusts(text, worktree, self.fold, &real).is_none())
    }
}

// ── Gemini CLI's lock, here and on a server ─────────────────────────────────

/// The CLI's own lock on its file, taken the way it takes it (a folder named
/// for the file as it really is, made only if it is not there), so a yes the
/// CLI is writing at that moment is not written over. Waited for a second;
/// one older than the CLI's own limit for a lock was left by a CLI that
/// stopped
enum Locked {
    Here(PathBuf),
    Far(crate::config::HostSpec, String),
}

/// Take the lock on a server: made there if it is not, or stale and taken
const FAR_LOCK: &str = r#"l="$1.lock"; i=0
while ! mkdir "$l" 2>/dev/null; do
  if [ -n "$(find "$l" -maxdepth 0 -mmin +0.17 2>/dev/null)" ]; then rmdir "$l" 2>/dev/null; continue; fi
  i=$((i+1)); [ "$i" -gt 20 ] && exit 9; sleep 0.05 2>/dev/null || sleep 1
done"#;

impl Locked {
    fn take(place: &Place) -> Result<Locked, String> {
        match &place.at {
            At::Here => {
                let lock = PathBuf::from(format!("{}.lock", place.file));
                if let Some(dir) = lock.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                for _ in 0..20 {
                    match std::fs::create_dir(&lock) {
                        Ok(()) => return Ok(Locked::Here(lock)),
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            let stale = std::fs::metadata(&lock)
                                .and_then(|m| m.modified())
                                .ok()
                                .and_then(|t| t.elapsed().ok())
                                .is_some_and(|age| age > Duration::from_secs(10));
                            if stale {
                                let _ = std::fs::remove_dir(&lock);
                                continue;
                            }
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Err(e) => return Err(format!("{}: {e}", lock.display())),
                    }
                }
                Err("Gemini CLI is writing its file".into())
            }
            At::Far(host) => {
                far_run(host, &format!("mkdir -p \"$(dirname \"$1\")\" && {{ {FAR_LOCK}\n}}"), &[&place.file])
                    .map_err(|e| format!("Gemini CLI's file is being written, or the lock could not be taken: {e}"))?;
                Ok(Locked::Far(host.clone(), place.file.clone()))
            }
        }
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        match self {
            Locked::Here(lock) => {
                let _ = std::fs::remove_dir(&*lock);
            }
            Locked::Far(host, file) => {
                let _ = far_run(host, r#"rmdir "$1.lock" 2>/dev/null; true"#, &[file]);
            }
        }
    }
}

/// Put a whole file in place on a server: written beside it, then moved over
/// it in one step, so the CLI never reads half of one
fn far_put(host: &crate::config::HostSpec, file: &str, text: &str) -> Result<(), String> {
    let tmp = format!("{file}.tmp.{}", std::process::id());
    crate::elsewhere::files(&there(host)?, crate::ssh::FileJob::Write { to: tmp.clone(), bytes: text.as_bytes().to_vec() }, FAR_WAIT_MS)
        .map_err(|e| format!("{e:#}"))?;
    far_run(host, r#"chmod 600 "$1" 2>/dev/null; mv -f "$1" "$2""#, &[&tmp, file]).map(|_| ())
}

/// Write the yes for `worktree` into the CLI's file -- if, read again under
/// its lock, the CLI still trusts the checkout and says nothing yet about the
/// worktree. Answers whether it was written
fn write(keeper: Keeper, at: &At, checkout: &str, worktree: &str) -> Result<bool, String> {
    debug_assert_eq!(keeper, GEMINI);
    let place = place(at)?;
    let _lock = Locked::take(&place)?;
    let text = place.read(&place.file, place.exists)?.unwrap_or_default();
    if !place.would_ask(&text, checkout, worktree)? {
        return Ok(false);
    }
    let real = place.reals(&[worktree.to_string()])?.remove(worktree).unwrap_or_else(|| worktree.to_string());
    let out = gemini_with(&text, &real, place.fold).ok_or("Gemini CLI's file is not one it could read")?;
    match at {
        At::Here => crate::crypto::write_atomic(Path::new(&place.file), &out).map_err(|e| e.to_string())?,
        At::Far(host) => far_put(host, &place.file, &out)?,
    }
    Ok(true)
}

/// Whether the CLI, on that machine, asks about folders at all, trusts the
/// checkout and would ask about the worktree
fn would_ask(keeper: Keeper, at: &At, checkout: &str, worktree: &str) -> Result<bool, String> {
    debug_assert_eq!(keeper, GEMINI);
    let place = place(at)?;
    let Some(text) = place.read(&place.file, place.exists)? else { return Ok(false) };
    if !place.settings_text().as_deref().is_none_or(gemini_asks) {
        return Ok(false);
    }
    place.would_ask(&text, checkout, worktree)
}

// ── The question ────────────────────────────────────────────────────────────

/// A worktree waiting on the person's answer about a CLI
#[derive(Debug, Clone)]
struct Waiting {
    at: At,
    checkout: String,
    worktree: String,
    /// Its making is still waiting for the answer, and writes the yes itself
    held: Arc<std::sync::atomic::AtomicBool>,
}

/// The question about one CLI, as the board shows it
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
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

struct Question {
    ai: &'static str,
    waiting: Vec<Waiting>,
}

static ASKING: Mutex<Vec<Question>> = Mutex::new(Vec::new());
/// Told when a question is answered or put away
static ANSWERED: Condvar = Condvar::new();

/// The questions up, one for each CLI
pub fn asking() -> Vec<Asking> {
    ASKING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(|q| {
            let first = q.waiting.first()?;
            Some(Asking {
                ai: q.ai.into(),
                folder: first.worktree.clone(),
                checkout: first.checkout.clone(),
                machine: first.at.shown(),
                more: q.waiting.len() - 1,
            })
        })
        .collect()
}

fn ask(keeper: Keeper, w: Waiting) {
    let mut all = ASKING.lock().unwrap_or_else(|e| e.into_inner());
    match all.iter_mut().find(|q| q.ai == keeper.name) {
        Some(q) => {
            if !q.waiting.iter().any(|x| x.at == w.at && x.worktree == w.worktree) {
                q.waiting.push(w);
            }
        }
        None => all.push(Question { ai: keeper.name, waiting: vec![w] }),
    }
}

/// What could not be done about an answer, for the board to say once
static SAID: Mutex<Option<String>> = Mutex::new(None);

/// What is to be said, since this was last asked
pub fn take_said() -> Option<String> {
    SAID.lock().unwrap_or_else(|e| e.into_inner()).take()
}

fn say(what: String) {
    *SAID.lock().unwrap_or_else(|e| e.into_inner()) = Some(what);
}

/// The person's answer about `ai`, from the board: yes or no, kept, and done
/// for every worktree that waited on it; or put away (`None`), and asked
/// again with the next worktree. What could not be done is said on the board
pub fn answered(ai: &str, yes: Option<bool>) {
    if let Err(e) = answer(ai, yes) {
        say(e);
    }
}

/// The answer kept, and the question taken down. The worktrees whose making
/// still waits write their own yes; the others are written on a thread of
/// their own, so whoever answered is not kept waiting on a server
fn answer(ai: &str, yes: Option<bool>) -> Result<(), String> {
    let Some(keeper) = KEEPERS.into_iter().find(|k| k.name == ai) else { return Ok(()) };
    if let Some(yes) = yes
        && !crate::config::save_worktree_trust(ai, yes)
    {
        return Err(crate::i18n::t("msg.trust.not_saved"));
    }
    let taken: Vec<Waiting> = {
        let mut all = ASKING.lock().unwrap_or_else(|e| e.into_inner());
        let i = all.iter().position(|q| q.ai == ai);
        let q = i.map(|i| all.remove(i));
        ANSWERED.notify_all();
        q.map(|q| q.waiting).unwrap_or_default()
    };
    match yes {
        None => crate::append_hook_log(&format!("worktree trust: the question about {ai} was put away")),
        Some(yes) => crate::append_hook_log(&format!("worktree trust: the person said {} for {ai}", if yes { ON } else { OFF })),
    }
    if yes != Some(true) {
        return Ok(());
    }
    let left: Vec<Waiting> = taken.into_iter().filter(|w| !w.held.load(std::sync::atomic::Ordering::SeqCst)).collect();
    if !left.is_empty() {
        std::thread::spawn(move || {
            for w in left {
                carry(keeper, &w.at, &w.checkout, &w.worktree);
            }
        });
    }
    Ok(())
}

/// Write the yes for one worktree, and say what became of it
fn carry(keeper: Keeper, at: &At, checkout: &str, worktree: &str) {
    match write(keeper, at, checkout, worktree) {
        Ok(true) => crate::append_hook_log(&format!("worktree trust: {worktree} is trusted by {} as {checkout} is", keeper.name)),
        Ok(false) => crate::append_hook_log(&format!(
            "worktree trust: {worktree} was not written for {}: it no longer trusts {checkout}, or was told about the worktree",
            keeper.name
        )),
        Err(e) => {
            crate::append_hook_log(&format!("worktree trust: {worktree} could not be written into {}'s file: {e}", keeper.name));
            say(crate::i18n::tp("msg.trust.not_written", &[("ai", keeper.name), ("folder", worktree)]));
        }
    }
}

/// A worktree was just made from `checkout`: each CLI that trusts the
/// checkout and would ask about the worktree is told the same yes, if the
/// person agreed to that; asked whether to, if they have not been -- and the
/// making waits a while for the answer, so the worktree's tabs start with it
/// written; and left to ask on its own screen if they said no. Called on the
/// thread making the worktree, before its tabs are started. `stop` is the
/// making being stopped, which stops the waiting too
pub fn worktree_made(at: At, checkout: &Path, worktree: &Path, stop: &dyn Fn() -> bool) {
    let path = |p: &Path| match at {
        At::Here => real_here(&p.display().to_string()),
        At::Far(_) => p.display().to_string().replace('\\', "/"),
    };
    let (checkout, worktree) = (path(checkout), path(worktree));
    let answers = crate::config::load().map(|c| c.worktree_trust).unwrap_or_default();
    for keeper in KEEPERS {
        let answer = answers.get(keeper.name).map(String::as_str);
        // Said no: nothing of the CLI's is even read
        if answer == Some(OFF) {
            continue;
        }
        match would_ask(keeper, &at, &checkout, &worktree) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(e) => {
                crate::append_hook_log(&format!("worktree trust: {}'s file could not be read for {worktree}: {e}", keeper.name));
                continue;
            }
        }
        if answer == Some(ON) {
            carry(keeper, &at, &checkout, &worktree);
            continue;
        }
        let held = Arc::new(std::sync::atomic::AtomicBool::new(true));
        ask(keeper, Waiting { at: at.clone(), checkout: checkout.clone(), worktree: worktree.clone(), held: held.clone() });
        // Waited for, the first time: the person is looking at the question
        let until = Instant::now() + WAIT_FOR_ANSWER;
        let mut all = ASKING.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let up = all.iter().any(|q| q.ai == keeper.name && q.waiting.iter().any(|w| Arc::ptr_eq(&w.held, &held)));
            if !up {
                drop(all);
                let now = crate::config::load().and_then(|c| c.worktree_trust.get(keeper.name).cloned());
                if now.as_deref() == Some(ON) {
                    carry(keeper, &at, &checkout, &worktree);
                }
                break;
            }
            if Instant::now() >= until || stop() {
                // Left on the question: written when it is answered
                held.store(false, std::sync::atomic::Ordering::SeqCst);
                break;
            }
            all = ANSWERED.wait_timeout(all, Duration::from_millis(500)).unwrap_or_else(|e| e.into_inner()).0;
        }
    }
}

/// The answer changed in the settings: kept at once, and -- when the
/// question about that CLI is up -- that question answered with it, its
/// worktrees written on a thread of their own
pub fn set(ai: &str, on: bool) -> Result<(), String> {
    if !KEEPERS.iter().any(|k| k.name == ai) {
        return Err(format!("{ai} keeps no list of trusted folders this app writes"));
    }
    answer(ai, Some(on))
}

/// How each CLI stands, for the settings: its name, the answer kept
/// ([`ON`], [`OFF`], or empty: not asked yet), and the file the yes goes in
/// on this PC
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

    fn same(p: &str) -> String {
        p.to_string()
    }

    #[test]
    fn a_folder_reads_as_gemini_compares_it() {
        assert_eq!(gemini_key(r"C:\Users\Me\Proj\", true), "c:/users/me/proj");
        assert_eq!(gemini_key("C:/", true), "c:/");
        assert_eq!(gemini_key("/home/Me/proj/", false), "/home/Me/proj");
        assert_eq!(gemini_key(r"\\nas\share\x", true), "//nas/share/x");
        assert_eq!(parent_of("d:/work/site/child"), "d:/work/site");
        assert_eq!(parent_of("c:/x"), "c:/");
        assert_eq!(parent_of("/x"), "/");
    }

    #[test]
    fn the_longest_rule_that_holds_the_folder_decides() {
        let text = r#"{
  "c:/users/me": "TRUST_FOLDER",
  "c:/users/me/secret": "DO_NOT_TRUST",
  "d:/work/site/child": "TRUST_PARENT"
}"#;
        let t = |f: &str| gemini_trusts(text, f, true, &same);
        assert_eq!(t(r"C:\Users\Me\proj"), Some(true), "under a trusted folder");
        assert_eq!(t(r"C:\Users\Me\secret\x"), Some(false), "a longer rule says no");
        assert_eq!(t(r"D:\work\site"), Some(true), "a parent's rule holds the parent");
        assert_eq!(t(r"D:\work\site-feature"), None, "a sibling with a longer name is not inside");
        assert_eq!(t(r"E:\x"), None);
        assert_eq!(gemini_trusts(r#"{"c:/a": "MAYBE"}"#, r"c:\a", true, &same), None, "a file the CLI refuses says nothing");
        assert_eq!(gemini_trusts("not json", r"c:\a", true, &same), None);
    }

    #[test]
    fn a_rule_is_read_where_its_link_leads() {
        // A refusal written through a link to the checkout: the CLI follows
        // the link, so the refusal is about the checkout
        let text = r#"{"c:/trusted": "TRUST_FOLDER", "c:/elsewhere/link-to-checkout": "DO_NOT_TRUST"}"#;
        let real = |p: &str| if p == "c:/elsewhere/link-to-checkout" { "c:/trusted/checkout".to_string() } else { p.to_string() };
        assert_eq!(gemini_trusts(text, "c:/trusted/checkout", true, &real), Some(false));
        assert_eq!(gemini_trusts(text, "c:/trusted/checkout", true, &same), Some(true), "not followed, it would be trusted");
    }

    #[test]
    fn comments_are_read_as_the_cli_reads_them() {
        let text = "{\n  // mine\n  \"c:/p\": \"TRUST_FOLDER\", /* and \"this\" */\n  \"c:/q//r\": \"DO_NOT_TRUST\"\n}";
        assert_eq!(gemini_trusts(text, "c:/p/x", true, &same), Some(true));
        assert_eq!(gemini_trusts(text, "c:/q//r", true, &same), Some(false), "a // inside a string is no comment");
        assert!(!gemini_asks("{ // off\n \"security\": {\"folderTrust\": {\"enabled\": false}} }"));
    }

    #[test]
    fn case_counts_only_where_the_system_counts_it() {
        let text = r#"{"/home/me/Proj": "TRUST_FOLDER"}"#;
        assert_eq!(gemini_trusts(text, "/home/me/Proj", false, &same), Some(true));
        assert_eq!(gemini_trusts(text, "/home/me/proj", false, &same), None);
        assert_eq!(gemini_trusts(text, "/home/me/proj", true, &same), Some(true));
    }

    #[test]
    fn the_yes_is_written_as_the_cli_writes_it_and_nothing_else_changes() {
        let text = "{\n  \"d:/other\": \"DO_NOT_TRUST\",\n  \"c:/users/me/proj\": \"TRUST_FOLDER\"\n}";
        let out = gemini_with(text, r"C:\Users\Me\proj-feature", true).expect("written");
        assert_eq!(
            out,
            "{\n  \"d:/other\": \"DO_NOT_TRUST\",\n  \"c:/users/me/proj\": \"TRUST_FOLDER\",\n  \"c:/users/me/proj-feature\": \"TRUST_FOLDER\"\n}"
        );
        assert_eq!(gemini_trusts(&out, r"C:\Users\Me\proj-feature", true, &same), Some(true));
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

    fn here(dir: &Path) -> Place {
        let file = dir.join("trustedFolders.json");
        Place {
            at: At::Here,
            exists: file.exists(),
            file: file.display().to_string(),
            settings: dir.join("settings.json").display().to_string(),
            fold: cfg!(windows) || cfg!(target_os = "macos"),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sk-aitrust-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_taken_for_empty() {
        let dir = scratch("unread");
        // A folder where the file should be: there, and not readable as one
        std::fs::create_dir_all(dir.join("trustedFolders.json")).unwrap();
        let p = here(&dir);
        assert!(p.read(&p.file, true).is_err());
        let gone = here(&scratch("none"));
        assert_eq!(gone.read(&gone.file, false), Ok(None), "no file at all is no rules");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refusal_given_while_the_question_waited_is_kept() {
        let dir = scratch("refused");
        let (checkout, worktree) = (dir.join("proj"), dir.join("proj-a"));
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        let (c, w) = (real_here(&checkout.display().to_string()), real_here(&worktree.display().to_string()));
        let fold = cfg!(windows) || cfg!(target_os = "macos");
        let text = serde_json::json!({ gemini_key(&c, fold): TRUST_FOLDER, gemini_key(&w, fold): DO_NOT_TRUST }).to_string();
        std::fs::write(dir.join("trustedFolders.json"), &text).unwrap();
        let p = here(&dir);
        assert_eq!(p.would_ask(&text, &c, &w), Ok(false), "told not to trust the worktree");
        let untrusted = serde_json::json!({ gemini_key(&c, fold): DO_NOT_TRUST }).to_string();
        assert_eq!(p.would_ask(&untrusted, &c, &w), Ok(false), "no longer trusts the checkout");
        let trusted = serde_json::json!({ gemini_key(&c, fold): TRUST_FOLDER }).to_string();
        assert_eq!(p.would_ask(&trusted, &c, &w), Ok(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_question_waits_once_for_each_cli_and_counts_the_worktrees() {
        ASKING.lock().unwrap().clear();
        let w = |f: &str| Waiting { at: At::Here, checkout: "c:/p".into(), worktree: f.into(), held: Default::default() };
        ask(GEMINI, w("c:/p-a"));
        ask(GEMINI, w("c:/p-b"));
        ask(GEMINI, w("c:/p-a"));
        let up = asking();
        assert_eq!(up.len(), 1);
        assert_eq!((up[0].folder.as_str(), up[0].more), ("c:/p-a", 1));
        assert!(answer(GEMINI.name, None).is_ok());
        assert!(asking().is_empty(), "put away");
    }

    /// The scripts a server runs, run here by the same shell
    #[cfg(unix)]
    fn sh(script: &str, args: &[&str], home: &Path, env: &[(&str, &str)]) -> (i32, String) {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", home)
            .envs(env.iter().copied())
            .output()
            .expect("sh runs");
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned())
    }

    #[cfg(unix)]
    #[test]
    fn a_server_gives_folders_as_they_really_are() {
        let home = scratch("far-real");
        let home = std::fs::canonicalize(&home).unwrap();
        std::fs::create_dir_all(home.join("proj")).unwrap();
        std::os::unix::fs::symlink(home.join("proj"), home.join("link")).unwrap();
        let (code, out) = sh(FAR_REAL, &["~/link", "~", "proj", "/nowhere/x", "~/not-yet"], &home, &[]);
        assert_eq!(code, 0);
        let h = home.display().to_string();
        assert_eq!(
            out.lines().collect::<Vec<_>>(),
            vec![format!("{h}/proj"), h.clone(), format!("{h}/proj"), "/nowhere/x".to_string(), format!("{h}/not-yet")]
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn a_server_says_where_gemini_keeps_its_files() {
        let home = std::fs::canonicalize(scratch("far-place")).unwrap();
        let h = home.display().to_string();
        let (_, out) = sh(FAR_PLACE, &[], &home, &[]);
        assert_eq!(out, format!("{h}/.gemini/trustedFolders.json\n0\n{h}/.gemini/settings.json\n"), "none yet");
        std::fs::create_dir_all(home.join("real")).unwrap();
        std::fs::write(home.join("real/trusted.json"), "{}").unwrap();
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::os::unix::fs::symlink(home.join("real/trusted.json"), home.join(".gemini/trustedFolders.json")).unwrap();
        let (_, out) = sh(FAR_PLACE, &[], &home, &[]);
        assert!(out.starts_with(&format!("{h}/real/trusted.json\n1\n")), "the link followed: {out}");
        let (_, out) = sh(FAR_PLACE, &[], &home, &[("GEMINI_CLI_HOME", "/elsewhere")]);
        assert_eq!(out, "/elsewhere/.gemini/trustedFolders.json\n0\n/elsewhere/.gemini/settings.json\n", "its home as it is told");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn a_server_takes_the_lock_once() {
        let home = std::fs::canonicalize(scratch("far-lock")).unwrap();
        let file = home.join("trustedFolders.json").display().to_string();
        assert_eq!(sh(FAR_LOCK, &[&file], &home, &[]).0, 0, "taken");
        assert!(home.join("trustedFolders.json.lock").is_dir());
        assert_eq!(sh(FAR_LOCK, &[&file], &home, &[]).0, 9, "not taken twice");
        std::fs::remove_dir(home.join("trustedFolders.json.lock")).unwrap();
        assert_eq!(sh(FAR_LOCK, &[&file], &home, &[]).0, 0, "taken again once given back");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_lock_is_taken_and_given_back() {
        let dir = scratch("lock");
        let p = here(&dir);
        {
            let held = Locked::take(&p).expect("taken");
            assert!(dir.join("trustedFolders.json.lock").is_dir());
            assert!(Locked::take(&p).is_err(), "not twice");
            drop(held);
        }
        assert!(!dir.join("trustedFolders.json.lock").exists(), "given back");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

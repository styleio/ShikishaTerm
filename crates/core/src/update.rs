//! A newer version: knowing about it, fetching it, and putting it in place.
//!
//! **Nothing here happens without a person pressing for it, except looking.**
//! Once at start and once a day while the program runs, the latest published
//! version is read (one request for a public page, nothing sent). If it is
//! newer, a card asks once -- and both of its answers lead to the same place:
//! the "Update" card in settings, where the one button that fetches, checks
//! and swaps lives. A person who said "not now" finds the same button there
//! later; a person who came from settings presses the same one. One road, so
//! that if it is broken, everyone notices.
//!
//! The download is what a person would do by hand, done for them: fetch the
//! zip, check it against the published SHA256 and the signature this build
//! carries the key for, unpack it beside the program's own data, and -- only
//! when pressed, and only after the same question quitting asks -- swap the
//! files. Windows lets a running executable be renamed but not overwritten,
//! so the swap is rename-and-copy, with a journal written before the first
//! rename so an interrupted swap is put back on the next start. What a person
//! wrote (`config/`, `data/`, `logs/`, `desks/`, `scripts/`) is never
//! overwritten. The version replaced is kept under `data/update/prev/`, so
//! "the previous version" is one press away.
//!
//! An installed (Store) copy runs from a folder that is read-only to it, and
//! the Store is what replaces it -- but the Store can only do that once the
//! program is closed, and a program that lives in the notification area is
//! rarely closed. So the Store copy asks the Store whether an update waits,
//! shows the same card and the same button, and that button lets the Store
//! install it and starts the program again afterwards. The Store says only
//! *that* one waits, never its number, so the Store's offer is drawn without
//! one and remembered by the version it would replace.
//!
//! All state lives behind one mutex that the drawing, the settings server
//! and the fetching thread each glance at. Nothing here may stop the program:
//! every failure is a phase on the card, never an error dialog.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;

/// The key that every published zip is signed with, as hex. A zip whose
/// signature this key does not accept is not put in place, whatever its
/// SHA256 says -- the SHA256 comes from the same place as the zip and proves
/// only that the download arrived whole. Rotating this key means every copy
/// built before the rotation refuses the zips signed after it, and has to
/// be replaced by hand once
pub const PUBLIC_KEY_HEX: &str = "935ea2b2bd6c6547c21384d01b35aa57a9e5027349a220c215eef70fea473bb4";

/// Where the Store copy's "what changed" leads
const STORE_URL: &str = "https://apps.microsoft.com/detail/9PB8XQVM87Z0";

/// The name of the executable inside the zip, and of the one it replaces
const EXE: &str = "SHIKISHA-TERM.exe";
/// Files that are loaded while the program runs, so they cannot be
/// overwritten in place and are renamed aside first
const HELD_OPEN: &[&str] = &[EXE, "conpty.dll", "OpenConsole.exe"];
/// Folders a person owns. A file in them is placed only where none exists
const OWNED: &[&str] = &["config", "data", "logs", "desks", "scripts"];
/// How often the latest version is read while the program runs
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
/// How long the new copy waits for the old one to leave before claiming the layout
#[cfg(windows)]
const HANDOFF_WAIT: Duration = Duration::from_secs(15);

/// What is known about the newest published version
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Release {
    pub version: String,
    /// Where the zip is, when the release carries one
    pub zip: Option<String>,
    pub sha256: Option<String>,
    pub sig: Option<String>,
    /// The page that says what changed
    pub notes: String,
}

/// Where the update stands. Drawn on the settings card as it is
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Phase {
    /// Never asked
    Idle,
    Checking,
    UpToDate,
    /// A newer version is out. `None` is the Store's, whose number the Store
    /// does not say
    Available { version: Option<String> },
    Downloading { version: String, got: u64, total: Option<u64> },
    Verifying { version: String },
    /// Fetched, checked and unpacked; waiting to be put in place
    Staged { version: String },
    /// The swap is under way; the program is about to end. `None` is the Store's
    Applying { version: Option<String> },
    /// The look itself did not get an answer
    CheckFailed { message: String },
    /// A newer version is known and could not be put in place. `None` is the Store's
    Failed { version: Option<String>, message: String },
}

/// The newer version on offer, as the card asks about it
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Offer {
    /// `None` for the Store's, whose number the Store does not say
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Offer {
    /// What an answer to this offer is remembered by. A number where there
    /// is one; the Store's offer is "whatever the Store holds for this
    /// version", so it is asked about again once the Store has replaced it
    fn key(&self) -> String {
        match &self.version {
            Some(v) => v.clone(),
            None => format!("store-after-{}", current_version()),
        }
    }
}

/// The offer a phase stands on, while it still stands (a failed one is the
/// settings card's to say, not the sidebar's)
fn offer_in(phase: &Phase) -> Option<Offer> {
    let version = match phase {
        Phase::Available { version } => version.clone(),
        Phase::Staged { version } | Phase::Downloading { version, .. } | Phase::Verifying { version } => Some(version.clone()),
        _ => return None,
    };
    Some(Offer { version })
}

/// What the main loop has been asked to do, once it has asked its question
#[derive(Debug, Clone, PartialEq)]
pub enum Apply {
    /// Put the staged version in place
    Fresh { version: String },
    /// Put the previous version back
    Rollback { version: String },
    /// Let the Store install what it holds
    Store,
}

/// A request from the settings server for the fetching thread
#[derive(Debug, Clone, Copy, PartialEq)]
enum Want {
    Check,
    Download,
}

struct State {
    phase: Phase,
    release: Option<Release>,
    /// Seconds since the epoch of the last look
    checked_at: Option<u64>,
    /// The version the card was answered for. It is not shown again
    notified: Option<String>,
    /// The version a person said not to install. Not offered again
    skipped: Option<String>,
    /// The version kept under `data/update/prev`, when there is one
    prev: Option<String>,
    /// Whether the daily look is wanted (a setting)
    auto: bool,
    want: Option<Want>,
    apply: Option<Apply>,
    /// Whether this copy is an installed one, whose updates the Store holds
    packaged: bool,
    /// What the first start of this version did to the files (migrate.rs)
    outcome: Option<crate::migrate::Outcome>,
    /// Whether a failed carry has been said on screen yet
    carry_said: bool,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            checked_at: read_stamp("update-checked"),
            notified: read_text("update-notified"),
            skipped: read_text("update-skipped"),
            ..State::new(crate::config::packaged())
        })
    })
}

impl State {
    /// Nothing looked at, nothing remembered
    fn new(packaged: bool) -> State {
        State {
            phase: Phase::Idle,
            release: None,
            checked_at: None,
            notified: None,
            skipped: None,
            prev: None,
            auto: true,
            want: None,
            apply: None,
            packaged,
            outcome: None,
            carry_said: false,
        }
    }
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|e| e.into_inner())
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ── Version numbers ────────────────────────────────────────────────

/// Numeric comparison such that "0.1.10" > "0.1.9". Any non-numeric part is
/// treated as 0.
fn parse(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().unwrap_or(0)
        })
        .collect()
}

pub fn is_newer(remote: &str, local: &str) -> bool {
    parse(remote) > parse(local)
}

/// The version that starts this update's folder names: digits and dots only,
/// so a tag nobody expected cannot name a path outside `data/update`
fn safe_version(v: &str) -> Option<String> {
    let s: String = v.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
    (!s.is_empty() && s == v).then_some(s)
}

// ── Small state files under data/ ──────────────────────────────────

fn read_text(name: &str) -> Option<String> {
    let s = std::fs::read_to_string(crate::config::state_path(name)).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn write_text(name: &str, v: &str) {
    let _ = crate::crypto::write_atomic(&crate::config::state_path(name), v);
}

fn read_stamp(name: &str) -> Option<u64> {
    read_text(name)?.parse().ok()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn update_dir() -> PathBuf {
    crate::config::state_path("update")
}

fn version_dir(v: &str) -> PathBuf {
    update_dir().join(v)
}

fn stage_dir(v: &str) -> PathBuf {
    version_dir(v).join("stage")
}

fn prev_root() -> PathBuf {
    update_dir().join("prev")
}

fn journal_path() -> PathBuf {
    update_dir().join("journal.json")
}

/// Whether a version has been fetched, checked and unpacked here
fn staged_ok(v: &str) -> bool {
    stage_dir(v).join("stage.ok").is_file()
}

/// The previous version kept for going back, when one is
fn find_prev() -> Option<String> {
    let rd = std::fs::read_dir(prev_root()).ok()?;
    let mut found: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().join(EXE).is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    found.sort_by_key(|v| std::cmp::Reverse(parse(v)));
    found.into_iter().next()
}

// ── What the rest of the program asks ──────────────────────────────

/// Starts the looking. `auto` is the setting; the looking happens at once
/// and then daily while it stays on. What the last swap left is settled
/// earlier, by [`finish_last`], before anything reads the files it touched
pub fn start(auto: bool) {
    {
        let mut s = lock();
        s.auto = auto;
        s.prev = find_prev();
        // A version fetched last time and not yet put in place is still here
        if let Some(v) = std::fs::read_dir(update_dir())
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| safe_version(n).is_some() && staged_ok(n) && is_newer(n, current_version()))
            .max_by(|a, b| parse(a).cmp(&parse(b)))
        {
            s.phase = Phase::Staged { version: v.clone() };
            s.release = Some(Release { version: v, ..Default::default() });
        }
    }
    std::thread::Builder::new()
        .name("update".into())
        .spawn(|| {
            loop {
                let want = {
                    let mut s = lock();
                    let due = s.auto && s.checked_at.is_none_or(|t| now_secs().saturating_sub(t) >= EVERY.as_secs());
                    match s.want.take() {
                        Some(w) => Some(w),
                        None if due && !matches!(s.phase, Phase::Downloading { .. } | Phase::Verifying { .. } | Phase::Applying { .. }) => {
                            Some(Want::Check)
                        }
                        None => None,
                    }
                };
                match want {
                    Some(Want::Check) => {
                        let _ = std::panic::catch_unwind(check);
                    }
                    Some(Want::Download) => {
                        let _ = std::panic::catch_unwind(download);
                    }
                    None => {}
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .ok();
}

/// The setting changed
pub fn set_auto(on: bool) {
    lock().auto = on;
}

/// The offer the card should ask about, if any: one that is newer, not
/// declined, and not yet answered
pub fn ask() -> Option<Offer> {
    let s = lock();
    let offer = offer_in(&s.phase)?;
    let key = offer.key();
    (s.notified.as_deref() != Some(key.as_str()) && s.skipped.as_deref() != Some(key.as_str())).then_some(offer)
}

/// The card was answered, either way. It is not shown again for this offer
pub fn card_answered() {
    let mut s = lock();
    let Some(offer) = offer_in(&s.phase) else { return };
    let key = offer.key();
    write_text("update-notified", &key);
    s.notified = Some(key);
}

/// Look now, whatever the setting says
pub fn request_check() {
    let mut s = lock();
    if !matches!(s.phase, Phase::Downloading { .. } | Phase::Verifying { .. } | Phase::Applying { .. }) {
        // Said to be looking from this moment, so a page that polls only
        // while something is under way keeps polling until the answer
        s.phase = Phase::Checking;
        s.want = Some(Want::Check);
    }
}

/// The one button. What it does depends on where things stand: a version
/// that is known is fetched; one that is staged is put in place; the Store's
/// is handed to the Store
pub fn request_install() -> Result<()> {
    let mut s = lock();
    match s.phase.clone() {
        Phase::Available { .. } | Phase::Failed { .. } if s.packaged => {
            s.apply = Some(Apply::Store);
        }
        Phase::Available { version: Some(version) } | Phase::Failed { version: Some(version), .. } => {
            if s.release.as_ref().is_none_or(|r| r.zip.is_none()) {
                bail!("no download in this release");
            }
            s.phase = Phase::Downloading { version, got: 0, total: None };
            s.want = Some(Want::Download);
        }
        Phase::Staged { version } => {
            s.apply = Some(Apply::Fresh { version });
        }
        _ => bail!("nothing to install"),
    }
    Ok(())
}

/// This version is not wanted. It is not offered again; the next one is
pub fn skip() {
    let mut s = lock();
    let offer = match &s.phase {
        Phase::Available { .. } | Phase::Staged { .. } => offer_in(&s.phase),
        Phase::Failed { version, .. } => Some(Offer { version: version.clone() }),
        _ => None,
    };
    let Some(offer) = offer else { return };
    let key = offer.key();
    write_text("update-skipped", &key);
    s.skipped = Some(key);
    if let Some(v) = offer.version.as_deref() {
        remove_version_dir(v);
    }
    s.phase = Phase::UpToDate;
}

/// Throw away what was fetched
pub fn discard() {
    let mut s = lock();
    if let Phase::Staged { version } | Phase::Failed { version: Some(version), .. } = s.phase.clone() {
        remove_version_dir(&version);
        s.phase = Phase::Available { version: Some(version) };
    }
}

/// Deletes what was fetched for one version. Only a plain version names a
/// folder: anything else -- an empty one above all -- would be the whole of
/// `data/update`, the previous version kept for going back included
fn remove_version_dir(version: &str) {
    if let Some(v) = safe_version(version) {
        let _ = std::fs::remove_dir_all(version_dir(&v));
    }
}

/// Put the previous version back
pub fn request_rollback() -> Result<()> {
    let mut s = lock();
    let Some(v) = s.prev.clone() else { bail!("no previous version kept") };
    s.apply = Some(Apply::Rollback { version: v });
    Ok(())
}

/// What the main loop has been asked to do. Taken once
pub fn take_apply() -> Option<Apply> {
    lock().apply.take()
}

/// The main loop declined (the person answered no to the quit question)
pub fn apply_declined() {
    let mut s = lock();
    if let Phase::Applying { version } = s.phase.clone() {
        s.phase = match version {
            Some(version) if !s.packaged => Phase::Staged { version },
            version => Phase::Available { version },
        };
    }
}

/// Everything the settings card draws, as JSON
pub fn snapshot() -> serde_json::Value {
    let s = lock();
    let mut v = serde_json::to_value(&s.phase).unwrap_or_default();
    v["current"] = serde_json::json!(current_version());
    v["checked_at"] = serde_json::json!(s.checked_at);
    v["auto"] = serde_json::json!(s.auto);
    v["packaged"] = serde_json::json!(s.packaged);
    v["prev"] = serde_json::json!(s.prev);
    v["skipped"] = serde_json::json!(s.skipped);
    v["notes"] = serde_json::json!(notes(&s));
    let o = s.outcome.as_ref();
    v["migrated_from"] = serde_json::json!(o.and_then(|o| o.from.clone()));
    v["migration_failed"] = serde_json::json!(o.and_then(|o| o.failed.clone()));
    v["backup"] = serde_json::json!(
        o.and_then(|o| o.backup.clone()).or_else(newest_backup).map(|p| p.display().to_string())
    );
    v
}

/// The page that says what changed in the version on offer
pub fn notes_url() -> Option<String> {
    notes(&lock()).filter(|u| u.starts_with("https://"))
}

/// The Store copy's changes are on its Store page; the download's, on its release
fn notes(s: &State) -> Option<String> {
    if s.packaged {
        return Some(STORE_URL.to_string());
    }
    s.release.as_ref().map(|r| r.notes.clone())
}

/// Where the newest copy of the settings was put, when one was
fn newest_backup() -> Option<PathBuf> {
    let rd = std::fs::read_dir(crate::config::state_path("backup")).ok()?;
    let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.pop()
}

/// What the first start of this version did to the files
pub fn set_outcome(o: crate::migrate::Outcome) {
    lock().outcome = Some(o);
}

/// A carry that stopped, to be said on screen once: the version the files
/// were in, and where the copy made first is
pub fn take_carry_failure() -> Option<(String, String)> {
    let mut s = lock();
    if s.carry_said {
        return None;
    }
    let o = s.outcome.as_ref()?;
    let failed = o.failed.as_ref()?;
    let _ = failed;
    let from = o.from.clone().unwrap_or_else(|| crate::migrate::BASELINE.to_string());
    let path = o.backup.as_ref().map(|p| p.display().to_string()).unwrap_or_default();
    s.carry_said = true;
    Some((from, path))
}

// ── Looking ────────────────────────────────────────────────────────

fn set_phase(p: Phase) {
    lock().phase = p;
}

/// The releases endpoint is built from the repository in Cargo.toml. A test
/// or a rehearsal points it elsewhere with `SHIKISHA_UPDATE_RELEASES_URL`
fn releases_api() -> Option<String> {
    if let Ok(u) = std::env::var("SHIKISHA_UPDATE_RELEASES_URL") {
        return Some(u);
    }
    let repo = env!("CARGO_PKG_REPOSITORY").strip_prefix("https://github.com/")?;
    Some(format!(
        "https://api.github.com/repos/{}/releases/latest",
        repo.trim_end_matches('/')
    ))
}

pub(crate) fn agent(body: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(body))
        .user_agent(concat!("shikisha-term/", env!("CARGO_PKG_VERSION")))
        .build()
        .new_agent()
}

/// Reads the newest release: its version and where its files are
fn fetch_latest() -> Result<Release> {
    let url = releases_api().ok_or_else(|| anyhow!("no repository"))?;
    let mut resp = agent(Duration::from_secs(30)).get(&url).call()?;
    let v: serde_json::Value = resp.body_mut().read_json()?;
    parse_release(&v).ok_or_else(|| anyhow!("no version in the answer"))
}

/// What a release's JSON says. `None` when it names no version
pub fn parse_release(v: &serde_json::Value) -> Option<Release> {
    let tag = v.get("tag_name")?.as_str()?;
    let version = tag.trim().trim_start_matches(['v', 'V']).to_string();
    if version.is_empty() {
        return None;
    }
    let mut r = Release { version, notes: v.get("html_url").and_then(|u| u.as_str()).unwrap_or_default().to_string(), ..Default::default() };
    for a in v.get("assets").and_then(|a| a.as_array()).into_iter().flatten() {
        let (Some(name), Some(url)) = (a.get("name").and_then(|n| n.as_str()), a.get("browser_download_url").and_then(|u| u.as_str())) else { continue };
        match name {
            "SHIKISHA-TERM.zip" => r.zip = Some(url.to_string()),
            "SHIKISHA-TERM.zip.sha256" => r.sha256 = Some(url.to_string()),
            "SHIKISHA-TERM.zip.sig" => r.sig = Some(url.to_string()),
            _ => {}
        }
    }
    Some(r)
}

/// What a look found
#[derive(Debug)]
enum Found {
    /// The newest published release, whatever its version
    Release(Release),
    /// The Store holds a newer package for this one, or does not
    Store { waiting: bool },
}

/// One look. Sets the phase to what it found
fn check() {
    set_phase(Phase::Checking);
    let packaged = lock().packaged;
    let found = if packaged { store::waiting().map(|waiting| Found::Store { waiting }) } else { fetch_latest().map(Found::Release) };
    let now = now_secs();
    write_text("update-checked", &now.to_string());
    let mut s = lock();
    s.checked_at = Some(now);
    settle(&mut s, found);
    crate::append_hook_log(&format!("Update check: {:?}", s.phase));
}

/// Sets the phase to what a look found. Not getting an answer is never
/// taken for "nothing newer": it is said as what it is
fn settle(s: &mut State, found: Result<Found>) {
    match found {
        Ok(Found::Release(r)) if is_newer(&r.version, current_version()) && safe_version(&r.version).is_some() => {
            let v = r.version.clone();
            s.phase = if staged_ok(&v) { Phase::Staged { version: v } } else { Phase::Available { version: Some(v) } };
            s.release = Some(r);
        }
        Ok(Found::Store { waiting: true }) => {
            s.phase = Phase::Available { version: None };
            s.release = None;
        }
        Ok(Found::Release(_) | Found::Store { waiting: false }) => {
            s.phase = Phase::UpToDate;
            s.release = None;
        }
        Err(e) => {
            // Say so only if nothing better is known
            if matches!(s.phase, Phase::Checking) {
                s.phase = Phase::CheckFailed { message: format!("{e:#}") };
            }
        }
    }
}

// ── Fetching ───────────────────────────────────────────────────────

/// Fetches, checks and unpacks the version the phase names
fn download() {
    let (rel, version) = {
        let s = lock();
        let Phase::Downloading { version, .. } = &s.phase else { return };
        let Some(rel) = s.release.clone() else { return };
        (rel, version.clone())
    };
    match download_into(&rel, &version_dir(&version)) {
        Ok(()) => {
            set_phase(Phase::Staged { version: version.clone() });
            crate::append_hook_log(&format!("Update {version} fetched, checked and unpacked"));
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(version_dir(&version));
            crate::append_hook_log(&format!("Update {version} could not be fetched: {e:#}"));
            set_phase(Phase::Failed { version: Some(version), message: format!("{e:#}") });
        }
    }
}

fn download_into(rel: &Release, dir: &Path) -> Result<()> {
    let zip_url = rel.zip.as_deref().ok_or_else(|| anyhow!("the release has no zip"))?;
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    let zip_path = dir.join("SHIKISHA-TERM.zip");
    fetch_file(zip_url, &zip_path, &rel.version)?;
    set_phase(Phase::Verifying { version: rel.version.clone() });
    let bytes = std::fs::read(&zip_path)?;
    // Whole: the published SHA256
    let sha_url = rel.sha256.as_deref().ok_or_else(|| anyhow!("the release publishes no SHA256"))?;
    let expected = fetch_small(sha_url)?;
    verify_sha256(&bytes, &expected)?;
    // Genuine: the signature, checked against the key this build carries
    if !PUBLIC_KEY_HEX.starts_with("__") {
        let sig_url = rel.sig.as_deref().ok_or_else(|| anyhow!("the release carries no signature"))?;
        let sig = fetch_small(sig_url)?;
        verify_signature(&bytes, &sig, PUBLIC_KEY_HEX)?;
    }
    let stage = dir.join("stage");
    unpack(&zip_path, &stage)?;
    if find_exe_root(&stage).is_none() {
        bail!("the zip holds no {EXE}");
    }
    std::fs::write(stage.join("stage.ok"), &rel.version)?;
    Ok(())
}

/// Streams a file to disk, keeping the phase's progress current
fn fetch_file(url: &str, to: &Path, version: &str) -> Result<()> {
    let mut resp = agent(Duration::from_secs(10 * 60)).get(url).call().with_context(|| format!("get {url}"))?;
    let total: Option<u64> = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|s| s.parse().ok());
    let mut reader = resp.body_mut().as_reader();
    let mut file = std::fs::File::create(to)?;
    let mut got: u64 = 0;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = std::io::Read::read(&mut reader, &mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])?;
        got += n as u64;
        set_phase(Phase::Downloading { version: version.to_string(), got, total });
    }
    Ok(())
}

fn fetch_small(url: &str) -> Result<String> {
    let mut resp = agent(Duration::from_secs(30)).get(url).call().with_context(|| format!("get {url}"))?;
    Ok(resp.body_mut().read_to_string()?)
}

/// The published SHA256 is "<hex>  <name>"; only the hex is compared
pub fn verify_sha256(bytes: &[u8], published: &str) -> Result<()> {
    use sha2::Digest;
    let want = published.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    let got = sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
    if want.len() != 64 || want != got {
        bail!("the download does not match its published SHA256");
    }
    Ok(())
}

fn from_hex(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        bail!("odd hex");
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| anyhow!("{e}"))).collect()
}

/// The zip's signature, checked with the key this build carries
pub fn verify_signature(bytes: &[u8], sig_hex: &str, key_hex: &str) -> Result<()> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let key: [u8; 32] = from_hex(key_hex)?.try_into().map_err(|_| anyhow!("the key is not 32 bytes"))?;
    let sig: [u8; 64] = from_hex(sig_hex)?.try_into().map_err(|_| anyhow!("the signature is not 64 bytes"))?;
    let key = VerifyingKey::from_bytes(&key)?;
    key.verify(bytes, &Signature::from_bytes(&sig)).map_err(|_| anyhow!("the download is not signed with this program's key"))
}

/// Unpacks a zip into a folder. Entries that would land outside it are refused
///
/// The mode each entry was packed with is put back on unix, because a file
/// that has lost its executable bit is a program that will not start -- and
/// the reason is invisible from anywhere except a directory listing
pub(crate) fn unpack(zip_path: &Path, into: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(into);
    std::fs::create_dir_all(into)?;
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else { bail!("the zip names a path outside itself") };
        let out = into.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut f = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut f)?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&out, std::fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// The folder inside an unpacked zip that holds the executable: the zip's
/// top folder, or the unpacked folder itself
fn find_exe_root(stage: &Path) -> Option<PathBuf> {
    if stage.join(EXE).is_file() {
        return Some(stage.to_path_buf());
    }
    let rd = std::fs::read_dir(stage).ok()?;
    rd.flatten().map(|e| e.path()).find(|p| p.is_dir() && p.join(EXE).is_file())
}

// ── Putting it in place ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct Journal {
    version: String,
    from: String,
    /// `swap` while files are being moved; `launched` once the new copy was started
    step: String,
    rollback: bool,
}

fn write_journal(j: &Journal) -> Result<()> {
    crate::crypto::write_atomic(&journal_path(), &serde_json::to_string(j)?)
}

fn read_journal() -> Option<Journal> {
    serde_json::from_str(&std::fs::read_to_string(journal_path()).ok()?).ok()
}

/// Whether a path is inside a folder a person owns
fn is_owned(rel: &Path) -> bool {
    rel.components().next().is_some_and(|c| OWNED.iter().any(|o| c.as_os_str() == *o))
}

/// Every file under a folder, as paths relative to it
fn walk(root: &Path) -> Result<Vec<PathBuf>> {
    fn go(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if p.is_dir() {
                go(&p, root, out)?;
            } else if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.to_path_buf());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    go(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

fn aside(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".old");
    PathBuf::from(s)
}

/// Puts one file where an old one may be. A file nothing holds open is
/// written over in place; one that is held (or that refuses) is set aside
/// under `.old` and the new one renamed in. The new file is written whole
/// beside the old one first, so the moment in which neither exists is two
/// renames long
fn place(src: &Path, dest: &Path, held: bool) -> Result<()> {
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d)?;
    }
    if !dest.exists() {
        std::fs::copy(src, dest).with_context(|| format!("copy to {}", dest.display()))?;
        return Ok(());
    }
    if !held && std::fs::copy(src, dest).is_ok() {
        return Ok(());
    }
    let mut fresh = dest.as_os_str().to_os_string();
    fresh.push(".new");
    let fresh = PathBuf::from(fresh);
    std::fs::copy(src, &fresh).with_context(|| format!("copy to {}", fresh.display()))?;
    let old = aside(dest);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(dest, &old).with_context(|| format!("set aside {}", dest.display()))?;
    if let Err(e) = std::fs::rename(&fresh, dest) {
        let _ = std::fs::rename(&old, dest);
        return Err(e).with_context(|| format!("put {} in place", dest.display()));
    }
    Ok(())
}

/// Copies a version's files over this layout. What a person owns is placed
/// only where nothing is. Files held open are set aside, not overwritten
fn swap_files(source: &Path, root: &Path) -> Result<()> {
    let exe_dest = std::env::current_exe().unwrap_or_else(|_| root.join(EXE));
    // The executable last: everything else can be half done and the program
    // still starts; the executable is the one file that cannot
    let mut files = walk(source)?;
    files.retain(|f| f.file_name().is_none_or(|n| n != "stage.ok"));
    files.sort_by_key(|f| f.as_os_str() == EXE);
    for rel in files {
        let src = source.join(&rel);
        let dest = if rel.as_os_str() == EXE { exe_dest.clone() } else { root.join(&rel) };
        if is_owned(&rel) && dest.exists() {
            continue;
        }
        // A file that is the same already is left alone -- and so is its
        // date, which is what tells "this one was updated" apart later
        if !HELD_OPEN.iter().any(|h| rel.as_os_str() == *h) && same_bytes(&src, &dest) {
            continue;
        }
        place(&src, &dest, HELD_OPEN.iter().any(|h| rel.as_os_str() == *h))?;
    }
    Ok(())
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) if ma.len() == mb.len() => {
            matches!((std::fs::read(a), std::fs::read(b)), (Ok(x), Ok(y)) if x == y)
        }
        _ => false,
    }
}

/// Moves what was set aside into `prev/<from>`, so it can be put back
fn keep_previous(root: &Path, from: &str) {
    let dir = prev_root().join(from);
    let _ = std::fs::remove_dir_all(prev_root());
    let _ = std::fs::create_dir_all(&dir);
    let exe = std::env::current_exe().unwrap_or_else(|_| root.join(EXE));
    for held in HELD_OPEN {
        let old = if *held == EXE { aside(&exe) } else { aside(&root.join(held)) };
        if old.exists() {
            let _ = std::fs::rename(&old, dir.join(held));
        }
    }
}

/// Does the swap and starts the new copy. The caller ends the program
/// afterwards; this copy's files are already the old ones, set aside
pub fn apply(what: &Apply) -> Result<()> {
    let root = crate::config::root_dir();
    let (version, source, rollback) = match what {
        Apply::Fresh { version } => (version.clone(), find_exe_root(&stage_dir(version)).ok_or_else(|| anyhow!("nothing staged for {version}"))?, false),
        Apply::Rollback { version } => (version.clone(), prev_root().join(version), true),
        Apply::Store => bail!("the Store's update is not applied here"),
    };
    if !source.join(EXE).is_file() {
        bail!("{} holds no {EXE}", source.display());
    }
    set_phase(Phase::Applying { version: Some(version.clone()) });
    let from = current_version().to_string();
    let mut j = Journal { version: version.clone(), from: from.clone(), step: "swap".into(), rollback };
    write_journal(&j)?;
    if let Err(e) = swap_files(&source, &root) {
        // Put back whatever was set aside, and say so
        restore_aside(&root);
        let _ = std::fs::remove_file(journal_path());
        set_phase(Phase::Failed { version: Some(version.clone()), message: format!("{e:#}") });
        crate::append_hook_log(&format!("Update to {version} could not be put in place, files put back: {e:#}"));
        return Err(e);
    }
    if rollback {
        // Going back keeps nothing older; going forward again refetches
        let _ = std::fs::remove_dir_all(prev_root());
        for held in HELD_OPEN {
            let p = if *held == EXE { aside(&std::env::current_exe().unwrap_or_else(|_| root.join(EXE))) } else { aside(&root.join(held)) };
            let _ = std::fs::remove_file(p);
        }
    } else {
        keep_previous(&root, &from);
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| root.join(EXE));
    std::process::Command::new(&exe)
        .current_dir(&root)
        .env("SHIKISHA_HANDOFF", std::process::id().to_string())
        .spawn()
        .with_context(|| format!("start {}", exe.display()))?;
    j.step = "launched".into();
    write_journal(&j)?;
    crate::append_hook_log(&format!("Update: {from} -> {version} put in place; the new copy is starting"));
    Ok(())
}

/// Renames every `*.old` under the root back over what replaced it
fn restore_aside(root: &Path) {
    let Ok(files) = walk(root) else { return };
    for rel in files {
        if rel.starts_with("data") {
            continue;
        }
        let p = root.join(&rel);
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else { continue };
        if let Some(orig) = name.strip_suffix(".old") {
            let dest = p.with_file_name(orig);
            let _ = std::fs::remove_file(&dest);
            let _ = std::fs::rename(&p, &dest);
        }
    }
}

/// Deletes every `*.old` under the root that can be deleted. The old copy
/// may still be leaving; what cannot go now goes next start
fn sweep_aside(root: &Path) {
    let Ok(files) = walk(root) else { return };
    for rel in files {
        if rel.starts_with("data") {
            continue;
        }
        let p = root.join(&rel);
        if p.extension().is_some_and(|e| e == "old" || e == "new") {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// The copy that was started to finish an update waits for the one that
/// started it to leave, so the layout is free to claim. Called before the
/// claim. The variable is dropped so children never inherit it
/// Nothing hands the running copy over here: a Linux install is replaced by
/// the package manager, with the old process already gone.
#[cfg(not(windows))]
pub fn wait_for_handoff() {}

#[cfg(windows)]
pub fn wait_for_handoff() {
    let Ok(pid) = std::env::var("SHIKISHA_HANDOFF") else { return };
    // SAFETY: nothing else reads the environment on another thread this early
    unsafe { std::env::remove_var("SHIKISHA_HANDOFF") };
    let Ok(pid) = pid.parse::<u32>() else { return };
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};
    const SYNCHRONIZE: u32 = 0x0010_0000;
    unsafe {
        let h = OpenProcess(SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return;
        }
        WaitForSingleObject(h, HANDOFF_WAIT.as_millis() as u32);
        CloseHandle(h);
    }
}

/// What the last swap left: a journal that says whether it finished. Called
/// before anything reads a file the swap may have touched (the translations
/// are read at start, and a half-swapped one would start the program in
/// English once), so an interrupted swap is put back first
pub fn finish_last() {
    let root = crate::config::root_dir();
    let Some(j) = read_journal() else {
        sweep_aside(&root);
        return;
    };
    let _ = std::fs::remove_file(journal_path());
    if j.step == "launched" && j.version == current_version() {
        sweep_aside(&root);
        if !j.rollback {
            let _ = std::fs::remove_dir_all(version_dir(&j.version));
        }
        crate::append_hook_log(&format!("Update: now running {} (was {})", j.version, j.from));
        return;
    }
    // Interrupted before the new copy ran, or not the copy it meant to start
    restore_aside(&root);
    crate::append_hook_log(&format!("Update to {} did not finish ({}); files put back", j.version, j.step));
}

// ── The Store copy ─────────────────────────────────────────────────

/// The installed copy asks the Store, and lets the Store install
/// There is no Store here to ask.
#[cfg(not(windows))]
pub mod store {
    use anyhow::{Result, bail};

    pub fn install(_hwnd: isize) {}

    /// There is no Store to ask, which is not the same as nothing waiting
    pub fn waiting() -> Result<bool> {
        bail!("there is no Store here to ask")
    }
}

#[cfg(windows)]
pub mod store {
    use super::*;

    /// Whether the Store holds a newer package for this one.
    ///
    /// Only whether: each update the Store lists names the package it would
    /// update -- the one installed here -- so the number it carries is this
    /// version's own, and the newer one's number is not said anywhere. An
    /// empty list is the Store's "nothing newer"; a question that could not
    /// be put is an error, never an empty list
    pub fn waiting() -> Result<bool> {
        use windows::Services::Store::StoreContext;
        let ctx = StoreContext::GetDefault().context("reach the Store")?;
        let updates = ctx.GetAppAndOptionalStorePackageUpdatesAsync().and_then(|op| op.get()).context("ask the Store for updates")?;
        Ok(updates.Size()? > 0)
    }

    /// Lets the Store download and install. The Store closes the program to
    /// do it; the helper started here starts it again once it is gone. Runs
    /// on a thread of its own: the Store shows its own dialog and takes as
    /// long as the download takes
    pub fn install(hwnd: isize) {
        std::thread::Builder::new()
            .name("store-update".into())
            .spawn(move || {
                set_phase(Phase::Applying { version: None });
                let helper = relaunch_after_exit();
                let ok = std::panic::catch_unwind(|| run(hwnd)).unwrap_or_else(|_| Err(anyhow!("the Store's update stopped")));
                match ok {
                    Ok(()) => {
                        // The Store ends the program from here. If it has
                        // not within a while, it did not install
                        std::thread::sleep(Duration::from_secs(60));
                        finish(helper, "the Store did not install".into());
                    }
                    Err(e) => finish(helper, format!("{e:#}")),
                }
            })
            .ok();
    }

    fn finish(helper: Option<std::process::Child>, message: String) {
        if let Some(mut h) = helper {
            let _ = h.kill();
        }
        crate::append_hook_log(&format!("Store update: {message}"));
        set_phase(Phase::Failed { version: None, message });
    }

    fn run(hwnd: isize) -> Result<()> {
        use windows::Services::Store::{StoreContext, StorePackageUpdateState};
        use windows::Win32::UI::Shell::IInitializeWithWindow;
        use windows::core::Interface;
        let ctx = StoreContext::GetDefault()?;
        let init: IInitializeWithWindow = ctx.cast()?;
        unsafe { init.Initialize(windows::Win32::Foundation::HWND(hwnd as *mut _))? };
        let updates = ctx.GetAppAndOptionalStorePackageUpdatesAsync()?.get()?;
        if updates.Size()? == 0 {
            bail!("the Store holds no update now");
        }
        let result = ctx.RequestDownloadAndInstallStorePackageUpdatesAsync(&updates)?.get()?;
        match result.OverallState()? {
            StorePackageUpdateState::Completed => Ok(()),
            StorePackageUpdateState::Canceled => bail!("cancelled"),
            other => bail!("the Store answered {}", other.0),
        }
    }

    /// The identity the shell starts this package by
    fn app_id() -> Option<String> {
        use windows_sys::Win32::Storage::Packaging::Appx::GetCurrentPackageFamilyName;
        let mut len: u32 = 0;
        unsafe { GetCurrentPackageFamilyName(&mut len, std::ptr::null_mut()) };
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        let rc = unsafe { GetCurrentPackageFamilyName(&mut len, buf.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        let name = String::from_utf16_lossy(&buf[..len.saturating_sub(1) as usize]);
        Some(format!("{name}!SHIKISHATERM"))
    }

    /// A small helper that waits for this process to end, then starts the
    /// package again. Killed if the install does not happen
    fn relaunch_after_exit() -> Option<std::process::Child> {
        use std::os::windows::process::CommandExt;
        let aumid = app_id()?;
        let script = format!(
            "Wait-Process -Id {} -ErrorAction SilentlyContinue; Start-Process explorer.exe 'shell:AppsFolder\\{}'",
            std::process::id(),
            aumid
        );
        std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.1.10", "0.1.9"), "as a string comparison 10 < 9");
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(is_newer("1.0", "0.99.99"));
    }

    #[test]
    fn api_url_comes_from_cargo_metadata() {
        assert_eq!(
            releases_api().as_deref(),
            Some("https://api.github.com/repos/styleio/ShikishaTerm/releases/latest")
        );
    }

    /// A release's JSON is read for its version and its three files; a
    /// release with none of them is still a version
    #[test]
    fn a_release_is_read_for_its_version_and_files() {
        let v = serde_json::json!({
            "tag_name": "v0.9.0", "html_url": "https://github.com/styleio/ShikishaTerm/releases/tag/v0.9.0",
            "assets": [
                {"name": "SHIKISHA-TERM.zip", "browser_download_url": "https://x/z.zip"},
                {"name": "SHIKISHA-TERM.zip.sha256", "browser_download_url": "https://x/z.sha"},
                {"name": "SHIKISHA-TERM.zip.sig", "browser_download_url": "https://x/z.sig"},
                {"name": "other.msix", "browser_download_url": "https://x/o"}
            ]});
        let r = parse_release(&v).unwrap();
        assert_eq!(r.version, "0.9.0");
        assert_eq!(r.zip.as_deref(), Some("https://x/z.zip"));
        assert_eq!(r.sha256.as_deref(), Some("https://x/z.sha"));
        assert_eq!(r.sig.as_deref(), Some("https://x/z.sig"));
        assert!(r.notes.ends_with("v0.9.0"));
        let bare = parse_release(&serde_json::json!({"tag_name": "0.9.1"})).unwrap();
        assert_eq!((bare.version.as_str(), bare.zip), ("0.9.1", None));
        assert!(parse_release(&serde_json::json!({"name": "x"})).is_none());
    }

    /// Only a version made of digits and dots names a folder
    #[test]
    fn only_a_plain_version_names_a_folder() {
        assert_eq!(safe_version("0.9.0").as_deref(), Some("0.9.0"));
        assert!(safe_version("../x").is_none());
        assert!(safe_version("0.9.0-beta").is_none());
        assert!(safe_version("").is_none(), "an empty version names data/update itself");
        assert!(safe_version(&Offer { version: None }.key()).is_none(), "the Store offer's key named a folder");
    }

    /// The Store says whether a newer package waits, and each of the three
    /// answers is drawn as itself: waiting is on offer (with no number, since
    /// the Store gives none), an empty list is up to date, and a question
    /// that could not be put is said as that -- never as either of the others
    #[test]
    fn the_stores_three_answers_are_not_confused() {
        let mut s = State::new(true);
        s.phase = Phase::Checking;
        settle(&mut s, Ok(Found::Store { waiting: true }));
        assert_eq!(s.phase, Phase::Available { version: None }, "an update is waiting and is not offered");
        assert_eq!(notes(&s).as_deref(), Some(STORE_URL));

        s.phase = Phase::Checking;
        settle(&mut s, Ok(Found::Store { waiting: false }));
        assert_eq!(s.phase, Phase::UpToDate, "an empty list is not read as up to date");

        s.phase = Phase::Checking;
        settle(&mut s, Err(anyhow!("the Store did not answer")));
        assert_eq!(s.phase, Phase::CheckFailed { message: "the Store did not answer".into() }, "a failed look is not said with its reason");

        // A failed look does not hide what an earlier one found
        s.phase = Phase::Available { version: None };
        settle(&mut s, Err(anyhow!("offline")));
        assert_eq!(s.phase, Phase::Available { version: None });
    }

    /// A release is on offer only when it is newer than this version
    #[test]
    fn a_release_is_on_offer_only_when_newer() {
        let mut s = State::new(false);
        let rel = |v: &str| Found::Release(Release { version: v.into(), ..Default::default() });
        s.phase = Phase::Checking;
        settle(&mut s, Ok(rel(current_version())));
        assert_eq!(s.phase, Phase::UpToDate);
        s.phase = Phase::Checking;
        settle(&mut s, Ok(rel("9999.0.0")));
        assert_eq!(s.phase, Phase::Available { version: Some("9999.0.0".into()) });
        assert_eq!(s.release.as_ref().map(|r| r.version.as_str()), Some("9999.0.0"));
    }

    /// An offer is remembered by its number, or -- the Store's, which has
    /// none -- by the version it would replace, so that once the Store has
    /// replaced this copy the next offer is asked about afresh
    #[test]
    fn an_offer_is_remembered_by_what_it_is() {
        assert_eq!(Offer { version: Some("0.12.0".into()) }.key(), "0.12.0");
        assert_eq!(Offer { version: None }.key(), format!("store-after-{}", current_version()));
        assert_eq!(offer_in(&Phase::Available { version: None }), Some(Offer { version: None }));
        assert_eq!(offer_in(&Phase::Staged { version: "0.12.0".into() }), Some(Offer { version: Some("0.12.0".into()) }));
        assert_eq!(offer_in(&Phase::UpToDate), None);
        assert_eq!(offer_in(&Phase::CheckFailed { message: String::new() }), None);
        // The sidebar card is told there is one even with no number to say
        assert_eq!(serde_json::to_string(&Offer { version: None }).unwrap(), "{}");
        assert_eq!(serde_json::to_string(&Offer { version: Some("0.12.0".into()) }).unwrap(), r#"{"version":"0.12.0"}"#);
    }

    /// The SHA256 line's first word is the hash; a wrong one is refused
    #[test]
    fn the_published_sha256_is_checked() {
        let bytes = b"hello";
        let ok = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824  SHIKISHA-TERM.zip";
        assert!(verify_sha256(bytes, ok).is_ok());
        assert!(verify_sha256(bytes, &ok.to_uppercase()).is_ok(), "upper case is the same");
        assert!(verify_sha256(b"hellp", ok).is_err());
        assert!(verify_sha256(bytes, "").is_err());
    }

    /// A signature made with the matching key passes; a changed byte fails
    #[test]
    fn the_signature_is_checked_against_the_key() {
        use ed25519_dalek::{Signer, SigningKey};
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let pubhex = key.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>();
        let sig = key.sign(b"the zip").to_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert!(verify_signature(b"the zip", &sig, &pubhex).is_ok());
        assert!(verify_signature(b"the zap", &sig, &pubhex).is_err());
        let other = SigningKey::from_bytes(&[8u8; 32]).verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>();
        assert!(verify_signature(b"the zip", &sig, &other).is_err(), "it got through with a different key");
        assert!(verify_signature(b"the zip", "zz", &pubhex).is_err());
    }

    /// The installer turns away a download this key did not sign, and it is
    /// this same key.
    ///
    /// It has to be written twice — as hex here, as a PEM in `install.sh` —
    /// because a shell script cannot read a Rust constant. Two copies of a
    /// key drift, and the way this one would drift is the worst kind: the
    /// program would go on refusing forged updates while the installer put
    /// anything at all on a fresh machine. So they are compared.
    #[test]
    fn the_installer_checks_against_this_key() {
        use base64::Engine as _;
        let script =
            std::fs::read_to_string(crate::repo_root().join("packaging").join("linux").join("install.sh"))
                .expect("packaging/linux/install.sh");
        let body: String = script
            .lines()
            .skip_while(|l| !l.contains("BEGIN PUBLIC KEY"))
            .skip(1)
            .take_while(|l| !l.contains("END PUBLIC KEY"))
            .collect();
        assert!(!body.is_empty(), "install.sh has no key");
        let der = base64::engine::general_purpose::STANDARD
            .decode(body.trim())
            .expect("the key is not base64");
        // An Ed25519 public key as OpenSSL reads one: twelve bytes saying what
        // it is, then the thirty-two that are the key
        assert_eq!(der.len(), 44, "the key has the wrong shape");
        let hex: String = der[12..].iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, PUBLIC_KEY_HEX, "the key in install.sh is not this version's key");
    }

    /// A person's folders are placed only where nothing is; the rest is
    /// overwritten; a file in use is set aside and the old one kept
    #[test]
    fn a_swap_keeps_what_the_person_wrote_and_replaces_the_rest() {
        let base = std::env::temp_dir().join(format!("shikisha-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, root) = (base.join("src"), base.join("root"));
        for (p, body) in [
            ("lang/ja.json", "new lang"),
            ("config.example.json", "new example"),
            ("config/config.json", "should not land"),
            ("scripts/example/a.lua", "new example script"),
            ("desks/projectx.example.json", "new desk example"),
            ("conpty.dll", "new dll"),
        ] {
            let f = src.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        }
        for (p, body) in [
            ("lang/ja.json", "old lang"),
            ("config/config.json", "mine"),
            ("scripts/example/a.lua", "mine too"),
            ("conpty.dll", "old dll"),
        ] {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        }
        swap_files(&src, &root).unwrap();
        let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap();
        assert_eq!(read("lang/ja.json"), "new lang");
        assert_eq!(read("config.example.json"), "new example");
        assert_eq!(read("config/config.json"), "mine", "the settings were overwritten");
        assert_eq!(read("scripts/example/a.lua"), "mine too", "an example the person touched was overwritten");
        assert_eq!(read("desks/projectx.example.json"), "new desk example", "a missing example is placed");
        assert_eq!(read("conpty.dll"), "new dll");
        assert_eq!(read("conpty.dll.old"), "old dll", "what is in use is kept to the side");
        assert!(!root.join("lang/ja.json.old").exists(), "a .old was left for something that could be overwritten");
        // Interrupted: what was set aside goes back
        restore_aside(&root);
        assert_eq!(read("conpty.dll"), "old dll");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The zip's top folder is found whether or not there is one, and a
    /// zip with a path outside itself is refused
    #[test]
    fn the_unpacked_folder_is_found_under_a_top_folder_or_not() {
        let base = std::env::temp_dir().join(format!("shikisha-stage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a/SHIKISHA-TERM")).unwrap();
        std::fs::write(base.join("a/SHIKISHA-TERM").join(EXE), "x").unwrap();
        assert_eq!(find_exe_root(&base.join("a")), Some(base.join("a/SHIKISHA-TERM")));
        std::fs::create_dir_all(base.join("b")).unwrap();
        std::fs::write(base.join("b").join(EXE), "x").unwrap();
        assert_eq!(find_exe_root(&base.join("b")), Some(base.join("b")));
        std::fs::create_dir_all(base.join("c/nothing")).unwrap();
        assert_eq!(find_exe_root(&base.join("c")), None);
        let _ = std::fs::remove_dir_all(&base);
    }
}

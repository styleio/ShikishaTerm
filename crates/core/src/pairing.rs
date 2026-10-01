//! Pairing a device with a board by a code used once (far-keep plan §6.2).
//!
//! The server version prints a code -- `shikisha-server pair`, on the
//! server's own terminal -- and a PC (or a phone, from a QR a paired PC
//! shows) gives it back once to be let in. In exchange the device is
//! written into the book of devices ([`crate::clients`]) and handed a key of
//! its own, kept on the PC in its secrets and never in a link.
//!
//! **A code** is 8 letters and digits from an alphabet with no look-alikes,
//! good for 10 minutes and one use. Only its hash is written down
//! ([`FILE`]), in the state folder the command and the running board share:
//! the command writes it, the board reads it, and nothing else carries it.
//! Wrong codes are counted: past [`TRIES`] in a window of [`TRIES_FOR`], every
//! code is refused until the window passes, so a code cannot be guessed by
//! asking.
//!
//! **A ticket** is how a paired PC opens the board in a window without its
//! key being in the window's address: it asks for one with its key, and the
//! window brings it once, within a minute. It is kept in memory only.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The codes waiting to be given back, by their hash
pub const FILE: &str = "pair-codes.json";
/// How long a code is good for
pub const CODE_LIFE: Duration = Duration::from_secs(10 * 60);
/// How many wrong codes are let through in [`TRIES_FOR`]
pub const TRIES: usize = 10;
pub const TRIES_FOR: Duration = Duration::from_secs(10 * 60);
/// How long a ticket is good for
pub const TICKET_LIFE: Duration = Duration::from_secs(60);
/// The letters a code is made of: no 0/O, 1/I/L, 2/Z, 5/S, 8/B
const ALPHABET: &[u8] = b"ACDEFGHJKMNPQRTUVWXY3469";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Codes {
    #[serde(default)]
    waiting: Vec<Waiting>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Waiting {
    hash: String,
    /// Seconds since 1970 it is good until
    until: u64,
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn hash(code: &str) -> String {
    use sha2::Digest as _;
    let plain: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_uppercase();
    sha2::Sha256::digest(plain.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// One writer at a time: the command and the board are two processes, but
/// within one of them the file is read, changed and written whole
static LOCK: Mutex<()> = Mutex::new(());

fn with_codes<R>(f: impl FnOnce(&mut Codes) -> R) -> R {
    let _one = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = crate::config::state_path(FILE);
    let mut codes: Codes = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let n = now();
    codes.waiting.retain(|w| w.until > n);
    let out = f(&mut codes);
    if let Err(e) = crate::crypto::write_atomic(&path, &serde_json::to_string(&codes).unwrap_or_default()) {
        crate::append_hook_log(&format!("pairing: the codes could not be written down: {e:#}"));
    }
    out
}

/// A new code, written down (as its hash) for the board to take
pub fn new_code() -> String {
    // Drawn from the system's random bytes, each kept only below the last
    // whole round of the alphabet, so every letter is as likely as any other
    let mut code = String::new();
    let fair = (256 / ALPHABET.len() * ALPHABET.len()) as u8;
    while code.len() < 8 {
        for b in crate::random_bytes(16).unwrap_or_else(|| crate::random_hex(16).into_bytes()) {
            if b < fair && code.len() < 8 {
                code.push(ALPHABET[usize::from(b) % ALPHABET.len()] as char);
            }
        }
    }
    let until = now() + CODE_LIFE.as_secs();
    let h = hash(&code);
    with_codes(|c| c.waiting.push(Waiting { hash: h, until }));
    format!("{}-{}", &code[..4], &code[4..])
}

/// Why a code was not taken
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Not a code waiting: wrong, used already, or out of time
    Unknown,
    /// Too many wrong codes lately: none is taken for a while
    TooMany,
}

/// The wrong codes lately, by when
static MISSES: Mutex<Vec<Instant>> = Mutex::new(Vec::new());

/// Take a code: right, waiting and in time, it is used up and the device may
/// be let in
pub fn take(code: &str) -> Result<(), Refused> {
    let mut misses = MISSES.lock().unwrap_or_else(|e| e.into_inner());
    misses.retain(|t| t.elapsed() < TRIES_FOR);
    if misses.len() >= TRIES {
        crate::append_hook_log("pairing: a code was refused, too many wrong ones lately");
        return Err(Refused::TooMany);
    }
    let h = hash(code);
    let found = with_codes(|c| {
        let at = c.waiting.iter().position(|w| crate::crypto::token_eq(&w.hash, &h));
        at.map(|i| c.waiting.remove(i)).is_some()
    });
    if found {
        crate::append_hook_log("pairing: a code was given back, and used up");
        Ok(())
    } else {
        misses.push(Instant::now());
        crate::append_hook_log("pairing: a wrong code was given");
        Err(Refused::Unknown)
    }
}

/// Tickets handed out, to the device key each stands for
static TICKETS: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);

/// A ticket for a paired device to open the board with, once
pub fn ticket_for(key: &str) -> String {
    let t = crate::random_hex(16);
    let mut all = TICKETS.lock().unwrap_or_else(|e| e.into_inner());
    let all = all.get_or_insert_with(HashMap::new);
    all.retain(|_, (_, at)| at.elapsed() < TICKET_LIFE);
    all.insert(t.clone(), (key.to_string(), Instant::now()));
    t
}

/// The device key a ticket stands for, taken once and in time
pub fn take_ticket(ticket: &str) -> Option<String> {
    if ticket.is_empty() {
        return None;
    }
    let mut all = TICKETS.lock().unwrap_or_else(|e| e.into_inner());
    let all = all.get_or_insert_with(HashMap::new);
    let (key, at) = all.remove(ticket)?;
    (at.elapsed() < TICKET_LIFE).then_some(key)
}

/// Where the board was last served, as the server writes it down when it
/// starts serving, so that `shikisha-server pair` can put it in its QR. The
/// key of the link is not written: a device pairs by the code
pub const BOARD_FILE: &str = "board-address";

pub fn write_board(url: &str) {
    let base = url.split('?').next().unwrap_or(url).trim_end_matches('/');
    let _ = crate::crypto::write_atomic(&crate::config::state_path(BOARD_FILE), base);
}

pub fn board() -> Option<String> {
    std::fs::read_to_string(crate::config::state_path(BOARD_FILE)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

// ── This PC's side: adding a server version, and opening it ──────────────

/// The ticket a paired PC was handed to open a server version's board with:
/// put in the environment of the window it starts, never on its command line,
/// where every program on the PC can read it
pub const TICKET_ENV: &str = "SHIKISHA_BOARD_TICKET";

/// Open a server version's board in a window of its own, as this PC that was
/// paired with it: a ticket asked for with its key, handed to the window
pub fn open_window(url: &str, key: &str) -> anyhow::Result<()> {
    let ticket = ticket(url, key)?;
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe).arg("--connect").arg(url).env(TICKET_ENV, ticket).spawn()?;
    crate::append_hook_log(&format!("pairing: a window opened on {url}"));
    Ok(())
}

/// The address of a board, as written: http or https, and nothing after the
/// host and port -- no key, no path. `None` when it is not one
pub fn board_url(text: &str) -> Option<String> {
    let t = text.trim();
    let t = t.split('?').next().unwrap_or(t).trim_end_matches('/');
    let rest = t.strip_prefix("https://").or_else(|| t.strip_prefix("http://"))?;
    (!rest.is_empty() && !rest.contains('/') && !rest.contains('@')).then(|| t.to_string())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

fn answer(r: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> anyhow::Result<serde_json::Value> {
    let mut r = r.map_err(|e| anyhow::anyhow!(crate::i18n::tp("err.board.unreachable", &[("e", &e.to_string())])))?;
    match r.status().as_u16() {
        200 => Ok(r.body_mut().read_json::<serde_json::Value>()?),
        403 => anyhow::bail!(crate::i18n::t("err.board.not_paired")),
        404 => anyhow::bail!(crate::i18n::t("err.board.too_old")),
        n => anyhow::bail!(crate::i18n::tp("err.board.refused", &[("status", &n.to_string())])),
    }
}

/// Give a code back to the board at `url`, as this PC, named `name`: the
/// key it hands this PC in exchange
pub fn pair_with(url: &str, code: &str, name: &str) -> anyhow::Result<String> {
    let v = answer(agent().post(&format!("{url}/pair")).send_json(serde_json::json!({ "code": code, "name": name })))?;
    match (v["ok"].as_bool(), v["key"].as_str(), v["why"].as_str()) {
        (Some(true), Some(key), _) => Ok(key.to_string()),
        (_, _, Some("too_many")) => anyhow::bail!(crate::i18n::t("err.board.too_many")),
        _ => anyhow::bail!(crate::i18n::t("err.board.bad_code")),
    }
}

/// A ticket to open the board in a window with, once
pub fn ticket(url: &str, key: &str) -> anyhow::Result<String> {
    let v = answer(agent().post(&format!("{url}/pair/ticket")).header("X-Device-Key", key).send(""))?;
    v["ticket"].as_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.board.bad_answer")))
}

/// How the board stands: its tabs, and how many are at work
pub fn status(url: &str, key: &str) -> anyhow::Result<serde_json::Value> {
    answer(agent().get(&format!("{url}/pair/status")).header("X-Device-Key", key).call())
}

/// A new code for a phone, which this PC shows as a QR
pub fn invite(url: &str, key: &str) -> anyhow::Result<String> {
    let v = answer(agent().post(&format!("{url}/pair/invite")).header("X-Device-Key", key).send(""))?;
    v["code"].as_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.board.bad_answer")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A code is taken once, whatever its case or its dash; a wrong one is
    /// refused, and past the limit every code is, for a while
    #[test]
    fn a_code_is_taken_once_and_guessing_is_stopped() {
        let code = new_code();
        assert_eq!(code.len(), 9, "{code}");
        assert!(code.chars().all(|c| c == '-' || ALPHABET.contains(&(c as u8))), "{code}");
        assert_eq!(take(&code.to_lowercase().replace('-', "")), Ok(()));
        assert_eq!(take(&code), Err(Refused::Unknown), "used twice");
        let more = new_code();
        for _ in 0..TRIES {
            let _ = take("WRONG-CODE");
        }
        assert_eq!(take(&more), Err(Refused::TooMany), "taken while guessing is stopped");
        MISSES.lock().unwrap().clear();
        assert_eq!(take(&more), Ok(()), "kept while guessing was stopped");
    }

    /// A board is named by its address alone: no key, no path
    #[test]
    fn a_board_is_its_address_alone() {
        assert_eq!(board_url(" https://vps1.example.ts.net:8787/ ").as_deref(), Some("https://vps1.example.ts.net:8787"));
        assert_eq!(board_url("http://10.0.0.2:8787/?t=secret").as_deref(), Some("http://10.0.0.2:8787"));
        assert_eq!(board_url("http://10.0.0.2:8787/settings"), None);
        assert_eq!(board_url("ftp://x"), None);
        assert_eq!(board_url("https://user@host"), None);
    }

    /// This PC's side against a board that runs (tools/debug/board-pair.win.mjs
    /// starts one, prints a code, and hands both over): paired by the code,
    /// then a ticket, how it stands, and a code for a phone, with the key it
    /// was handed
    #[test]
    #[ignore = "needs a board that runs: tools/debug/board-pair.win.mjs"]
    fn live_board() {
        let url = board_url(&std::env::var("SHIKISHA_TEST_BOARD").expect("SHIKISHA_TEST_BOARD")).expect("an address");
        let code = std::env::var("SHIKISHA_TEST_CODE").expect("SHIKISHA_TEST_CODE");
        let key = pair_with(&url, &code, "test PC").expect("paired");
        assert!(pair_with(&url, &code, "test PC").is_err(), "a code used twice");
        assert!(!ticket(&url, &key).expect("a ticket").is_empty());
        let s = status(&url, &key).expect("a status");
        assert_eq!(s["ok"], true, "{s}");
        assert!(status(&url, "not-a-key").is_err(), "a key nobody holds");
        assert_eq!(invite(&url, &key).expect("a phone code").len(), 9);
    }

    /// A ticket stands for its key once
    #[test]
    fn a_ticket_is_taken_once() {
        let t = ticket_for("device-key");
        assert_eq!(take_ticket(&t).as_deref(), Some("device-key"));
        assert_eq!(take_ticket(&t), None);
        assert_eq!(take_ticket(""), None);
    }
}

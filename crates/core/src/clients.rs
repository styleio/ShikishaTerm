//! The devices allowed to open this runtime's board, one row each.
//!
//! There used to be a single access token. Anyone holding it was in, every
//! holder was indistinguishable from every other, and "disconnect" meant
//! throwing all of them out at once. That is bearable while the only thing
//! behind the door is one person's own PC and one person's own phone.
//!
//! It stops being bearable the moment the runtime lives on a server. A server
//! is shared on purpose -- a desktop, a phone, a laptop, maybe somebody else's
//! -- and it holds the credentials the agents run with, because a login on a
//! laptop does not carry to it. One key for all of that means losing a phone
//! costs everybody their access, and getting it back means handing the same
//! key out again to each device in turn.
//!
//! So every device gets its own, named, revocable key.
//!
//! **The book lives beside the runtime, never on the client.** That is the
//! same reason the secrets do: the runtime is the thing being shared, so the
//! list of who may share it belongs there. A client holds one key and knows
//! nothing about the others.
//!
//! **A key is stored as its hash.** The file is read by whoever can read the
//! server's disk; a stolen file should not be a set of working keys. What is
//! shown once, at pairing, is the only time the key itself exists in the open.

use serde::{Deserialize, Serialize};

/// How many devices may be paired at once. Not a security limit -- a paired
/// device is one somebody deliberately let in -- but a file that grows without
/// end is a file nobody ever prunes, and an unused key is still a key
const MAX_CLIENTS: usize = 32;

/// One device that may open the board.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Client {
    /// Names the row, not the device. Short and stable, so revoking is a
    /// matter of naming a row rather than matching a hash by eye
    pub id: String,
    /// What a person calls it: "台所のiPad", "work laptop". Empty until the
    /// device first connects and says something about itself
    #[serde(default)]
    pub name: String,
    /// The key's SHA-256, never the key
    pub hash: String,
    /// When it was paired, and when it was last seen. Seconds since the epoch,
    /// because this is written to a file that outlives the process
    pub added: u64,
    #[serde(default)]
    pub seen: u64,
}

impl Client {
    /// Whether this row is the one that key opens.
    fn opens(&self, key: &str) -> bool {
        crate::crypto::token_eq(&hash_of(key), &self.hash)
    }
}

/// The book, as it sits on disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub clients: Vec<Client>,
}

fn path() -> std::path::PathBuf {
    #[cfg(test)]
    if let Some(p) = tests::book_here() {
        return p;
    }
    crate::config::state_path("clients.json")
}

/// A key's hash, as the file stores it.
pub fn hash_of(key: &str) -> String {
    use sha2::Digest as _;
    let mut h = sha2::Sha256::new();
    h.update(key.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Everyone currently allowed in. A book that cannot be read is an empty one:
/// a runtime that refused to start because of a damaged list would be a
/// runtime locked out of itself, and the list is rebuildable by pairing again
pub fn load() -> Book {
    std::fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

fn save(book: &Book) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(book)?;
    crate::crypto::write_atomic(&path(), &text)
}

/// Read the book, change it, write it back, with nobody else in between.
///
/// Without this the three steps are a lost update waiting to happen, and the
/// way it loses matters: a device pairing while another is being revoked reads
/// the book before the revocation, and writes it back after -- **putting the
/// revoked key back**. A key that comes back on its own is not a race anybody
/// gets to shrug at.
///
/// One lock for the whole process rather than one per file, because there is
/// one book. `write_atomic` already makes a reader see one version or the
/// other, so readers need nothing.
fn with_book<T>(change: impl FnOnce(&mut Book) -> T) -> anyhow::Result<T> {
    static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut book = load();
    let out = change(&mut book);
    save(&book)?;
    Ok(out)
}

/// Let a new device in, and hand back the key it must present.
///
/// The key is returned once and never stored: from here on the book holds only
/// its hash, so this return value is the single moment it can be shown to
/// anybody. Whoever calls this is responsible for putting it in front of the
/// person pairing, and nowhere else.
pub fn pair(name: &str) -> anyhow::Result<(Client, String)> {
    let key = crate::random_hex(24);
    let row = Client {
        id: crate::random_hex(6),
        name: name.trim().to_string(),
        hash: hash_of(&key),
        added: now(),
        seen: 0,
    };
    with_book(|book| {
        // The oldest goes when the book is full. Oldest by pairing date rather
        // than by last use: a device that paired long ago and was never used
        // again is the one nobody will miss, and "last used" would evict the
        // spare phone somebody keeps for exactly the day their laptop dies
        while book.clients.len() >= MAX_CLIENTS {
            book.clients.remove(0);
        }
        book.clients.push(row.clone());
    })?;
    Ok((row, key))
}

/// Which device this key belongs to, if any still holds it.
pub fn who(key: &str) -> Option<Client> {
    if key.is_empty() {
        return None;
    }
    load().clients.into_iter().find(|c| c.opens(key))
}

/// Note that this device was heard from, at most once a minute.
///
/// Rate-limited because every request would otherwise rewrite the file, and
/// the only thing this answers is "when did I last see it" -- a question
/// nobody asks to the second.
pub fn touch(id: &str) {
    // Looked at before taking the lock, so the common case -- seen a moment
    // ago, nothing to write -- costs nothing and blocks nobody
    let when = now();
    let due = load()
        .clients
        .iter()
        .any(|c| c.id == id && when.saturating_sub(c.seen) >= 60);
    if !due {
        return;
    }
    let _ = with_book(|book| {
        if let Some(row) = book.clients.iter_mut().find(|c| c.id == id) {
            row.seen = when;
        }
    });
}

/// Give this device a name, so the list reads as devices rather than hashes.
pub fn rename(id: &str, name: &str) -> anyhow::Result<()> {
    with_book(|book| {
        if let Some(row) = book.clients.iter_mut().find(|c| c.id == id) {
            row.name = name.trim().chars().take(40).collect();
        }
    })
}

/// Take this device's key away. Everything else keeps working.
pub fn revoke(id: &str) -> anyhow::Result<bool> {
    with_book(|book| {
        let before = book.clients.len();
        book.clients.retain(|c| c.id != id);
        book.clients.len() != before
    })
}

/// Take every device's key away. For the day something is wrong and working
/// out which device it was costs more than pairing them all again
pub fn revoke_all() -> anyhow::Result<()> {
    with_book(|book| book.clients.clear())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Where the book is while a test is holding one, for the whole process.
    ///
    /// Not per thread: the door is opened on the server's own threads, not on
    /// the test's, so a thread-local redirection would leave the server
    /// writing the machine's real book while the test read a temporary one.
    static BOOK: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
    /// Held for as long as a test has its own book, so two of them cannot be
    /// pointed at once
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    pub(crate) fn book_here() -> Option<std::path::PathBuf> {
        BOOK.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A book of this test's own, taken away again afterwards.
    pub(crate) struct OwnBook {
        file: std::path::PathBuf,
        /// Held, not read: what it does is keep the next test waiting
        _held: std::sync::MutexGuard<'static, ()>,
    }

    impl OwnBook {
        pub(crate) fn new() -> Self {
            let held = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
            let p = std::env::temp_dir()
                .join(format!("shikisha-book-{}.json", crate::random_hex(8)));
            *BOOK.lock().unwrap_or_else(|e| e.into_inner()) = Some(p.clone());
            Self { file: p, _held: held }
        }
    }

    impl Drop for OwnBook {
        fn drop(&mut self) {
            *BOOK.lock().unwrap_or_else(|e| e.into_inner()) = None;
            let _ = std::fs::remove_file(&self.file);
        }
    }

    fn alone<T>(body: impl FnOnce() -> T) -> T {
        let _own = OwnBook::new();
        body()
    }

    /// The key is shown once and never stored. A book that leaked would be a
    /// list of devices, not a set of working keys.
    #[test]
    fn the_book_holds_the_hash_and_never_the_key() {
        alone(|| {
            let (row, key) = pair("台所のiPad").unwrap();
            let text = std::fs::read_to_string(path()).unwrap();
            assert!(!text.contains(&key), "鍵そのものが書かれている");
            assert!(text.contains(&row.hash), "照合に使うものが書かれていない");
            assert_eq!(who(&key).map(|c| c.id), Some(row.id), "本人だと分からない");
        });
    }

    /// The whole point: one device loses its key without taking the others
    /// with it.
    #[test]
    fn revoking_one_device_leaves_the_others_alone() {
        alone(|| {
            let (phone, phone_key) = pair("phone").unwrap();
            let (laptop, laptop_key) = pair("laptop").unwrap();

            assert!(revoke(&phone.id).unwrap(), "消えたと言わない");
            assert!(who(&phone_key).is_none(), "取り上げた鍵がまだ開く");
            assert_eq!(who(&laptop_key).map(|c| c.id), Some(laptop.id), "巻き添えで閉め出された");

            assert!(!revoke(&phone.id).unwrap(), "二度目も消したと言っている");
        });
    }

    /// A key nobody was given opens nothing, and neither does an empty one --
    /// which is what a request with no `?t=` at all presents
    #[test]
    fn a_key_that_was_never_handed_out_opens_nothing() {
        alone(|| {
            let (_, key) = pair("phone").unwrap();
            assert!(who("").is_none(), "空の鍵が通った");
            assert!(who(&crate::random_hex(24)).is_none(), "配っていない鍵が通った");
            // And the real one still does, so the test above is not passing
            // because everything is refused
            assert!(who(&key).is_some());
        });
    }

    /// Devices are named after the fact, because a phone opening a link cannot
    /// say what it is called until somebody tells it
    #[test]
    fn a_device_can_be_named_and_the_name_stays() {
        alone(|| {
            let (row, key) = pair("").unwrap();
            assert_eq!(row.name, "");
            rename(&row.id, "  仕事のノート  ").unwrap();
            assert_eq!(who(&key).map(|c| c.name), Some("仕事のノート".to_string()));
        });
    }

    /// The book does not grow without end, and what goes is the oldest pairing
    #[test]
    fn the_book_stops_growing() {
        alone(|| {
            let (first, first_key) = pair("first").unwrap();
            for i in 0..MAX_CLIENTS {
                pair(&format!("d{i}")).unwrap();
            }
            let book = load();
            assert_eq!(book.clients.len(), MAX_CLIENTS, "際限なく増えている");
            assert!(who(&first_key).is_none(), "いちばん古いものが残っている");
            assert!(!book.clients.iter().any(|c| c.id == first.id));
        });
    }

    /// Everything at once, for the day it is needed
    #[test]
    fn every_device_can_be_shut_out_at_once() {
        alone(|| {
            let (_, a) = pair("a").unwrap();
            let (_, b) = pair("b").unwrap();
            revoke_all().unwrap();
            assert!(who(&a).is_none() && who(&b).is_none(), "誰かが残っている");
            assert!(load().clients.is_empty());
        });
    }

    /// A damaged file must not lock the runtime out of itself
    #[test]
    fn an_unreadable_book_is_an_empty_one() {
        alone(|| {
            std::fs::write(path(), "{ this is not json").unwrap();
            assert!(load().clients.is_empty(), "壊れた名簿で起動できなくなる");
            // And pairing writes a good one over it
            let (_, key) = pair("after").unwrap();
            assert!(who(&key).is_some());
        });
    }
}

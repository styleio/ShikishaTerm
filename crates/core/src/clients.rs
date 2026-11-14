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
    /// What a person calls it: "kitchen iPad", "work laptop". Empty until the
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
    /// Further keys of the same device, as hashes: one per address the board
    /// has been reached at. A browser keeps a key per address, so a board that
    /// moved (LAN to Tailscale, say) hands the device a key for the new
    /// address rather than taking away the one the old address still holds
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub more: Vec<String>,
}

impl Client {
    /// Whether this row is the one that key opens.
    fn opens(&self, key: &str) -> bool {
        let h = hash_of(key);
        crate::crypto::token_eq(&h, &self.hash) || self.more.iter().any(|m| crate::crypto::token_eq(&h, m))
    }
}

/// The book, as it sits on disk.
#[derive(Debug, Clone, Default)]
pub struct Book {
    pub clients: Vec<Client>,
    /// Rows this version could not read (written by a newer one, or damaged),
    /// exactly as they were found. They let nobody in, and they are written
    /// back untouched: one row that cannot be read is not a reason to drop it,
    /// and certainly not the readable rows beside it
    unread: Vec<serde_json::Value>,
}

impl Book {
    /// The book in a file's text; `None` when the text is not a book at all
    fn parse(text: &str) -> Option<Book> {
        let whole: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
        let rows = match whole.get("clients") {
            Some(serde_json::Value::Array(rows)) => rows.clone(),
            None if whole.is_object() => Vec::new(),
            _ => return None,
        };
        let mut book = Book::default();
        for row in rows {
            match serde_json::from_value::<Client>(row.clone()) {
                Ok(c) => book.clients.push(c),
                Err(_) => book.unread.push(row),
            }
        }
        Some(book)
    }

    fn text(&self) -> anyhow::Result<String> {
        let mut rows: Vec<serde_json::Value> = Vec::with_capacity(self.clients.len() + self.unread.len());
        for c in &self.clients {
            rows.push(serde_json::to_value(c)?);
        }
        rows.extend(self.unread.iter().cloned());
        Ok(serde_json::to_string_pretty(&serde_json::json!({ "clients": rows }))?)
    }
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
    std::fs::read_to_string(path()).ok().and_then(|t| Book::parse(&t)).unwrap_or_default()
}

/// The book for somebody about to write it back.
///
/// Reading for a write is held to more than reading to let somebody in: an
/// empty book written back is every device shut out. So a file that is there
/// and could not be read this once (another program holding it: a virus
/// scanner, a sync client) is an error and nothing is written; only a file
/// that is not there is an empty book. A file that is not a book at all is
/// moved aside under another name before a new one takes its place
fn load_to_write() -> anyhow::Result<Book> {
    let path = path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Book::default()),
        Err(e) => anyhow::bail!(crate::i18n::tp("err.clients.unreadable", &[("why", &e.to_string())])),
    };
    if let Some(book) = Book::parse(&text) {
        return Ok(book);
    }
    let aside = path.with_extension(format!("unreadable-{}.json", now()));
    let _ = std::fs::rename(&path, &aside);
    crate::append_hook_log(&format!("clients: {} is not a device list; kept as {}", path.display(), aside.display()));
    Ok(Book::default())
}

fn save(book: &Book) -> anyhow::Result<()> {
    crate::crypto::write_atomic(&path(), &book.text()?)
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
    let mut book = load_to_write()?;
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
        more: Vec::new(),
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

/// How many further keys one device keeps. The addresses a board is reached
/// at are few -- the LAN, Tailscale's address, a tailnet name in front of it --
/// so this holds every one of them with room for a network that changed, and
/// the oldest goes first beyond that
const MAX_MORE_KEYS: usize = 4;

/// Give a device already in the book one more key of its own, for an address
/// it has not held one for, and hand the key back. The key it holds already
/// keeps working. `None` when the device is no longer in the book
pub fn add_key(id: &str) -> anyhow::Result<Option<String>> {
    let key = crate::random_hex(24);
    let hash = hash_of(&key);
    let found = with_book(|book| match book.clients.iter_mut().find(|c| c.id == id) {
        Some(row) => {
            while row.more.len() >= MAX_MORE_KEYS {
                row.more.remove(0);
            }
            row.more.push(hash);
            true
        }
        None => false,
    })?;
    Ok(found.then_some(key))
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
///
/// What was kept for it goes too: its phone notifications carry what the AI
/// said, and would otherwise go on reaching a device that is out
pub fn revoke(id: &str) -> anyhow::Result<bool> {
    let gone = with_book(|book| {
        let before = book.clients.len();
        book.clients.retain(|c| c.id != id);
        book.clients.len() != before
    })?;
    if gone {
        crate::push::forget_device(Some(id));
    }
    Ok(gone)
}

/// Take every device's key away. For the day something is wrong and working
/// out which device it was costs more than pairing them all again
pub fn revoke_all() -> anyhow::Result<()> {
    // Every row, the ones this version cannot read too: one of them may be
    // a key
    with_book(|book| {
        book.clients.clear();
        book.unread.clear();
    })?;
    crate::push::forget_device(None);
    Ok(())
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
            let _ = std::fs::remove_file(self.file.with_extension("push.json"));
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
            assert!(!text.contains(&key), "the key itself is written");
            assert!(text.contains(&row.hash), "what is used for matching is not written");
            assert_eq!(who(&key).map(|c| c.id), Some(row.id), "it cannot tell it is the same client");
        });
    }

    /// The whole point: one device loses its key without taking the others
    /// with it.
    #[test]
    fn revoking_one_device_leaves_the_others_alone() {
        alone(|| {
            let (phone, phone_key) = pair("phone").unwrap();
            let (laptop, laptop_key) = pair("laptop").unwrap();

            assert!(revoke(&phone.id).unwrap(), "it does not say it was removed");
            assert!(who(&phone_key).is_none(), "a key that was taken away still opens");
            assert_eq!(who(&laptop_key).map(|c| c.id), Some(laptop.id), "it was locked out by mistake");

            assert!(!revoke(&phone.id).unwrap(), "it says it removed it a second time too");
        });
    }

    /// A board that moved to another address gives the device a key there;
    /// the key it holds at the old address still opens, both are the same
    /// row, and taking the device away takes every key it had
    #[test]
    fn a_device_given_a_key_for_a_new_address_keeps_the_old_one() {
        alone(|| {
            let (phone, old) = pair("phone").unwrap();
            let new = add_key(&phone.id).unwrap().expect("no key for a device in the book");
            assert_ne!(new, old);
            let text = std::fs::read_to_string(path()).unwrap();
            assert!(!text.contains(&new), "the new key itself is written");
            assert_eq!(who(&old).map(|c| c.id), Some(phone.id.clone()), "the old address's key stopped opening");
            assert_eq!(who(&new).map(|c| c.id), Some(phone.id.clone()), "the new key opens someone else, or nothing");
            // Only so many: the oldest further key goes first
            let extra: Vec<String> = (0..MAX_MORE_KEYS).map(|_| add_key(&phone.id).unwrap().unwrap()).collect();
            assert!(who(&new).is_none(), "the oldest further key was kept beyond the limit");
            assert!(who(extra.last().unwrap()).is_some() && who(&old).is_some());
            assert!(revoke(&phone.id).unwrap());
            assert!(who(&old).is_none() && who(extra.last().unwrap()).is_none(), "a revoked device kept a key");
            assert_eq!(add_key(&phone.id).unwrap(), None, "a device no longer in the book was given a key");
        });
    }

    /// A key nobody was given opens nothing, and neither does an empty one --
    /// which is what a request with no `?t=` at all presents
    #[test]
    fn a_key_that_was_never_handed_out_opens_nothing() {
        alone(|| {
            let (_, key) = pair("phone").unwrap();
            assert!(who("").is_none(), "an empty key got through");
            assert!(who(&crate::random_hex(24)).is_none(), "a key never handed out got through");
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
            assert_eq!(book.clients.len(), MAX_CLIENTS, "it grows without limit");
            assert!(who(&first_key).is_none(), "the oldest one is still there");
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
            assert!(who(&a).is_none() && who(&b).is_none(), "someone is still there");
            assert!(load().clients.is_empty());
        });
    }

    /// One row this version cannot read costs nobody else their key, and is
    /// still there afterwards for the version that can
    #[test]
    fn a_row_that_cannot_be_read_takes_nothing_with_it() {
        alone(|| {
            let (_, key) = pair("phone").unwrap();
            let mut whole: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path()).unwrap()).unwrap();
            whole["clients"].as_array_mut().unwrap().push(serde_json::json!({"id": 7, "from": "a newer version"}));
            std::fs::write(path(), whole.to_string()).unwrap();
            assert!(who(&key).is_some(), "the readable row stopped letting its device in");
            let (_, other) = pair("tablet").unwrap();
            assert!(who(&key).is_some() && who(&other).is_some());
            let after = std::fs::read_to_string(path()).unwrap();
            assert!(after.contains("a newer version"), "the row was dropped on the next write: {after}");
            revoke_all().unwrap();
            assert!(!std::fs::read_to_string(path()).unwrap().contains("a newer version"), "shutting everyone out left a row");
        });
    }

    /// A book that is there and cannot be read right now is not an empty
    /// book to write back: that would shut every device out
    #[test]
    fn a_book_that_cannot_be_read_right_now_is_not_written_over() {
        alone(|| {
            let (_, key) = pair("phone").unwrap();
            let kept = std::fs::read_to_string(path()).unwrap();
            // A folder where the file should be: reading it fails, and not
            // with "there is none"
            std::fs::remove_file(path()).unwrap();
            std::fs::create_dir(path()).unwrap();
            assert!(pair("tablet").is_err(), "it wrote over a book it could not read");
            std::fs::remove_dir(path()).unwrap();
            std::fs::write(path(), kept).unwrap();
            assert!(who(&key).is_some());
        });
    }

    /// A damaged file must not lock the runtime out of itself
    #[test]
    fn an_unreadable_book_is_an_empty_one() {
        alone(|| {
            std::fs::write(path(), "{ this is not json").unwrap();
            assert!(load().clients.is_empty(), "a broken list stops it from starting");
            // And pairing writes a good one over it
            let (_, key) = pair("after").unwrap();
            assert!(who(&key).is_some());
        });
    }
}

//! Pins and notes a person puts on what was said in a conversation.
//!
//! Kept in `config/conversation-marks.json`, beside the settings and the
//! ideas: what somebody wrote is theirs, and `data` is a folder the app may
//! clear out. Not SQLite, for the same reason the ideas are not: the settings
//! folder can be one another PC writes to as well (a synced drive), and a
//! database with a write-ahead log beside it is exactly what such a folder
//! breaks. One JSON file, read again for every request and written whole.
//!
//! A mark is kept against a place in a CLI's record -- the conversation's id
//! and the byte the thing said begins at. The records are only ever added to,
//! so the place stays the place. No words are copied here: a synced settings
//! folder is no place for a conversation.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The file's shape, versioned so a later one can refuse to read this rather
/// than half-understand it
const VERSION: u32 = 1;
const FILE: &str = "conversation-marks.json";

/// One mark
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mark {
    /// The CLI's id for the conversation
    pub record: String,
    /// Where in its record the thing said begins
    pub at: u64,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// When it was made and last changed, in milliseconds since the epoch
    #[serde(default)]
    pub made: i64,
    #[serde(default)]
    pub changed: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    marks: Vec<Mark>,
}

/// Where the marks are kept
pub fn path() -> PathBuf {
    crate::config::root_dir().join("config").join(FILE)
}

/// One change at a time: a read, a change and a write are one step, or two
/// requests at once would each write back what the other had not seen
static WRITING: Mutex<()> = Mutex::new(());

fn read(file: &Path) -> Result<Store, String> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Ok(Store { version: VERSION, marks: Vec::new() });
    };
    match serde_json::from_str::<Store>(text.trim_start_matches('\u{feff}')) {
        Ok(s) if s.version <= VERSION => Ok(s),
        Ok(_) => Err(crate::i18n::t("err.marks.newer")),
        // Not written over: what somebody wrote is still in there
        Err(e) => Err(crate::i18n::tp("err.marks.unreadable", &[("why", &e.to_string())])),
    }
}

/// Every mark, on every conversation
pub fn all(file: &Path) -> Result<Vec<Mark>, String> {
    Ok(read(file)?.marks)
}

/// The marks on one conversation
pub fn on(file: &Path, record: &str) -> Result<Vec<Mark>, String> {
    Ok(read(file)?.marks.into_iter().filter(|m| m.record == record).collect())
}

/// Pin or unpin what was said at `at` in `record`, or write its note. A mark
/// with neither a pin nor a note is taken away. What it is afterwards is
/// handed back (`None`: nothing is kept)
pub fn set(file: &Path, record: &str, at: u64, pinned: Option<bool>, note: Option<&str>) -> Result<Option<Mark>, String> {
    let _one = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = read(file)?;
    store.version = VERSION;
    let now = crate::sqlite::now_ms();
    let found = store.marks.iter().position(|m| m.record == record && m.at == at);
    let mut mark = match found {
        Some(i) => store.marks.remove(i),
        None => Mark { record: record.to_string(), at, pinned: false, note: String::new(), made: now, changed: now },
    };
    if let Some(p) = pinned {
        mark.pinned = p;
    }
    if let Some(n) = note {
        mark.note = n.trim().to_string();
    }
    mark.changed = now;
    let kept = (mark.pinned || !mark.note.is_empty()).then_some(mark);
    if let Some(m) = &kept {
        store.marks.push(m.clone());
    }
    let text = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    crate::crypto::write_atomic(file, &text).map_err(|e| crate::i18n::tp("err.marks.write", &[("why", &format!("{e:#}"))]))?;
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("marks-{name}-{}-{}", std::process::id(), crate::sqlite::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(FILE)
    }

    #[test]
    fn a_pin_and_a_note_are_kept_until_both_are_taken_away() {
        let f = temp("keep");
        assert!(on(&f, "r").unwrap().is_empty(), "no file is no marks");
        let m = set(&f, "r", 10, Some(true), None).unwrap().unwrap();
        assert!(m.pinned);
        set(&f, "r", 10, None, Some("  the fix  ")).unwrap();
        set(&f, "other", 10, Some(true), None).unwrap();
        let here = on(&f, "r").unwrap();
        assert_eq!((here.len(), here[0].note.as_str(), here[0].pinned), (1, "the fix", true));
        assert!(set(&f, "r", 10, Some(false), None).unwrap().is_some(), "the note still holds it");
        assert!(set(&f, "r", 10, None, Some("")).unwrap().is_none(), "nothing left, nothing kept");
        assert!(on(&f, "r").unwrap().is_empty());
        assert_eq!(on(&f, "other").unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_written_over() {
        let f = temp("bad");
        std::fs::write(&f, "{ not json").unwrap();
        assert!(set(&f, "r", 1, Some(true), None).is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "{ not json");
        std::fs::write(&f, r#"{"version": 99, "marks": []}"#).unwrap();
        assert!(on(&f, "r").is_err(), "a later version's file is left alone");
        let _ = std::fs::remove_dir_all(f.parent().unwrap());
    }
}

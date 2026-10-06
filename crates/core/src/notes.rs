//! Notes on a document, for an AI: a person reads a Markdown file drawn,
//! writes beside a paragraph what should change there, and hands the notes to
//! an AI tab together, each with the lines of the file it is about.
//!
//! Kept in `config/review-notes.json`: what a person wrote is theirs, the way
//! the ideas are (`ideas.rs`), and is read again for every request for the
//! same reason -- the settings folder may be shared with another PC. A note
//! goes when it is handed to an AI: it has been said, and a list that kept
//! what was already said would be handed again.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const VERSION: u32 = 1;
const FILE: &str = "review-notes.json";

/// How much of the words a note is about it keeps, in characters: enough to
/// find the place again, not a copy of the document
const QUOTE_ROOM: usize = 160;

/// One note
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub id: u64,
    /// The folder, by its place key (a folder on another machine says which)
    pub place: String,
    /// The file, from the folder, with forward slashes
    pub path: String,
    /// The lines of the file it is about
    pub from: u64,
    pub to: u64,
    /// The words it was written beside
    #[serde(default)]
    pub quote: String,
    pub text: String,
    /// When it was written, in seconds since 1970
    #[serde(default)]
    pub made: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    notes: Vec<Note>,
}

pub fn path() -> PathBuf {
    crate::config::root_dir().join("config").join(FILE)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Read the file. One that is there and cannot be read is refused rather
/// than taken for empty: taken for empty, the next note would be written over
/// every note in it
fn read(file: &Path) -> Result<Store, String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Store::default()),
        Err(e) => return Err(crate::i18n::tp("err.notes.unreadable", &[("why", &e.to_string())])),
    };
    match serde_json::from_str::<Store>(text.trim_start_matches('\u{feff}')) {
        Ok(s) if s.version <= VERSION => Ok(s),
        Ok(_) => Err(crate::i18n::t("err.notes.newer")),
        Err(e) => Err(crate::i18n::tp("err.notes.unreadable", &[("why", &e.to_string())])),
    }
}

fn write(file: &Path, store: &Store) -> Result<(), String> {
    // Always written as this version: what it reads is this version now
    let store = Store { version: VERSION, notes: store.notes.clone() };
    let text = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
    crate::crypto::write_atomic(file, &text).map_err(|e| format!("{e:#}"))
}

fn cut(s: &str, room: usize) -> String {
    let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() <= room {
        t
    } else {
        t.chars().take(room).collect::<String>() + "…"
    }
}

/// The notes of one folder, in the order they are read in: by file, then by
/// where in it
pub fn of_place(file: &Path, place: &str) -> Result<Vec<Note>, String> {
    let mut notes: Vec<Note> = read(file)?.notes.into_iter().filter(|n| n.place == place).collect();
    notes.sort_by(|a, b| a.path.cmp(&b.path).then(a.from.cmp(&b.from)).then(a.id.cmp(&b.id)));
    Ok(notes)
}

/// Answer one request from the drawn document, and write what it changed.
/// Every answer carries the folder's notes, so a window and a phone looking
/// at the same document show the same notes
pub fn answer(file: &Path, act: &str, args: &Value) -> Value {
    let str_of = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let num_of = |k: &str| args.get(k).and_then(Value::as_u64).unwrap_or(0);
    let place = str_of("place");
    let fail = |e: String| json!({"act": act, "ok": false, "place": place, "error": e});
    let mut store = match read(file) {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let changed = match act {
        "list" => false,
        "add" => {
            let text = str_of("text");
            let path = str_of("path");
            if text.trim().is_empty() || path.is_empty() || place.is_empty() {
                return fail(crate::i18n::t("err.notes.empty"));
            }
            let id = store.notes.iter().map(|n| n.id).max().unwrap_or(0) + 1;
            let (from, to) = (num_of("from").max(1), num_of("to"));
            store.notes.push(Note {
                id, place: place.clone(), path, from, to: to.max(from),
                quote: cut(&str_of("quote"), QUOTE_ROOM), text: text.trim().to_string(), made: now(),
            });
            true
        }
        "edit" => {
            let id = num_of("id");
            let text = str_of("text");
            match store.notes.iter_mut().find(|n| n.id == id) {
                Some(n) if !text.trim().is_empty() => n.text = text.trim().to_string(),
                Some(_) => store.notes.retain(|n| n.id != id),
                None => return fail(crate::i18n::t("err.notes.gone")),
            }
            true
        }
        "drop" => {
            let id = num_of("id");
            store.notes.retain(|n| n.id != id);
            true
        }
        // Every note of one file, or of the folder when no file is named
        "clear" => {
            let path = str_of("path");
            store.notes.retain(|n| !(n.place == place && (path.is_empty() || n.path == path)));
            true
        }
        _ => return fail(format!("unknown act: {act}")),
    };
    if changed && let Err(e) = write(file, &store) {
        return fail(e);
    }
    let mut notes: Vec<Note> = store.notes.into_iter().filter(|n| n.place == place).collect();
    notes.sort_by(|a, b| a.path.cmp(&b.path).then(a.from.cmp(&b.from)).then(a.id.cmp(&b.id)));
    json!({"act": act, "ok": true, "place": place, "notes": notes})
}

/// The notes as an AI tab is handed them: one line of what they are, then a
/// line each -- the place as `path:line` (the form every AI CLI reads, and
/// the one the editor's "Tell the AI" writes), the words it is beside, and
/// what the person wrote
pub fn describe(notes: &[Note]) -> String {
    let mut out = crate::i18n::tp("md.notes.handed", &[("n", &notes.len().to_string())]);
    for n in notes {
        let at = if n.from == n.to { format!("{}:{}", n.path, n.from) } else { format!("{}:{}-{}", n.path, n.from, n.to) };
        out.push_str("\n- ");
        out.push_str(&at);
        if !n.quote.is_empty() {
            out.push_str(&format!(" \u{300c}{}\u{300d}", n.quote));
        }
        out.push_str(" \u{2192} ");
        out.push_str(&n.text.replace('\n', " / "));
    }
    out
}

/// Take the notes that were handed out of the file
pub fn forget(file: &Path, ids: &[u64]) -> Result<(), String> {
    let mut store = read(file)?;
    store.notes.retain(|n| !ids.contains(&n.id));
    write(file, &store)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("notes-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join(FILE)
    }

    #[test]
    fn notes_are_kept_by_folder_and_read_in_the_order_of_the_file() {
        let f = scratch("order");
        let add = |path: &str, from: u64, text: &str| answer(&f, "add", &json!({"place": "D:/w", "path": path, "from": from, "to": from, "text": text, "quote": "  some   words  "}));
        add("docs/b.md", 3, "later file");
        add("docs/a.md", 9, "second");
        let last = add("docs/a.md", 2, "first");
        let order: Vec<&str> = last["notes"].as_array().unwrap().iter().map(|n| n["text"].as_str().unwrap()).collect();
        assert_eq!(order, ["first", "second", "later file"]);
        assert_eq!(last["notes"][0]["quote"], "some words");
        answer(&f, "add", &json!({"place": "D:/other", "path": "x.md", "from": 1, "text": "elsewhere"}));
        assert_eq!(answer(&f, "list", &json!({"place": "D:/w"}))["notes"].as_array().unwrap().len(), 3);
        assert_eq!(answer(&f, "add", &json!({"place": "D:/w", "path": "a.md", "text": "  "}))["ok"], false);
    }

    #[test]
    fn a_note_edited_empty_goes_and_a_file_cleared_keeps_the_others() {
        let f = scratch("edit");
        let a = answer(&f, "add", &json!({"place": "P", "path": "a.md", "from": 1, "text": "x"}));
        let id = a["notes"][0]["id"].as_u64().unwrap();
        assert_eq!(answer(&f, "edit", &json!({"place": "P", "id": id, "text": "better"}))["notes"][0]["text"], "better");
        answer(&f, "add", &json!({"place": "P", "path": "b.md", "from": 1, "text": "y"}));
        let left = answer(&f, "clear", &json!({"place": "P", "path": "a.md"}));
        assert_eq!(left["notes"].as_array().unwrap().len(), 1);
        let gone = answer(&f, "edit", &json!({"place": "P", "id": left["notes"][0]["id"], "text": ""}));
        assert!(gone["notes"].as_array().unwrap().is_empty());
    }

    #[test]
    fn the_notes_are_handed_as_places_the_ai_reads() {
        let notes = vec![
            Note { id: 1, place: "P".into(), path: "docs/a.md".into(), from: 4, to: 4, quote: "対象の文".into(), text: "短く".into(), made: 0 },
            Note { id: 2, place: "P".into(), path: "docs/a.md".into(), from: 10, to: 12, quote: String::new(), text: "line one\nline two".into(), made: 0 },
        ];
        let text = describe(&notes);
        assert!(text.contains("\n- docs/a.md:4 \u{300c}対象の文\u{300d} \u{2192} 短く"), "{text}");
        assert!(text.contains("\n- docs/a.md:10-12 \u{2192} line one / line two"), "{text}");
    }
}

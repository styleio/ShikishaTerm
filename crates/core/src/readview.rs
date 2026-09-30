//! Text this program made, shown to a person in an editor tab that only reads.
//!
//! A page's source, its DOM, a check's log from the git panel, and whatever
//! comes next of the same sort: the
//! app has the text, nobody is to change it, and it is not a file anywhere.
//! Every one of them goes through here, so that each gets the same promises
//! without writing them again (`.claude/RULES.md`, DRY):
//!
//! - the text is held in memory by the editor's key and never written to a
//!   disk; it goes when the editor is closed ([`Held::keep_open`])
//! - it is held to the most the editor opens of a file ([`crate::files::READ_LIMIT`]),
//!   cut on a character's edge, and a text that was cut says so on its first
//!   line, in the way its kind of text writes a line nobody should mistake
//!   for its content ([`Kind`])
//! - a save is refused ([`Held::answer`]); the page hides saving and "Tell the
//!   AI" for an editor marked `read_only`, and Ace is set to refuse typing
//!
//! Something this entry point cannot do yet is added here, not worked around
//! at the caller.

use crate::i18n;
use crate::view::EditorOpen;
use std::collections::HashMap;
use std::path::PathBuf;

/// How the editors showing held text are named. Not a name a setting can give
/// an editor, so the two never meet
pub const KEY_PREFIX: &str = "view:";

/// What sort of text it is: which end of a text too big is kept, and how the
/// line saying so is written
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A page's HTML. Its start is kept -- the head and the top of the body
    /// are where a reader begins -- and the line is an HTML comment, so the
    /// text still reads as HTML
    Html,
    /// A job's log. Its end is kept -- a run fails at its end -- and the line
    /// is a plain line, as a log has no comments
    Log,
}

/// An editor just opened on held text
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// The editor's key, for it to be brought forward
    pub key: String,
    /// Whether the text was cut, so that a line counted in the whole text is
    /// one further down in the editor (the line saying so comes first)
    pub cut: bool,
}

/// The texts being shown, by the editor's key
#[derive(Debug, Default)]
pub struct Held {
    texts: HashMap<String, String>,
    /// How many have been opened: the next key's number. Never reused, so an
    /// editor closed and another opened are never confused
    made: u64,
}

impl Held {
    /// Open `text` in an editor that only reads, called `title` on its tab
    /// ("shop (Source code).html": what it is, of what) and listed under the
    /// folder `under` when it has one on this PC
    pub fn open(&mut self, editors: &mut Vec<EditorOpen>, title: String, kind: Kind, text: String, under: Option<PathBuf>) -> Opened {
        self.made += 1;
        let key = format!("{KEY_PREFIX}{}", self.made);
        let (text, cut) = fit(text, kind);
        self.texts.insert(key.clone(), text);
        editors.push(EditorOpen {
            key: key.clone(),
            dir: under,
            showing: Some(title),
            stamp: None,
            scratch: true,
            at: None,
            on: None,
            diff: None,
            read_only: true,
        });
        Opened { key, cut }
    }

    /// What the page asks of an editor showing held text: it reads what is
    /// held, and a save is refused. `None` for an editor that is not one of
    /// these, or for any other act, which goes on to the folder the editor
    /// stands in (the column's file list)
    pub fn answer(&self, panel: &str, act: &str, args: &serde_json::Value) -> Option<String> {
        let text = self.texts.get(panel)?;
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or_default();
        match act {
            "read" => Some(crate::runtime::read_reply(panel, path, text.as_bytes(), args, String::new())),
            "write" => Some(
                serde_json::json!({"act": "write", "panel": panel, "path": path, "ok": false,
                    "error": i18n::t("err.page_view.read_only")})
                .to_string(),
            ),
            _ => None,
        }
    }

    /// Let go of every text whose editor has been closed
    pub fn keep_open(&mut self, editors: &[EditorOpen]) {
        self.texts.retain(|k, _| editors.iter().any(|e| &e.key == k));
    }
}

/// A text as it goes into the editor: whole when it is within the editor's
/// limit, else cut on a character's edge with a first line saying it was cut
/// and how big it was -- a text that stopped short with nothing said would
/// read as the whole of it. Says whether it cut
pub fn fit(text: String, kind: Kind) -> (String, bool) {
    let most = crate::files::READ_LIMIT as usize;
    if text.len() <= most {
        return (text, false);
    }
    let mb = |n: usize| format!("{:.1}", n as f64 / (1024.0 * 1024.0));
    let fitted = match kind {
        Kind::Html => {
            let mut end = most;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let note = i18n::tp("msg.page_view.cut", &[("shown", &mb(end)), ("whole", &mb(text.len()))]);
            format!("<!-- {note} -->\n{}", &text[..end])
        }
        Kind::Log => {
            let mut start = text.len() - most;
            while !text.is_char_boundary(start) {
                start += 1;
            }
            let note = i18n::tp("msg.ci_log.cut", &[("shown", &mb(text.len() - start)), ("whole", &mb(text.len()))]);
            format!("{note}\n{}", &text[start..])
        }
    };
    (fitted, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Held text is read from what is held, a save of it is refused whatever
    /// it says, and anything else -- or an editor that is not one of these --
    /// goes on to the folder
    #[test]
    fn held_text_reads_and_refuses_to_save() {
        let mut held = Held::default();
        let mut editors = Vec::new();
        let key = held.open(&mut editors, "shop (Source code).html".into(), Kind::Html, "<p>hi</p>".into(), None).key;
        assert!(key.starts_with(KEY_PREFIX));
        assert!(editors[0].read_only && editors[0].scratch, "the editor only reads");
        let args = serde_json::json!({"path": "shop (Source code).html"});
        let read: serde_json::Value = serde_json::from_str(&held.answer(&key, "read", &args).unwrap()).unwrap();
        assert_eq!(read["ok"], true);
        assert_eq!(read["text"], "<p>hi</p>");
        let write: serde_json::Value =
            serde_json::from_str(&held.answer(&key, "write", &serde_json::json!({"path": "x", "text": "changed"})).unwrap()).unwrap();
        assert_eq!(write["ok"], false);
        assert!(held.answer(&key, "ls", &args).is_none());
        assert!(held.answer("some-editor", "read", &args).is_none(), "an editor on a file is not answered here");
    }

    /// Two opened are two keys, and a closed one's text is let go
    #[test]
    fn a_closed_view_lets_go_of_its_text() {
        let mut held = Held::default();
        let mut editors = Vec::new();
        let a = held.open(&mut editors, "a".into(), Kind::Html, "a".into(), None).key;
        let b = held.open(&mut editors, "b".into(), Kind::Log, "b".into(), None).key;
        assert_ne!(a, b);
        editors.retain(|e| e.key != a);
        held.keep_open(&editors);
        let args = serde_json::json!({"path": "a"});
        assert!(held.answer(&a, "read", &args).is_none());
        assert!(held.answer(&b, "read", &args).is_some());
    }

    /// Past the editor's own limit an HTML text keeps its start, is cut on a
    /// character's edge, and says so in a comment on its first line
    #[test]
    fn html_too_big_keeps_its_start_and_says_so() {
        let most = crate::files::READ_LIMIT as usize;
        assert_eq!(fit("<p>short</p>".into(), Kind::Html), ("<p>short</p>".to_string(), false));
        let big = "あ".repeat(most / 3 + 10);
        let (cut, was) = fit(big.clone(), Kind::Html);
        assert!(was);
        assert!(cut.starts_with("<!-- "), "no line saying it was cut");
        let body = &cut[cut.find('\n').unwrap() + 1..];
        assert!(body.len() <= most && big.starts_with(body));
    }

    /// A log too big keeps its end, where a run fails, and says so on a
    /// plain first line
    #[test]
    fn a_log_too_big_keeps_its_end_and_says_so() {
        let most = crate::files::READ_LIMIT as usize;
        assert_eq!(fit("ok\n".into(), Kind::Log), ("ok\n".to_string(), false));
        let big = format!("{}the end\n", "あ".repeat(most / 3 + 10));
        let (cut, was) = fit(big.clone(), Kind::Log);
        assert!(was);
        let (note, body) = cut.split_once('\n').unwrap();
        assert!(!note.starts_with("<!--") && !note.is_empty(), "no plain line saying it was cut");
        assert!(body.len() <= most && big.ends_with(body) && body.ends_with("the end\n"));
    }
}

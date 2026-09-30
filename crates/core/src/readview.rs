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
    /// Which lines of the whole text the editor shows, for a caller that
    /// knows a line of the whole text and wants it in the editor
    lines: Kept,
}

impl Opened {
    /// Where line `whole` (counted from 1 in the text as it was given) is in
    /// the editor, or `None` when cutting the text to size took that line
    /// away. The one place that knows how a cut moves lines: a caller adding
    /// its own offset would be right only until the next kind of text
    pub fn editor_line(&self, whole: usize) -> Option<usize> {
        self.lines.editor_line(whole)
    }
}

/// Which lines of a text made it into the editor after [`fit`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Kept {
    /// Lines of the whole text before the first one kept, even in part
    dropped_before: usize,
    /// The last line of the whole text kept, even in part (`None`: through
    /// the end)
    last: Option<usize>,
    /// Lines added in front of the text: the line saying it was cut
    note: usize,
}

impl Kept {
    fn editor_line(&self, whole: usize) -> Option<usize> {
        if whole == 0 || whole <= self.dropped_before || self.last.is_some_and(|last| whole > last) {
            return None;
        }
        Some(whole - self.dropped_before + self.note)
    }
}

/// The texts being shown, by the editor's key
#[derive(Debug, Default)]
pub struct Held {
    texts: HashMap<String, String>,
    /// How many have been opened: the next key's number. Never reused, so an
    /// editor closed and another opened are never confused
    made: u64,
}

/// The folder held text is listed under: the one it is about (the page's, the
/// git panel's), wherever that folder is. On another machine it carries the
/// machine too, so the editor stands in that folder's row and not in a folder
/// of the same path on this PC -- nor, as it did before, in no folder at all
#[derive(Debug, Clone, Default)]
pub struct Under {
    pub dir: PathBuf,
    /// The machine's name, when the folder is on another machine
    pub on: Option<String>,
    /// How to reach it, for what the column beside the editor lists
    pub at: Option<crate::elsewhere::Elsewhere>,
}

impl Under {
    /// The folder a place key names (see [`crate::uistate::place_key`]), as
    /// the desk has it. A path on this PC the desk does not have is still
    /// where it is; a place on another machine the desk does not have is
    /// nowhere this program can reach, and nothing to stand in
    pub fn of_place(desk: &crate::config::Desk, key: &std::path::Path) -> Option<Under> {
        match desk.folder_at(key) {
            Some(folder) => Some(Under {
                dir: folder.cwd.clone()?,
                on: folder.host.as_ref().map(|h| h.name.clone()),
                at: folder.host.as_ref().and_then(|h| crate::elsewhere::Elsewhere::of(h).ok()),
            }),
            None => match crate::uistate::place_of(key) {
                (None, dir) if !dir.as_os_str().is_empty() => Some(Under { dir, on: None, at: None }),
                _ => None,
            },
        }
    }
}

impl Held {
    /// Open `text` in an editor that only reads, called `title` on its tab
    /// ("shop (Source code).html": what it is, of what) and listed under the
    /// folder `under`
    pub fn open(&mut self, editors: &mut Vec<EditorOpen>, title: String, kind: Kind, text: String, under: Option<Under>) -> Opened {
        let (text, lines) = fit(text, kind);
        self.place(editors, title, text, lines, under)
    }

    /// Open the end of a text that was too big to hold, read with
    /// [`read_tail`]: its first line says the start is not here, and lines are
    /// counted in the whole text, as [`Opened::editor_line`] expects
    pub fn open_tail(&mut self, editors: &mut Vec<EditorOpen>, title: String, tail: Tail, under: Option<Under>) -> Opened {
        if !tail.cut {
            return self.open(editors, title, Kind::Log, tail.text, under);
        }
        let mb = |n: u64| format!("{:.1}", n as f64 / (1024.0 * 1024.0));
        let note = i18n::tp("msg.ci_log.cut", &[("shown", &mb(tail.text.len() as u64)), ("whole", &mb(tail.whole))]);
        // Already cut to size as it was read; cut again here, it would lose
        // the lines the count above is about
        let lines = Kept { dropped_before: tail.dropped_lines, last: None, note: 1 };
        self.place(editors, title, format!("{note}\n{}", tail.text), lines, under)
    }

    fn place(&mut self, editors: &mut Vec<EditorOpen>, title: String, text: String, lines: Kept, under: Option<Under>) -> Opened {
        self.made += 1;
        let key = format!("{KEY_PREFIX}{}", self.made);
        self.texts.insert(key.clone(), text);
        let under = under.unwrap_or_default();
        editors.push(EditorOpen {
            key: key.clone(),
            dir: Some(under.dir).filter(|d| !d.as_os_str().is_empty()),
            showing: Some(title),
            stamp: None,
            scratch: true,
            at: under.at,
            on: under.on,
            diff: None,
            read_only: true,
        });
        Opened { key, lines }
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

/// The end of a text read from a stream, never holding more than a bounded
/// amount of it. What a download too big to hold turns into
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tail {
    /// The last lines, whole ones only once anything was cut
    pub text: String,
    /// Whole lines of the stream before the first one in `text`
    pub dropped_lines: usize,
    /// How many bytes the stream carried in all
    pub whole: u64,
    /// Whether anything was left out
    pub cut: bool,
}

/// Read a stream to its end, keeping its last `most` bytes at most.
///
/// Held at most twice `most` (and one read's worth) at any moment: the start
/// of a stream too big is let go as it goes by, counting its lines so that a
/// line of the whole can still be found in what is kept. Once anything is let
/// go, the line the cut fell inside is let go too -- half a line reads as a
/// line that says something else
pub fn read_tail(mut from: impl std::io::Read, most: usize) -> std::io::Result<Tail> {
    let lines_in = |b: &[u8]| b.iter().filter(|&&c| c == b'\n').count();
    let mut kept: Vec<u8> = Vec::new();
    let mut tail = Tail::default();
    let mut chunk = vec![0u8; 64 * 1024];
    let let_go = |kept: &mut Vec<u8>, tail: &mut Tail| {
        if kept.len() > most {
            let gone = kept.len() - most;
            tail.dropped_lines += lines_in(&kept[..gone]);
            kept.drain(..gone);
            tail.cut = true;
        }
    };
    loop {
        let n = match from.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        tail.whole += n as u64;
        kept.extend_from_slice(&chunk[..n]);
        // Let go in large steps, not on every read: draining the front is a
        // copy of what stays
        if kept.len() > most.saturating_mul(2) {
            let_go(&mut kept, &mut tail);
        }
    }
    let_go(&mut kept, &mut tail);
    if tail.cut {
        match kept.iter().position(|&c| c == b'\n') {
            Some(end) => {
                kept.drain(..=end);
                tail.dropped_lines += 1;
            }
            None => kept.clear(),
        }
    }
    tail.text = String::from_utf8_lossy(&kept).into_owned();
    Ok(tail)
}

/// A text as it goes into the editor: whole when it is within the editor's
/// limit, else cut on a character's edge with a first line saying it was cut
/// and how big it was -- a text that stopped short with nothing said would
/// read as the whole of it. Says which lines of the whole it kept
pub fn fit(text: String, kind: Kind) -> (String, Kept) {
    let most = crate::files::READ_LIMIT as usize;
    if text.len() <= most {
        return (text, Kept::default());
    }
    let mb = |n: usize| format!("{:.1}", n as f64 / (1024.0 * 1024.0));
    let lines_in = |s: &str| s.bytes().filter(|&b| b == b'\n').count();
    match kind {
        Kind::Html => {
            let mut end = most;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let note = i18n::tp("msg.page_view.cut", &[("shown", &mb(end)), ("whole", &mb(text.len()))]);
            // The line the cut falls in is kept in part
            let kept = Kept { dropped_before: 0, last: Some(lines_in(&text[..end]) + 1), note: 1 };
            (format!("<!-- {note} -->\n{}", &text[..end]), kept)
        }
        Kind::Log => {
            let mut start = text.len() - most;
            while !text.is_char_boundary(start) {
                start += 1;
            }
            let note = i18n::tp("msg.ci_log.cut", &[("shown", &mb(text.len() - start)), ("whole", &mb(text.len()))]);
            // Every line that ended before the cut is gone; the one it falls
            // in is kept in part
            let kept = Kept { dropped_before: lines_in(&text[..start]), last: None, note: 1 };
            (format!("{note}\n{}", &text[start..]), kept)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream that fits is kept whole. One that does not keeps its end, from
    /// the start of a line, and counts the lines let go -- never holding the
    /// whole of it on the way
    #[test]
    fn a_stream_too_big_keeps_its_end_by_whole_lines() {
        let small = read_tail("a\nb\n".as_bytes(), 100).unwrap();
        assert_eq!(small, Tail { text: "a\nb\n".into(), dropped_lines: 0, whole: 4, cut: false });

        let log: String = (1..=1000).map(|i| format!("line {i}\n")).collect();
        let tail = read_tail(log.as_bytes(), 100).unwrap();
        assert!(tail.cut);
        assert_eq!(tail.whole, log.len() as u64);
        assert!(tail.text.len() <= 100, "kept more than asked: {}", tail.text.len());
        // Whole lines only, and the count says which line comes first
        let first = tail.text.lines().next().unwrap();
        assert_eq!(first, format!("line {}", tail.dropped_lines + 1));
        assert!(tail.text.ends_with("line 1000\n"));

        // A stream read a little at a time is kept the same way
        struct Drip<'a>(&'a [u8]);
        impl std::io::Read for Drip<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let n = self.0.len().min(7).min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        assert_eq!(read_tail(Drip(log.as_bytes()), 100).unwrap(), tail);
    }

    /// The end of a stream opens with its line saying the start is missing,
    /// and a line of the whole log is found where the editor shows it; a line
    /// that was let go is nowhere
    #[test]
    fn the_end_of_a_stream_is_found_by_the_whole_logs_lines() {
        let log: String = (1..=1000).map(|i| format!("line {i}\n")).collect();
        let tail = read_tail(log.as_bytes(), 100).unwrap();
        let first_kept = tail.dropped_lines + 1;
        let mut held = Held::default();
        let mut editors = Vec::new();
        let opened = held.open_tail(&mut editors, "job @ abc".into(), tail, None);
        let text = held.texts.get(&opened.key).unwrap().clone();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(opened.editor_line(first_kept), Some(2), "the note is line 1");
        assert_eq!(lines[1], format!("line {first_kept}"));
        assert_eq!(opened.editor_line(1000).map(|l| lines[l - 1]), Some("line 1000"));
        assert_eq!(opened.editor_line(first_kept - 1), None);
    }

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

    /// Held text about a folder on another machine stands in that folder --
    /// with its machine, so not in a folder of the same path on this PC, and
    /// not in no folder at all, which is where it used to land
    #[test]
    fn held_text_stands_in_its_folder_wherever_that_is() {
        let cfg: crate::config::Config = serde_json::from_str(
            r#"{"hosts": [{"name": "bench", "at": "ssh://me@bench"}],
              "desks": [{"name": "Demo", "folders": [
                {"cwd": "/srv/proj", "host": "bench", "tabs": []},
                {"cwd": "/srv/proj", "tabs": []}]}]}"#,
        )
        .unwrap();
        let desk = &cfg.resolve_desks().0[0];
        let far = std::path::PathBuf::from(crate::uistate::place_key(Some("bench"), std::path::Path::new("/srv/proj")));
        let under = Under::of_place(desk, &far).expect("the folder over there is not found");
        assert_eq!(under.on.as_deref(), Some("bench"));
        assert!(under.at.is_some(), "the folder's machine cannot be reached from its editor");
        let mut held = Held::default();
        let mut editors = Vec::new();
        held.open(&mut editors, "ci @ abc1234".into(), Kind::Log, "log".into(), Some(under));
        assert_eq!(editors[0].on.as_deref(), Some("bench"));
        assert_eq!(editors[0].dir.as_deref(), Some(std::path::Path::new("/srv/proj")));
        // This PC's folder of the same path is its own place
        let here = Under::of_place(desk, std::path::Path::new("/srv/proj")).unwrap();
        assert!(here.on.is_none() && here.at.is_none());
        // A machine the desk does not have is nowhere to stand
        let nowhere = std::path::PathBuf::from(crate::uistate::place_key(Some("gone"), std::path::Path::new("/srv/proj")));
        assert!(Under::of_place(desk, &nowhere).is_none());
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
        assert_eq!(fit("<p>short</p>".into(), Kind::Html), ("<p>short</p>".to_string(), Kept::default()));
        let big = "あ".repeat(most / 3 + 10);
        let (cut, was) = fit(big.clone(), Kind::Html);
        assert_ne!(was, Kept::default());
        assert!(cut.starts_with("<!-- "), "no line saying it was cut");
        let body = &cut[cut.find('\n').unwrap() + 1..];
        assert!(body.len() <= most && big.starts_with(body));
    }

    /// A log too big keeps its end, where a run fails, and says so on a
    /// plain first line
    #[test]
    fn a_log_too_big_keeps_its_end_and_says_so() {
        let most = crate::files::READ_LIMIT as usize;
        assert_eq!(fit("ok\n".into(), Kind::Log), ("ok\n".to_string(), Kept::default()));
        let big = format!("{}the end\n", "あ".repeat(most / 3 + 10));
        let (cut, was) = fit(big.clone(), Kind::Log);
        assert_ne!(was, Kept::default());
        let (note, body) = cut.split_once('\n').unwrap();
        assert!(!note.starts_with("<!--") && !note.is_empty(), "no plain line saying it was cut");
        assert!(body.len() <= most && big.ends_with(body) && body.ends_with("the end\n"));
    }

    /// A line of the whole text is found where the editor has it once the
    /// text is cut: a log that lost its head moves every line up by what was
    /// lost and down by the note, a line that was lost is nowhere, and an
    /// HTML text that lost its tail keeps its lines where they were, below
    /// the note. Adding only the note put the jump on the wrong line, or on a
    /// line that did not exist, for any log too big to show whole
    #[test]
    fn a_line_of_the_whole_is_found_where_the_editor_has_it() {
        let most = crate::files::READ_LIMIT as usize;
        // A log of lines of 100 bytes, well over the limit, with an error
        // near its end and one near its start
        let line = |i: usize| format!("{:0>98}\n", i);
        let count = most / 100 + 500;
        let mut log = String::new();
        for i in 1..=count {
            log.push_str(&line(i));
        }
        let (text, kept) = fit(log.clone(), Kind::Log);
        let lines: Vec<&str> = text.lines().collect();
        let late = count - 3;
        let at = kept.editor_line(late).expect("a line near the end was kept");
        assert_eq!(lines[at - 1], line(late).trim_end(), "the jump lands on the line asked for");
        assert_eq!(kept.editor_line(2), None, "a line cut away is nowhere");
        assert_eq!(kept.editor_line(count).map(|n| lines[n - 1]), Some(line(count).trim_end()));

        // Whole text: lines stay where they are
        assert_eq!(fit("a\nb\n".into(), Kind::Log).1.editor_line(2), Some(2));

        // HTML keeps its head, below the note; past the cut is nowhere
        let (html, kept) = fit(log, Kind::Html);
        let lines: Vec<&str> = html.lines().collect();
        assert_eq!(lines[kept.editor_line(7).unwrap() - 1], line(7).trim_end());
        assert_eq!(kept.editor_line(count), None);
    }
}

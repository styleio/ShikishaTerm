//! Reading a CLI's own record back as a conversation.
//!
//! A long answer cannot be read off the terminal from a phone, and the reason
//! is not the link. Claude Code in its full-screen mode runs on the alternate
//! screen, which keeps no scrollback at all: what scrolls past is gone from
//! our side the moment it moves, which is why the phone's pager does not
//! scroll our copy of the screen — it asks the CLI to scroll its own, one
//! round trip at a time. The only complete copy of what was said is the record
//! the CLI keeps for itself, so that is what this reads.
//!
//! Read from the END, backwards. These files grow for the whole life of a
//! conversation (318 MB on an ordinary week of work, measured), and the part
//! anyone opens a reader for is the last thing said. A reader that walked from
//! the front would get slower every day it was used.
//!
//! Format-blind on purpose — the same stance `vault.rs` takes, for the same
//! reason: every CLI arranges its JSON differently and rearranges it between
//! releases, so a spec per CLI is a promise to keep chasing them. Two rules
//! hold across all of them instead:
//!
//!   - the message is the object carrying a `role` — the record itself, or the
//!     single field it is wrapped in (`message` for Claude, `payload` for Codex).
//!     A record with no role anywhere names its speaker in its own `type`
//!     instead (`user` / `gemini` for Gemini)
//!   - the words are the blocks whose type ENDS in "text" (`text` for Claude,
//!     `output_text` / `input_text` for Codex), or blocks with no type at all
//!     that carry a `text` and are not marked as a thought (Gemini's parts).
//!     Everything else in a content list is machinery — a tool call, its
//!     result, the model's own thinking — and machinery is not what a person
//!     opens a reader to read
//!
//! And not everything said was said to anybody. An AI working its way through
//! a job writes a line before each tool it reaches for ("Now the reload
//! clamp:"), and those lines are filed exactly like an answer. Read back with
//! the tool calls taken out they run together into a page of orphaned
//! sentences, in whatever language the CLI narrates its own work in — which is
//! the opposite of what a reader is opened for. A record said in the same
//! breath as the tool call that follows it is an aside about the work, and is
//! left out with the rest of the machinery.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most of one stretch of work read to open it. A stretch longer than
/// this is a tool that printed megabytes, and what is shown of it is its start
const WORK_MOST: u64 = 16 * 1024 * 1024;

/// How much of the record to pull in one step. Small enough that a reader
/// asking for the last few turns touches a fraction of a megabyte
const CHUNK: usize = 256 * 1024;

/// The most one request may read before giving up and saying "there is more".
/// A conversation whose recent turns are buried under megabytes of tool output
/// still has to answer in the time a person will wait for a tap
const BUDGET: usize = 8 * 1024 * 1024;

/// Who said it. Named for the reader, not for the API underneath: the person
/// holding the phone is "you", and everything the CLI produced is the AI
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Who {
    You,
    Ai,
}

/// One thing said, as text. No markup is applied here — the reading side
/// decides how a fenced code block or a heading should look, and it is the
/// only side that knows how wide the screen is
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Turn {
    pub who: Who,
    pub text: String,
    /// Where in the record it begins: the byte its first line starts at. The
    /// same place in the same record is the same thing said, which is what a
    /// pin or a note is kept against. Set by the walk that found it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    /// When it was said, in milliseconds since the epoch, where the record
    /// says (a `timestamp` beside the message). For an answer said in several
    /// pieces, when its last piece was written
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<i64>,
}

/// A stretch of the conversation, oldest turn first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Page {
    pub turns: Vec<Turn>,
    /// Where in the file this page starts. Handed back so the next request can
    /// ask for what comes BEFORE it — the cursor for reading further into the
    /// past. A byte offset, never a turn count: turns are not addressable, and
    /// counting them from the front is the walk this module exists to avoid
    pub from: u64,
    /// Whether anything older is left. False only when the head of the file
    /// has actually been reached, so the reader can stop asking
    pub more: bool,
    /// The work the AI did between the things said on this page: where each
    /// stretch of it lies, to be opened when somebody asks (`work_at`)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub work: Vec<Stretch>,
}

/// One stretch of work between two things said: the tools the AI reached
/// for, what they gave back, and what it said on the way. Only where it lies
/// and how many tools it called -- its contents are nearly all of a record's
/// bytes, and are read when somebody opens it
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Stretch {
    /// Where it begins in the record, and where the next thing said begins
    pub from: u64,
    pub to: u64,
    pub calls: usize,
    /// Whether it holds what was searched for (`read_after` with a needle)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hit: bool,
}

/// A stretch of the conversation read forwards from a place in it (see
/// [`read_after`]), oldest turn first.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Later {
    pub turns: Vec<Turn>,
    /// Where the next stretch begins: the cursor for reading on
    pub to: u64,
    /// Whether anything newer is left in the record as it is now
    pub more: bool,
    /// The work between the things said (see [`Page::work`])
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub work: Vec<Stretch>,
}

/// The last `want` things said in `path`, before byte `before`.
///
/// `want` counts THINGS SAID, not records. A CLI writes one turn as a run of
/// records — a paragraph, a tool call, another paragraph — and Claude files a
/// tool's result as a message from the user, so a handful of records is
/// routinely one side of one exchange and the human's own words sit a long way
/// further back. Counting records would make "the last exchange" mean whatever
/// the tool traffic happened to look like.
///
/// A block is only handed back once it has been read to its head: the walk
/// stops when it meets the START of one block too many, which is the proof
/// that the block before it is whole. Half a turn, missing its opening, is the
/// one thing this must never return — it is the failure the reader exists to
/// end.
///
/// `before` is `u64::MAX` for "the end of the file" — the first page. Every
/// page after that passes the previous page's `from`.
pub fn read_back(path: &Path, before: u64, want: usize) -> std::io::Result<Page> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    read_back_by(len, before, want, &mut |start, n| {
        let mut buf = vec![0u8; n];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buf)?;
        Ok(buf)
    })
}

/// The last thing the AI said in what was written to `path` after byte `from`.
///
/// `from` is how long the record was when a turn began, so what is read is
/// that turn and nothing before it: an answer the record has not caught up
/// with yet reads as `None`, never as the previous turn's answer. The file
/// written from its start again (a new conversation, a rewrite) is read whole
pub fn said_after(path: &Path, from: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = if from > len { 0 } else { from };
    let page = read_back_by(len - from, u64::MAX, 1, &mut |start, n| {
        let mut buf = vec![0u8; n];
        file.seek(SeekFrom::Start(from + start))?;
        file.read_exact(&mut buf)?;
        Ok(buf)
    })
    .ok()?;
    page.turns
        .into_iter()
        .last()
        .filter(|t| t.who == Who::Ai)
        .map(|t| t.text)
}

/// The first `want` things said at or after byte `from`, read forwards: what
/// a reader opened at one place in a conversation shows when it is asked for
/// what came next. The same reading as [`read_back`] -- an answer said in
/// pieces is one answer, what a person sent is one thing each, and a line
/// said on the way to a tool is an aside, not an answer
///
/// `needle` (lowercase, or empty) marks each stretch of work that holds it
pub fn read_after(path: &Path, from: u64, want: usize, needle: &str) -> std::io::Result<Later> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    read_after_by(len, from, want, needle, &mut |start, n| {
        let mut buf = vec![0u8; n];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buf)?;
        Ok(buf)
    })
}

/// [`read_after`] over a record read a piece at a time by `read_at`
pub fn read_after_by(
    len: u64,
    from: u64,
    want: usize,
    needle: &str,
    read_at: &mut dyn FnMut(u64, usize) -> std::io::Result<Vec<u8>>,
) -> std::io::Result<Later> {
    let mut at = from.min(len);
    let mut found: Vec<Turn> = Vec::new();
    let mut work: Vec<Stretch> = Vec::new();
    // The work since somebody last spoke, not closed yet: it ends where the
    // next thing said begins
    let mut open: Option<Stretch> = None;
    // What the AI has said since somebody last spoke or it last reached for a
    // tool: an answer if somebody speaks next, an aside if a tool comes first
    let mut waiting: Vec<Turn> = Vec::new();
    let mut read = 0usize;
    // Where the line being looked at starts, and the part of it read so far
    let mut carried: Vec<u8> = Vec::new();
    let mut carried_at = at;
    let mut stop_at: Option<u64> = None;
    fn settle(found: &mut Vec<Turn>, waiting: &mut Vec<Turn>) {
        for turn in waiting.drain(..) {
            match found.last_mut() {
                Some(last) if joins(&turn, last) => {
                    last.text = format!("{}\n\n{}", last.text, turn.text);
                    last.when = turn.when.or(last.when);
                }
                _ => found.push(turn),
            }
        }
    }
    // The stretch is over: it ends where the answer after it begins, or where
    // the next person's line does
    fn close(open: &mut Option<Stretch>, work: &mut Vec<Stretch>, waiting: &[Turn], next: u64) {
        if let Some(mut s) = open.take() {
            s.to = waiting.first().and_then(|t| t.at).unwrap_or(next);
            work.push(s);
        }
    }
    'walk: while at < len && read < BUDGET {
        let n = ((len - at) as usize).min(CHUNK);
        let buf = read_at(at, n)?;
        if buf.len() != n {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the record changed while it was read"));
        }
        read += n;
        carried.extend_from_slice(&buf);
        at += n as u64;
        let mut line_start = 0usize;
        while let Some(nl) = carried[line_start..].iter().position(|b| *b == b'\n') {
            let end = line_start + nl;
            let line_at = carried_at + line_start as u64;
            let line = &carried[line_start..end];
            let seen = look_at(line);
            match seen {
                Seen::Reaching(calls) => {
                    // What was said since the last speaker was on the way here
                    let began = waiting.first().and_then(|t| t.at).unwrap_or(line_at);
                    let s = open.get_or_insert(Stretch { from: began, to: line_at, calls: 0, hit: false });
                    s.calls += calls;
                    if !needle.is_empty() && !s.hit {
                        s.hit = waiting.iter().any(|t| find_in(&t.text, needle).is_some()) || mention(line, needle).is_some();
                    }
                    waiting.clear();
                }
                Seen::Said(mut turn) => {
                    turn.at = Some(line_at);
                    if turn.who == Who::You {
                        close(&mut open, &mut work, &waiting, line_at);
                        settle(&mut found, &mut waiting);
                        // One block past what was asked for begins here: the
                        // next stretch starts with it
                        if found.len() >= want {
                            stop_at = Some(line_at);
                            break 'walk;
                        }
                        found.push(turn);
                    } else {
                        waiting.push(turn);
                    }
                }
                Seen::Nothing => {
                    // What a tool gave back lies inside the stretch
                    if let Some(s) = open.as_mut()
                        && waiting.is_empty()
                        && !needle.is_empty()
                        && !s.hit
                    {
                        s.hit = mention(line, needle).is_some();
                    }
                }
            }
            line_start = end + 1;
        }
        carried_at += line_start as u64;
        carried.drain(..line_start);
    }
    // The end of what is there: a last line with no newline yet is still
    // being written, and is read the next time
    let end = stop_at.unwrap_or(carried_at);
    if stop_at.is_none() {
        close(&mut open, &mut work, &waiting, end);
        settle(&mut found, &mut waiting);
    }
    // Read to the end of the record as it is now, the last exchange may still
    // be going on -- an answer written a piece at a time, a tool still running.
    // Reading on starts again at it, so it comes back whole (the same place,
    // so the same thing said) rather than as the half already shown and a
    // second half on its own
    let reached_end = stop_at.is_none() && carried_at >= len;
    let to = match reached_end {
        true => found.iter().rev().find(|t| t.who == Who::You).and_then(|t| t.at).unwrap_or(end),
        false => end,
    };
    Ok(Later { turns: found, to, more: !reached_end, work })
}

/// The same walk over a record read a piece at a time by `read_at` (from, how
/// many bytes): a file here, or one on the machine a folder is on, fetched a
/// piece at a time as the walk needs it
pub fn read_back_by(
    len: u64,
    before: u64,
    want: usize,
    read_at: &mut dyn FnMut(u64, usize) -> std::io::Result<Vec<u8>>,
) -> std::io::Result<Page> {
    let mut end = before.min(len);
    let mut from = end;
    // The back half of a line whose start lies further back than we have read.
    // Carried into the next round rather than parsed half-read
    let mut carried: Vec<u8> = Vec::new();
    // Newest first while collecting; turned the right way round at the end
    let mut found: Vec<Turn> = Vec::new();
    let mut read = 0usize;
    // Set when the walk meets the start of one block too many
    let mut enough = false;
    // Whether the record just AFTER the one being looked at was the AI
    // reaching for a tool. The walk runs backwards, so what follows a line has
    // always been read before it -- and words said in that breath are an aside
    // about the work, not an answer. Only somebody speaking clears it; the
    // machinery in between (thinking, a tool's result, the CLI's own notes)
    // leaves it standing, because a tool call can be a line or two further on
    let mut reaching_below = false;
    // The work between the things said, newest first while collecting. The
    // one being walked through ends where the thing said after it begins
    let mut work: Vec<Stretch> = Vec::new();
    let mut open: Option<Stretch> = None;
    let mut next_said = end;

    while end > 0 && read < BUDGET && !enough {
        let start = end.saturating_sub(CHUNK as u64);
        let mut buf = read_at(start, (end - start) as usize)?;
        if buf.len() != (end - start) as usize {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the record changed while it was read"));
        }
        read += buf.len();
        buf.extend_from_slice(&carried);

        // Where each line begins within `buf`
        let mut heads: Vec<usize> = vec![0];
        for (i, b) in buf.iter().enumerate() {
            if *b == b'\n' {
                heads.push(i + 1);
            }
        }
        // The piece before the first newline continues before `start`, so it
        // is only whole once we have reached the head of the file
        let first = usize::from(start > 0);

        for k in (first..heads.len()).rev() {
            let at = heads[k];
            let stop = heads.get(k + 1).map_or(buf.len(), |n| n - 1);
            let line_at = start + at as u64;
            let said = match look_at(&buf[at..stop.max(at)]) {
                Seen::Reaching(calls) => {
                    reaching_below = true;
                    let s = open.get_or_insert(Stretch { from: line_at, to: next_said, calls: 0, hit: false });
                    s.calls += calls;
                    s.from = line_at;
                    None
                }
                // Said on the way to that tool, so it belongs to the work
                Seen::Said(turn) if turn.who == Who::Ai && reaching_below => {
                    if let Some(s) = open.as_mut() {
                        s.from = line_at;
                    }
                    None
                }
                Seen::Said(turn) => {
                    reaching_below = false;
                    Some(turn)
                }
                Seen::Nothing => None,
            };
            // Somebody speaking ends the stretch of work after them
            if said.is_some()
                && let Some(s) = open.take()
            {
                work.push(s);
            }
            // One block past what was asked for: stop WITHOUT taking this line,
            // so the next page begins with it and the block it opens is read
            // whole rather than beheaded
            if let Some(turn) = &said
                && found.len() >= want
                && !found.last().is_some_and(|last| joins(last, turn))
            {
                enough = true;
                break;
            }
            // Moved for every line looked at, not only the ones that said
            // something: the next page must start strictly before whatever this
            // one has already been through, or it hands back the same thing
            from = start + at as u64;
            match (said, found.last_mut()) {
                // Older words from the same speaker join the block already
                // being built — in FRONT of it, because this walk goes backwards
                (Some(turn), Some(last)) if joins(last, &turn) => {
                    last.text = format!("{}\n\n{}", turn.text, last.text);
                    last.at = Some(line_at);
                    last.when = last.when.or(turn.when);
                    next_said = line_at;
                }
                (Some(mut turn), _) => {
                    turn.at = Some(line_at);
                    found.push(turn);
                    next_said = line_at;
                }
                (None, _) => {}
            }
        }

        carried = match heads.get(1) {
            Some(&n) => buf[..n - 1].to_vec(),
            // No newline anywhere in this chunk: all of it belongs to a line
            // that begins even further back
            None => buf,
        };
        end = start;
    }

    work.extend(open);
    found.reverse();
    work.reverse();
    Ok(Page {
        turns: found,
        from,
        more: from > 0,
        work,
    })
}

/// Whether `older`, met walking back, belongs to the block `newer` began.
/// An answer said in pieces is one answer. What a person sent is never
/// joined: two things sent one after the other can have come from two
/// different senders -- the person, then a job -- and are told apart one by
/// one (`convo`)
fn joins(newer: &Turn, older: &Turn) -> bool {
    newer.who == Who::Ai && older.who == Who::Ai
}

/// Where a tab's CLI keeps its record of the conversation: the pattern that
/// finds it and the conversation's id, on this PC or on the machine the tab
/// runs on. Two strings and a machine, no filesystem: finding the file is a
/// walk of a folder here and a round trip there, so it is done when a page is
/// asked for, not when this is made.
#[derive(Clone, Debug)]
pub enum Record {
    Here { glob: String, id: String },
    Far { at: crate::elsewhere::Elsewhere, glob: String, id: String },
}

impl Record {
    /// The record named by `glob` and `id`, on `at` when that is another
    /// machine. `None` when either is unknown
    pub fn named(glob: &str, id: &str, at: Option<crate::elsewhere::Elsewhere>) -> Option<Self> {
        if glob.is_empty() || id.is_empty() {
            return None;
        }
        let (glob, id) = (glob.to_string(), id.to_string());
        Some(match at {
            Some(at) => Record::Far { at, glob, id },
            None => Record::Here { glob, id },
        })
    }

    /// Whether reading it goes over the network, and so off the thread that
    /// serves everything else
    pub fn is_far(&self) -> bool {
        matches!(self, Record::Far { .. })
    }

    /// The last `want` things said before byte `before` (see [`read_back`]).
    /// `None` when the CLI has not written the record yet -- the ordinary
    /// state of a tab nobody has spoken to
    pub fn page(&self, before: u64, want: usize) -> Option<std::io::Result<Page>> {
        match self {
            Record::Here { glob, id } => {
                crate::sessionfind::locate(glob, id).map(|path| read_back(&path, before, want))
            }
            Record::Far { at, glob, id } => {
                // Read over there by the bridge, when the person put one there
                // and its line is up: one request instead of a remote command
                // for every piece. Otherwise as before
                if crate::farlink::is_up(at) {
                    let asked = serde_json::json!({"glob": glob, "id": id, "before": before, "want": want});
                    match crate::farlink::call(at, "read_page", asked) {
                        Ok(serde_json::Value::Null) => return None,
                        Ok(v) => return Some(serde_json::from_value::<Page>(v).map_err(std::io::Error::other)),
                        Err(e) => crate::append_hook_log(&format!("reader: the bridge could not read it ({e}); reading it the long way")),
                    }
                }
                locate_far(at, glob, id).map(|path| read_back_far(at, &path, before, want))
            }
        }
    }

    /// The first `want` things said at or after byte `from` (see
    /// [`read_after`]). `None` when the CLI has not written the record yet
    pub fn after(&self, from: u64, want: usize, needle: &str) -> Option<std::io::Result<Later>> {
        match self {
            Record::Here { glob, id } => {
                crate::sessionfind::locate(glob, id).map(|path| read_after(&path, from, want, needle))
            }
            Record::Far { at, glob, id } => {
                if crate::farlink::is_up(at) {
                    let asked = serde_json::json!({"glob": glob, "id": id, "from": from, "want": want, "q": needle});
                    match crate::farlink::call(at, "read_after", asked) {
                        Ok(serde_json::Value::Null) => return None,
                        Ok(v) => return Some(serde_json::from_value::<Later>(v).map_err(std::io::Error::other)),
                        Err(e) => crate::append_hook_log(&format!("reader: the bridge could not read on ({e}); reading it the long way")),
                    }
                }
                locate_far(at, glob, id).map(|path| {
                    let (len, mut read_at) = far_pieces(at, &path)?;
                    read_after_by(len, from, want, needle, &mut read_at)
                })
            }
        }
    }

    /// One stretch of work opened (see [`work_at`]): the lines between
    /// `from` and `to` of the record, read as the pieces they are. `None`
    /// when the record is not there
    pub fn work(&self, from: u64, to: u64, needle: &str) -> Option<std::io::Result<Work>> {
        let n = to.saturating_sub(from).min(WORK_MOST) as usize;
        match self {
            Record::Here { glob, id } => crate::sessionfind::locate(glob, id).map(|path| {
                let mut file = File::open(path)?;
                let mut buf = vec![0u8; n];
                file.seek(SeekFrom::Start(from))?;
                let got = file.read(&mut buf)?;
                buf.truncate(got);
                Ok(work_at(&buf, 0, buf.len() as u64, needle))
            }),
            Record::Far { at, glob, id } => {
                if crate::farlink::is_up(at) {
                    let asked = serde_json::json!({"glob": glob, "id": id, "from": from, "to": to, "q": needle});
                    match crate::farlink::call(at, "read_work", asked) {
                        Ok(serde_json::Value::Null) => return None,
                        Ok(v) => return Some(serde_json::from_value::<Work>(v).map_err(std::io::Error::other)),
                        Err(e) => crate::append_hook_log(&format!("reader: the bridge could not open the work ({e}); reading it the long way")),
                    }
                }
                locate_far(at, glob, id).map(|path| {
                    let (_, mut read_at) = far_pieces(at, &path)?;
                    let buf = read_at(from, n)?;
                    Ok(work_at(&buf, 0, buf.len() as u64, needle))
                })
            }
        }
    }

    /// The conversation's id
    pub fn id(&self) -> &str {
        match self {
            Record::Here { id, .. } | Record::Far { id, .. } => id,
        }
    }
}

/// A CLI's record of one conversation on another machine: where it is there,
/// found from the profile's pattern (`{home}/.../{id}.jsonl`) with the id the
/// app handed the CLI. Waits on the machine
pub fn locate_far(at: &crate::elsewhere::Elsewhere, glob: &str, id: &str) -> Option<String> {
    // Only what a glob and an id are made of: both go into a shell there
    let safe = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || "/*._-".contains(c));
    if !safe(id) {
        return None;
    }
    let rest = crate::sessionfind::fill_id(glob.strip_prefix("{home}/")?, id);
    if !safe(&rest) {
        return None;
    }
    let ran = crate::elsewhere::exec(at, &format!("cd \"$HOME\" && ls -1d {rest} 2>/dev/null | head -n 1 | sed \"s#^#$HOME/#\""), 30_000).ok()?;
    let path = ran.out.lines().next()?.trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// `read_back` for a record on another machine, fetched a piece at a time
pub fn read_back_far(at: &crate::elsewhere::Elsewhere, path: &str, before: u64, want: usize) -> std::io::Result<Page> {
    let (len, mut read_at) = far_pieces(at, path)?;
    read_back_by(len, before, want, &mut read_at)
}

/// How long a record on another machine is, and a way to fetch a piece of it
/// (from, how many bytes) -- one remote command a piece
fn far_pieces<'a>(at: &'a crate::elsewhere::Elsewhere, path: &str) -> std::io::Result<(u64, Pieces<'a>)> {
    use base64::Engine as _;
    let err = |e: anyhow::Error| std::io::Error::other(format!("{e:#}"));
    let quoted = crate::worktree::for_a_shell(&[path.to_string()]);
    let len: u64 = crate::elsewhere::exec(at, &format!("stat -c %s -- {quoted}"), 30_000)
        .map_err(err)?
        .out
        .trim()
        .parse()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "no such record"))?;
    Ok((len, Box::new(move |start: u64, n: usize| {
        let ran = crate::elsewhere::exec(
            at,
            &format!("tail -c +{} -- {quoted} | head -c {n} | base64 -w0", start + 1),
            60_000,
        )
        .map_err(err)?;
        base64::engine::general_purpose::STANDARD
            .decode(ran.out.trim())
            .map_err(|e| std::io::Error::other(e.to_string()))
    })))
}

/// A way to fetch a piece of a record (from, how many bytes)
type Pieces<'a> = Box<dyn FnMut(u64, usize) -> std::io::Result<Vec<u8>> + 'a>;

/// What one record turns out to be.
enum Seen {
    /// Somebody speaking, and what they said
    Said(Turn),
    /// The AI reaching for tools (how many). Nothing was said here, but
    /// whatever was said just before it was said on the way to this
    Reaching(usize),
    /// Machinery, or a record this reader has no use for
    Nothing,
}

/// What one record is: somebody speaking, a tool being reached for, or
/// neither.
fn look_at(line: &[u8]) -> Seen {
    // A cheap gate ahead of the JSON parser. Most of these lines are tool
    // results, some of them megabytes each, and parsing every one of them only
    // to find out it is not a message is where the whole cost of a read would go.
    //
    // Three spellings have to survive it, and forgetting the second cost this
    // module every human word in the file: an answer arrives as blocks
    // (`"content":[{"type":"text"…`), but what a PERSON typed is filed as a bare
    // string (`"content":"…`), which carries no `text` key at all. The third is
    // a tool call, which one CLI files with a role and the other without
    let Ok(text) = std::str::from_utf8(line) else {
        return Seen::Nothing;
    };
    let named = text.contains("\"role\"") || SPEAKERS.iter().any(|(s, _)| text.contains(&format!("\"type\":\"{s}\"")));
    let spoken = named && (text.contains("\"text\"") || text.contains("\"content\":\""));
    if !spoken && !text.contains("_use\"") && !text.contains("_call\"") && !text.contains("\"toolCalls\"") {
        return Seen::Nothing;
    }
    let Ok(record) = serde_json::from_str::<Value>(text) else {
        return Seen::Nothing;
    };
    // A side conversation — a sub-agent's own turns, written into the same
    // file. It is a different conversation that happens to share a log, and
    // splicing it in would read as the AI interrupting itself
    if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return Seen::Nothing;
    }
    if reaching_for_a_tool(&record) {
        let calls = calls_of(&record).iter().filter(|p| p.kind == PieceKind::Call).count();
        return Seen::Reaching(calls.max(1));
    }
    match turn_of(&record) {
        Some(turn) => Seen::Said(turn),
        None => Seen::Nothing,
    }
}

/// One record, if it is somebody speaking.
fn turn_of(record: &Value) -> Option<Turn> {
    let message = message_of(record)?;
    let named = message.get("role").or_else(|| message.get("type")).and_then(Value::as_str)?;
    // "developer", "system", "tool", "info", "error" — written by machinery,
    // for machinery
    let who = SPEAKERS.iter().find(|(s, _)| *s == named)?.1;
    let said = words_of(message.get("content")?);
    let said = match who {
        Who::You => human_part(&said),
        Who::Ai => said.trim().to_string(),
    };
    (!said.is_empty()).then_some(Turn { who, text: said, at: None, when: when_of(record) })
}

/// When a record says it was written: a `timestamp` beside the message, as
/// every CLI that keeps one spells it -- an ISO 8601 time, or a number of
/// milliseconds
fn when_of(record: &Value) -> Option<i64> {
    let stamp = record
        .get("timestamp")
        .or_else(|| ["message", "payload"].iter().find_map(|k| record.get(*k)?.get("timestamp")))?;
    match stamp {
        Value::String(s) => crate::limits::epoch_ms_of(s),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// Whether this record is the AI reaching for a tool.
///
/// Told by the shape of the name, like everything else here. Both CLIs spell
/// it the same two ways and have kept on spelling it that way through their
/// renamings: a call ENDS in `_use` (`tool_use`, `server_tool_use`,
/// `mcp_tool_use`) or in `_call` (`function_call`, `local_shell_call`,
/// `custom_tool_call`). Named in a block of the message, or -- where the
/// record is the call and carries no message at all -- on the record itself
fn reaching_for_a_tool(record: &Value) -> bool {
    let named = |v: &Value| {
        v.get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t.ends_with("_use") || t.ends_with("_call"))
    };
    let here = |v: &Value| {
        named(v) || v.get("content").and_then(Value::as_array).is_some_and(|bs| bs.iter().any(named))
    };
    // A record that files its calls beside what it said, in a list of their own
    let listed = record
        .get("toolCalls")
        .and_then(Value::as_array)
        .is_some_and(|calls| !calls.is_empty());
    listed || here(record) || ["message", "payload"].iter().any(|k| record.get(*k).is_some_and(here))
}

/// The names a speaker goes by, where a record says who is speaking: `role`
/// for Claude and Codex, the record's own `type` for Gemini
const SPEAKERS: [(&str, Who); 4] = [
    ("assistant", Who::Ai),
    ("gemini", Who::Ai),
    ("model", Who::Ai),
    ("user", Who::You),
];

/// The object carrying `role`: the record itself, or the one field it is
/// wrapped in. With no role anywhere, the record itself when its `type` names
/// a speaker.
fn message_of(record: &Value) -> Option<&Value> {
    if record.get("role").is_some() {
        return Some(record);
    }
    ["message", "payload"]
        .iter()
        .find_map(|key| record.get(*key).filter(|m| m.get("role").is_some()))
        .or_else(|| {
            let kind = record.get("type").and_then(Value::as_str)?;
            (record.get("content").is_some() && SPEAKERS.iter().any(|(s, _)| *s == kind)).then_some(record)
        })
}

/// The words out of a `content`: a bare string, or every block whose type ends
/// in "text" -- or that has no type and is not a thought. Blocks are joined with a blank line because that is what they
/// are — separate paragraphs of one answer, split by the tool calls between them
fn words_of(content: &Value) -> String {
    if let Some(one) = content.as_str() {
        return one.to_string();
    }
    let Some(blocks) = content.as_array() else {
        return String::new();
    };
    let mut said: Vec<&str> = Vec::new();
    for block in blocks {
        let words = match block.get("type").and_then(Value::as_str) {
            Some(kind) => kind.ends_with("text"),
            None => block.get("thought").and_then(Value::as_bool) != Some(true),
        };
        if !words {
            continue;
        }
        if let Some(words) = block.get("text").and_then(Value::as_str) {
            let words = words.trim();
            if !words.is_empty() {
                said.push(words);
            }
        }
    }
    said.join("\n\n")
}

/// What a person actually typed, out of a user message.
///
/// Both CLIs hand machine-written material over as though the person had typed
/// it: the reminders Claude injects into a turn, the environment and
/// instruction blocks Codex puts in front of one. Read back as a conversation
/// those are noise nobody wrote and nobody can answer.
///
/// They are told apart by the SHAPE of the tag rather than by a list of names:
/// every envelope of this kind is spelled with a hyphen or an underscore
/// (`system-reminder`, `user_instructions`, `environment_context`), and no HTML
/// tag a person might paste into a message is. A list of names would have to be
/// kept in step with two CLIs' releases; the shape does not.
///
/// An envelope that is never closed is left alone. Cutting to the end of the
/// message on the strength of one opening tag would swallow the very words
/// this is trying to rescue.
pub fn human_part(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        let after = &rest[at + 1..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let envelope = name.starts_with(|c: char| c.is_ascii_alphabetic())
            && (name.contains('-') || name.contains('_'));
        let closing = format!("</{name}>");
        match envelope.then(|| after.find(&closing)).flatten() {
            Some(shut) => {
                kept.push_str(&rest[..at]);
                rest = &after[shut + closing.len()..];
            }
            None => {
                kept.push_str(&rest[..at + 1]);
                rest = after;
            }
        }
    }
    kept.push_str(rest);
    kept.trim().to_string()
}

// -- The work, and what a search finds ----------------------------------------
//
// Everything the AI did between two things said -- the tools it reached for,
// what they gave back, what it said on the way -- is where nearly all of a
// record's bytes are (a 70 MB record held 325 KB of words, measured). It is
// read as pieces when somebody opens it (`work_at`), and it is where a search
// looks as well as in what was said (`mention`).

/// How many characters of one piece of the work are shown. Past that it is a
/// command's output or a file's contents, and the reader says how much was
/// left out rather than handing over megabytes nobody asked for
const PIECE_CAP: usize = 4_000;

/// How many pieces of one stretch of work are handed over at once. A long
/// job reaches for hundreds of tools between two things said
const PIECES_CAP: usize = 200;

/// What a piece of the work is: something the AI said on the way, a tool it
/// reached for, or what came back from one
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PieceKind {
    Say,
    Call,
    Out,
}

/// One piece of the work, as text. Nothing here knows any tool: what a call
/// asked for and what a result held are read the same way, as the words in
/// them (`leaves`)
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Piece {
    pub kind: PieceKind,
    /// The tool's name, for a call
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub text: String,
    /// Characters left out in front of what is shown, and after it
    #[serde(default, skip_serializing_if = "is_none_left")]
    pub before: usize,
    #[serde(default, skip_serializing_if = "is_none_left")]
    pub after: usize,
    /// Whether it holds what was searched for
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hit: bool,
    /// How many pieces were left out just before this one (see `keep`)
    #[serde(default, skip_serializing_if = "is_none_left")]
    pub skipped: usize,
}

fn is_none_left(n: &usize) -> bool {
    *n == 0
}

/// A stretch of work, opened: its pieces in the order they happened, and how
/// many more there were than are handed over
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Work {
    pub pieces: Vec<Piece>,
    #[serde(default, skip_serializing_if = "is_none_left")]
    pub more: usize,
}

/// What one line of a record is, read forwards
enum Line {
    Said(Turn),
    /// The AI reaching for tools, with what it said in the same breath
    Reaching(Vec<Piece>),
    /// What came back from a tool
    Out(Vec<Piece>),
    Nothing,
}

/// One line of a record, read for the whole conversation. The same reading
/// as `look_at`, and one thing more: what came back from a tool, which the
/// walk from the end has no use for and the whole conversation shows
fn read_line(line: &[u8]) -> Line {
    let Ok(text) = std::str::from_utf8(line) else {
        return Line::Nothing;
    };
    let named = text.contains("\"role\"") || SPEAKERS.iter().any(|(s, _)| text.contains(&format!("\"type\":\"{s}\"")));
    let spoken = named && (text.contains("\"text\"") || text.contains("\"content\":\""));
    let tooling = text.contains("_use\"")
        || text.contains("_call\"")
        || text.contains("\"toolCalls\"")
        || text.contains("_result\"")
        || text.contains("_output\"");
    if !spoken && !tooling {
        return Line::Nothing;
    }
    let Ok(record) = serde_json::from_str::<Value>(text) else {
        return Line::Nothing;
    };
    if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return Line::Nothing;
    }
    if reaching_for_a_tool(&record) {
        return Line::Reaching(calls_of(&record));
    }
    let out = outputs_of(&record);
    if !out.is_empty() {
        return Line::Out(out);
    }
    match turn_of(&record) {
        Some(turn) => Line::Said(turn),
        None => Line::Nothing,
    }
}

/// The places a record keeps what it is: the record itself, and the one
/// field a message is wrapped in
fn layers(record: &Value) -> impl Iterator<Item = &Value> {
    std::iter::once(record).chain(["message", "payload"].into_iter().filter_map(|k| record.get(k)))
}

/// Whether a block's type names it as what its name ends in
fn typed(v: &Value, ends: &[&str]) -> bool {
    v.get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| ends.iter().any(|e| t.ends_with(e)))
}

/// The tools a record reaches for, each as a piece, after whatever was said
/// with them. Found where `reaching_for_a_tool` finds them
fn calls_of(record: &Value) -> Vec<Piece> {
    let mut out = Vec::new();
    if let Some(message) = message_of(record)
        && let Some(content) = message.get("content")
    {
        let said = words_of(content);
        if !said.trim().is_empty() {
            out.push(piece(PieceKind::Say, String::new(), said.trim().to_string()));
        }
    }
    let call = |v: &Value| {
        let name = v.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
        piece(PieceKind::Call, name, leaves(v))
    };
    for layer in layers(record) {
        if typed(layer, &["_use", "_call"]) {
            out.push(call(layer));
        }
        for block in layer.get("content").and_then(Value::as_array).into_iter().flatten() {
            if typed(block, &["_use", "_call"]) {
                out.push(call(block));
            }
        }
    }
    for c in record.get("toolCalls").and_then(Value::as_array).into_iter().flatten() {
        out.push(call(c));
    }
    out
}

/// What came back from tools in a record, each as a piece. Told by the shape
/// of the name, as a call is: a result's type ends in `_result`
/// (`tool_result`) or `_output` (`function_call_output`)
fn outputs_of(record: &Value) -> Vec<Piece> {
    let mut out = Vec::new();
    for layer in layers(record) {
        if typed(layer, &["_result", "_output"]) {
            out.push(piece(PieceKind::Out, String::new(), leaves(layer)));
        }
        for block in layer.get("content").and_then(Value::as_array).into_iter().flatten() {
            if typed(block, &["_result", "_output"]) {
                out.push(piece(PieceKind::Out, String::new(), leaves(block)));
            }
        }
    }
    out
}

fn piece(kind: PieceKind, name: String, text: String) -> Piece {
    Piece { kind, name, text, before: 0, after: 0, hit: false, skipped: 0 }
}

/// The pieces of a stretch of work that are handed over, when there are more
/// than `PIECES_CAP`: every one that holds what was searched for, and the
/// rest of the room given to the first ones, in the order they happened. What
/// is left out between two that are kept is counted on the one after it, and
/// what is left out after the last in `more`: a piece that holds the words
/// must never be the one left out, or the page it opens says the words are
/// not there
fn keep(pieces: Vec<Piece>) -> Work {
    let total = pieces.len();
    if total <= PIECES_CAP {
        return Work { pieces, more: 0 };
    }
    let hits = pieces.iter().filter(|p| p.hit).count().min(PIECES_CAP);
    let mut room = PIECES_CAP - hits;
    let mut hits_left = hits;
    let mut kept: Vec<Piece> = Vec::with_capacity(PIECES_CAP);
    let mut gap = 0;
    for mut p in pieces {
        let take = match p.hit {
            true if hits_left > 0 => {
                hits_left -= 1;
                true
            }
            true => false,
            false if room > 0 => {
                room -= 1;
                true
            }
            false => false,
        };
        if !take {
            gap += 1;
            continue;
        }
        p.skipped = gap;
        gap = 0;
        kept.push(p);
    }
    Work { pieces: kept, more: gap }
}

/// The words in a value: every string in it, in order, one to a line.
///
/// Format-blind, like the rest of this module. What is left out is what no
/// reader reads -- a block's own `type`, and the ids that tie a call to its
/// result (any key ending in "id"). A picture a tool handed back is left out
/// whole: its bytes are text, but not words. An argument list a CLI files as a
/// JSON string is read as the JSON it is
fn leaves(v: &Value) -> String {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                let t = s.trim();
                if t.is_empty() {
                    return;
                }
                if t.starts_with('{')
                    && let Ok(inner @ Value::Object(_)) = serde_json::from_str::<Value>(t)
                {
                    walk(&inner, out);
                    return;
                }
                out.push(t.to_string());
            }
            Value::Array(list) => list.iter().for_each(|x| walk(x, out)),
            Value::Object(map) => {
                if map.contains_key("media_type") || map.get("type").and_then(Value::as_str) == Some("image") {
                    return;
                }
                for (k, x) in map {
                    let lower = k.to_ascii_lowercase();
                    if lower == "type" || lower == "name" || lower.ends_with("id") || lower == "signature" {
                        continue;
                    }
                    walk(x, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, &mut out);
    out.join("\n")
}

/// Where `needle` (already lowercase) first is in `text`, as a byte offset
/// into `text` itself. Lowercasing can change how long a character is, so the
/// search runs on a lowercased copy and the answer is carried back
pub fn find_in(text: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let mut low = String::with_capacity(text.len());
    let mut back: Vec<usize> = Vec::with_capacity(text.len());
    for (i, c) in text.char_indices() {
        for l in c.to_lowercase() {
            let before = low.len();
            low.push(l);
            back.extend(std::iter::repeat_n(i, low.len() - before));
        }
    }
    low.find(needle).map(|at| back[at])
}

/// A piece cut down to what is shown: its start, or the stretch around what was
/// searched for when that lies further in
fn clip(mut p: Piece, needle: &str) -> Piece {
    let at = find_in(&p.text, needle);
    p.hit = at.is_some();
    let count = p.text.chars().count();
    if count <= PIECE_CAP {
        return p;
    }
    // From the start, unless what was searched for lies past what the start
    // shows: then a quarter of the window in front of it, so it is read in
    // its sentence rather than at the top edge
    let hit_char = at.map(|b| p.text[..b].chars().count()).unwrap_or(0);
    let start = match hit_char < PIECE_CAP {
        true => 0,
        false => (hit_char - PIECE_CAP / 4).min(count - PIECE_CAP),
    };
    let shown: String = p.text.chars().skip(start).take(PIECE_CAP).collect();
    p.before = start;
    p.after = count - start - shown.chars().count();
    p.text = shown;
    p
}

/// Where each line of `bytes` starts and ends, the newline left out
fn lines_of(bytes: &[u8]) -> impl Iterator<Item = (usize, usize)> + '_ {
    let mut at = 0;
    std::iter::from_fn(move || {
        if at >= bytes.len() {
            return None;
        }
        let start = at;
        let end = bytes[at..].iter().position(|b| *b == b'\n').map_or(bytes.len(), |n| at + n);
        at = end + 1;
        Some((start, end))
    })
}

/// One stretch of work, opened: the lines between `from` and `to` of a record,
/// as the pages that counted it say where it lies
pub fn work_at(bytes: &[u8], from: u64, to: u64, needle: &str) -> Work {
    let from = (from as usize).min(bytes.len());
    let to = (to as usize).clamp(from, bytes.len());
    let needle = needle.trim().to_lowercase();
    let mut pieces = Vec::new();
    for (start, end) in lines_of(&bytes[from..to]) {
        match read_line(&bytes[from + start..from + end]) {
            Line::Said(t) if t.who == Who::Ai => pieces.push(piece(PieceKind::Say, String::new(), t.text)),
            Line::Reaching(ps) | Line::Out(ps) => pieces.extend(ps),
            _ => {}
        }
    }
    keep(pieces.into_iter().map(|p| clip(p, &needle)).collect())
}

/// The words of one line of a record that a reader would see: what was said,
/// or what a tool was asked and gave back. What a search is matched against,
/// so a word that only appears in a record's bookkeeping -- the folder it ran
/// in, the branch, an id -- does not make a conversation a match
fn words_seen(line: &[u8]) -> Vec<(String, SaidBy)> {
    match read_line(line) {
        Line::Said(t) => vec![(t.text, Some((t.who, t.when)))],
        Line::Reaching(ps) | Line::Out(ps) => ps.into_iter().map(|p| (p.text, None)).collect(),
        Line::Nothing => Vec::new(),
    }
}

/// Who said some words and when (milliseconds, when the record says), for
/// words that were said; `None` for a tool's
pub type SaidBy = Option<(Who, Option<i64>)>;

/// Where a search found what it looked for in a record
#[derive(Debug, Clone, PartialEq)]
pub struct Mention {
    /// The words it was found in, as a reader sees them
    pub words: String,
    /// Where in those words, as a byte offset
    pub at: usize,
    /// Where the record's line holding them starts, as a byte offset into
    /// what was searched: where a reader opened on it begins
    pub line: u64,
    /// Who said them and when, when they were something said rather than a
    /// tool's words
    pub said: SaidBy,
}

/// `needle`, lowercase, found in `bytes` where a reader would see it: the
/// words it was found in, where in them, and where its line starts. `None` when it is nowhere but in
/// the record's bookkeeping.
///
/// Found first as bytes, which is what makes reading every record on a
/// machine affordable: only the lines the bytes turn up are parsed. JSON keeps
/// a quote or a backslash escaped, so the bytes looked for are the needle as
/// JSON writes it. A needle with letters whose case lies outside ASCII cannot
/// be found that way -- the bytes of "É" are not the bytes of "é" -- and is
/// looked for line by line instead
pub fn mention(bytes: &[u8], needle: &str) -> Option<Mention> {
    if needle.is_empty() {
        return None;
    }
    let seen = |start: usize, end: usize| {
        words_seen(&bytes[start..end])
            .into_iter()
            .find_map(|(w, said)| find_in(&w, needle).map(|at| Mention { words: w, at, line: start as u64, said }))
    };
    let caseful = needle.chars().any(|c| !c.is_ascii() && c.to_uppercase().ne(c.to_lowercase()));
    if caseful {
        return lines_of(bytes).find_map(|(s, e)| seen(s, e));
    }
    let raw = serde_json::to_string(needle).unwrap_or_default();
    let raw = raw.get(1..raw.len().saturating_sub(1)).unwrap_or(needle).as_bytes();
    let mut from = 0;
    while let Some(at) = find_ascii_blind(bytes, raw, from) {
        let start = bytes[..at].iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1);
        let end = bytes[at..].iter().position(|b| *b == b'\n').map_or(bytes.len(), |n| at + n);
        if let Some(found) = seen(start, end) {
            return Some(found);
        }
        from = end;
    }
    None
}

/// Where `needle` first is in `hay` at or after `from`, ASCII letters matched
/// whatever their case. `needle` is lowercase
pub fn find_ascii_blind(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let n = needle.len();
    if n == 0 || hay.len() < n {
        return None;
    }
    let first = needle[0];
    let upper = first.to_ascii_uppercase();
    let last = hay.len() - n;
    let mut i = from;
    while i <= last {
        let rel = hay[i..=last].iter().position(|b| *b == first || *b == upper)?;
        i += rel;
        if hay[i..i + n].iter().zip(needle).all(|(a, b)| a.to_ascii_lowercase() == *b) {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("shikisha-reader");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{name}.jsonl"));
        let _ = std::fs::remove_file(&file);
        file
    }

    /// What the walk would take from one record, and nothing else
    fn said(line: &str) -> Option<Turn> {
        match look_at(line.as_bytes()) {
            Seen::Said(turn) => Some(turn),
            _ => None,
        }
    }

    /// Claude's shape: the message is under `message`, the words under blocks
    /// of type "text"
    #[test]
    fn claude_records_are_read() {
        let line = r#"{"type":"assistant","isSidechain":false,"message":{"role":"assistant","content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"直しました"}]}}"#;
        let turn = said(line).expect("the assistant's turn");
        assert_eq!(turn.who, Who::Ai);
        assert_eq!(turn.text, "直しました");
    }

    /// Codex's shape: the message is under `payload`, the words under blocks
    /// of type "output_text". Nothing about the reader knows which is which
    #[test]
    fn codex_records_are_read() {
        let line = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"原因が確定しました"}]}}"#;
        let turn = said(line).expect("the assistant's turn");
        assert_eq!(turn.who, Who::Ai);
        assert_eq!(turn.text, "原因が確定しました");
    }

    /// Gemini's shape: no role anywhere, the speaker in the record's own
    /// `type`, the person's words in parts with no type, the answer a bare
    /// string. What it notes for itself (`info`) and its thoughts are not speech
    #[test]
    fn gemini_records_are_read() {
        let asked = r#"{"id":"da60","timestamp":"2026-09-28T02:12:30.055Z","type":"user","content":[{"text":"ping"}]}"#;
        let turn = said(asked).expect("the person's turn");
        assert_eq!((turn.who, turn.text.as_str()), (Who::You, "ping"));

        let answered = r#"{"id":"7810","type":"gemini","content":"pong","thoughts":[{"subject":"s","description":"d"}],"model":"gemini-3.5-flash"}"#;
        let turn = said(answered).expect("the AI's turn");
        assert_eq!((turn.who, turn.text.as_str()), (Who::Ai, "pong"));

        let parts = r#"{"type":"gemini","content":[{"text":"thinking it over","thought":true},{"text":"done"}]}"#;
        assert_eq!(said(parts).expect("the AI's turn").text, "done");

        let note = r#"{"id":"c693","type":"info","content":"Update successful!"}"#;
        assert!(said(note).is_none(), "the CLI's own note is not a turn");
        // The whole history written again in one line is not one thing said
        let rewrite = r#"{"$set":{"messages":[{"type":"user","content":[{"text":"<session_context>…</session_context>"}]}]}}"#;
        assert!(said(rewrite).is_none());
    }

    /// Gemini files the tools it called beside what it said with them, so the
    /// line said in that breath is an aside about the work, as with the others
    #[test]
    fn a_gemini_turn_that_calls_tools_is_reaching() {
        let line = r#"{"type":"gemini","content":"Let me look.","toolCalls":[{"id":"1","name":"read_file","args":{}}]}"#;
        assert!(matches!(look_at(line.as_bytes()), Seen::Reaching(_)));
        let quiet = r#"{"type":"gemini","content":"pong","toolCalls":[]}"#;
        assert!(matches!(look_at(quiet.as_bytes()), Seen::Said(_)));
    }

    /// What a person typed is filed differently from what the AI answered: a
    /// bare string where the answer has blocks. Missing this shape is not a
    /// cosmetic loss — it is every human word in the file
    #[test]
    fn a_person_types_a_plain_string() {
        let line = r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":"バグを発見しました"}}"#;
        let turn = said(line).expect("the person's turn");
        assert_eq!(turn.who, Who::You);
        assert_eq!(turn.text, "バグを発見しました");
    }

    #[test]
    fn machinery_is_not_speech() {
        // A tool result rides in a user record; it has a role, but nobody said it
        let result = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","text":"ok"}]}}"#;
        assert!(said(result).is_none(), "a tool result is not a turn");
        // A sub-agent's transcript shares the file
        let side = r#"{"isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"別の会話"}]}}"#;
        assert!(said(side).is_none(), "a subagent's record is a different conversation");
        // Neither is anything without a role
        let meta = r#"{"type":"summary","text":"…"}"#;
        assert!(said(meta).is_none());
    }

    /// The line an AI writes on its way to a tool is filed exactly like an
    /// answer. Told apart by what follows it, not by what it says -- a rule
    /// about wording would be a rule about one model's habits
    #[test]
    fn the_line_said_on_the_way_to_a_tool_is_not_an_answer() {
        let path = tmp("asides");
        let call = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{\"file_path\":\"a\"}}]}}";
        let result = "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"content\":\"ok\"}]}}";
        let think = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"hm\"}]}}";
        let mut lines = String::new();
        lines.push_str(&record("user", "直してください"));
        lines.push('\n');
        for i in 0..3 {
            lines.push_str(&record("assistant", &format!("Now the {i} block:")));
            lines.push('\n');
            // The call can be a record or two further on, with the model's own
            // thinking in between, and the aside is still an aside
            lines.push_str(think);
            lines.push('\n');
            lines.push_str(call);
            lines.push('\n');
            lines.push_str(result);
            lines.push('\n');
        }
        lines.push_str(&record("assistant", "直しました"));
        lines.push('\n');
        std::fs::write(&path, &lines).unwrap();

        let page = read_back(&path, u64::MAX, 2).unwrap();
        assert_eq!(page.turns.len(), 2, "the last exchange: {:?}", page.turns);
        assert_eq!(page.turns[0].text, "直してください");
        assert_eq!(
            page.turns[1].text, "直しました",
            "the lines said on the way to each tool were read back as the answer"
        );
    }

    /// A tool call is told by the shape of its name, which is the one thing
    /// the two CLIs spell the same way through their renamings
    #[test]
    fn a_tool_call_is_told_by_the_shape_of_its_name() {
        let call = |line: &str| {
            matches!(look_at(line.as_bytes()), Seen::Reaching(_))
        };
        assert!(call(r#"{"message":{"role":"assistant","content":[{"type":"tool_use","name":"Read"}]}}"#));
        assert!(call(r#"{"message":{"role":"assistant","content":[{"type":"mcp_tool_use","name":"x"}]}}"#));
        // Codex files the call as a record of its own, with no role anywhere
        assert!(call(r#"{"type":"response_item","payload":{"type":"function_call","name":"shell"}}"#));
        assert!(call(r#"{"type":"response_item","payload":{"type":"local_shell_call","action":{}}}"#));
        // What comes back from one is not a call, and neither is an answer
        assert!(!call(r#"{"message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]}}"#));
        assert!(!call(r#"{"message":{"role":"assistant","content":[{"type":"text","text":"直しました"}]}}"#));
    }

    #[test]
    fn envelopes_are_peeled_off_what_a_person_typed() {
        let line = r#"{"message":{"role":"user","content":[{"type":"text","text":"<system-reminder>machine</system-reminder>直して<user_instructions>rules</user_instructions>"}]}}"#;
        let turn = said(line).expect("the person's turn");
        assert_eq!(turn.who, Who::You);
        assert_eq!(turn.text, "直して", "an envelope the machine inserted is not the person's words");
    }

    /// The tag shape is the rule, so pasted HTML survives — it has no hyphen
    #[test]
    fn pasted_markup_is_left_alone() {
        assert_eq!(human_part("<div>hello</div>"), "<div>hello</div>");
        // ...and an envelope that never closes is not an excuse to cut
        assert_eq!(human_part("<system-reminder>ここから先"), "<system-reminder>ここから先");
    }

    fn record(who: &str, text: &str) -> String {
        format!(
            "{{\"message\":{{\"role\":\"{who}\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}"
        )
    }

    /// A turn's answer is only ever what was written after the turn began. The
    /// record catches up a moment after the screen, and in that moment the
    /// last answer in it is the previous turn's
    #[test]
    fn a_turn_s_answer_is_only_what_came_after_it_began() {
        let path = tmp("after");
        let before = format!("{}\n{}\n", record("user", "first"), record("assistant", "old answer"));
        std::fs::write(&path, &before).unwrap();
        let began = before.len() as u64;
        assert_eq!(said_after(&path, began), None, "nothing said yet this turn");

        let asked = format!("{before}{}\n", record("user", "second"));
        std::fs::write(&path, &asked).unwrap();
        assert_eq!(said_after(&path, began), None, "asked, not yet answered");

        let tool = r#"{"message":{"role":"assistant","content":[{"type":"text","text":"Let me look."},{"type":"tool_use","name":"Read"}]}}"#;
        let answered = format!("{asked}{tool}\n{}\n", record("assistant", "new answer"));
        std::fs::write(&path, &answered).unwrap();
        assert_eq!(said_after(&path, began).as_deref(), Some("new answer"));

        // Written again from its start: all of it is new
        std::fs::write(&path, format!("{}\n", record("assistant", "fresh"))).unwrap();
        assert_eq!(said_after(&path, began).as_deref(), Some("fresh"));
    }

    /// What `want` counts. A CLI writes one answer as a run of records with the
    /// tool traffic threaded through it — and Claude files a tool's result as a
    /// message from the USER, so counting records would call that traffic the
    /// exchange and never reach the person's own words.
    #[test]
    fn asking_for_the_last_exchange_reaches_past_the_tool_traffic() {
        let path = tmp("exchange");
        let mut lines = String::new();
        lines.push_str(&record("user", "直してください"));
        lines.push('\n');
        for i in 0..30 {
            lines.push_str(&record("assistant", &format!("段落{i}")));
            lines.push('\n');
            // A tool result: a user record carrying no words at all
            lines.push_str(
                "{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"text\":\"ok\"}]}}",
            );
            lines.push('\n');
        }
        std::fs::write(&path, &lines).unwrap();

        let page = read_back(&path, u64::MAX, 2).unwrap();
        assert_eq!(page.turns.len(), 2, "the last exchange = the person's turn + its reply");
        assert_eq!(page.turns[0].who, Who::You);
        assert_eq!(page.turns[0].text, "直してください", "it goes back as far as the person's words");
        assert_eq!(page.turns[1].who, Who::Ai);
        // The answer arrives whole, head included -- the failure this exists to end
        assert!(page.turns[1].text.starts_with("段落0"), "the start of the reply is not cut off");
        assert!(page.turns[1].text.ends_with("段落29"));
        assert!(!page.more, "it has read all the way to the start");
    }

    /// A block is handed back only once its head has been read. Asking for one
    /// block, out of a conversation that has more, must not return half of one
    #[test]
    fn a_block_is_never_handed_back_beheaded() {
        let path = tmp("whole");
        let mut lines = String::new();
        lines.push_str(&record("user", "古い質問"));
        lines.push('\n');
        for i in 0..5 {
            lines.push_str(&record("assistant", &format!("段落{i}")));
            lines.push('\n');
        }
        std::fs::write(&path, &lines).unwrap();

        let page = read_back(&path, u64::MAX, 1).unwrap();
        assert_eq!(page.turns.len(), 1);
        assert_eq!(page.turns[0].text, "段落0\n\n段落1\n\n段落2\n\n段落3\n\n段落4");
        assert!(page.more, "earlier turns are still left");
        // ...and the next page begins exactly where this one stopped
        let older = read_back(&path, page.from, 1).unwrap();
        assert_eq!(older.turns[0].text, "古い質問");
        assert!(!older.more);
    }

    #[test]
    fn a_record_is_named_by_its_pattern_and_id_and_read_where_it_is() {
        assert!(Record::named("", "abc", None).is_none(), "no pattern, no record");
        assert!(Record::named("{home}/x/{id}.jsonl", "", None).is_none(), "no conversation, no record");
        assert!(!Record::named("{home}/x/{id}.jsonl", "abc", None).unwrap().is_far(), "a tab here reads here");

        let dir = std::env::temp_dir().join("shikisha-reader").join("named").join("proj");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("c0ffee.jsonl");
        std::fs::write(&file, format!("{}\n{}\n", record("user", "hello"), record("assistant", "hi there"))).unwrap();
        let glob = format!("{}/*/{{id}}.jsonl", dir.parent().unwrap().display());
        let page = Record::named(&glob, "c0ffee", None).unwrap().page(u64::MAX, 6).unwrap().unwrap();
        assert_eq!(page.turns.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(), ["hello", "hi there"]);
        assert!(
            Record::named(&glob, "not-written-yet", None).unwrap().page(u64::MAX, 6).is_none(),
            "a record the CLI has not written yet reads as nothing, not an error"
        );
    }

    #[test]
    fn the_newest_turns_come_back_first_and_the_rest_follow() {
        let path = tmp("paging");
        let mut lines = String::new();
        for i in 0..40 {
            lines.push_str(&record("user", &format!("q{i}")));
            lines.push('\n');
            lines.push_str(&record("assistant", &format!("a{i}")));
            lines.push('\n');
        }
        std::fs::write(&path, &lines).unwrap();

        let last = read_back(&path, u64::MAX, 4).unwrap();
        assert_eq!(last.turns.len(), 4);
        assert_eq!(last.turns[3].text, "a39", "the last turn comes at the end");
        assert_eq!(last.turns[3].who, Who::Ai);
        assert!(last.more, "there is more before");

        // The page before it, asked for by where the last one started
        let older = read_back(&path, last.from, 4).unwrap();
        assert_eq!(older.turns[3].text, "a37");
        assert!(!older.turns.iter().any(|t| t.text == "a38"), "it does not return the same turn twice");

        // Walking back far enough reaches the head, and says so
        let mut at = older.from;
        for _ in 0..40 {
            let page = read_back(&path, at, 4).unwrap();
            at = page.from;
            if !page.more {
                break;
            }
        }
        assert_eq!(at, 0, "it can go back to the start");
    }

    /// Read a real record, named by `SHIKISHA_READ_PROBE`, and print what comes
    /// back. Ignored by default because it needs a machine that has actually
    /// been working: the fixtures above pin the shapes, and this is how you
    /// find out that a CLI has quietly changed one.
    ///
    ///   cargo test reader::tests::probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe() {
        let Ok(path) = std::env::var("SHIKISHA_READ_PROBE") else {
            panic!("pass the path of a record file in SHIKISHA_READ_PROBE");
        };
        let path = PathBuf::from(path);
        let size = std::fs::metadata(&path).unwrap().len();
        let began = std::time::Instant::now();
        let page = read_back(&path, u64::MAX, 6).unwrap();
        println!(
            "{} MB / {:?} for {} turns (more={})",
            size / 1_048_576,
            began.elapsed(),
            page.turns.len(),
            page.more
        );
        for turn in &page.turns {
            let head: String = turn.text.chars().take(90).collect();
            println!("[{:?}] {} …", turn.who, head.replace('\n', " "));
        }
        assert!(!page.turns.is_empty(), "not a single turn can be read from a real record");
    }

    /// A line longer than one read step must not be cut in half by the chunk
    /// boundary it happens to straddle
    #[test]
    fn a_line_bigger_than_a_chunk_survives() {
        let path = tmp("huge");
        let filler = "x".repeat(CHUNK * 2);
        let mut lines = record("assistant", "先頭の発言");
        lines.push('\n');
        lines.push_str(&format!(
            "{{\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"text\":\"{filler}\"}}]}}}}"
        ));
        lines.push('\n');
        lines.push_str(&record("assistant", "最後の発言"));
        lines.push('\n');
        std::fs::write(&path, &lines).unwrap();

        let page = read_back(&path, u64::MAX, 8).unwrap();
        // The two are one turn: the same speaker either side of a tool call
        let said: Vec<&str> = page.turns.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(said, vec!["先頭の発言\n\n最後の発言"], "turns beyond a huge line are kept too");
        assert!(!page.more, "it has read all the way to the start");
    }

    // -- The whole conversation --

    fn call(name: &str, input: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"tool_use","id":"toolu_1","name":"{name}","input":{{"command":"{input}"}}}}]}}}}"#
        )
    }

    fn result(text: &str) -> String {
        format!(
            r#"{{"type":"user","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"{text}"}}]}}}}"#
        )
    }

    fn conversation() -> String {
        [
            record("user", "ログインが落ちます"),
            record("assistant", "Now the logs:"),
            call("Bash", "tail app.log"),
            result("panic at auth.rs:42 token expired"),
            record("assistant", "トークンの期限切れが原因でした。"),
            record("user", "直して"),
            record("assistant", "直しました。"),
        ]
        .join("\n")
    }

    /// The work between a question and its answer opens to what was said on
    /// the way, the call, and what came back
    #[test]
    fn the_work_opens_to_its_pieces() {
        let bytes = conversation();
        let lines: Vec<&str> = bytes.split('\n').collect();
        let from = (lines[0].len() + 1) as u64;
        let to = from + (lines[1].len() + lines[2].len() + lines[3].len() + 2) as u64;
        let work = work_at(bytes.as_bytes(), from, to, "");
        let kinds: Vec<(PieceKind, &str)> = work.pieces.iter().map(|p| (p.kind, p.text.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (PieceKind::Say, "Now the logs:"),
                (PieceKind::Call, "tail app.log"),
                (PieceKind::Out, "panic at auth.rs:42 token expired"),
            ]
        );
        assert_eq!(work.pieces[1].name, "Bash");
        let found = work_at(bytes.as_bytes(), from, to, "TOKEN EXPIRED");
        assert!(found.pieces.iter().any(|p| p.hit && p.kind == PieceKind::Out), "what was looked for is marked");
    }

    /// A tool's output of megabytes is shown as its start -- or the stretch
    /// around what was searched for, when that lies further in -- with how
    /// much was left out on each side
    #[test]
    fn a_long_piece_is_cut_around_what_was_looked_for() {
        let text = format!("{}NEEDLE{}", "a".repeat(10_000), "b".repeat(10_000));
        let p = clip(piece(PieceKind::Out, String::new(), text), "needle");
        assert!(p.hit);
        assert!(p.text.contains("NEEDLE"), "the cut left out what was looked for");
        assert_eq!(p.before + p.text.chars().count() + p.after, 20_006);
        assert!(p.before > 0 && p.after > 0);
        let q = clip(piece(PieceKind::Out, String::new(), "c".repeat(9_000)), "");
        assert_eq!((q.before, q.text.chars().count(), q.after), (0, PIECE_CAP, 9_000 - PIECE_CAP));
    }

    /// Found where a reader would see it, and not in the bookkeeping around it
    #[test]
    fn a_mention_is_in_the_words_not_the_bookkeeping() {
        let line = r#"{"type":"user","cwd":"D:/work/Refund","message":{"role":"user","content":"hello"}}"#;
        assert!(mention(line.as_bytes(), "refund").is_none(), "the folder's name counted as a mention");
        let found = mention(conversation().as_bytes(), "期限切れ").expect("said");
        assert_eq!(found.said.map(|(who, _)| who), Some(Who::Ai), "who said it");
        let tool = mention(conversation().as_bytes(), "auth.rs:42").expect("in a tool's output");
        assert_eq!(tool.said, None, "a tool's words were said by nobody");
        assert_eq!(&found.words[found.at..found.at + "期限切れ".len()], "期限切れ");
        // ...and its line is where the record says it
        let bytes = conversation();
        assert!(bytes[found.line as usize..].starts_with(&record("assistant", "トークンの期限切れが原因でした。")));
        // A quote is escaped in the record, and still found
        let quoted = record("assistant", r#"say \"yes\" now"#);
        assert!(mention(quoted.as_bytes(), "\"yes\"").is_some());
        // A letter whose case lies outside ASCII is found whatever its case
        let accent = record("assistant", "Élan vital");
        assert!(mention(accent.as_bytes(), "élan").is_some());
    }

    /// A stretch of work too long to hand over whole keeps every piece that
    /// holds what was searched for, and says what it left out where it left it
    #[test]
    fn a_long_stretch_of_work_keeps_what_holds_the_words() {
        let mut pieces: Vec<Piece> = (0..500).map(|i| piece(PieceKind::Out, String::new(), format!("line {i}"))).collect();
        pieces[450].hit = true;
        let work = keep(pieces);
        assert_eq!(work.pieces.len(), PIECES_CAP);
        let kept = work.pieces.iter().find(|p| p.hit).expect("the piece with the words is kept");
        assert_eq!(kept.text, "line 450");
        assert_eq!(kept.skipped, 450 - (PIECES_CAP - 1), "what was left out before it is counted on it");
        assert_eq!(work.more, 49);
        let shown = work.pieces.len() + work.pieces.iter().map(|p| p.skipped).sum::<usize>() + work.more;
        assert_eq!(shown, 500, "every piece is either shown or counted");
    }

    /// Two things sent one after the other stay two -- they may have come
    /// from two senders -- while an answer said in pieces is one answer. Each
    /// says where in the record it begins and when it was said
    #[test]
    fn what_was_sent_is_kept_apart_and_says_where_and_when() {
        let path = tmp("apart");
        let at = |who: &str, text: &str, stamp: &str| {
            format!(
                "{{\"timestamp\":\"{stamp}\",\"message\":{{\"role\":\"{who}\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n"
            )
        };
        let lines = [
            at("user", "fix the test", "2026-09-28T02:00:00Z"),
            at("user", "and the docs", "2026-09-28T02:00:05Z"),
            at("assistant", "First part.", "2026-09-28T02:01:00Z"),
            at("assistant", "Second part.", "2026-09-28T02:02:00.250Z"),
        ];
        std::fs::write(&path, lines.concat()).unwrap();
        let page = read_back(&path, u64::MAX, 10).unwrap();
        let said: Vec<(Who, &str)> = page.turns.iter().map(|t| (t.who, t.text.as_str())).collect();
        assert_eq!(
            said,
            vec![(Who::You, "fix the test"), (Who::You, "and the docs"), (Who::Ai, "First part.\n\nSecond part.")]
        );
        let starts: Vec<u64> = page.turns.iter().map(|t| t.at.unwrap()).collect();
        let first = lines[0].len() as u64;
        assert_eq!(starts, vec![0, first, first + lines[1].len() as u64], "an answer begins where its first piece does");
        assert_eq!(page.turns[1].when, crate::limits::epoch_ms_of("2026-09-28T02:00:05Z"));
        assert_eq!(page.turns[2].when, crate::limits::epoch_ms_of("2026-09-28T02:02:00.250Z"), "an answer is as late as its last piece");
        // Asking for one thing gives one person's line, not both
        let last_asked = read_back(&path, page.turns[2].at.unwrap(), 1).unwrap();
        assert_eq!(last_asked.turns.len(), 1);
        assert_eq!(last_asked.turns[0].text, "and the docs");
        let _ = std::fs::remove_file(&path);
    }

    /// Read forwards from a place, a stretch is what read backwards to that
    /// place's end would have shown: the same turns, the asides left out, and
    /// a cursor that picks up exactly where it stopped
    #[test]
    fn reading_on_from_a_place_gives_what_reading_back_would() {
        let path = tmp("onwards");
        let call = "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\",\"input\":{}}]}}";
        let mut lines = String::new();
        for (who, text) in [("user", "one"), ("assistant", "looking:")] {
            lines.push_str(&record(who, text));
            lines.push('\n');
        }
        lines.push_str(call);
        lines.push('\n');
        for (who, text) in [("assistant", "found it"), ("assistant", "and fixed it"), ("user", "two"), ("user", "three"), ("assistant", "done")] {
            lines.push_str(&record(who, text));
            lines.push('\n');
        }
        std::fs::write(&path, &lines).unwrap();
        let back = read_back(&path, u64::MAX, 100).unwrap();
        let first = read_after(&path, 0, 2, "").unwrap();
        let said = |ts: &[Turn]| ts.iter().map(|t| t.text.clone()).collect::<Vec<_>>();
        assert_eq!(said(&first.turns), vec!["one", "found it\n\nand fixed it"]);
        assert!(first.more);
        let rest = read_after(&path, first.to, 100, "").unwrap();
        assert_eq!(said(&rest.turns), vec!["two", "three", "done"]);
        assert!(!rest.more);
        // The last exchange may still be going on: reading on starts at it again
        let again = read_after(&path, rest.to, 100, "").unwrap();
        assert_eq!(said(&again.turns), vec!["three", "done"]);
        assert_eq!(again.turns[0].at, rest.turns[1].at, "the same place, so the same thing said");
        let both: Vec<Turn> = first.turns.into_iter().chain(rest.turns).collect();
        assert_eq!(said(&both), said(&back.turns), "forwards and backwards read the same conversation");
        let places = |ts: &[Turn]| ts.iter().map(|t| t.at).collect::<Vec<_>>();
        assert_eq!(places(&both), places(&back.turns), "and find each thing at the same place");
        let _ = std::fs::remove_file(&path);
    }

    /// The work between two things said is one stretch, found the same way
    /// whichever way the record is read: from the first line said on the way
    /// to a tool to where the answer begins, with the tools it called counted.
    /// Read with a needle, a stretch holding it in what a tool gave back says so
    #[test]
    fn the_work_between_two_things_said_is_one_stretch_either_way() {
        let path = tmp("stretch");
        let call = |name: &str| {
            format!("{{\"type\":\"assistant\",\"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"tool_use\",\"name\":\"{name}\",\"input\":{{}}}}]}}}}\n")
        };
        let out = |text: &str| {
            format!("{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"tool_result\",\"content\":\"{text}\"}}]}}}}\n")
        };
        let lines = [
            format!("{}\n", record("user", "fix it")),
            format!("{}\n", record("assistant", "Looking first:")),
            call("Read"),
            out("fn parse() { panic!() }"),
            call("Edit"),
            out("ok"),
            format!("{}\n", record("assistant", "Fixed.")),
            format!("{}\n", record("user", "thanks")),
        ];
        std::fs::write(&path, lines.concat()).unwrap();
        let start = |i: usize| lines[..i].iter().map(|l| l.len() as u64).sum::<u64>();
        let want = Stretch { from: start(1), to: start(6), calls: 2, hit: false };
        let back = read_back(&path, u64::MAX, 10).unwrap();
        assert_eq!(back.work, vec![want.clone()]);
        let on = read_after(&path, 0, 10, "").unwrap();
        assert_eq!(on.work, vec![want.clone()], "forwards finds the same stretch");
        let found = read_after(&path, 0, 10, "panic!").unwrap();
        assert_eq!(found.work, vec![Stretch { hit: true, ..want.clone() }], "a tool's output holds the words");
        assert!(!read_after(&path, 0, 10, "nowhere").unwrap().work[0].hit);
        let bytes = std::fs::read(&path).unwrap();
        let opened = work_at(&bytes, want.from, want.to, "");
        assert_eq!(opened.pieces.iter().filter(|p| p.kind == PieceKind::Call).count(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_place_in_lowercase_is_carried_back_to_the_text() {
        assert_eq!(find_in("Hello World", "world"), Some(6));
        assert_eq!(find_in("İstanbul x", "x"), Some("İstanbul ".len()));
        assert_eq!(find_in("abc", ""), None);
        assert_eq!(find_ascii_blind(b"xxABCxx", b"abc", 0), Some(2));
        assert_eq!(find_ascii_blind(b"xxABCxx", b"abc", 3), None);
    }
}

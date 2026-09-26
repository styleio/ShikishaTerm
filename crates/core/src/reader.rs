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
//!     single field it is wrapped in (`message` for Claude, `payload` for Codex)
//!   - the words are the blocks whose type ENDS in "text" (`text` for Claude,
//!     `output_text` / `input_text` for Codex). Everything else in a content
//!     list is machinery — a tool call, its result, the model's own thinking —
//!     and machinery is not what a person opens a reader to read
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

use serde::Serialize;
use serde_json::Value;

/// How much of the record to pull in one step. Small enough that a reader
/// asking for the last few turns touches a fraction of a megabyte
const CHUNK: usize = 256 * 1024;

/// The most one request may read before giving up and saying "there is more".
/// A conversation whose recent turns are buried under megabytes of tool output
/// still has to answer in the time a person will wait for a tap
const BUDGET: usize = 8 * 1024 * 1024;

/// Who said it. Named for the reader, not for the API underneath: the person
/// holding the phone is "you", and everything the CLI produced is the AI
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Who {
    You,
    Ai,
}

/// One thing said, as text. No markup is applied here — the reading side
/// decides how a fenced code block or a heading should look, and it is the
/// only side that knows how wide the screen is
#[derive(Clone, Debug, Serialize)]
pub struct Turn {
    pub who: Who,
    pub text: String,
}

/// A stretch of the conversation, oldest turn first.
#[derive(Clone, Debug, Serialize)]
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
            let said = match look_at(&buf[at..stop.max(at)]) {
                Seen::Reaching => {
                    reaching_below = true;
                    None
                }
                // Said on the way to that tool, so it belongs to the work
                Seen::Said(turn) if turn.who == Who::Ai && reaching_below => None,
                Seen::Said(turn) => {
                    reaching_below = false;
                    Some(turn)
                }
                Seen::Nothing => None,
            };
            // One block past what was asked for: stop WITHOUT taking this line,
            // so the next page begins with it and the block it opens is read
            // whole rather than beheaded
            if let Some(turn) = &said
                && found.len() >= want
                && found.last().is_none_or(|last| last.who != turn.who)
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
                (Some(turn), Some(last)) if last.who == turn.who => {
                    last.text = format!("{}\n\n{}", turn.text, last.text);
                }
                (Some(turn), _) => found.push(turn),
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

    found.reverse();
    Ok(Page {
        turns: found,
        from,
        more: from > 0,
    })
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
    let rest = glob.strip_prefix("{home}/")?.replace("{id}", id);
    if !safe(&rest) {
        return None;
    }
    let ran = crate::elsewhere::exec(at, &format!("cd \"$HOME\" && ls -1d {rest} 2>/dev/null | head -n 1 | sed \"s#^#$HOME/#\""), 30_000).ok()?;
    let path = ran.out.lines().next()?.trim().to_string();
    (!path.is_empty()).then_some(path)
}

/// `read_back` for a record on another machine, fetched a piece at a time
pub fn read_back_far(at: &crate::elsewhere::Elsewhere, path: &str, before: u64, want: usize) -> std::io::Result<Page> {
    use base64::Engine as _;
    let err = |e: anyhow::Error| std::io::Error::other(format!("{e:#}"));
    let quoted = crate::worktree::for_a_shell(&[path.to_string()]);
    let len: u64 = crate::elsewhere::exec(at, &format!("stat -c %s -- {quoted}"), 30_000)
        .map_err(err)?
        .out
        .trim()
        .parse()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::NotFound, "no such record"))?;
    read_back_by(len, before, want, &mut |start, n| {
        let ran = crate::elsewhere::exec(
            at,
            &format!("tail -c +{} -- {quoted} | head -c {n} | base64 -w0", start + 1),
            60_000,
        )
        .map_err(err)?;
        base64::engine::general_purpose::STANDARD
            .decode(ran.out.trim())
            .map_err(|e| std::io::Error::other(e.to_string()))
    })
}

/// What one record turns out to be.
enum Seen {
    /// Somebody speaking, and what they said
    Said(Turn),
    /// The AI reaching for a tool. Nothing was said here, but whatever was
    /// said just before it was said on the way to this
    Reaching,
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
    let spoken =
        text.contains("\"role\"") && (text.contains("\"text\"") || text.contains("\"content\":\""));
    if !spoken && !text.contains("_use\"") && !text.contains("_call\"") {
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
        return Seen::Reaching;
    }
    match turn_of(&record) {
        Some(turn) => Seen::Said(turn),
        None => Seen::Nothing,
    }
}

/// One record, if it is somebody speaking.
fn turn_of(record: &Value) -> Option<Turn> {
    let message = message_of(record)?;
    let who = match message.get("role").and_then(Value::as_str)? {
        "assistant" => Who::Ai,
        "user" => Who::You,
        // "developer", "system", "tool" — written by machinery, for machinery
        _ => return None,
    };
    let said = words_of(message.get("content")?);
    let said = match who {
        Who::You => human_part(&said),
        Who::Ai => said.trim().to_string(),
    };
    (!said.is_empty()).then_some(Turn { who, text: said })
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
    here(record) || ["message", "payload"].iter().any(|k| record.get(*k).is_some_and(here))
}

/// The object carrying `role`: the record itself, or the one field it is
/// wrapped in.
fn message_of(record: &Value) -> Option<&Value> {
    if record.get("role").is_some() {
        return Some(record);
    }
    ["message", "payload"]
        .iter()
        .find_map(|key| record.get(*key).filter(|m| m.get("role").is_some()))
}

/// The words out of a `content`: a bare string, or every block whose type ends
/// in "text". Blocks are joined with a blank line because that is what they
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
        let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
        if !kind.ends_with("text") {
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
fn human_part(text: &str) -> String {
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
            matches!(look_at(line.as_bytes()), Seen::Reaching)
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
}

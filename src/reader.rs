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

/// The last `want` turns of `path` that begin before byte `before`.
///
/// `before` is `u64::MAX` for "the end of the file" — the first page. Every
/// page after that passes the previous page's `from`.
pub fn read_back(path: &Path, before: u64, want: usize) -> std::io::Result<Page> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut end = before.min(len);
    let mut from = end;
    // The back half of a line whose start lies further back than we have read.
    // Carried into the next round rather than parsed half-read
    let mut carried: Vec<u8> = Vec::new();
    // Newest first while collecting; turned the right way round at the end
    let mut found: Vec<Turn> = Vec::new();
    let mut read = 0usize;

    while end > 0 && read < BUDGET && found.len() < want {
        let start = end.saturating_sub(CHUNK as u64);
        let mut buf = vec![0u8; (end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buf)?;
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
            if let Some(turn) = turn_of(&buf[at..stop.max(at)]) {
                found.push(turn);
            }
            // Set for every line looked at, not only the ones that said
            // something: the next page must start strictly before whatever
            // this one has already been through, or it repeats it
            from = start + at as u64;
            if found.len() >= want {
                break;
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
        turns: merge(found),
        from,
        more: from > 0,
    })
}

/// One record, if it is somebody speaking.
fn turn_of(line: &[u8]) -> Option<Turn> {
    // A cheap gate ahead of the JSON parser. Most of these lines are tool
    // results, some of them megabytes each, and parsing every one of them only
    // to find out it is not a message is where the whole cost of a read would go
    let text = std::str::from_utf8(line).ok()?;
    if !text.contains("\"role\"") || !text.contains("\"text\"") {
        return None;
    }
    let record: Value = serde_json::from_str(text).ok()?;
    // A side conversation — a sub-agent's own turns, written into the same
    // file. It is a different conversation that happens to share a log, and
    // splicing it in would read as the AI interrupting itself
    if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let message = message_of(&record)?;
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

/// One turn per thing said, not one per record.
///
/// A CLI writes a single turn as several records — a paragraph, a tool call,
/// another paragraph — so read back record by record an answer arrives in
/// shards with the speaker's name repeated between them. Neighbours from the
/// same speaker are one thing said.
fn merge(turns: Vec<Turn>) -> Vec<Turn> {
    let mut out: Vec<Turn> = Vec::with_capacity(turns.len());
    for turn in turns {
        match out.last_mut() {
            Some(last) if last.who == turn.who => {
                last.text.push_str("\n\n");
                last.text.push_str(&turn.text);
            }
            _ => out.push(turn),
        }
    }
    out
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

    /// Claude's shape: the message is under `message`, the words under blocks
    /// of type "text"
    #[test]
    fn claude_records_are_read() {
        let line = r#"{"type":"assistant","isSidechain":false,"message":{"role":"assistant","content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"直しました"}]}}"#.as_bytes();
        let turn = turn_of(line).expect("assistantの発言");
        assert_eq!(turn.who, Who::Ai);
        assert_eq!(turn.text, "直しました");
    }

    /// Codex's shape: the message is under `payload`, the words under blocks
    /// of type "output_text". Nothing about the reader knows which is which
    #[test]
    fn codex_records_are_read() {
        let line = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"原因が確定しました"}]}}"#.as_bytes();
        let turn = turn_of(line).expect("assistantの発言");
        assert_eq!(turn.who, Who::Ai);
        assert_eq!(turn.text, "原因が確定しました");
    }

    #[test]
    fn machinery_is_not_speech() {
        // A tool result rides in a user record; it has a role, but nobody said it
        let result = r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","text":"ok"}]}}"#.as_bytes();
        assert!(turn_of(result).is_none(), "ツール結果は発言ではない");
        // A sub-agent's transcript shares the file
        let side = r#"{"isSidechain":true,"message":{"role":"assistant","content":[{"type":"text","text":"別の会話"}]}}"#.as_bytes();
        assert!(turn_of(side).is_none(), "サブエージェントの記録は別の会話");
        // Neither is anything without a role
        let meta = r#"{"type":"summary","text":"…"}"#.as_bytes();
        assert!(turn_of(meta).is_none());
    }

    #[test]
    fn envelopes_are_peeled_off_what_a_person_typed() {
        let line = r#"{"message":{"role":"user","content":[{"type":"text","text":"<system-reminder>machine</system-reminder>直して<user_instructions>rules</user_instructions>"}]}}"#.as_bytes();
        let turn = turn_of(line).expect("人の発言");
        assert_eq!(turn.who, Who::You);
        assert_eq!(turn.text, "直して", "機械が挟んだ封筒は人の言葉ではない");
    }

    /// The tag shape is the rule, so pasted HTML survives — it has no hyphen
    #[test]
    fn pasted_markup_is_left_alone() {
        assert_eq!(human_part("<div>hello</div>"), "<div>hello</div>");
        // ...and an envelope that never closes is not an excuse to cut
        assert_eq!(human_part("<system-reminder>ここから先"), "<system-reminder>ここから先");
    }

    #[test]
    fn one_turn_per_speaker_not_per_record() {
        let turns = merge(vec![
            Turn { who: Who::Ai, text: "調べます".into() },
            Turn { who: Who::Ai, text: "直しました".into() },
            Turn { who: Who::You, text: "ありがとう".into() },
        ]);
        assert_eq!(turns.len(), 2, "続けて喋った分は1つ");
        assert_eq!(turns[0].text, "調べます\n\n直しました");
    }

    fn record(who: &str, text: &str) -> String {
        format!(
            "{{\"message\":{{\"role\":\"{who}\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}"
        )
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
        assert_eq!(last.turns[3].text, "a39", "最後の発言が末尾に来る");
        assert_eq!(last.turns[3].who, Who::Ai);
        assert!(last.more, "まだ前がある");

        // The page before it, asked for by where the last one started
        let older = read_back(&path, last.from, 4).unwrap();
        assert_eq!(older.turns[3].text, "a37");
        assert!(!older.turns.iter().any(|t| t.text == "a38"), "同じ発言を二度返さない");

        // Walking back far enough reaches the head, and says so
        let mut at = older.from;
        for _ in 0..40 {
            let page = read_back(&path, at, 4).unwrap();
            at = page.from;
            if !page.more {
                break;
            }
        }
        assert_eq!(at, 0, "先頭まで遡れる");
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
            panic!("SHIKISHA_READ_PROBE に記録ファイルのパスを渡す");
        };
        let path = PathBuf::from(path);
        let size = std::fs::metadata(&path).unwrap().len();
        let began = std::time::Instant::now();
        let page = read_back(&path, u64::MAX, 6).unwrap();
        println!(
            "{} MB / {:?} で {} turn (more={})",
            size / 1_048_576,
            began.elapsed(),
            page.turns.len(),
            page.more
        );
        for turn in &page.turns {
            let head: String = turn.text.chars().take(90).collect();
            println!("[{:?}] {} …", turn.who, head.replace('\n', " "));
        }
        assert!(!page.turns.is_empty(), "本物の記録から1つも読めない");
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
        assert_eq!(said, vec!["先頭の発言\n\n最後の発言"], "巨大な行の向こうの発言も残る");
        assert!(!page.more, "先頭まで読み切っている");
    }
}

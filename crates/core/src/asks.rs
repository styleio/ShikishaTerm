//! What a person asked an AI, read out of the record the AI already keeps.
//!
//! A folder that names itself ([`crate::labels`]) is written from the requests
//! typed into its AI tabs. Two of those roads were already here: the app's own
//! input bar knows what it handed over, and a CLI can be asked to report each
//! request through a hook. Neither covers the ordinary case -- somebody typing
//! straight into the terminal of a CLI that has had nothing installed in it --
//! and it was that case, silently, that left folders unnamed.
//!
//! This is the third road, and it asks nothing of anyone. Every CLI worth
//! resuming writes down its own conversation, one line of JSON at a time, and
//! this app already knows which file that is: it hands Claude the id it will
//! use, and finds out Codex's from Codex's own records. So the requests are
//! already on this machine, in a file that is already located, and the only
//! work left is telling a person's words apart from everything else on those
//! lines.
//!
//! That last part is the whole difficulty. A record is not a list of requests:
//! what a tool returned is written as something the model was told, and so is
//! what the app injected around it, under the same `user` as the person. Both
//! CLIs wrap everything they inject in a tag, which is the one thing that
//! holds across the two, so it is what the reading leans on -- alongside the
//! shapes each CLI's own profile describes.
//!
//! Nothing here decides what is worth keeping. What comes out goes into the
//! same funnel every request has always gone into ([`crate::labels::Board`]):
//! too-light requests dropped, code cut out, the newest few kept, the whole
//! thing asked for at most once in ten minutes.

use std::io::{BufRead as _, Seek as _};
use std::path::Path;

use crate::profile::AskSpec;

/// The most of one record we read in one go.
///
/// A conversation left running all day is a long file, and the reading happens
/// on the loop's own thread. Everything past this waits for the next look,
/// which costs nothing: the only reader is a summary of the last few requests
const MOST_AT_ONCE: u64 = 1 << 20;

/// The longest line worth parsing.
///
/// A record's lines carry whole files that were read and whole screens that
/// were printed. None of that is a person typing, and a person does not type a
/// megabyte
const LONGEST_LINE: usize = 256 * 1024;

/// Where to begin in a record nothing has read yet.
///
/// Not its start: a conversation carried over from last week is megabytes of
/// history, and reading all of it again on every launch buys nothing, since
/// only the newest few requests are ever used. Not its end either, because the
/// first request of a new tab is written down before anything looks for it. So
/// the last stretch, which holds the recent ones and costs one read. A line cut
/// in half at the front is not JSON, and the reading drops it for that reason
/// rather than needing to be told
pub fn begin_at(file: &Path) -> u64 {
    const TAIL: u64 = 256 * 1024;
    std::fs::metadata(file).map(|m| m.len().saturating_sub(TAIL)).unwrap_or(0)
}

/// The requests written into `file` after byte `from`, and where to read from
/// next time.
///
/// Reading picks up where it left off, so a conversation is never read twice.
/// A file that has grown shorter than where we were -- a new conversation in
/// the same place, a record rewritten -- is read from its start again, because
/// the alternative is a tab that never speaks again.
pub fn read_from(file: &Path, spec: &AskSpec, from: u64) -> (Vec<String>, u64) {
    let Ok(f) = std::fs::File::open(file) else { return (Vec::new(), from) };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let at = match size < from {
        true => 0,
        false => from,
    };
    if size <= at {
        return (Vec::new(), at);
    }
    let end = size.min(at + MOST_AT_ONCE);
    let mut f = std::io::BufReader::new(f);
    if f.seek(std::io::SeekFrom::Start(at)).is_err() {
        return (Vec::new(), at);
    }
    let mut said = Vec::new();
    let mut read = at;
    let mut line = Vec::new();
    while read < end {
        line.clear();
        let Ok(n) = f.read_until(b'\n', &mut line) else { break };
        if n == 0 {
            break;
        }
        // A line with no newline on the end is one still being written. Left
        // where it is, so the next look reads it whole
        if !line.ends_with(b"\n") {
            break;
        }
        read += n as u64;
        if line.len() > LONGEST_LINE {
            continue;
        }
        if let Some(text) = ask_of(&line, spec) {
            said.push(text);
        }
    }
    (said, read)
}

/// A person's words on one line of a record, when that is what it is.
fn ask_of(line: &[u8], spec: &AskSpec) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(line).ok()?;
    if v.pointer(&spec.role_at)?.as_str()? != spec.role {
        return None;
    }
    // Lines the CLI itself marks as not being somebody speaking
    if spec.skip_when.iter().any(|at| truthy(v.pointer(at))) {
        return None;
    }
    let held = v.pointer(&spec.text_at)?;
    let text = match spec.parts.is_empty() {
        // The words stand there as they were typed
        true => held.as_str()?.to_string(),
        // The words are in blocks, beside blocks that are not words at all --
        // what a tool returned, a picture, a file that came along
        false => held
            .as_array()?
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()).is_some_and(|t| spec.parts.iter().any(|p| p == t)))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    let text = text.trim();
    // Everything either CLI injects arrives wrapped in a tag -- the folder it
    // is standing in, the plugins it could install, a background job that
    // finished, a picture's dimensions. A person opening with `<` is a person
    // writing about HTML, and losing that one request costs less than
    // describing a folder from a list of plugins
    match text.is_empty() || text.starts_with('<') {
        true => None,
        false => Some(text.to_string()),
    }
}

/// Whether a pointer holds something that means "yes" -- a flag a CLI writes
/// only when it is set
fn truthy(v: Option<&serde_json::Value>) -> bool {
    match v {
        Some(serde_json::Value::Bool(b)) => *b,
        Some(serde_json::Value::Null) | None => false,
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> AskSpec {
        AskSpec {
            role_at: "/message/role".into(),
            role: "user".into(),
            text_at: "/message/content".into(),
            parts: Vec::new(),
            skip_when: vec!["/isMeta".into()],
        }
    }

    fn codex() -> AskSpec {
        AskSpec {
            role_at: "/payload/role".into(),
            role: "user".into(),
            text_at: "/payload/content".into(),
            parts: vec!["input_text".into()],
            skip_when: Vec::new(),
        }
    }

    fn one(line: &str, spec: &AskSpec) -> Option<String> {
        ask_of(line.as_bytes(), spec)
    }

    /// The shapes taken from real records, a line of each. What is wanted is
    /// the two a person typed; everything else on these lines is written by
    /// something that is not a person, under the same name
    #[test]
    fn a_persons_words_are_told_apart_from_everything_else_a_cli_writes() {
        let spec = claude();
        assert_eq!(
            one(r#"{"type":"user","message":{"role":"user","content":"fix the login page"}}"#, &spec).as_deref(),
            Some("fix the login page")
        );
        // What a tool returned, which is written as something the model was
        // told and carries whole files with it
        assert_eq!(
            one(r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"x","content":"...40kB..."}]}}"#, &spec),
            None
        );
        // The app talking to itself around a person's message
        assert_eq!(
            one(r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"[Image: 2560x1720]"}}"#, &spec),
            None
        );
        assert_eq!(
            one("{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"<task-notification>\\n<task-id>b7a</task-id>\\n</task-notification>\"}}", &spec),
            None
        );
        // Not a person at all
        assert_eq!(
            one(r#"{"type":"assistant","message":{"role":"assistant","content":"done"}}"#, &spec),
            None
        );
        assert_eq!(one("not json at all", &spec), None);

        let spec = codex();
        assert_eq!(
            one(r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"push to main"}]}}"#, &spec).as_deref(),
            Some("push to main")
        );
        // Everything this one injects is wrapped in a tag as well
        assert_eq!(
            one(r#"{"type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":"<environment_context>\n  <cwd>D:\\work</cwd>\n</environment_context>"}]}}"#, &spec),
            None
        );
        // A picture that came along with nothing typed beside it
        assert_eq!(
            one(r#"{"type":"response_item","payload":{"role":"user","content":[{"type":"input_image","image_url":"data:..."}]}}"#, &spec),
            None
        );
    }

    /// The CLIs that ship with this app describe their own records, and what
    /// they describe reads a line of the real thing.
    ///
    /// The lines below were taken from records on a working machine, one shape
    /// each. A CLI that changes how it writes them breaks this, which is the
    /// point -- against a real record, `asks_probe` says what changed
    #[test]
    fn each_shipped_profile_says_how_to_read_its_own_record() {
        let how = |file: &str| {
            let text =
                std::fs::read_to_string(crate::repo_root().join(format!("profiles/{file}.json")))
                    .unwrap();
            let f: crate::profile::ProfileFile = serde_json::from_str(&text).unwrap();
            crate::profile::Profile::compile(f).unwrap().resume.and_then(|r| r.asks)
        };

        let claude = how("claude").expect("Claude Code says nothing about reading its record");
        assert_eq!(
            one(r#"{"parentUuid":"a","type":"user","message":{"role":"user","content":"why does the left side still show the old project?"}}"#, &claude).as_deref(),
            Some("why does the left side still show the old project?")
        );
        assert_eq!(
            one(r#"{"type":"user","toolUseResult":{},"message":{"role":"user","content":[{"tool_use_id":"toolu_01","type":"tool_result","content":"<persisted-output>..."}]}}"#, &claude),
            None,
            "what a tool returned was taken for something a person typed"
        );

        let codex = how("codex").expect("Codex CLI says nothing about reading its record");
        assert_eq!(
            one(r#"{"timestamp":"2026-09-18T07:40:57.723Z","ordinal":3,"type":"response_item","payload":{"type":"message","id":"msg_01","role":"user","content":[{"type":"input_text","text":"push it to main"}]}}"#, &codex).as_deref(),
            Some("push it to main")
        );
        assert_eq!(
            one(r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<recommended_plugins>\nAirtable\n</recommended_plugins>"}]}}"#, &codex),
            None,
            "what the CLI told itself was taken for something a person typed"
        );

        // A CLI that has nothing to read says so by saying nothing, rather
        // than by being read wrongly
        assert!(how("aider").is_none());
    }

    /// A record is read where it was left, and a request half written is read
    /// whole on the next look rather than in halves
    #[test]
    fn a_record_is_read_once_and_never_in_halves() {
        let dir = std::env::temp_dir().join(format!("shikisha-asks-{}", crate::random_hex(6)));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rec.jsonl");
        let spec = claude();
        let line = |t: &str| format!("{{\"type\":\"user\",\"message\":{{\"role\":\"user\",\"content\":\"{t}\"}}}}\n");

        std::fs::write(&file, line("first thing")).unwrap();
        let (said, at) = read_from(&file, &spec, 0);
        assert_eq!(said, vec!["first thing".to_string()]);

        // Nothing new: nothing said, and the place does not move
        let (said, same) = read_from(&file, &spec, at);
        assert!(said.is_empty(), "it read the same request twice: {said:?}");
        assert_eq!(same, at);

        // Half a line is not a request yet
        let half = format!("{}{}", line("second thing"), r#"{"type":"user","message":{"role":"user","content":"thi"#);
        std::fs::write(&file, format!("{}{half}", line("first thing"))).unwrap();
        let (said, at) = read_from(&file, &spec, at);
        assert_eq!(said, vec!["second thing".to_string()], "a half-written line was read");

        // Finished, it is read whole
        std::fs::write(&file, format!("{}{}{}", line("first thing"), line("second thing"), line("third thing"))).unwrap();
        let (said, _) = read_from(&file, &spec, at);
        assert_eq!(said, vec!["third thing".to_string()]);

        // A record that got shorter is another conversation in the same place
        std::fs::write(&file, line("a new one")).unwrap();
        let (said, _) = read_from(&file, &spec, 9_000);
        assert_eq!(said, vec!["a new one".to_string()], "it went quiet after the record was replaced");

        // A record that is not there says nothing and keeps its place
        let (said, at) = read_from(&dir.join("gone.jsonl"), &spec, 12);
        assert!(said.is_empty());
        assert_eq!(at, 12);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Calls to a model, read out of the lines of a CLI's conversation record.
//!
//! Claude Code writes one line per message, and an assistant message carries
//! what its call used (`message.usage`). The same message is written again
//! while it streams, with larger numbers, under the same message and request
//! ids: one call, whose largest numbers are the ones it ended with.
//!
//! Codex writes a running count (`token_count` events): what the last call
//! used, and the conversation's total so far. The model and the folder are on
//! earlier lines (`turn_context`, `session_meta`), so they are carried from
//! one line to the next -- and from one reading of a record to the next, for
//! a record read a piece at a time.

use serde_json::Value;

/// One call, as a record says it
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Call {
    pub key: String,
    pub cli: &'static str,
    /// Seconds since 1970
    pub at: u64,
    pub model: String,
    pub folder: String,
    pub session: String,
    pub fresh_in: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cache_write_hour: u64,
    pub out: u64,
    pub reasoning: u64,
    pub is_fast: bool,
    pub is_us: bool,
}

impl Call {
    /// Every token that went into the call: what a price's prompt length is
    /// measured by
    pub fn prompt(&self) -> u64 {
        self.fresh_in + self.cache_read + self.cache_write + self.cache_write_hour
    }

    fn spent_nothing(&self) -> bool {
        self.prompt() == 0 && self.out == 0
    }
}

fn n(v: &Value, path: &str) -> u64 {
    v.pointer(path).and_then(Value::as_u64).unwrap_or(0)
}

fn s(v: &Value, path: &str) -> String {
    v.pointer(path).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// A line of a Claude Code record, when it is a call that used something.
/// Lines without "usage" in them are not parsed at all: most of a record is
/// tool output, and this is read over every record on the machine
pub fn claude_line(line: &[u8]) -> Option<Call> {
    if !contains(line, b"\"usage\"") || !contains(line, b"assistant") {
        return None;
    }
    let v: Value = serde_json::from_slice(line).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let usage = v.pointer("/message/usage")?;
    let at = crate::e2b::epoch_of(v.get("timestamp").and_then(Value::as_str)?);
    if at == 0 {
        return None;
    }
    let (id, request, uuid) = (s(&v, "/message/id"), s(&v, "/requestId"), s(&v, "/uuid"));
    let key = match (id.is_empty(), request.is_empty()) {
        (false, false) => format!("c:{id}:{request}"),
        (false, true) => format!("c:{id}"),
        _ if !uuid.is_empty() => format!("c:u:{uuid}"),
        _ => return None,
    };
    let written = n(usage, "/cache_creation_input_tokens");
    // The hour-long share of what was written, never more than the whole
    let hour = n(usage, "/cache_creation/ephemeral_1h_input_tokens").min(written);
    let call = Call {
        key,
        cli: "claude",
        at,
        model: s(&v, "/message/model"),
        folder: s(&v, "/cwd"),
        session: s(&v, "/sessionId"),
        fresh_in: n(usage, "/input_tokens"),
        cache_read: n(usage, "/cache_read_input_tokens"),
        cache_write: written - hour,
        cache_write_hour: hour,
        out: n(usage, "/output_tokens"),
        reasoning: n(usage, "/output_tokens_details/thinking_tokens"),
        is_fast: usage.get("speed").and_then(Value::as_str) == Some("fast"),
        is_us: usage.get("inference_geo").and_then(Value::as_str) == Some("us"),
    };
    (!call.spent_nothing()).then_some(call)
}

/// What a Codex record's later lines need from its earlier ones
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CodexCarry {
    #[serde(default)]
    pub session: String,
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub model: String,
    /// The conversation's running total at the last count: input (cached
    /// included), cached, output, reasoning
    #[serde(default)]
    pub total: [u64; 4],
    /// The running total of input written to the cache, beside it. A field
    /// of its own so a carry kept before it was read still reads
    #[serde(default)]
    pub written: u64,
}

/// A line of a Codex record: a call when it is a count of one, and the
/// carry brought up to date either way
pub fn codex_line(line: &[u8], carry: &mut CodexCarry) -> Option<Call> {
    let kind = if contains(line, b"\"token_count\"") {
        "count"
    } else if contains(line, b"\"turn_context\"") || contains(line, b"\"session_meta\"") {
        "about"
    } else {
        return None;
    };
    let v: Value = serde_json::from_slice(line).ok()?;
    let p = v.get("payload")?;
    if kind == "about" {
        match v.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let id = s(p, "/id");
                if !id.is_empty() {
                    carry.session = id;
                }
                let cwd = s(p, "/cwd");
                if !cwd.is_empty() {
                    carry.folder = cwd;
                }
            }
            Some("turn_context") => {
                let model = s(p, "/model");
                if !model.is_empty() {
                    carry.model = model;
                }
                let cwd = s(p, "/cwd");
                if !cwd.is_empty() {
                    carry.folder = cwd;
                }
            }
            _ => {}
        }
        return None;
    }
    if p.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    // A count with no numbers only says how much of the plan is left
    let info = p.get("info").filter(|i| !i.is_null())?;
    let read = |u: &Value| [n(u, "/input_tokens"), n(u, "/cached_input_tokens").max(n(u, "/cache_read_input_tokens")), n(u, "/output_tokens"), n(u, "/reasoning_output_tokens")];
    let total = info.get("total_token_usage").filter(|u| !u.is_null());
    let total = total.map(|u| (read(u), n(u, "/cache_write_input_tokens")));
    // The same total counted again is not another call. Codex says it again
    // when only what is left of the plan changed -- with the last call's
    // numbers beside it, under a new time
    if let Some((t, w)) = total
        && t == carry.total && w == carry.written {
            return None;
        }
    let (last, written) = match info.get("last_token_usage").filter(|u| !u.is_null()) {
        Some(u) => (read(u), n(u, "/cache_write_input_tokens")),
        // Only the total: the call is what it grew by. A total that went
        // down began again (the conversation was compacted or forked), so
        // all of it is this call's
        None => {
            let (t, w) = total?;
            if t.iter().zip(carry.total.iter()).all(|(a, b)| a >= b) && w >= carry.written {
                ([t[0] - carry.total[0], t[1] - carry.total[1], t[2] - carry.total[2], t[3] - carry.total[3]], w - carry.written)
            } else {
                (t, w)
            }
        }
    };
    if let Some((t, w)) = total {
        (carry.total, carry.written) = (t, w);
    }
    let stamp = v.get("timestamp").and_then(Value::as_str)?;
    let at = crate::e2b::epoch_of(stamp);
    if at == 0 {
        return None;
    }
    // Input counts all that went in: what was read from the cache, what was
    // written to it, and the rest
    let cached = last[1].min(last[0]);
    let written = written.min(last[0] - cached);
    let call = Call {
        // Codex's own records give a call no id. When it was counted and what
        // it counted is the same wherever the line is copied to -- a resumed
        // conversation is written under another id, so that is left out
        key: format!("x:{stamp}:{}:{}:{}:{}", last[0], last[1], last[2], last[3]),
        cli: "codex",
        at,
        model: carry.model.clone(),
        folder: carry.folder.clone(),
        session: carry.session.clone(),
        fresh_in: last[0] - cached - written,
        cache_read: cached,
        cache_write: written,
        cache_write_hour: 0,
        out: last[2],
        reasoning: last[3],
        is_fast: false,
        is_us: false,
    };
    (!call.spent_nothing()).then_some(call)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = r#"{"type":"assistant","timestamp":"2026-10-12T01:02:03.000Z","cwd":"/work/app","sessionId":"s1","requestId":"req_1","uuid":"u1","message":{"id":"msg_1","model":"claude-opus-5-5","usage":{"input_tokens":2,"cache_creation_input_tokens":22700,"cache_read_input_tokens":33813,"output_tokens":273,"output_tokens_details":{"thinking_tokens":89},"cache_creation":{"ephemeral_1h_input_tokens":22000,"ephemeral_5m_input_tokens":700},"inference_geo":"us","speed":"fast"}}}"#;

    #[test]
    fn a_claude_call_is_read_with_every_kind_of_token() {
        let c = claude_line(CLAUDE.as_bytes()).unwrap();
        assert_eq!(c.key, "c:msg_1:req_1");
        assert_eq!((c.fresh_in, c.cache_read, c.cache_write, c.cache_write_hour, c.out, c.reasoning), (2, 33813, 700, 22000, 273, 89));
        assert_eq!((c.model.as_str(), c.folder.as_str(), c.session.as_str()), ("claude-opus-5-5", "/work/app", "s1"));
        assert!(c.is_fast && c.is_us);
        assert_eq!(c.at, 1_791_766_923);
        assert!(claude_line(br#"{"type":"user","message":{"content":"usage of assistant"}}"#).is_none());
        let none = CLAUDE.replace("273", "0").replace("22700", "0").replace("33813", "0").replace(r#""input_tokens":2"#, r#""input_tokens":0"#);
        assert!(claude_line(none.as_bytes()).is_none(), "a call that used nothing is not one");
    }

    #[test]
    fn a_codex_count_carries_its_model_and_folder() {
        let mut carry = CodexCarry::default();
        assert!(codex_line(br#"{"type":"session_meta","payload":{"id":"sess-9","cwd":"D:\\work"}}"#, &mut carry).is_none());
        assert!(codex_line(br#"{"type":"turn_context","payload":{"model":"gpt-6-astra","cwd":"D:\\work\\app"}}"#, &mut carry).is_none());
        let line = br#"{"timestamp":"2026-10-09T01:07:00.091Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":16116,"cached_input_tokens":16000,"output_tokens":286,"reasoning_output_tokens":10},"last_token_usage":{"input_tokens":16116,"cached_input_tokens":16000,"output_tokens":286,"reasoning_output_tokens":10}}}}"#;
        let c = codex_line(line, &mut carry).unwrap();
        assert_eq!((c.fresh_in, c.cache_read, c.out, c.reasoning), (116, 16000, 286, 10));
        assert_eq!((c.model.as_str(), c.folder.as_str(), c.session.as_str()), ("gpt-6-astra", r"D:\work\app", "sess-9"));
        assert_eq!(carry.total, [16116, 16000, 286, 10]);
        assert!(codex_line(br#"{"type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{}}}"#, &mut carry).is_none(), "a count of the plan alone is no call");
        // Said again when the plan changed: the same total, the same last
        // call, a new time
        let again = String::from_utf8_lossy(line).replace("01:07:00.091Z", "01:07:05.000Z");
        assert!(codex_line(again.as_bytes(), &mut carry).is_none(), "the same total under a new time is not another call");
    }

    #[test]
    fn a_codex_cache_write_is_its_own_part_of_the_input() {
        let mut carry = CodexCarry::default();
        let line = br#"{"timestamp":"2026-10-09T01:07:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":300,"cache_write_input_tokens":200,"output_tokens":5},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":300,"cache_write_input_tokens":200,"output_tokens":5}}}}"#;
        let c = codex_line(line, &mut carry).unwrap();
        assert_eq!((c.fresh_in, c.cache_read, c.cache_write), (500, 300, 200));
        // A carry kept before writes were read still reads
        let old: CodexCarry = serde_json::from_str(r#"{"session":"s","folder":"f","model":"m","total":[1,2,3,4]}"#).unwrap();
        assert_eq!((old.total, old.written), ([1, 2, 3, 4], 0));
    }

    #[test]
    fn a_codex_total_alone_is_counted_by_how_much_it_grew() {
        let mut carry = CodexCarry { total: [100, 50, 10, 0], ..Default::default() };
        let line = br#"{"timestamp":"2026-10-09T01:08:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":160,"cached_input_tokens":80,"output_tokens":25,"reasoning_output_tokens":0}}}}"#;
        let c = codex_line(line, &mut carry).unwrap();
        assert_eq!((c.fresh_in, c.cache_read, c.out), (30, 30, 15));
        assert!(codex_line(line, &mut carry).is_none(), "the same total again is not another call");
        // Begun again: smaller than before, so all of it is new
        let again = br#"{"timestamp":"2026-10-09T01:09:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":40,"cached_input_tokens":0,"output_tokens":5,"reasoning_output_tokens":0}}}}"#;
        let c = codex_line(again, &mut carry).unwrap();
        assert_eq!((c.fresh_in, c.out), (40, 5));
    }
}

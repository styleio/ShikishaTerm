//! Asking another tab, and waiting for what it says back.
//!
//! `send_to_tab` puts words in front of another AI and returns at once, which
//! is right for a script with hooks to catch the answer and wrong for an AI
//! that wants the answer as the result of the call it made. `ask_tab` is the
//! second shape: the call stays open until the tab it went to has finished,
//! and what comes back is that tab's reply.
//!
//! **Why it is answered here and not in Lua.** A call through the pipe is
//! carried out on the main loop, and a primitive that stops to wait stops the
//! loop with it -- every screen, every detector, including the one that would
//! notice the other tab had finished. So the call is taken apart: Lua checks
//! it may be made (the permission table, the tab existing), the loop keeps the
//! line's answer channel, and a tick at a time looks at the other tab until
//! there is something to say.
//!
//! **Where the reply comes from.** The CLI's own record of the conversation,
//! read the way the phone's reader reads it -- not the screen, which is a
//! picture of the reply cut to the window's width and wrapped in the tool's
//! own frame. A tab whose profile names no record falls back to the screen.
//!
//! **What is never lost.** The caller may stop holding the line: its client
//! has a timeout of its own, or a person pressed Esc. The other tab goes on
//! working all the same, and when it finishes its reply is typed into the
//! caller's tab instead, once the caller is free to read it.

use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::detect::TabState;
use crate::reader::Who;
use crate::tab::Tab;

/// How long a caller is held when it names no time of its own. Shorter than
/// the hour this app writes into a CLI's settings for the call, so the answer
/// "still working" arrives before the client gives up on the line
pub const DEFAULT_WAIT: Duration = Duration::from_secs(50 * 60);

/// How long the pipe itself holds a line open for this one call. Longer than
/// anything a caller may ask for; the loop is what decides when to answer
pub const LINE_HOLD: Duration = Duration::from_secs(6 * 60 * 60);

/// A finished-looking tab has to stay finished this long before it counts. A
/// turn has quiet moments -- between a tool and the next thought -- that the
/// detector can read as the end
const SETTLE: Duration = Duration::from_millis(1500);

/// With no record to read, the screen is read instead once the tab has been
/// quiet this long
const SCREEN_SETTLE: Duration = Duration::from_secs(3);

/// How long a tab may sit with background work after its turn before the reply
/// it has written is taken as its answer. An AI that put a wait in the
/// background comes back and says more when it finishes; a tab that keeps a
/// server running never will, and the caller should not be held for an hour
/// on a reply that is already written
const BACKGROUND_GRACE: Duration = Duration::from_secs(180);

/// A tab that never looked busy after being sent something is taken to have
/// started (and finished) quickly once this has passed
const NEVER_SEEN_BUSY: Duration = Duration::from_secs(8);

/// How much of what was sent is looked for in the record, to find the turn
/// the reply belongs to. Enough to tell two questions apart, short enough to
/// survive a CLI trimming the end of a long paste
const MATCH_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Not sent yet: the other tab was busy, and words typed into a busy CLI
    /// are queued at best and lost at worst
    Queued,
    /// Sent; the caller is holding the line for the reply
    Waiting,
    /// The caller stopped holding the line. The reply goes into its tab
    Deliver,
    /// The reply is ready and waits for the caller's tab to be free
    Handing(String),
}

/// One ask in flight.
pub struct Ask {
    pub reply: Option<Sender<Result<Value, String>>>,
    pub caller: Option<String>,
    pub target: String,
    pub text: String,
    pub phase: Phase,
    pub asked_at: Instant,
    pub sent_at: Option<Instant>,
    pub seen_busy: bool,
    pub quiet_since: Option<Instant>,
    /// Since when the tab has sat in `Background` after the turn
    pub background_since: Option<Instant>,
    pub deadline: Instant,
    pub round: u32,
    pub max_rounds: u32,
}

/// What the loop should do for an ask this tick.
pub enum Step {
    Nothing,
    /// Send the words now
    Send,
    /// Answer the caller (or, with no line left, type it into the caller's tab)
    Answer(Value),
    /// Type the finished reply into the caller's tab now
    Hand(String),
    /// Forget it
    Drop,
}

/// `ask_tab(tab, text, {timeout_ms})`, taken apart. Only a tab's id names a
/// tab here: a position changes when tabs are moved, and a display name is
/// not unique
pub fn parse(params: &[Value]) -> Result<(String, String, Duration), String> {
    let target = params
        .first()
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("ask_tab needs the tab's id first")?
        .trim()
        // Handed over the way it appears in a message -- `<@codex>` -- or bare
        .trim_start_matches('<')
        .trim_start_matches('@')
        .trim_end_matches('>')
        .to_string();
    let text = params
        .get(1)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("ask_tab needs what to say second")?
        .to_string();
    let wait = params
        .get(2)
        .and_then(|o| o.get("timeout_ms"))
        .and_then(Value::as_u64)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_WAIT)
        .min(LINE_HOLD - Duration::from_secs(60));
    Ok((target, text, wait))
}

/// Words with their runs of space made single, for finding one text in another
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The reply to `sent` in the record at `path`: the last thing the AI said
/// after the turn in which it was given `sent`. `None` while that turn has not
/// been written, or the AI has not said anything since
pub fn reply_in(path: &Path, sent: &str) -> Option<String> {
    let page = crate::reader::read_back(path, u64::MAX, 16).ok()?;
    let want: String = flat(sent).chars().take(MATCH_CHARS).collect();
    let asked = page
        .turns
        .iter()
        .rposition(|t| t.who == Who::You && flat(&t.text).contains(&want))?;
    page.turns[asked + 1..]
        .iter()
        .rev()
        .find(|t| t.who == Who::Ai && !t.text.trim().is_empty())
        .map(|t| t.text.trim().to_string())
}

/// Where a tab's record is, when its profile says how to find one and the
/// conversation it is on is known
pub fn record_of(t: &Tab) -> Option<std::path::PathBuf> {
    let id = t.session.as_ref()?.id.clone();
    let glob = t.resume.as_ref()?.verify.clone()?;
    if t.remote().is_some() || t.cloud().is_some() {
        return None;
    }
    crate::sessionfind::locate(&glob, &id)
}

/// The turn is over and nothing it started is still running. `Background` is
/// not quiet: an AI that put a long command in the background has ended its
/// turn without the answer, and asking it now gets "still waiting". The app's
/// own MCP server in the tab's job is not counted as background work
/// ([`crate::job::is_this_program`]), so an AI with nothing running does read
/// as `Done`
pub fn quiet(state: TabState) -> bool {
    matches!(state, TabState::Done | TabState::Wait)
}

/// The tab's turn is over, whether or not something it started runs on: free
/// to be typed into
pub fn turn_over(state: TabState) -> bool {
    quiet(state) || state == TabState::Background
}

/// The answer sent back to the caller
fn answer(
    a: &Ask,
    state: &str,
    reply: Option<&str>,
    source: &str,
    same_folder: Option<bool>,
    note: Option<&str>,
) -> Value {
    let mut v = json!({
        "tab": a.target,
        "state": state,
        "reply": reply,
        "source": source,
        "round": a.round,
        "max_rounds": if a.max_rounds == 0 { Value::Null } else { json!(a.max_rounds) },
        "seconds": a.asked_at.elapsed().as_secs(),
    });
    if let Some(same) = same_folder {
        v["same_folder"] = json!(same);
    }
    if let Some(n) = note {
        v["note"] = json!(n);
    }
    v
}

/// One tick of one ask, against the tab it went to (`None`: gone) and whether
/// the caller's tab is free to be typed into.
pub fn step(
    a: &mut Ask,
    target: Option<&Tab>,
    caller_free: bool,
    same_folder: Option<bool>,
) -> Step {
    let now = Instant::now();
    if let Phase::Handing(text) = &a.phase {
        return if caller_free {
            Step::Hand(text.clone())
        } else {
            Step::Nothing
        };
    }
    let Some(t) = target else {
        return Step::Answer(answer(
            a,
            "GONE",
            None,
            "none",
            None,
            Some("the tab was closed"),
        ));
    };
    if a.phase == Phase::Queued {
        if matches!(t.state, TabState::Exited) {
            return Step::Answer(answer(
                a,
                "EXIT",
                None,
                "none",
                same_folder,
                Some("the tab's program has ended"),
            ));
        }
        if turn_over(t.state) {
            return Step::Send;
        }
        if now >= a.deadline {
            return Step::Answer(answer(
                a,
                "BUSY",
                None,
                "none",
                same_folder,
                Some("the tab stayed busy; nothing was sent"),
            ));
        }
        return Step::Nothing;
    }
    // Sent: watch it work
    match t.state {
        TabState::Busy => {
            a.seen_busy = true;
            a.quiet_since = None;
            a.background_since = None;
        }
        TabState::Background => {
            a.seen_busy = true;
            a.quiet_since = None;
            let since = *a.background_since.get_or_insert(now);
            if since.elapsed() >= BACKGROUND_GRACE {
                if let Some(reply) = record_of(t).and_then(|path| reply_in(&path, &a.text)) {
                    return Step::Answer(answer(
                        a,
                        "DONE",
                        Some(&reply),
                        "record",
                        same_folder,
                        Some(
                            "the tab still has work running in the background; this is what it has said so far",
                        ),
                    ));
                }
            }
        }
        TabState::Question => {
            // A tab waiting on a person is not going to answer by itself, and
            // holding the line until the deadline would hide that for an hour
            let screen = tail(&t.last_screen, 12);
            return Step::Answer(answer(
                a,
                "QUESTION",
                Some(&screen),
                "screen",
                same_folder,
                Some("the tab is waiting for someone to approve or choose; its screen is in reply"),
            ));
        }
        TabState::Exited => {
            return Step::Answer(answer(
                a,
                "EXIT",
                None,
                "none",
                same_folder,
                Some("the tab's program has ended"),
            ));
        }
        TabState::Limit => {
            return Step::Answer(answer(
                a,
                "LIMIT",
                None,
                "none",
                same_folder,
                Some("the tab hit its usage limit"),
            ));
        }
        TabState::Failed => {
            let screen = tail(&t.last_screen, 12);
            return Step::Answer(answer(
                a,
                "FAILED",
                Some(&screen),
                "screen",
                same_folder,
                Some("the turn ended in an error"),
            ));
        }
        TabState::Done | TabState::Wait => {
            let sent = a.sent_at.unwrap_or(a.asked_at);
            if a.seen_busy || sent.elapsed() >= NEVER_SEEN_BUSY {
                a.quiet_since.get_or_insert(now);
            }
        }
    }
    if let Some(since) = a.quiet_since {
        match record_of(t) {
            Some(path) => {
                if since.elapsed() >= SETTLE {
                    if let Some(reply) = reply_in(&path, &a.text) {
                        return Step::Answer(answer(
                            a,
                            "DONE",
                            Some(&reply),
                            "record",
                            same_folder,
                            None,
                        ));
                    }
                }
            }
            None => {
                if since.elapsed() >= SCREEN_SETTLE {
                    let reply = t
                        .last_response
                        .clone()
                        .unwrap_or_else(|| tail(&t.last_screen, 40));
                    return Step::Answer(answer(
                        a,
                        "DONE",
                        Some(&reply),
                        "screen",
                        same_folder,
                        None,
                    ));
                }
            }
        }
    }
    if a.phase == Phase::Waiting && now >= a.deadline {
        return Step::Answer(answer(
            a,
            "PENDING",
            None,
            "none",
            same_folder,
            Some("still working; its reply will be typed into your tab when it finishes"),
        ));
    }
    Step::Nothing
}

/// The last `n` lines of a screen with the blank ones at the bottom dropped
fn tail(screen: &str, n: usize) -> String {
    let lines: Vec<&str> = screen.lines().collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    lines[end.saturating_sub(n)..end].join("\n")
}

/// What is typed into the caller's tab when the reply outlived the line
pub fn handed(target: &str, reply: &str) -> String {
    format!("[Reply from <@{target}> to your earlier ask_tab]\n{reply}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    struct Record(std::path::PathBuf);
    impl Record {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Record {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn record(lines: &[Value]) -> Record {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("asktab-{}-{n}.jsonl", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        Record(path)
    }

    fn said(role: &str, text: &str) -> Value {
        json!({"type": role, "message": {"role": role, "content": [{"type": "text", "text": text}]}})
    }

    #[test]
    fn the_reply_is_what_followed_the_question_and_nothing_before_it() {
        let f = record(&[
            said("user", "an older question"),
            said("assistant", "an older answer"),
            said(
                "user",
                "What is in secret.txt?  Reply with only its contents.",
            ),
            said("assistant", "Let me look."),
            said("assistant", "ALPHA-42"),
        ]);
        let got = reply_in(
            f.path(),
            "What is in secret.txt? Reply with only its contents.",
        );
        // One turn of the AI, read as the reader reads it: its paragraphs together
        assert_eq!(
            got.as_deref(),
            Some(
                "Let me look.

ALPHA-42"
            )
        );
    }

    #[test]
    fn no_reply_until_the_question_is_in_the_record() {
        let f = record(&[
            said("user", "an older question"),
            said("assistant", "an older answer"),
        ]);
        assert_eq!(reply_in(f.path(), "a new question"), None);
        let f = record(&[said("user", "a new question")]);
        assert_eq!(reply_in(f.path(), "a new question"), None);
    }

    #[test]
    fn a_tab_is_named_by_its_id_as_a_mention_or_bare() {
        let (t, x, w) = parse(&[json!("<@codex>"), json!("hi")]).unwrap();
        assert_eq!(t, "codex");
        let (t, x, w) = parse(&[json!("@codex"), json!("hi")]).unwrap();
        assert_eq!((t.as_str(), x.as_str(), w), ("codex", "hi", DEFAULT_WAIT));
        let (_, _, w) = parse(&[json!("codex"), json!("hi"), json!({"timeout_ms": 5000})]).unwrap();
        assert_eq!(w, Duration::from_secs(5));
        assert!(parse(&[json!("codex")]).is_err());
    }
}

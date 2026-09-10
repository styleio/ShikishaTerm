//! Handing a paste to a tab, one chunk at a time.
//!
//! Nothing here draws anything: it is about what a recipient can swallow and
//! when the Enter that finishes a paste is safe to press. It lived in the
//! window's file until the runtime became a crate of its own, and the tab tests
//! had always treated it as the runtime's.

use crate::tab::Tab;

/// Minimum gap between sending the text body and sending submit (Enter).
/// How long the recipient actually takes to process it depends on device and load,
/// so this is only a floor.
pub const SUBMIT_FLOOR_MS: u64 = 100;

/// The cap on how long to wait before sending submit anyway, when the recipient
/// keeps responding and never settles
pub const SUBMIT_GIVE_UP_MS: u64 = 8_000;

/// The no-output duration after which paste intake is considered finished.
///
/// This waits for "finished", not "started responding". A long paste keeps
/// redrawing over several round trips, so sending Enter as soon as it starts
/// would arrive mid-intake and get dropped.
/// (measured: around 600 chars goes through fine, around 1900 chars fails)
pub const SUBMIT_QUIET_MS: u64 = 400;

/// How much of a paste to hand over at a time.
///
/// The recipient takes a paste in one character at a time — on Windows the
/// console turns the stream into individual key events — and it is far slower at
/// that than we are at writing. Give it the whole thing in one go and it spends
/// seconds working through a backlog nothing on our side can see.
pub const PASTE_CHUNK: usize = 1024;

/// How long to give the recipient to draw something before handing over the next
/// chunk anyway. Drawing is the only way it has of saying "I've caught up";
/// the timeout is for a recipient that draws nothing at all.
pub const PASTE_ACK_MS: u64 = 600;

/// What to do with a send on this pass.
pub enum Step {
    /// Nothing yet: the recipient hasn't caught up, or hasn't settled
    Wait,
    /// Hand over this much more of the paste
    Hand(Vec<u8>),
    /// The body is all in. Press Enter (or, for a draft, stop here)
    Submit { settled: bool },
}

/// A paste on its way to a tab, and the Enter that finishes it.
///
/// Both halves are here because they are one problem. The recipient reads the
/// paste one character at a time and falls behind; the Enter we write next joins
/// the same queue, so it is taken *inside* the paste, where it counts as a
/// newline and not as "send this" — the text sits in the input box, unsent,
/// until the next thing typed carries it in. Waiting longer before pressing
/// Enter cannot fix that: the wait is on our clock, and the queue is on theirs.
///
/// So the paste is handed over a chunk at a time, and the next chunk only goes
/// out once the recipient has drawn something (= caught up). Nothing here is a
/// guess about how fast the recipient is; it sets its own pace, and by the time
/// the last chunk is out it is at most one chunk behind.
///
/// Measured against a real Codex CLI on Windows: 20,000 characters written in
/// one go left the whole thing in the input box (it drew *nothing at all* for
/// two seconds mid-intake, so "output has stopped" looked exactly like
/// "finished"). Handed over in chunks, the same text sends.
pub struct PendingSend {
    pub tab: usize,
    /// The paste, already encoded for the recipient, split at character
    /// boundaries. Split before encoding so no character is ever cut in half.
    chunks: Vec<Vec<u8>>,
    /// How many chunks have gone out
    pub handed: usize,
    /// Whether to press Enter at the end. A draft is placed for a person to
    /// finish, so it stops with the text in the box.
    pub submit: bool,
    /// The cumulative output amount last seen. A change means the recipient drew.
    seen: u64,
    /// When the last chunk was handed over
    handed_ms: u64,
    /// The point output stopped (None = hasn't stopped yet)
    quiet_since: Option<u64>,
    /// The earliest time submission is allowed, to prevent sending too early
    not_before: u64,
    /// The time to give up and send anyway, if things never settle
    give_up: u64,
}

/// The paste to hand a tab, cut into chunks small enough for it to swallow one
/// at a time. Cut before encoding, so a character never straddles two writes.
///
/// One place builds this, for every door that pastes into a tab: a person's
/// line, an automated hand-off, and a draft left for someone to finish.
pub fn paste_chunks(t: &Tab, text: &str) -> Vec<Vec<u8>> {
    let bracketed = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste();
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    // If bracketed paste is supported, multi-line text still arrives as a single input
    let payload = if bracketed {
        format!("\x1b[200~{body}\x1b[201~")
    } else {
        body
    };
    let mut chunks = Vec::new();
    let mut rest = payload.as_str();
    while !rest.is_empty() {
        let mut cut = PASTE_CHUNK.min(rest.len());
        while !rest.is_char_boundary(cut) {
            cut += 1;
        }
        let (head, tail) = rest.split_at(cut);
        chunks.push(t.encode_out(head));
        rest = tail;
    }
    chunks
}

impl PendingSend {
    pub fn new(tab: usize, chunks: Vec<Vec<u8>>, submit: bool, seen: u64, now_ms: u64) -> Self {
        Self {
            tab,
            chunks,
            handed: 0,
            submit,
            seen,
            handed_ms: now_ms,
            quiet_since: None,
            not_before: now_ms + SUBMIT_FLOOR_MS,
            give_up: now_ms + SUBMIT_GIVE_UP_MS,
        }
    }

    /// Everything still owed, handed over at once.
    ///
    /// For when something else is about to write to the same tab. A paste that
    /// goes over in pieces owns that tab until the last piece is in: a
    /// keystroke, or another message, arriving in the gaps is typed into the
    /// middle of somebody's sentence. The pacing is what gets given up here,
    /// never the order.
    pub fn rest(&mut self, now_ms: u64) -> Vec<u8> {
        if self.handed >= self.chunks.len() {
            return Vec::new();
        }
        let out = self.chunks[self.handed..].concat();
        self.handed = self.chunks.len();
        self.quiet_since = None;
        self.not_before = now_ms + SUBMIT_FLOOR_MS;
        self.give_up = now_ms + SUBMIT_GIVE_UP_MS;
        out
    }

    /// The next thing to do for this send.
    ///
    /// While the body is going out, what we wait on is the recipient drawing.
    /// Once it is all out, what we wait on is the drawing *stopping* — the same
    /// "it has taken it in" signal as before, which is now trustworthy because
    /// the recipient was never allowed to fall behind.
    pub fn step(&mut self, output_count: u64, now_ms: u64) -> Step {
        if self.handed < self.chunks.len() {
            let drew = output_count != self.seen;
            if !drew && now_ms.saturating_sub(self.handed_ms) < PASTE_ACK_MS && self.handed > 0 {
                return Step::Wait;
            }
            let chunk = self.chunks[self.handed].clone();
            self.handed += 1;
            self.seen = output_count;
            self.handed_ms = now_ms;
            // The clock for "has it settled?" starts when the last chunk is out
            if self.handed == self.chunks.len() {
                self.quiet_since = None;
                self.not_before = now_ms + SUBMIT_FLOOR_MS;
                self.give_up = now_ms + SUBMIT_GIVE_UP_MS;
            }
            return Step::Hand(chunk);
        }
        if output_count != self.seen {
            // Still mid-intake. Restart the measurement from when it stops.
            self.seen = output_count;
            self.quiet_since = None;
        } else if self.quiet_since.is_none() {
            self.quiet_since = Some(now_ms);
        }
        if now_ms < self.not_before {
            return Step::Wait;
        }
        let settled = self
            .quiet_since
            .is_some_and(|q| now_ms.saturating_sub(q) >= SUBMIT_QUIET_MS);
        if settled || now_ms >= self.give_up {
            Step::Submit { settled }
        } else {
            Step::Wait
        }
    }
}

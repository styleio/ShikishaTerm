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

/// How long after its Enter an AI tab gets to show it has started before
/// the Enter is looked at again. A real turn shows "working" within about a
/// second (Codex CLI, measured: 0.85s).
pub const LANDING_LOOK_MS: u64 = 2_500;

/// How still the screen has to be before a second Enter goes in. A tab that
/// is still drawing is still doing something with what it was given.
pub const LANDING_QUIET_MS: u64 = 1_000;

/// How many more times Enter is pressed for one message
pub const LANDING_TRIES: u8 = 2;

/// After this long, stop watching whether the Enter landed
pub const LANDING_GIVE_UP_MS: u64 = 20_000;

/// How much of the end of a message is looked for on screen
const TAIL_CHARS: usize = 24;

/// What to do with a send on this pass.
pub enum Step {
    /// Nothing yet: the recipient hasn't caught up, or hasn't settled
    Wait,
    /// Hand over this much more of the paste
    Hand(Vec<u8>),
    /// The body is all in. Press Enter (or, for a draft, stop here)
    Submit { settled: bool },
}

/// What to do about an Enter that may not have landed.
#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    /// Nothing yet: too early to tell
    Wait,
    /// Stop watching: it started, someone else typed, or it cannot be told
    Done,
    /// The message is still in the input box and nothing started: press Enter
    Again,
    /// Still in the input box after every try. Stop pressing and say so
    Stuck,
}

/// What a tab looks like on this pass, for `Landing::check`
pub struct Look<'a> {
    /// The tab shows it started on what it was given (`Tab::answered_since_submit`)
    pub started: bool,
    /// `Tab::typed_in` now
    pub typed_in: u64,
    /// Another paste to this tab is on its way
    pub sending: bool,
    /// How long the screen has not changed (`Tab::ms_since_change`)
    pub still_ms: u64,
    /// The screen as it stands (`Tab::last_screen`)
    pub screen: &'a str,
    /// The screen shows a paste folded up in the input box, where a long
    /// message is not in its own words (`Tab::shows_pasted`)
    pub pasted: bool,
}

/// An Enter that has gone into an AI tab, watched until the turn it asked for
/// shows.
///
/// The Enter can be taken *inside* the paste, as a newline, and leave the
/// whole message in the input box: `PendingSend` reads "the recipient has
/// caught up" off its drawing, and a recipient that is slow to take a paste
/// in (a Codex CLI with a long conversation behind it) can stop drawing
/// before it has read all of it. On 2026-10-30 two requests to a reviewing
/// Codex sat that way for seven hours, the second one joining the first in
/// the same box, until a person pressed Enter.
///
/// No wait before the Enter can be long enough, because how far behind the
/// recipient is cannot be seen from here. What can be seen is the result:
/// a tab that shows nothing started, with the end of the message still on
/// its screen (or the paste folded up in its input box), did not take the
/// Enter. One more Enter then sends what is in
/// the box. An Enter on an empty input box does nothing in an AI CLI, so a
/// wrong guess costs nothing.
pub struct Landing {
    /// Which tab (`Tab::serial`)
    pub serial: u64,
    /// The end of the message, without white space, as the screen would show it
    tail: String,
    /// When Enter last went in
    pressed_ms: u64,
    /// `Tab::typed_in` right after that Enter. Anything typed since, by a
    /// person or another message, means the box is no longer ours to send
    typed: u64,
    /// How many more times Enter has been pressed
    pub again: u8,
}

/// The last few characters of what was said, white space taken out: the
/// part of a message that is still on screen when it sits in an input box,
/// whichever way the box wrapped it. None when there is nothing to look for
pub fn landing_tail(text: &str) -> Option<String> {
    let flat: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if flat.is_empty() {
        return None;
    }
    Some(flat[flat.len().saturating_sub(TAIL_CHARS)..].iter().collect())
}

/// Whether the screen shows that tail, across any line breaks the box made
pub fn screen_holds(screen: &str, tail: &str) -> bool {
    let flat: String = screen.chars().filter(|c| !c.is_whitespace()).collect();
    flat.contains(tail)
}

impl Landing {
    pub fn new(serial: u64, tail: String, typed: u64, now_ms: u64) -> Self {
        Self { serial, tail, pressed_ms: now_ms, typed, again: 0 }
    }

    /// The next thing to do about this Enter
    pub fn check(&mut self, look: &Look, now_ms: u64) -> Check {
        if look.started || look.sending || look.typed_in != self.typed {
            return Check::Done;
        }
        let since = now_ms.saturating_sub(self.pressed_ms);
        if since < LANDING_LOOK_MS {
            return Check::Wait;
        }
        if look.still_ms < LANDING_QUIET_MS {
            return if since >= LANDING_GIVE_UP_MS { Check::Done } else { Check::Wait };
        }
        if !look.pasted && !screen_holds(look.screen, &self.tail) {
            return Check::Done;
        }
        if self.again >= LANDING_TRIES {
            return Check::Stuck;
        }
        Check::Again
    }

    /// Enter has gone in again; `typed` is `Tab::typed_in` right after it
    pub fn pressed(&mut self, typed: u64, now_ms: u64) {
        self.again += 1;
        self.typed = typed;
        self.pressed_ms = now_ms;
    }

    /// How long it has been since the last Enter
    pub fn since_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.pressed_ms)
    }
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
    /// Where the tab sits on the desk in front, for the screen: 0 while it is
    /// on another desk. Found again from `serial` every pass (`follow`)
    pub tab: usize,
    /// Which tab it is going to (`Tab::serial`). A place on the desk is not
    /// that: a desk switched mid-paste puts another desk's tabs at the same
    /// places, and the rest of the text, and its Enter, went into one of them
    pub serial: u64,
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
    /// How many characters the person wrote. Kept only so the screen can say
    /// so while it goes over: a long paste is handed across in pieces and
    /// takes seconds, and a composer that empties with nothing else happening
    /// reads as a message that was lost.
    chars: usize,
    /// The end of the message, for seeing whether its Enter landed (`Landing`)
    pub tail: Option<String>,
    /// When the last chunk was handed over
    handed_ms: u64,
    /// The point output stopped (None = hasn't stopped yet)
    quiet_since: Option<u64>,
    /// The earliest time submission is allowed, to prevent sending too early
    not_before: u64,
    /// The time to give up and send anyway, if things never settle
    give_up: u64,
    /// The tab (by uid) this tells of its mail, for orchestration to hear
    /// whether it went in (`Orchestra::typed`)
    pub tells: Option<String>,
    /// A piece of it, or its Enter, could not be written
    pub went_wrong: bool,
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
    /// `text` is what the chunks were cut from: its length in characters is
    /// the number a person would recognise, not the bytes it became on the
    /// way out, and its end is what shows whether the Enter landed.
    pub fn new(tab: usize, serial: u64, chunks: Vec<Vec<u8>>, submit: bool, seen: u64, now_ms: u64, text: &str) -> Self {
        Self {
            tab,
            serial,
            chunks,
            handed: 0,
            submit,
            seen,
            chars: text.chars().count(),
            tail: if submit { landing_tail(text) } else { None },
            handed_ms: now_ms,
            quiet_since: None,
            not_before: now_ms + SUBMIT_FLOOR_MS,
            give_up: now_ms + SUBMIT_GIVE_UP_MS,
            tells: None,
            went_wrong: false,
        }
    }

    /// How far the text has got, 0..=1, and how long the whole of it is.
    ///
    /// Both numbers, because one on its own says nothing worth reading: a bar
    /// at a third with "34,765 characters" beside it is a wait somebody can
    /// sit through, and "33%" alone is a riddle.
    pub fn sending(&self) -> (f32, usize) {
        let share = match self.chunks.len() {
            0 => 1.0,
            n => self.handed as f32 / n as f32,
        };
        (share, self.chars)
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

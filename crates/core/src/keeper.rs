//! Keeping a window over a runtime that has none of its own.
//!
//! Split in two, the runtime starts one window and then watches it. While it
//! is there nothing happens here. When it stops being there, the first thing
//! to work out is which of two entirely different things happened:
//!
//! * **the person pressed ✕** -- an ordinary end to looking at it. Whether the
//!   work behind it should stop as well is the `resident` setting's business
//!   and always has been, so this says only that the ✕ was pressed and the
//!   runtime reads the setting exactly where it read it when there was one
//!   process (`runtime::run`, beside `close_requested`).
//! * **the window died** -- killed for its memory, or gone in a crash. This is
//!   the case the split exists for. Nobody asked for it at any setting, so the
//!   ✕'s meaning does not apply and the runtime puts the window back instead.
//!
//! Told apart by the way the window left, not guessed at afterwards: a window
//! that reaches the end of its own run leaves quietly and with nothing to
//! report, and a window that is killed cannot. See [`Parting::of`].
//!
//! Whether to put one back is [`crate::revive`]'s decision, borrowed whole: a
//! screen dying for want of memory must not be rebuilt over and over into a
//! machine that has none. What is here is only the bookkeeping between the two
//! -- what has been tried, and when to look again -- so that the part of the
//! program holding the actual process can be a loop with no memory of its own.
//!
//! Deliberately ignorant of processes, windows and clocks: it is handed the
//! code a window ended with and the moment it happened, and answers with one
//! word. That is also the only way the ten-minute restraint can be tested
//! without waiting ten minutes.

use crate::revive::{self, Next};

/// How the window stopped being there
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parting {
    /// It was closed. The person pressed ✕, and the window left the way a
    /// program leaves when it is finished
    Pressed,
    /// It stopped without leaving: killed, or dead in a crash. Nobody asked
    /// for this
    Vanished,
}

impl Parting {
    /// Read from how the window's process ended.
    ///
    /// `Some(0)` is the one quiet ending: the window's own run returned, which
    /// happens when its ✕ was pressed and nothing else. Every other code is
    /// something that went wrong -- a failure on the way up (1), the abort
    /// Windows reports when an allocation cannot be met (0xC0000409), an
    /// access violation, a kill from outside -- and none of those is a request
    /// to stop working. `None` is a process whose code could not be read at
    /// all, which is the same kind of not-knowing and is treated the same way.
    ///
    /// A window ended from the Task Manager therefore counts as vanished, and
    /// is put back once or twice before the runtime gives up and offers the
    /// icon instead. That is the right way round: of the two mistakes
    /// available, putting back a window somebody wanted gone costs a press,
    /// and leaving a machine with no screen and a day's work behind it does
    /// not.
    pub fn of(code: Option<i32>) -> Self {
        match code {
            Some(0) => Self::Pressed,
            _ => Self::Vanished,
        }
    }
}

/// What the runtime should do about its window
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Do {
    /// Nothing, for now. Either the window is there, or none is owed yet
    Nothing,
    /// It was closed on purpose. Say so the way a window says it, and let the
    /// setting decide whether the work stops with it
    Closed,
    /// Start a window
    Open,
    /// Start one, but not this instant: look again in this many milliseconds
    Later(u64),
    /// Stop trying, and say so. The icon is the way back, and pressing it is
    /// not a retry -- it is a person who can see the machine deciding
    Offer,
}

/// The ending Windows reports for a process that aborted itself.
///
/// `STATUS_STACK_BUFFER_OVERRUN`, despite the name: it is what the runtime
/// raises for any abort, and a Rust program aborts when an allocation cannot
/// be met. On a machine that has run out of memory this is the code the window
/// leaves behind, and it is the one case where putting the window straight
/// back is the worst available move -- rebuilding a browser costs more memory
/// than the one that just died gave back.
///
/// Not proof of anything: an abort has other causes. But of the two ways to be
/// wrong, waiting to be asked costs a press, and rebuilding costs the machine
/// the memory it is already short of.
const ABORTED: i32 = 0xC000_0409u32 as i32;

/// Whether the way this window ended could have been the machine running out
/// of memory
fn out_of_memory(code: Option<i32>) -> bool {
    code == Some(ABORTED)
}

/// What has been tried, and when to look again
#[derive(Debug, Default)]
pub struct Keeper {
    /// When each window was started, on a clock that only goes forwards
    tried: Vec<u64>,
    /// The moment to start one, when one is owed
    wake: Option<u64>,
}

impl Keeper {
    pub fn new() -> Self {
        Self::default()
    }

    /// The window is gone. `code` is what its process ended with, and `now`
    /// the moment, in milliseconds
    pub fn parted(&mut self, code: Option<i32>, now: u64) -> Do {
        self.wake = None;
        if Parting::of(code) == Parting::Pressed {
            return Do::Closed;
        }
        match revive::decide(out_of_memory(code), &self.tried, now) {
            Next::Now => Do::Open,
            Next::Wait(ms) => {
                self.wake = Some(now.saturating_add(ms));
                Do::Later(ms)
            }
            Next::Ask => Do::Offer,
        }
    }

    /// Time has passed with no window. Answers `Open` once the wait asked for
    /// by an earlier `Later` is over, and `Nothing` at every other moment
    pub fn tick(&mut self, now: u64) -> Do {
        match self.wake {
            Some(at) if now >= at => {
                self.wake = None;
                Do::Open
            }
            _ => Do::Nothing,
        }
    }

    /// Somebody pressed the icon. A person asking for their screen back is not
    /// the program retrying, so what has been tried is forgotten and a window
    /// is started -- however many times they press
    pub fn asked(&mut self) -> Do {
        self.tried.clear();
        self.wake = None;
        Do::Open
    }

    /// A window was started. Counted here rather than where `Open` was
    /// answered, so that an attempt which failed to start still counts as one
    pub fn opened(&mut self, now: u64) {
        self.tried = revive::recent(&self.tried, now);
        self.tried.push(now);
        self.wake = None;
    }

    /// Whether a window is owed at some later moment. What a loop with nothing
    /// else to do sleeps until
    pub fn owed(&self) -> Option<u64> {
        self.wake
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ✕ is handed on as a ✕ and nothing more. What it costs -- the work
    /// stopping, or the program waiting in the notification area -- is the
    /// setting's answer, and the runtime is where that answer already lives
    #[test]
    fn a_closed_window_is_reported_and_not_ruled_on() {
        let mut k = Keeper::new();
        assert_eq!(k.parted(Some(0), 1_000), Do::Closed);
        assert_eq!(k.tick(9_999_999), Do::Nothing, "it opened a window nobody asked for");
        // ...and the icon is still the way back to one
        assert_eq!(k.asked(), Do::Open);
    }

    /// The case the split exists for: the screen is taken and the work is not.
    /// Never reported as a closing, because a ✕ is the one thing that did not
    /// happen -- read as one, with the setting turned off, it would hand the
    /// machine's owner exactly the loss the split was made to prevent
    #[test]
    fn a_window_that_was_taken_is_put_back_and_never_called_a_closing() {
        for code in [Some(1), Some(0xC000_0005u32 as i32), Some(137), None] {
            let mut k = Keeper::new();
            assert_eq!(k.parted(code, 1_000), Do::Open, "code {code:?}");
        }
    }

    /// Twice in ten minutes is a bad moment; three times is something
    /// rebuilding does not fix, and then the person decides
    #[test]
    fn it_stops_trying_before_it_becomes_the_problem() {
        let mut k = Keeper::new();
        assert_eq!(k.parted(Some(1), 1_000), Do::Open);
        k.opened(1_000);
        assert_eq!(k.parted(Some(1), 2_000), Do::Later(revive::BREATH_MS));
        assert_eq!(k.owed(), Some(2_000 + revive::BREATH_MS));
        // Nothing happens until the breath is over, and then exactly once
        assert_eq!(k.tick(2_000 + revive::BREATH_MS - 1), Do::Nothing);
        assert_eq!(k.tick(2_000 + revive::BREATH_MS), Do::Open);
        assert_eq!(k.tick(2_000 + revive::BREATH_MS), Do::Nothing, "it opened twice for one wait");
        k.opened(7_000);
        assert_eq!(k.parted(Some(1), 8_000), Do::Offer);
        // ...and a press gets a window anyway, because a person can see the
        // machine and this cannot
        assert_eq!(k.asked(), Do::Open);
    }

    /// A machine with no memory left must not be answered with a browser
    #[test]
    fn a_window_that_aborted_is_never_rebuilt_unasked() {
        let mut k = Keeper::new();
        assert_eq!(k.parted(Some(ABORTED), 1_000), Do::Offer);
        assert_eq!(k.owed(), None, "it left itself a reminder to try anyway");
    }

    /// Ten quiet minutes later it is a new day
    #[test]
    fn a_bad_afternoon_is_not_held_against_the_evening() {
        let mut k = Keeper::new();
        k.opened(1_000);
        k.opened(2_000);
        let later = 2_000 + revive::WITHIN_MS + 1;
        assert_eq!(k.parted(Some(1), later), Do::Open);
    }

    /// Closing a window that was already being waited on takes the wait back
    /// with it: what was owed was a screen for somebody who has now gone
    #[test]
    fn a_closing_takes_back_a_wait_that_was_owed() {
        let mut k = Keeper::new();
        k.opened(1_000);
        assert_eq!(k.parted(Some(1), 2_000), Do::Later(revive::BREATH_MS));
        assert_eq!(k.parted(Some(0), 2_500), Do::Closed);
        assert_eq!(k.owed(), None);
        assert_eq!(k.tick(9_999_999), Do::Nothing);
    }
}

//! Keeping a window over a runtime that has none of its own.
//!
//! The runtime starts one window and then watches it. While it is there
//! nothing happens here. When it stops being there, two questions are asked
//! in order, and each already has a module: [`crate::goodbye`] says whether
//! the window was meant to go, and [`crate::revive`] says whether putting it
//! back now would help or would make a bad moment worse.
//!
//! This is the little bit of bookkeeping between them -- what has been tried,
//! and when to look again -- so that the part of the program holding the
//! actual process can be a loop with no memory of its own.
//!
//! Deliberately ignorant of processes, windows and clocks: it is handed the
//! code a window ended with and the moment it happened, and answers with one
//! word. Everything that can only be done on a real machine is done by the
//! caller, which is also the only way the ten-minute restraint can be tested
//! without waiting ten minutes.

use crate::goodbye::{Parting, Then};
use crate::revive::{self, Next};

/// What the runtime should do about its window
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Do {
    /// Nothing, for now. Either the window is there, or it was closed on
    /// purpose and the notification area is where the program waits
    Rest,
    /// Start a window
    Open,
    /// Start one, but not this instant: look again in this many milliseconds
    Later(u64),
    /// Stop trying and say so. The icon is the way back, and pressing it is
    /// not a retry -- it is a person who can see the machine deciding
    Offer,
    /// End the runtime. The window was closed and it was not asked to stay
    Stop,
}

/// The ending Windows reports for a process that aborted itself.
///
/// `STATUS_STACK_BUFFER_OVERRUN`, despite the name: it is what the runtime
/// raises for any abort, and a Rust program aborts when an allocation cannot
/// be met. On a machine that has run out of memory this is the code the
/// window leaves behind, and it is the one case where putting the window
/// straight back is the worst available move -- rebuilding a browser costs
/// more memory than the one that just died gave back.
///
/// Not proof of anything: an abort has other causes. But of the two ways to
/// be wrong, waiting to be asked costs a press, and rebuilding costs the
/// machine the memory it is already short of.
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

    /// The window is gone. `code` is what its process ended with, `resident`
    /// the setting as it reads right now, and `now` the moment in
    /// milliseconds
    pub fn parted(&mut self, code: Option<i32>, resident: bool, now: u64) -> Do {
        self.wake = None;
        match crate::goodbye::decide(Parting::of(code), resident) {
            Then::Wait => Do::Rest,
            Then::Stop => Do::Stop,
            Then::Rescue => match revive::decide(out_of_memory(code), &self.tried, now) {
                Next::Now => Do::Open,
                Next::Wait(ms) => {
                    self.wake = Some(now.saturating_add(ms));
                    Do::Later(ms)
                }
                Next::Ask => Do::Offer,
            },
        }
    }

    /// Time has passed with no window. Answers `Open` once the wait asked for
    /// by an earlier `Later` is over, and `Rest` at every other moment
    pub fn tick(&mut self, now: u64) -> Do {
        match self.wake {
            Some(at) if now >= at => {
                self.wake = None;
                Do::Open
            }
            _ => Do::Rest,
        }
    }

    /// Somebody pressed the icon. A person asking for their screen back is
    /// not the program retrying, so what has been tried is forgotten and the
    /// window is started -- however many times they press
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

    /// Whether a window is owed at some later moment. What a loop with
    /// nothing else to do sleeps until
    pub fn owed(&self) -> Option<u64> {
        self.wake
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ordinary evening: the window is closed, the setting says the work
    /// carries on, and the runtime sits in the notification area until it is
    /// asked for again
    #[test]
    fn a_closed_window_is_waited_out_and_reopened_on_request() {
        let mut k = Keeper::new();
        assert_eq!(k.parted(Some(0), true, 1_000), Do::Rest);
        assert_eq!(k.tick(9_999_999), Do::Rest, "it opened a window nobody asked for");
        assert_eq!(k.asked(), Do::Open);
    }

    /// The same evening with the setting the other way
    #[test]
    fn a_closed_window_ends_the_runtime_when_nothing_was_asked_to_stay() {
        let mut k = Keeper::new();
        assert_eq!(k.parted(Some(0), false, 1_000), Do::Stop);
    }

    /// The case the split exists for: the screen is taken and the work is not
    #[test]
    fn a_window_that_died_is_put_back_at_once() {
        let mut k = Keeper::new();
        // An access violation: gone, and not by anybody's request
        assert_eq!(k.parted(Some(0xC000_0005u32 as i32), true, 1_000), Do::Open);
        k.opened(1_100);
        // ...and with the setting off as well, because "stop when the window
        // closes" is an answer about a ✕
        let mut k = Keeper::new();
        assert_eq!(k.parted(Some(1), false, 1_000), Do::Open);
    }

    /// Twice in ten minutes is a bad moment; three times is something
    /// rebuilding does not fix, and then the person decides
    #[test]
    fn it_stops_trying_before_it_becomes_the_problem() {
        let mut k = Keeper::new();
        assert_eq!(k.parted(Some(1), true, 1_000), Do::Open);
        k.opened(1_000);
        assert_eq!(k.parted(Some(1), true, 2_000), Do::Later(revive::BREATH_MS));
        assert_eq!(k.owed(), Some(2_000 + revive::BREATH_MS));
        // Nothing happens until the breath is over, and then exactly once
        assert_eq!(k.tick(2_000 + revive::BREATH_MS - 1), Do::Rest);
        assert_eq!(k.tick(2_000 + revive::BREATH_MS), Do::Open);
        assert_eq!(k.tick(2_000 + revive::BREATH_MS), Do::Rest, "it opened twice for one wait");
        k.opened(7_000);
        assert_eq!(k.parted(Some(1), true, 8_000), Do::Offer);
        // ...and a press gets a window anyway, because a person can see the
        // machine and this cannot
        assert_eq!(k.asked(), Do::Open);
    }

    /// A machine with no memory left must not be answered with a browser
    #[test]
    fn a_window_that_aborted_is_never_rebuilt_unasked() {
        let mut k = Keeper::new();
        assert_eq!(k.parted(Some(ABORTED), true, 1_000), Do::Offer);
        assert_eq!(k.owed(), None, "it left itself a reminder to try anyway");
        // Off as well: the reason not to rebuild has nothing to do with the
        // setting
        let mut k = Keeper::new();
        assert_eq!(k.parted(Some(ABORTED), false, 1_000), Do::Offer);
    }

    /// Ten quiet minutes later it is a new day
    #[test]
    fn a_bad_afternoon_is_not_held_against_the_evening() {
        let mut k = Keeper::new();
        k.opened(1_000);
        k.opened(2_000);
        let later = 2_000 + revive::WITHIN_MS + 1;
        assert_eq!(k.parted(Some(1), true, later), Do::Open);
    }

    /// Closing a window that was already being waited on takes the wait back
    /// with it: what was owed was a screen for somebody who has now gone
    #[test]
    fn a_closing_takes_back_a_wait_that_was_owed() {
        let mut k = Keeper::new();
        k.opened(1_000);
        assert_eq!(k.parted(Some(1), true, 2_000), Do::Later(revive::BREATH_MS));
        assert_eq!(k.parted(Some(0), true, 2_500), Do::Rest);
        assert_eq!(k.owed(), None);
        assert_eq!(k.tick(9_999_999), Do::Rest);
    }
}

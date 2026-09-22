//! What the ✕ means once the window and the runtime are two programs.
//!
//! In one process the question never came up: closing the window either put
//! the program away in the notification area or ended it, and the setting for
//! that (`resident`) decided which. Split in two, the same ✕ has to be read
//! twice -- once by the window, which simply goes, and once by the runtime,
//! which has to work out whether it was meant to go as well.
//!
//! The runtime cannot ask. All it sees is that the window it started is no
//! longer there, and that can mean two entirely different things:
//!
//! * **the person pressed ✕** -- an ordinary end to looking at it. Whether
//!   the work behind it should stop too is exactly what `resident` says, so
//!   that setting goes on meaning what it always meant.
//! * **the window died** -- killed for its memory, or gone in a crash. This
//!   is the case the split exists for: the work must NOT stop, because
//!   nobody asked it to, and the runtime puts the window back instead.
//!
//! Told apart by the way the window left, not by guessing afterwards: a
//! window that reaches the end of its own run leaves quietly and with
//! nothing to report, and anything else -- a kill, an abort, a failure on
//! the way up -- cannot. See [`Parting::of`].
//!
//! Putting it back is [`crate::revive`]'s decision, not this module's, and
//! for the same reason: a screen that is being killed for its memory must not
//! be rebuilt over and over into a machine that has none. This says only
//! whether putting it back is wanted at all.

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
    /// something that went wrong -- an error on the way up (1), the abort
    /// Windows reports when an allocation fails (0xC0000409), an access
    /// violation, a kill from outside -- and none of those is a request to
    /// stop working. `None` is a process whose code could not be read at all,
    /// which is the same kind of not-knowing and is treated the same way.
    ///
    /// A window ended from the Task Manager therefore counts as vanished and
    /// is put back once or twice before the runtime gives up and offers the
    /// icon instead. That is the right way round: of the two mistakes
    /// available, putting a window back that somebody wanted gone is a
    /// nuisance, and leaving a machine with no screen and a day's work behind
    /// it is not.
    pub fn of(code: Option<i32>) -> Self {
        match code {
            Some(0) => Self::Pressed,
            _ => Self::Vanished,
        }
    }
}

/// What the runtime does about it
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    /// Go on working with no screen, and wait in the notification area. The
    /// icon is how the window is asked for again, and how the program is
    /// ended
    Wait,
    /// Stop. Everything the window was showing stops with it
    Stop,
    /// Put the window back, as far as [`crate::revive`] allows
    Rescue,
}

/// What a runtime does when the window it started is gone.
///
/// `resident` is the setting as it reads at this moment, not as it read when
/// the window opened: somebody who turns "keep working in the background" off
/// and then closes the window means the one they turned off last.
pub fn decide(parting: Parting, resident: bool) -> Then {
    match parting {
        // Asked for. The setting says what "asked for" is worth
        Parting::Pressed if resident => Then::Wait,
        Parting::Pressed => Then::Stop,
        // Not asked for, by anybody, at any setting. This is the whole reason
        // the two are separate programs: a screen can be taken and the work
        // behind it kept
        Parting::Vanished => Then::Rescue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The setting goes on meaning what it meant when there was one process:
    /// the ✕ either puts the program away or ends it
    #[test]
    fn a_pressed_cross_is_read_the_way_the_setting_says() {
        assert_eq!(decide(Parting::Pressed, true), Then::Wait);
        assert_eq!(decide(Parting::Pressed, false), Then::Stop);
    }

    /// The one the split is for. "Stop when the window closes" is an answer
    /// about a ✕, and a window killed for its memory was never closed -- read
    /// as one, it would hand the machine's owner exactly the loss they split
    /// the program in two to avoid
    #[test]
    fn a_window_that_was_taken_is_put_back_at_either_setting() {
        assert_eq!(decide(Parting::Vanished, true), Then::Rescue);
        assert_eq!(
            decide(Parting::Vanished, false),
            Then::Rescue,
            "turning off 'keep working in the background' threw away the work an OOM took the screen from"
        );
    }

    /// Nothing but a run that finished counts as a closing
    #[test]
    fn only_a_quiet_ending_counts_as_a_closing() {
        assert_eq!(Parting::of(Some(0)), Parting::Pressed);
        // What Windows reports for a Rust process that aborted, which is what
        // an allocation that cannot be met does
        assert_eq!(Parting::of(Some(0xC000_0409u32 as i32)), Parting::Vanished);
        // An access violation, and the code a failure on the way up returns
        assert_eq!(Parting::of(Some(0xC000_0005u32 as i32)), Parting::Vanished);
        assert_eq!(Parting::of(Some(1)), Parting::Vanished);
        // Killed from outside: `TerminateProcess` hands over whatever the
        // killer chose, and nothing chosen there is a request to stop working
        assert_eq!(Parting::of(Some(137)), Parting::Vanished);
        assert_eq!(Parting::of(None), Parting::Vanished);
    }
}

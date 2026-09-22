//! When the screen dies, whether to bring it back now or wait to be asked.
//!
//! The display is a browser of its own, in its own processes, and it can die
//! without taking the program with it -- the tabs keep running, the agents
//! keep working, and what is lost is the view of them. Bringing it back is
//! usually right and takes a moment.
//!
//! Usually. A display that died because the machine had no memory left will
//! take more memory to rebuild than it just freed, and a program that keeps
//! rebuilding it is a program making a bad situation worse on a timer. So the
//! reason is asked first, and the answer is allowed to be "not now".
//!
//! The numbers here are deliberately small. A browser does not silently
//! reload a tab that crashed either -- it shows the sad face and waits to be
//! told. One attempt covers the ordinary case, where a single renderer fell
//! over and everything else is fine. A second covers a bad moment. A third
//! has never yet been the one that worked, and by then each attempt is
//! costing a fresh set of processes. After that the person decides, because
//! they can see the machine and this code cannot.

/// What to do about a display that has just died
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Bring it back at once
    Now,
    /// Bring it back, but not this instant: wait this many milliseconds, so
    /// whatever went wrong has a moment to pass
    Wait(u64),
    /// Leave it down and say so. The person brings it back when they are
    /// ready -- pressing for it is not a storm, however often they press
    Ask,
}

/// How many times running it may be rebuilt before the person is asked
pub const TRIES: usize = 2;
/// Over what stretch those tries are counted, in milliseconds
pub const WITHIN_MS: u64 = 10 * 60 * 1000;
/// How long to wait before the second try
pub const BREATH_MS: u64 = 5_000;

/// Whether to bring the display back, given why it died and what has already
/// been tried.
///
/// `tried` is when each earlier attempt was made, and `now` the moment being
/// decided, both in milliseconds on any clock that only goes forwards
pub fn decide(out_of_memory: bool, tried: &[u64], now: u64) -> Next {
    // Rebuilding a display costs memory. When memory is the thing that ran
    // out, the one move guaranteed to make it worse is the one that would
    // otherwise be automatic -- so it never is
    if out_of_memory {
        return Next::Ask;
    }
    let lately = tried.iter().filter(|&&t| now.saturating_sub(t) < WITHIN_MS).count();
    match lately {
        0 => Next::Now,
        n if n < TRIES => Next::Wait(BREATH_MS),
        _ => Next::Ask,
    }
}

/// The attempts still worth counting, so the list does not grow for ever
pub fn recent(tried: &[u64], now: u64) -> Vec<u64> {
    tried.iter().copied().filter(|&t| now.saturating_sub(t) < WITHIN_MS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ordinary case: something fell over, nothing else is wrong, and the
    /// screen comes back before anybody has finished noticing
    #[test]
    fn the_first_failure_is_simply_put_right() {
        assert_eq!(decide(false, &[], 1_000), Next::Now);
    }

    /// A second in the same stretch is a worse sign than the first, so it
    /// waits -- and a third stops, because by then something is wrong that
    /// rebuilding does not fix
    #[test]
    fn it_gives_up_before_it_becomes_the_problem() {
        assert_eq!(decide(false, &[1_000], 2_000), Next::Wait(BREATH_MS));
        assert_eq!(decide(false, &[1_000, 2_000], 3_000), Next::Ask);
        // ...and once it has given up it stays given up, however many more
        // failures arrive
        assert_eq!(decide(false, &[1_000, 2_000, 3_000], 4_000), Next::Ask);
    }

    /// Ten quiet minutes later, whatever it was is not this afternoon's
    /// problem any more
    #[test]
    fn a_failure_long_past_is_not_held_against_it() {
        let old = [1_000, 2_000];
        // Past the last of them, not merely past the first: while the second
        // is still inside the window it still counts, which is the point of
        // counting within a window at all
        let after = 2_000 + WITHIN_MS + 1;
        assert_eq!(decide(false, &old, after), Next::Now);
        assert!(recent(&old, after).is_empty());
        // ...and in between, the one still inside the window is still held
        assert_eq!(decide(false, &old, 1_000 + WITHIN_MS + 1), Next::Wait(BREATH_MS));
    }

    /// The case this exists for. Rebuilding a display takes memory; when
    /// memory is what ran out, doing it automatically is doing harm on a
    /// timer. Not once, not after a pause -- the person is asked
    #[test]
    fn a_display_that_died_of_memory_is_never_rebuilt_unasked() {
        assert_eq!(decide(true, &[], 1_000), Next::Ask);
        assert_eq!(decide(true, &[1_000], 2_000), Next::Ask);
    }

    /// A person pressing for it clears the count: asking for something twice
    /// is not the same as a program retrying in a loop
    #[test]
    fn asking_for_it_starts_the_counting_over() {
        let tried = vec![1_000, 2_000];
        assert_eq!(decide(false, &tried, 3_000), Next::Ask);
        // What the press does: forget what was tried, then decide again
        assert_eq!(decide(false, &[], 3_000), Next::Now);
    }
}

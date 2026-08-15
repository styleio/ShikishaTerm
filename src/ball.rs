//! Makes the automation chain's "invisible ball" visible.
//!
//! The concept of the ball itself already existed (`Tab::chain_depth`).
//! What this adds is remembering which tab currently has it, and where it
//! last flew in from. Rendering is done by main.rs.
//!
//! It's worth showing not because it's decoration, but because
//! - which tab currently has the work
//! - how close the chain is to its limit (runaway protection visibly working)
//! - the chain breaking the instant a human types
//! are exactly the state of the safety mechanism.

/// Apparent flight time from throw to landing
const FLIGHT_MS: u64 = 420;
/// How long the receiving tab glows after landing
const CATCH_MS: u64 = 260;

/// Current location of the ball. `holder` is a 1-based tab number; 0 means "the human has it"
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ball {
    /// Tab currently holding the ball (0 = human)
    pub holder: usize,
    /// Where it was thrown from most recently (0 = from the human)
    pub from: usize,
    /// Chain depth. Resets to 0 when a human types
    pub depth: u32,
    /// Time it was thrown (relative ms). Used to drive the animation
    pub thrown_ms: u64,
    /// A draft was left, waiting for a human to add to it.
    ///
    /// The chain hasn't ended. The human is part of the ring too, and it
    /// starts turning again once they add to it. Depth carries over; this
    /// just marks that the human moves next
    pub awaiting_human: bool,
}

/// Display state. main.rs reads this to decide how to draw things
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// Nobody is running automatically (the ball is in the human's hands)
    Idle,
    /// Currently flying from `from` to `holder`. 0.0..=1.0 is progress
    Flying { from: usize, to: usize, progress: f32 },
    /// Just landed. The receiving side glows
    Caught { at: usize },
    /// `holder` is holding it (at rest)
    Held { at: usize },
}

impl Ball {
    /// An auto-send happened: passed from `from` to `to` at depth `depth`
    pub fn throw(&mut self, from: usize, to: usize, depth: u32, now_ms: u64) {
        self.awaiting_human = false;
        self.from = from;
        self.holder = to;
        self.depth = depth;
        self.thrown_ms = now_ms;
    }

    /// A draft was left: the work is at `to`, and a human moves next.
    ///
    /// Depth is counted the same as an auto-send. If a human adds to it and
    /// sends, the chain continues, so resetting to 0 here would make it
    /// impossible to tell what link in the chain we're on from that point
    pub fn draft(&mut self, from: usize, to: usize, depth: u32, now_ms: u64) {
        self.from = from;
        self.holder = to;
        self.depth = depth;
        self.thrown_ms = now_ms;
        self.awaiting_human = true;
    }

    /// A human typed by hand: the chain breaks, the ball returns to hand
    pub fn reset(&mut self) {
        *self = Ball::default();
    }

    pub fn phase(&self, now_ms: u64) -> Phase {
        if self.holder == 0 {
            return Phase::Idle;
        }
        let since = now_ms.saturating_sub(self.thrown_ms);
        if since < FLIGHT_MS {
            return Phase::Flying {
                from: self.from,
                to: self.holder,
                progress: since as f32 / FLIGHT_MS as f32,
            };
        }
        if since < FLIGHT_MS + CATCH_MS {
            return Phase::Caught { at: self.holder };
        }
        Phase::Held { at: self.holder }
    }

    /// If a tab it points at disappeared/got reordered away, let go of it
    pub fn clamp_to(&mut self, tab_count: usize) {
        if self.holder > tab_count || self.from > tab_count {
            self.reset();
        }
    }
}

#[cfg(test)]
mod tests {

    /// Confirms that after a draft is left, the ball waits there for a human.
    ///
    /// The human is part of the ring too, and it starts turning again once
    /// they add to it. So depth carries over. Resetting it to 0 would make
    /// it impossible to tell what link in the chain we're on from that
    /// point, and the runaway-protection count would also break off at the human
    #[test]
    fn a_draft_leaves_the_ball_waiting_for_a_person() {
        let mut b = Ball::default();
        b.throw(0, 1, 1, 0);
        assert_eq!(b.holder, 1);
        assert!(!b.awaiting_human, "自動送信は人待ちではない");

        b.draft(1, 2, 2, 100);
        assert_eq!(b.holder, 2, "仕事は渡した先にある");
        assert_eq!(b.from, 1, "どこから来たかは残る");
        assert_eq!(b.depth, 2, "人も輪の一部。連鎖はここで途切れない");
        assert!(b.awaiting_human, "人待ちになっていない");

        // The next auto-send clears the "waiting for human" flag
        b.throw(2, 3, 1, 200);
        assert!(!b.awaiting_human, "自動送信で人待ちが解けていない");

        // A human typing by hand returns the ball to hand
        b.draft(1, 2, 3, 300);
        b.reset();
        assert_eq!(b.holder, 0);
        assert!(!b.awaiting_human, "リセットで人待ちが残っている");
    }
    use super::*;

    #[test]
    fn starts_in_human_hands() {
        let b = Ball::default();
        assert_eq!(b.phase(0), Phase::Idle, "誰も自動で動いていない");
        assert_eq!(b.depth, 0);
    }

    #[test]
    fn a_throw_flies_then_lands_then_rests() {
        let mut b = Ball::default();
        b.throw(1, 2, 1, 1000);

        match b.phase(1000) {
            Phase::Flying { from, to, progress } => {
                assert_eq!((from, to), (1, 2));
                assert!(progress < 0.01, "投げた瞬間は始点");
            }
            other => panic!("飛行中のはず: {other:?}"),
        }
        match b.phase(1000 + FLIGHT_MS / 2) {
            Phase::Flying { progress, .. } => assert!((0.4..0.6).contains(&progress), "中間地点"),
            other => panic!("まだ飛行中のはず: {other:?}"),
        }
        assert_eq!(b.phase(1000 + FLIGHT_MS), Phase::Caught { at: 2 });
        assert_eq!(b.phase(1000 + FLIGHT_MS + CATCH_MS), Phase::Held { at: 2 });
        // The holder doesn't change even after time passes
        assert_eq!(b.phase(9_999_999), Phase::Held { at: 2 });
    }

    #[test]
    fn typing_returns_the_ball_to_the_human() {
        let mut b = Ball::default();
        b.throw(1, 2, 3, 1000);
        b.reset();
        assert_eq!(b.phase(2000), Phase::Idle, "人間が入力したら連鎖は切れる");
        assert_eq!(b.depth, 0);
    }


    #[test]
    fn a_vanished_tab_drops_the_ball() {
        let mut b = Ball::default();
        b.throw(1, 3, 2, 0);
        b.clamp_to(3);
        assert_eq!(b.holder, 3, "居るうちは持ったまま");
        // Switched workspace and the tab count went down
        b.clamp_to(2);
        assert_eq!(b.phase(0), Phase::Idle, "居なくなったら手放す");
    }
}

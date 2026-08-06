//! 自動チェーンの「透明のボール」を目に見えるようにする。
//!
//! ボールという概念そのものは前からあった (`Tab::chain_depth`)。
//! ここでやるのは、それが今どのタブにあり、直前にどこから飛んできたかを
//! 覚えておくことだけ。表示は main.rs 側が行う。
//!
//! 見せる価値があるのは、飾りだからではなく、
//! - どのタブが今仕事を持っているのか
//! - 連鎖が上限にどれだけ近いのか (暴走対策が効いている様子)
//! - 人間が入力した瞬間に連鎖が切れること
//! がそのまま安全機構の状態だから。

/// 投げてから着弾までの見かけ上の飛行時間
const FLIGHT_MS: u64 = 420;
/// 着弾後、受け取ったタブが光っている時間
const CATCH_MS: u64 = 260;

/// ボールの現在地。`holder` は 1始まりのタブ番号で、0 は「人間が持っている」
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ball {
    /// 今ボールを持っているタブ (0 = 人間)
    pub holder: usize,
    /// 直前の投げ元 (0 = 人間から)
    pub from: usize,
    /// 連鎖の深さ。人間が入力すると0に戻る
    pub depth: u32,
    /// 投げられた時刻 (相対ms)。アニメの進行に使う
    pub thrown_ms: u64,
    /// 下書きを置いて、人が書き足すのを待っている状態。
    ///
    /// 連鎖が終わったわけではない。人も輪の一部で、書き足せばまた回り出す。
    /// 深さは引き継いだまま、次に動くのが人だということだけを表す
    pub awaiting_human: bool,
}

/// 表示上の状態。main.rs はこれを見て描き分ける
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    /// 誰も自動で動いていない (ボールは人間の手元)
    Idle,
    /// from から holder へ飛んでいる最中。0.0..=1.0 が進捗
    Flying { from: usize, to: usize, progress: f32 },
    /// 着弾直後。受け取った側を光らせる
    Caught { at: usize },
    /// holder が保持している (静止)
    Held { at: usize },
}

impl Ball {
    /// 自動送信が起きた: `from` から `to` へ、深さ `depth` で渡った
    pub fn throw(&mut self, from: usize, to: usize, depth: u32, now_ms: u64) {
        self.awaiting_human = false;
        self.from = from;
        self.holder = to;
        self.depth = depth;
        self.thrown_ms = now_ms;
    }

    /// 下書きを置いた: 仕事は to にあり、次に動くのは人。
    ///
    /// 深さは自動送信と同じに数える。人が書き足して流せば連鎖は続くので、
    /// ここで0に戻すと、そこから先が何連鎖目なのか分からなくなる
    pub fn draft(&mut self, from: usize, to: usize, depth: u32, now_ms: u64) {
        self.from = from;
        self.holder = to;
        self.depth = depth;
        self.thrown_ms = now_ms;
        self.awaiting_human = true;
    }

    /// 人間が手で入力した: 連鎖は切れ、ボールは手元に戻る
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

    /// 上限に対する余裕。0.0 = まだ余裕、1.0 = 上限
    pub fn heat(&self, max_chain: u32) -> f32 {
        if max_chain == 0 {
            return 0.0;
        }
        (self.depth as f32 / max_chain as f32).clamp(0.0, 1.0)
    }

    /// タブが消えた/並び替わったときに、指しているタブが居なくなっていたら手放す
    pub fn clamp_to(&mut self, tab_count: usize) {
        if self.holder > tab_count || self.from > tab_count {
            self.reset();
        }
    }
}

#[cfg(test)]
mod tests {

    /// 下書きを置いたら、ボールはそこで人を待つこと。
    ///
    /// 人も輪の一部で、書き足せばまた回り出す。だから深さは引き継ぐ。
    /// 0に戻すと、そこから先が何連鎖目なのか分からなくなり、
    /// 暴走対策の数え上げも人のところで途切れてしまう
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

        // 次に自動送信が起きたら人待ちは解ける
        b.throw(2, 3, 1, 200);
        assert!(!b.awaiting_human, "自動送信で人待ちが解けていない");

        // 人が手で入力したら、ボールは手元へ戻る
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
        // 時間が経っても持ち主は変わらない
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
    fn heat_reaches_one_at_the_limit() {
        let mut b = Ball::default();
        b.throw(1, 2, 5, 0);
        assert!((b.heat(10) - 0.5).abs() < 0.01);
        b.throw(2, 1, 10, 0);
        assert_eq!(b.heat(10), 1.0, "上限で振り切れる");
        b.throw(1, 2, 99, 0);
        assert_eq!(b.heat(10), 1.0, "上限を超えても1.0で頭打ち");
        assert_eq!(b.heat(0), 0.0, "上限0でも壊れない");
    }

    #[test]
    fn a_vanished_tab_drops_the_ball() {
        let mut b = Ball::default();
        b.throw(1, 3, 2, 0);
        b.clamp_to(3);
        assert_eq!(b.holder, 3, "居るうちは持ったまま");
        // ワークスペースを切り替えてタブが減った
        b.clamp_to(2);
        assert_eq!(b.phase(0), Phase::Idle, "居なくなったら手放す");
    }
}

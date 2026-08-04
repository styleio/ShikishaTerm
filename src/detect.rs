//! 状態検出エンジン: 複数の独立した信号 (画面パターン / ベル / 沈黙タイマー) を
//! 重ねてタブ状態を判定する状態機械。DESIGN.md 4.2章。

use crate::profile::Profile;

/// 直近このms以内に出力があれば「動作中」とみなす
const ACTIVITY_MS: u64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabState {
    /// 黄: 処理中 (出力が流れている / BUSYパターンにマッチ)
    Busy,
    /// 緑: 応答完了 (動作後に沈黙 or ベル)
    Done,
    /// 青: 選択肢・確認待ち (QUESTIONパターンにマッチ)
    Question,
    /// 青: 待機 (動作なし)
    Wait,
    /// 赤: 子プロセス終了 (Detectorではなく Tab が設定する)
    Exited,
}

impl TabState {
    pub fn label(&self) -> &'static str {
        match self {
            TabState::Busy => "BUSY",
            TabState::Done => "DONE",
            TabState::Question => "QUESTION",
            TabState::Wait => "WAIT",
            TabState::Exited => "EXIT",
        }
    }
}

pub struct Detector {
    profile: Profile,
    state: TabState,
    /// 直近に「動作」があったか (Done判定は動作→沈黙の遷移でのみ発火)
    was_active: bool,
    last_bell: u64,
}

impl Detector {
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            state: TabState::Wait,
            was_active: false,
            last_bell: 0,
        }
    }

    pub fn profile_name(&self) -> &str {
        &self.profile.name
    }

    /// 定期実行 (200ms毎目安)。
    /// 優先度: QUESTION > BUSY(パターン) > ベル完了 > 活動タイマー > 沈黙タイマー
    pub fn tick(&mut self, screen_text: &str, ms_since_output: u64, bell_count: u64) -> TabState {
        let bell_rang = bell_count > self.last_bell;
        self.last_bell = bell_count;

        if self.profile.question.iter().any(|r| r.is_match(screen_text)) {
            self.state = TabState::Question;
            return self.state;
        }
        if self.profile.busy.iter().any(|r| r.is_match(screen_text)) {
            self.was_active = true;
            self.state = TabState::Busy;
            return self.state;
        }
        if bell_rang && self.was_active {
            self.was_active = false;
            self.state = TabState::Done;
            return self.state;
        }
        if ms_since_output < ACTIVITY_MS {
            self.was_active = true;
            self.state = TabState::Busy;
        } else if ms_since_output >= self.profile.silence_ms {
            if self.was_active {
                self.was_active = false;
                self.state = TabState::Done;
            } else if self.state == TabState::Busy {
                self.state = TabState::Wait;
            }
            // Done / Wait は次の動作まで維持
        }
        // ACTIVITY_MS..silence_ms の間は直前の状態を維持 (チラつき防止)
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Profile, ProfileFile};

    fn claude_like() -> Profile {
        Profile::compile(ProfileFile {
            name: "test".into(),
            command_match: vec![],
            busy_patterns: vec!["esc to interrupt".into()],
            question_patterns: vec!["Do you want".into(), "❯\\s*1\\.".into()],
            silence_ms: 2000,
        })
        .unwrap()
    }

    #[test]
    fn busy_pattern_wins_over_silence() {
        let mut d = Detector::new(claude_like());
        assert_eq!(d.tick("... esc to interrupt ...", 10_000, 0), TabState::Busy);
    }

    #[test]
    fn question_has_top_priority() {
        let mut d = Detector::new(claude_like());
        let screen = "Do you want to proceed?\n❯ 1. Yes\n  2. No";
        assert_eq!(d.tick(screen, 100, 0), TabState::Question);
    }

    #[test]
    fn activity_then_silence_becomes_done() {
        let mut d = Detector::new(claude_like());
        assert_eq!(d.tick("output flowing", 100, 0), TabState::Busy);
        assert_eq!(d.tick("output stopped", 3000, 0), TabState::Done);
        // 動作を挟まない限りDoneを維持
        assert_eq!(d.tick("output stopped", 10_000, 0), TabState::Done);
    }

    #[test]
    fn bell_after_activity_means_done() {
        let mut d = Detector::new(claude_like());
        d.tick("working", 100, 0);
        assert_eq!(d.tick("finished", 600, 1), TabState::Done);
    }

    #[test]
    fn grace_period_keeps_previous_state() {
        let mut d = Detector::new(claude_like());
        d.tick("working", 100, 0);
        // 500ms〜2000msの間は判定を保留してBusyを維持 (チラつき防止)
        assert_eq!(d.tick("quiet", 1000, 0), TabState::Busy);
    }

    #[test]
    fn generic_profile_uses_timers_only() {
        let mut d = Detector::new(Profile::generic());
        assert_eq!(d.tick("anything", 100, 0), TabState::Busy);
        assert_eq!(d.tick("anything", 3000, 0), TabState::Done);
    }
}

//! 画面に必要な状態を、見た目から切り離して運ぶ。
//!
//! ここには「どう見せるか」を一切書かない。何が起きているかだけを持つ。
//! 見た目は受け取った側が決める。
//!
//! そうする理由は、同じ状態を複数の場所が使うから。
//!   - 窓 (自前のウィンドウ)
//!   - 手元の端末 (今までのTUI)
//!   - スマホ (リモート表示)
//!
//! 以前スマホ表示だけ別に作った結果、片方だけが壊れた。
//! アスキーアートが崩れたのも、行が全部つながったのも、
//! 「画面を見せる」を2回書いていたから。ここを通せば1回になる。

use serde::Serialize;

/// タブ1つぶんの状態
#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct TabState {
    /// 1始まりのタブ番号 (人が押す番号と同じ)
    pub index: usize,
    pub name: String,
    /// 自動化から指す名前
    pub id: Option<String>,
    /// WAIT / BUSY / DONE / ASK / EXIT
    pub state: String,
    /// 検出プロファイル名 (Codex CLI 等)。合っているか目で確かめられる
    pub profile: String,
    pub locked: bool,
    /// 連鎖の深さ。0 = 人が始めた会話
    pub depth: u32,
    /// 直近の出力量 (古い→新しい、各 0..=7)。棒グラフにする
    pub activity: Vec<u8>,
    /// "pty" か "browser"。見せ方が変わる
    pub kind: String,
}

/// 自動化の輪の現在地
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct BallState {
    /// 今持っているタブ (0 = 人の手元)
    pub holder: usize,
    /// 直前の投げ元
    pub from: usize,
    pub depth: u32,
    pub max: u32,
    /// "idle" / "flying" / "caught" / "held"
    pub phase: String,
    /// 飛行中の進捗 0.0..=1.0
    pub progress: f32,
    /// 人が書き足すのを待っている
    pub awaiting_human: bool,
}

/// 画面に出す状態のすべて。
///
/// 見た目に関する語 (色・幅・記号) はここに入れない。
/// 入れた瞬間、受け取る側が3つとも同じ見た目に縛られる
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct UiState {
    pub workspace: String,
    pub workspaces: Vec<String>,
    pub ws_index: usize,
    /// 見ているタブ (0 = INDEX)
    pub active: usize,
    pub auto_enabled: bool,
    pub remote_on: bool,
    /// 設定がまだ無い初回起動
    pub first_run: bool,
    pub tabs: Vec<TabState>,
    pub ball: BallState,
    /// 一時的な通知 (保存しました、緊急停止、等)
    pub flash: Option<String>,
    /// ヘルプを出している
    pub help_open: bool,
    /// ワークスペースの選択を出している
    pub ws_open: bool,
    /// スマホ接続のQRを出しているなら、その接続先
    pub qr: Option<String>,
    /// どのビルドか (古い実行ファイルを掴んでいないか確かめられる)
    pub build: String,
}

impl TabState {
    /// 動いているタブから作る
    pub fn of(index: usize, t: &crate::tab::Tab) -> Self {
        Self {
            index,
            name: t.title.clone(),
            id: t.id.clone(),
            state: t.state.label().to_string(),
            profile: t.profile_name().to_string(),
            locked: t.locked,
            depth: t.chain_depth,
            activity: t.activity().to_vec(),
            kind: "pty".into(),
        }
    }
}

impl BallState {
    pub fn of(b: &crate::ball::Ball, max: u32, now_ms: u64) -> Self {
        use crate::ball::Phase;
        let (phase, progress) = match b.phase(now_ms) {
            Phase::Idle => ("idle", 0.0),
            Phase::Flying { progress, .. } => ("flying", progress),
            Phase::Caught { .. } => ("caught", 1.0),
            Phase::Held { .. } => ("held", 1.0),
        };
        Self {
            holder: b.holder,
            from: b.from,
            depth: b.depth,
            max,
            phase: phase.into(),
            progress,
            awaiting_human: b.awaiting_human,
        }
    }
}

impl UiState {
    /// 押せるタブ番号か
    pub fn has_tab(&self, index: usize) -> bool {
        index >= 1 && index <= self.tabs.len()
    }

    /// 連鎖が上限にどれだけ近いか (0.0..=1.0)。
    /// 暴走対策が効いている様子を、そのまま出すためのもの
    pub fn heat(&self) -> f32 {
        if self.ball.max == 0 {
            return 0.0;
        }
        (self.ball.depth as f32 / self.ball.max as f32).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(index: usize, name: &str) -> TabState {
        TabState {
            index,
            name: name.into(),
            id: None,
            state: "WAIT".into(),
            profile: "GENERIC".into(),
            locked: false,
            depth: 0,
            activity: vec![0; 4],
            kind: "pty".into(),
        }
    }

    /// 状態に見た目が混ざっていないこと。
    ///
    /// 色や記号をここに入れると、窓もTUIもスマホも同じ見た目に縛られる。
    /// 分けておけば、それぞれが自分に合った見せ方をできる
    #[test]
    fn the_state_carries_no_appearance() {
        let s = UiState {
            workspace: "検証".into(),
            tabs: vec![tab(1, "実装")],
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        for looky in ["color", "#", "rgb", "▁", "●", "width", "px"] {
            assert!(
                !json.contains(looky),
                "見た目が混ざっている ({looky}): {json}"
            );
        }
    }

    /// 押せる番号かどうかを、受け取る側が判断できること
    #[test]
    fn tab_numbers_are_checked_against_what_exists() {
        let s = UiState {
            tabs: vec![tab(1, "a"), tab(2, "b")],
            ..Default::default()
        };
        assert!(!s.has_tab(0), "0 は INDEX であってタブではない");
        assert!(s.has_tab(1));
        assert!(s.has_tab(2));
        assert!(!s.has_tab(3), "無いタブを押せることになっている");
    }

    /// 連鎖の熱が、上限との比で出ること。
    /// 上限を超えても 1.0 で止まる (色が振り切れて壊れないように)
    #[test]
    fn the_heat_shows_how_close_the_chain_is_to_its_limit() {
        let mut s = UiState::default();
        assert_eq!(s.heat(), 0.0, "上限0で割らない");

        s.ball.max = 10;
        s.ball.depth = 0;
        assert_eq!(s.heat(), 0.0);
        s.ball.depth = 5;
        assert_eq!(s.heat(), 0.5);
        s.ball.depth = 10;
        assert_eq!(s.heat(), 1.0);
        s.ball.depth = 40;
        assert_eq!(s.heat(), 1.0, "上限を超えても振り切れない");
    }

    /// 同じ状態なら同じもの、と判定できること。
    ///
    /// 毎フレーム送ると、変わっていない画面でも書き換えが走る。
    /// 比べられるようにしておけば、変わったときだけ送れる
    #[test]
    fn identical_states_compare_equal() {
        let a = UiState {
            tabs: vec![tab(1, "実装")],
            ..Default::default()
        };
        let mut b = a.clone();
        assert_eq!(a, b);
        b.tabs[0].state = "BUSY".into();
        assert_ne!(a, b, "状態が変わったのに同じと判定された");
    }
}

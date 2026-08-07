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
    /// WAIT / BUSY / DONE / ASK / EXIT。見た目の出し分けに使う
    pub state: String,
    /// 人が読む状態名 (翻訳される)。表示はこちらを使う
    pub state_label: String,
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

/// ブラウザの上に出す操作の並び。
///
/// 出す・出さないは設定かLuaが決め、押せる・押せないはブラウザが答える。
/// 戻れないのに押せる顔で出ていると、押した人は壊れたのかと思う
#[derive(Clone, Serialize, PartialEq, Debug, Default)]
pub struct NavState {
    pub back: bool,
    pub forward: bool,
    pub reload: bool,
    /// URL欄 (人が任意のページへ移る手段)
    pub edit: bool,
    pub can_back: bool,
    pub can_forward: bool,
    /// いま開いている場所
    pub at: String,
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
    /// 見ているブラウザの上に出す操作 (None = 出さない)
    pub nav: Option<NavState>,
    /// 今の画面から何行遡って見ているか (0 = 今)。
    /// 遡っていることが分からないと、出力が止まったように見える
    pub scrolled: usize,
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
            state_label: t.state.display(),
            profile: t.profile_name().to_string(),
            locked: t.locked,
            depth: t.chain_depth,
            activity: t.activity().to_vec(),
            kind: "pty".into(),
        }
    }

    /// 窓の中に置いたブラウザから作る。
    ///
    /// セッションではないので、状態も出力量も無い。
    /// 無いものを埋めて似せるより、無いまま出す方が読める
    pub fn browser(index: usize, key: &str, name: &str) -> Self {
        Self {
            index,
            name: name.to_string(),
            id: Some(key.to_string()),
            state: "WEB".into(),
            state_label: crate::i18n::t("tui.state.web"),
            profile: String::new(),
            locked: false,
            depth: 0,
            activity: Vec::new(),
            kind: "browser".into(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 窓の中に置いたブラウザが、セッションの続きの番号で並ぶこと。
    ///
    /// 以前は一覧にも盤面にも行が無く、切り替えられなかった。
    /// 出し入れは Ctrl+B o という別の操作で、「タブの一部」と
    /// 言いながらタブではなかった。
    ///
    /// 番号が続いていないと、人が押す番号と中身がずれる
    #[test]
    fn a_browser_takes_the_next_tab_number() {
        let a = TabState::browser(3, "shop", "通販サイト");
        let b = TabState::browser(4, "mail", "メール");
        assert_eq!((a.index, b.index), (3, 4));
        assert_eq!(a.kind, "browser", "セッションと同じ見せ方になっている");
        assert_eq!(a.id.as_deref(), Some("shop"), "自動化から指す名前が違う");
        // 人が読む名前と、自動化から指す名前は別のもの
        assert_eq!(a.name, "通販サイト", "設定した表示名が出ていない");
        // セッションではないので、埋めて似せない
        assert!(a.activity.is_empty() && a.profile.is_empty() && a.depth == 0);
        assert!(!a.locked);
    }


    fn tab(index: usize, name: &str) -> TabState {
        TabState {
            index,
            name: name.into(),
            id: None,
            state: "WAIT".into(),
            state_label: "WAIT".into(),
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

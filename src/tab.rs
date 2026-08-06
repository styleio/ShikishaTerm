//! タブ = 1つのPTYセッション (子プロセス + vt100パーサ + 状態検出)。DESIGN.md 4章。

use std::io::{Read as _, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::detect::{Detector, TabState};
use crate::profile::Profile;

pub const SCROLLBACK_LINES: usize = 5000;

/// タブごとの端末設定 (PuTTYのセッション設定に相当するもの)
#[derive(Clone)]
pub struct TabOptions {
    /// 起動時の作業フォルダ (AI CLIはここのプロジェクトを見る)
    pub cwd: Option<std::path::PathBuf>,
    /// スクロールバック行数
    pub scrollback: usize,
    /// 文字コード ("utf-8" / "shift_jis" / "euc-jp" 等)。既定はUTF-8
    pub encoding: Option<&'static encoding_rs::Encoding>,
    /// セッションログを logs/ に保存する
    pub log: bool,
}

impl Default for TabOptions {
    fn default() -> Self {
        Self {
            cwd: None,
            scrollback: SCROLLBACK_LINES,
            encoding: None,
            log: false,
        }
    }
}

impl TabOptions {
    /// 設定の文字列から文字コードを解決する (未知の名前はUTF-8扱い)
    pub fn encoding_from_name(name: Option<&str>) -> Option<&'static encoding_rs::Encoding> {
        let n = name?.trim();
        if n.is_empty() || n.eq_ignore_ascii_case("utf-8") || n.eq_ignore_ascii_case("utf8") {
            return None;
        }
        encoding_rs::Encoding::for_label(n.as_bytes())
            .filter(|e| *e != encoding_rs::UTF_8)
    }
}

pub type PtyWriter = Arc<Mutex<Box<dyn std::io::Write + Send>>>;
pub type SharedParser = Arc<Mutex<vt100::Parser<QueryResponder>>>;

/// コピーモードの状態 (Ctrl+B [ / マウスで開始)
pub struct CopyState {
    /// ペイン内のカーソル行 (0 = 最上行)
    pub cursor_row: u16,
    /// 選択開始位置 (画面最下行から数えた行数)。None = 未選択
    pub anchor: Option<usize>,
    /// 押してから離すまでにマウスが動いたか。
    ///
    /// 選択は行単位なので、単クリックでも「1行選択した」形になる。
    /// 動いたかどうかを見ないと、置くだけのつもりのクリックで
    /// クリップボードが書き換わり、貼り付けようとしていた中身が消える
    pub dragged: bool,
}

/// 子プロセスからの端末照会 (DSR/DA) への応答係。
/// ConPTY配下のプログラム (ssh等) はカーソル位置照会 `\x1b[6n` への応答を
/// 待ってブロックするため、本物のターミナルと同様にPTYへ書き戻す。
/// あわせてベル文字 (完了通知によく使われる) を数え、状態検出の信号にする。
pub struct QueryResponder {
    writer: PtyWriter,
    bell: Arc<AtomicU64>,
}

impl QueryResponder {
    fn reply(&self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }
}

impl vt100::Callbacks for QueryResponder {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell.fetch_add(1, Ordering::Relaxed);
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let p0 = params.first().and_then(|p| p.first()).copied();
        match (i1, c, p0) {
            // DSR-CPR: カーソル位置照会 → \x1b[{row};{col}R (1始まり)
            (None, 'n', Some(6)) => {
                let (row, col) = screen.cursor_position();
                self.reply(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            // DSR: 端末ステータス照会 → 正常
            (None, 'n', Some(5)) => self.reply(b"\x1b[0n"),
            // DA1: 端末種別照会 → VT102相当
            (None, 'c', _) => self.reply(b"\x1b[?6c"),
            // DA2: 二次端末種別照会
            (Some(b'>'), 'c', _) => self.reply(b"\x1b[>0;0;0c"),
            _ => {}
        }
    }
}

pub fn pty_write(writer: &PtyWriter, bytes: &[u8]) -> Result<()> {
    let mut w = writer.lock().expect("pty writer lock");
    w.write_all(bytes)?;
    Ok(())
}

/// 起動コマンドを組み立てる。
/// npmシム等の拡張子なしスクリプトは CreateProcess が直接起動できない
/// (os error 193) ため、PATH+PATHEXT を探索して .cmd/.bat は cmd.exe /c 経由にする
pub fn build_command(cmd_args: &[String]) -> CommandBuilder {
    let Some(prog) = cmd_args.first() else {
        return CommandBuilder::new("powershell.exe");
    };
    let rest = &cmd_args[1..];
    match resolve_windows_command(prog) {
        Some(path) => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            if matches!(ext.as_deref(), Some("cmd") | Some("bat")) {
                let mut c = CommandBuilder::new("cmd.exe");
                c.arg("/c");
                c.arg(path);
                for a in rest {
                    c.arg(a);
                }
                c
            } else {
                let mut c = CommandBuilder::new(path);
                for a in rest {
                    c.arg(a);
                }
                c
            }
        }
        // 解決できなければそのまま渡してエラーを表面化させる
        None => {
            let mut c = CommandBuilder::new(prog);
            for a in rest {
                c.arg(a);
            }
            c
        }
    }
}

/// PATH と実行可能拡張子 (.exe/.com/.cmd/.bat) でコマンドを実ファイルに解決する
pub fn resolve_command(prog: &str) -> Option<std::path::PathBuf> {
    resolve_windows_command(prog)
}

fn resolve_windows_command(prog: &str) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    const EXTS: [&str; 4] = ["exe", "com", "cmd", "bat"];

    let has_exec_ext = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
    };
    let try_base = |base: PathBuf| -> Option<PathBuf> {
        if has_exec_ext(&base) && base.is_file() {
            return Some(base);
        }
        EXTS.iter()
            .map(|e| base.with_extension(e))
            .find(|cand| cand.is_file())
    };

    let p = Path::new(prog);
    // パス区切りを含む指定はPATH探索せずそのまま解決
    if p.components().count() > 1 {
        return try_base(p.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var).find_map(|dir| try_base(dir.join(prog)))
}

/// 答えとして取り出す範囲を、画面下端からの深さで返す (深さ0 = 最下行)。
///
/// カーソルは入力欄の中にいる。その下にあるのは答えではなく枠なので、
/// 数え始めをカーソル行に合わせる。文字を見ないので、答えの文面が
/// たまたま枠の文言と一致しても巻き込まれない
pub fn capture_range(rows: u16, cursor_row: u16, since: usize) -> (usize, usize) {
    let below = rows.saturating_sub(1).saturating_sub(cursor_row) as usize;
    // below を足すのは目的の行まで届くため。届いたら下駄は履いたままにしない
    (below, below.saturating_add(since))
}

/// スクロールバック内の行範囲 (画面最下行からの行数 lo..=hi) をテキスト化する。
/// 折返し行は連結し、行末の空白は除去する。
pub fn extract_text<CB: vt100::Callbacks>(
    p: &mut vt100::Parser<CB>,
    lo: usize,
    hi: usize,
    cols: u16,
) -> String {
    let saved = p.screen().scrollback();
    p.screen_mut().set_scrollback(usize::MAX / 2);
    let max = p.screen().scrollback();
    let (rows, _) = p.screen().size();
    let top = max + rows.saturating_sub(1) as usize;
    let mut out = String::new();
    for d in (lo..=hi.min(top)).rev() {
        let s = d.min(max);
        p.screen_mut().set_scrollback(s);
        let r = (rows as usize - 1 - (d - s)) as u16;
        // rows(start_col, width) は各可視行の水平スライスを返すイテレータ
        let line = p
            .screen()
            .rows(0, cols)
            .nth(r as usize)
            .unwrap_or_default();
        if p.screen().row_wrapped(r) {
            out.push_str(&line);
        } else {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    p.screen_mut().set_scrollback(saved);
    out
}

/// この入力に「実行」が含まれるか。
///
/// 括弧付き貼り付けの中身は本文なので、その中の改行は実行ではない。
/// 打っただけ・貼っただけを実行と数えると、手が止まった画面を
/// 応答完了と読んでしまう
pub fn contains_submit(bytes: &[u8]) -> bool {
    const START: &[u8] = b"\x1b[200~";
    const END: &[u8] = b"\x1b[201~";
    let mut in_paste = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(START) {
            in_paste = true;
            i += START.len();
        } else if bytes[i..].starts_with(END) {
            in_paste = false;
            i += END.len();
        } else {
            if !in_paste && matches!(bytes[i], b'\r' | b'\n') {
                return true;
            }
            i += 1;
        }
    }
    false
}

/// 画面を見たままのテキストにする (1画面行 = 1行)。
///
/// `Screen::contents()` は折り返し扱いの行を改行なしで連結する。
/// テキストとしては正しいが、行末いっぱいまで描くアスキーアートは
/// 全行が折り返し扱いになり、画面全体が1行に潰れてしまう。
/// 見た目を運ぶ用途では行を保つ必要がある
pub fn visible_text(screen: &vt100::Screen) -> String {
    let (_, cols) = screen.size();
    screen
        .rows(0, cols)
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 画面内容のハッシュ。最下部 ignore_bottom 行は判定から除外する
/// (byobu/tmux等のステータスバーは時計が毎秒更新され、
///  生の出力活動を見ると永遠にBUSYになってしまうため)
pub fn screen_hash(screen: &vt100::Screen, ignore_bottom: u16) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let (rows, cols) = screen.size();
    let keep = rows.saturating_sub(ignore_bottom) as usize;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for line in screen.rows(0, cols).take(keep) {
        line.hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod changed_span_tests {
    use super::Tab;

    fn rows(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// 全画面TUIでは、実行前から出ていたものを答えに含めないこと。
    ///
    /// Claude Code はスクロールしないので「実行してから書かれた行」が
    /// 数えられず、可視画面を丸ごと取っていた。その結果、起動バナーと
    /// 入力欄の枠まで相手のAIへ送っていた
    #[test]
    fn what_was_already_on_screen_is_not_the_answer() {
        let before = rows(&[
            "  Claude Code v2.1.223", // 起動バナー
            "  D:\\project",
            "",
            "────────",             // 枠の上辺
            "> 質問",
            "────────",             // 枠の下辺
            "  ? for shortcuts",
        ]);
        let now = rows(&[
            "  Claude Code v2.1.223", // 変わらない
            "  D:\\project",
            "",
            "答えの1行目", // ここから変わった
            "答えの2行目",
            "────────", // また変わらない (枠に戻った)
            "  ? for shortcuts",
        ]);
        assert_eq!(
            Tab::changed_span(&before, &now),
            (3, 5),
            "バナーと枠を外し、変わった2行だけを残す"
        );
    }

    /// 答えが届いていなければ、何も渡さないこと
    #[test]
    fn an_unchanged_screen_yields_nothing() {
        let same = rows(&["a", "b", "c"]);
        assert_eq!(Tab::changed_span(&same, &same), (0, 0));
    }

    /// 端だけを削り、真ん中は触らないこと。
    ///
    /// 答えの途中にたまたま実行前と同じ行があっても、そこで切ってはいけない
    #[test]
    fn a_coincidence_in_the_middle_does_not_split_the_answer() {
        let before = rows(&["枠", "x", "同じ行", "y", "枠"]);
        let now = rows(&["枠", "答え1", "同じ行", "答え2", "枠"]);
        assert_eq!(
            Tab::changed_span(&before, &now),
            (1, 4),
            "真ん中の一致では切らない"
        );
    }

    /// 行数が変わっていたら、下端どうしは対応しないので上端だけで判断すること
    #[test]
    fn a_resized_screen_falls_back_to_the_top_edge() {
        let before = rows(&["枠", "x"]);
        let now = rows(&["枠", "答え", "増えた行"]);
        assert_eq!(Tab::changed_span(&before, &now), (1, 3));
    }

    /// 実行前の画面を撮れていなければ、削らないこと (安全側に倒す)
    #[test]
    fn without_a_snapshot_nothing_is_removed() {
        let now = rows(&["a", "b", "c"]);
        assert_eq!(Tab::changed_span(&[], &now), (0, 3));
    }
}

#[cfg(test)]
mod capture_range_tests {
    use super::capture_range;

    /// 入力欄の下にあるものは、答えとして渡さないこと。
    ///
    /// 利用者が見たのは 'Use /skills to list available skills' と
    /// 'gpt-5.5 medium  D:\\Test' が相手に転送される現象。どちらも
    /// カーソルより下に描かれる枠で、答えではない。
    ///
    /// 文字ではなく位置で切る。答えの文面がたまたま枠の文言と一致しても
    /// 巻き込まれないし、CLI が文言を変えても壊れない
    #[test]
    fn the_frame_below_the_cursor_is_not_part_of_the_answer() {
        // 24行の画面、カーソルは入力欄 (下から4行目) にいる。
        // その下の3行はヒント行とステータス行
        let (lo, hi) = capture_range(24, 20, 10);
        assert_eq!(lo, 3, "カーソルより下の3行を飛ばして数え始める");
        assert_eq!(hi, 13, "実行してから書かれた10行ぶんを取る");

        // 素のシェル: カーソルは最下行のプロンプトにいる
        let (lo, hi) = capture_range(24, 23, 5);
        assert_eq!((lo, hi), (0, 5), "下に枠がなければ最下行から数える");

        // 何も書かれていなければ、何も取らない (lo == hi で1行だが、
        // それはカーソル行そのもの = 入力欄なので trim で落ちる)
        let (lo, hi) = capture_range(24, 20, 0);
        assert_eq!(lo, hi, "実行後に何も書かれていなければ範囲は空に近い");
    }

    /// 画面が壊れた値でも、範囲計算だけは破綻しないこと
    #[test]
    fn a_broken_screen_size_does_not_panic() {
        assert_eq!(capture_range(0, 0, 0), (0, 0), "高さ0");
        assert_eq!(capture_range(1, 5, 3), (0, 3), "カーソルが画面外");
        let (lo, hi) = capture_range(24, 0, usize::MAX);
        assert_eq!(lo, 23);
        assert_eq!(hi, usize::MAX, "足し算が溢れない");
    }
}

#[cfg(test)]
mod tests {
    use super::screen_hash;

    /// 画面いっぱいに描かれた行が1行に潰れないこと。
    /// contents() は折り返し行を連結するので、アスキーアートが
    /// 全部つながって「改行されていない」表示になる (実際に起きた)
    #[test]
    fn full_width_rows_keep_their_line_breaks() {
        let mut p = vt100::Parser::new(4, 10, 0);
        // 10桁ちょうどの行を3つ。端末はこれを折り返しとして記録する
        p.process(b"##########$$$$$$$$$$%%%%%%%%%%");

        assert!(
            !p.screen().contents().contains('\n'),
            "contents() は改行を落とす (この前提が崩れたら本関数は不要)"
        );
        let visible = super::visible_text(p.screen());
        assert_eq!(
            visible.split('\n').collect::<Vec<_>>(),
            vec!["##########", "$$$$$$$$$$", "%%%%%%%%%%", ""],
            "画面の行がそのまま残る (4行目は空行)"
        );
    }

    /// 実行が効かなかったときに、それを応答と呼ばないこと。
    ///
    /// 貼り付けが `[Pasted Content …]` の形に描き変わるだけでも、出力は増え
    /// 画面も動く。そこを応答の合図にすると、実行が届いていなくてもボールが渡る。
    /// AIが働き始めた表示が出たかどうかで見る
    #[test]
    fn an_answer_requires_the_ai_to_have_started_working() {
        use super::{Tab, TabOptions};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        // 作業中の表示を持つプロファイルを当てる
        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn(
            "shell".into(),
            &argv,
            Some("claude".into()),
            12,
            60,
            TabOptions::default(),
        )
        .unwrap();
        let start = Instant::now();
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }

        t.write_bytes(b"echo REPLY\r").unwrap();
        assert!(t.was_prompted(), "実行として記録される");
        assert!(!t.answered_since_submit(), "実行した直後はまだ応答が無い");

        // 出力も画面も動いたが、AIは働いていない状態 (貼り付けの描き変わり)
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }
        assert!(t.output_count() > 0 && t.had_output(), "出力そのものは動いている");
        assert!(
            !t.answered_since_submit(),
            "画面が動いただけで応答ありと数えている (実行が効いていなくてもボールが渡る)"
        );

        // 働き始めた表示を見たら応答
        t.saw_working.store(true, Ordering::Relaxed);
        assert!(t.answered_since_submit(), "働き始めたら応答として数える");

        // 次の実行で、また待つ状態に戻る
        t.write_bytes(b"echo AGAIN\r").unwrap();
        assert!(
            !t.answered_since_submit(),
            "実行のたびに数え直す (前の応答が残らない)"
        );

        t.kill();
    }

    /// 応答の始まりが「実行した瞬間」に決まること。
    ///
    /// 「最初に画面が動いた位置」にすると、貼り付けの表示や入力欄の描き直しも
    /// 画面を動かすので、答えではなく枠を掴む (実際にそうなっていた)
    #[test]
    fn a_response_starts_where_the_instruction_was_submitted() {
        use super::{Tab, TabOptions};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn("shell".into(), &argv, None, 10, 60, TabOptions::default()).unwrap();
        let start = Instant::now();
        let marker = |t: &Tab| t.response_marker.load(Ordering::Relaxed);
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }
        assert_eq!(marker(&t), u64::MAX, "実行していないうちは始まりが無い");

        // 実行した時点で決まる (画面が動くのを待たない)
        t.write_bytes(b"echo ONE\r").unwrap();
        let began = marker(&t);
        assert_ne!(began, u64::MAX, "実行した瞬間に始まりが決まる");

        // そのあと画面がどれだけ動いても取り直さない
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }
        assert_eq!(marker(&t), began, "画面が動いても始まりは動かない");

        // 受け取り切ったら次の実行を待つ
        t.finish_response();
        assert_eq!(marker(&t), u64::MAX, "次の応答は新しく取り直す");
        assert!(!t.was_prompted(), "次の実行を待つ状態に戻る");

        t.kill();
    }

    /// 打っただけ・貼っただけを「実行した」と数えないこと。
    ///
    /// 数えてしまうと、入力しかけて手を止めた画面が静かになった瞬間に
    /// 応答完了と読まれ、書きかけの内容が他のタブへ転送される
    #[test]
    fn only_a_real_enter_counts_as_submitting() {
        use super::contains_submit;

        assert!(!contains_submit(b"hello"), "打っただけ");
        assert!(!contains_submit(b""), "空");
        assert!(contains_submit(b"hello\r"), "改行で実行");
        assert!(contains_submit(b"\r"), "改行だけでも実行");
        assert!(contains_submit(b"\n"), "LFも実行として扱う");

        // 括弧付き貼り付けの中身は本文。中の改行は実行ではない
        assert!(
            !contains_submit(b"\x1b[200~one\rtwo\x1b[201~"),
            "貼り付けた本文の改行は実行ではない"
        );
        // 貼り付けたあとの改行は実行
        assert!(
            contains_submit(b"\x1b[200~one\rtwo\x1b[201~\r"),
            "貼り付けを閉じたあとの改行は実行"
        );
        // 閉じ忘れても、中身を実行と誤認しない
        assert!(
            !contains_submit(b"\x1b[200~one\rtwo"),
            "閉じられていない貼り付けの中身"
        );
    }

    /// 画面の大きさを変えただけで「応答が来た」ことにならないこと。
    ///
    /// 子プロセスは端末が変わると画面を描き直す。中身は同じでも画面は動くので、
    /// 活動と数えると BUSY→DONE を通り、再描画が新しい応答として
    /// 他のタブへ転送されてしまう
    #[test]
    fn resizing_the_window_is_not_a_new_answer() {
        use super::{Tab, TabOptions};
        use crate::detect::TabState;
        use std::time::{Duration, Instant};

        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn("shell".into(), &argv, None, 20, 60, TabOptions::default()).unwrap();
        let start = Instant::now();

        // 起動後、落ち着くまで進める
        let settle = |t: &mut Tab| {
            for _ in 0..120 {
                std::thread::sleep(Duration::from_millis(50));
                if t.tick(start).1 != TabState::Busy {
                    return t.state;
                }
            }
            t.state
        };
        let calm = settle(&mut t);
        assert_ne!(calm, TabState::Busy, "まず落ち着かせる");

        // 大きさを変える。子プロセスは描き直すが、応答ではない
        t.resize(30, 100).unwrap();
        let mut went_busy = false;
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(50));
            if t.tick(start).1 == TabState::Busy {
                went_busy = true;
                break;
            }
        }
        assert!(
            !went_busy,
            "描き直しを処理中と見なしている (このあと DONE になり応答として転送される)"
        );

        t.kill();
    }

    /// 起動時の出力だけで DONE になること、そしてそれを応答扱いしないこと。
    ///
    /// どんなプログラムも起動時に何か出力するので、画面は「動いて→止まる」を
    /// 通り、状態は必ず DONE になる。ここを応答完了として扱うと、
    /// 誰も聞いていないバナーが自動化で他のタブへ転送されてしまう
    #[test]
    fn startup_output_reaches_done_but_is_not_an_answer() {
        use super::{Tab, TabOptions};
        use crate::detect::TabState;
        use std::time::{Duration, Instant};

        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn("shell".into(), &argv, None, 20, 60, TabOptions::default()).unwrap();

        // 起動時の出力だけで DONE まで行くことを確かめる (前提の確認)
        let start = Instant::now();
        let mut saw_done = false;
        for _ in 0..120 {
            std::thread::sleep(Duration::from_millis(50));
            if t.tick(start).1 == TabState::Done {
                saw_done = true;
                break;
            }
        }
        assert!(saw_done, "起動しただけで DONE になる");

        // 誰も入力していないので、応答として扱ってはいけない
        assert!(
            !t.was_prompted(),
            "何も聞いていないのに応答完了として扱われている"
        );

        // 入力したあとの DONE は本物の応答
        t.write_bytes(b"echo hi\r").unwrap();
        assert!(t.was_prompted(), "入力したら応答を待つ状態になる");

        t.kill();
    }

    /// 起動直後の自動化は、プログラムが入力を受け取れるようになるまで待つこと。
    ///
    /// AI CLIは起動して入力欄を描き終わるまで入力を捨てる。
    /// すぐ流し込むと、設定した文章がどこにも入らない (実際に起きた)
    #[test]
    fn the_startup_hook_waits_until_the_program_settles() {
        use super::{Tab, TabOptions};
        use std::time::{Duration, Instant};

        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn("shell".into(), &argv, None, 20, 60, TabOptions::default()).unwrap();

        // まだ何も出ていない = 起動途中なので流し込まない
        assert!(!t.had_output(), "起動直後は無出力");
        assert!(!t.ready_for_startup_hook(), "無出力のうちは待つ");

        // 出力が出て画面が落ち着いたら準備完了
        let start = Instant::now();
        let mut became_ready = false;
        for _ in 0..120 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
            if t.ready_for_startup_hook() {
                became_ready = true;
                break;
            }
        }
        assert!(became_ready, "落ち着いたら準備完了になる");
        assert!(t.had_output(), "出力が出たことを根拠にしている");
        assert!(
            t.age_ms() < 15_000,
            "時間切れではなく、落ち着いたことで判定できている ({}ms)",
            t.age_ms()
        );

        t.kill();
    }

    #[test]
    fn bottom_status_rows_are_ignored() {
        let mut p = vt100::Parser::new(5, 20, 0);
        p.process(b"main content\r\n");
        let before = screen_hash(p.screen(), 2);
        // 最下行 (byobuの時計に相当) だけを書き換える
        p.process(b"\x1b[5;1H12:34:56");
        assert_eq!(before, screen_hash(p.screen(), 2), "最下部の変化は無視");
        // 本文が変わればハッシュも変わる
        p.process(b"\x1b[1;1Hchanged!");
        assert_ne!(before, screen_hash(p.screen(), 2));
    }
}

/// 起動条件の指紋。これが変われば新しいセッションを作らないと反映できない
pub fn signature_of(argv: &[String], opts: &TabOptions) -> String {
    format!(
        "{}|{}|{}|{}",
        argv.join(" "),
        opts.encoding.map(|e| e.name()).unwrap_or("UTF-8"),
        opts.scrollback,
        opts.cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    )
}

/// 波形の横幅 (サンプル数)。tickごとに1つ進む
pub const ACTIVITY_LEN: usize = 24;

pub struct Tab {
    pub title: String,
    /// 自動化から指すID (任意)。未設定ならタブ名で指す
    pub id: Option<String>,
    pub parser: SharedParser,
    pub writer: PtyWriter,
    pub state: TabState,
    pub spinner_idx: usize,
    pub copy: Option<CopyState>,
    /// 自動送信チェーンの深度 (透明のボールに記録された「渡された回数」。
    /// 自動送信で+1を継承、人間の手動入力で0にリセット)
    pub chain_depth: u32,
    /// 入力ロック (ソフトロック)。人間の誤入力を防ぐだけで、自動送信は通る
    pub locked: bool,
    /// 子プロセス終了時に自動再起動するか
    pub auto_restart: bool,
    /// 設定が変わったが、反映にはセッションの作り直しが必要な状態
    /// (実行中のAIを勝手に切らないため、再起動は利用者に委ねる)
    pub needs_restart: bool,
    /// タブバーの表示インデント段数 (0 = 親)
    pub depth: u16,
    /// 再起動用に起動条件を保持する (プロセス終了後に同じ設定で再spawnできる)
    argv: Vec<String>,
    profile_spec: Option<String>,
    opts: TabOptions,
    /// 最後に人間が手動入力した時刻 (相対ms)。直後の自動送信をガードする。
    ///
    /// None は「まだ一度も触られていない」。0 で表すと、アプリ起動から
    /// ガード時間のあいだ「たった今触った」と誤認してしまう
    pub last_manual_ms: Option<u64>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child_exited: Arc<AtomicBool>,
    bell_count: Arc<AtomicU64>,
    /// PTYから読んだ累計バイト数 (読み取りスレッドが加算する)
    bytes_out: Arc<AtomicU64>,
    /// このセッションを作った時刻。起動直後かどうかの判定に使う
    created: Instant,
    /// 直近でリサイズした時刻 (生成からの経過ms)。
    ///
    /// 端末の大きさが変わると子プロセスは画面を描き直す。中身は同じでも
    /// 画面は変化するので、そのまま活動と数えると BUSY→DONE を通り、
    /// 応答が来たように見えてしまう
    last_resize_ms: AtomicU64,
    /// 実行してから「作業中」の表示を見たか。
    ///
    /// 画面が変わったかどうかでは足りない。貼り付けが `[Pasted Content …]`
    /// の形に描き変わるだけでも画面は変わるが、AIは何もしていない
    saw_working: AtomicBool,
    /// 実行した時点の画面の中身 (ハッシュ)。
    ///
    /// 出力のバイト数だと、カーソルの点滅や枠の描き直しでも増えるので
    /// 「答えた」と数えてしまう。実行が届かなければ画面の中身は変わらない
    submitted_screen: AtomicU64,
    /// 実行した時点の累計出力量。
    ///
    /// 実行が相手に届かなかった場合、画面は貼り付けが見えたまま静かになる。
    /// 「動いて→止まった」形は応答と同じなので、実行より後に出力が
    /// あったかどうかまで見ないと、答えていないものを答えたと扱ってしまう
    submitted_output: AtomicU64,
    /// 実行された入力を待っているか。
    ///
    /// 「入力された」ではなく「実行された」であることが大事。文字を打っただけ、
    /// 貼り付けただけでも画面は動いて止まるので、入力を根拠にすると
    /// 手が止まった瞬間を応答完了と読んでしまう
    prompted: AtomicBool,
    /// 直近の出力量の履歴 (古い→新しい)。INDEXの波形用
    activity: [u8; ACTIVITY_LEN],
    /// 前回サンプル時点の累計バイト数
    activity_mark: u64,
    last_hash: u64,
    last_change_ms: u64,
    /// 最新応答のキャプチャ (DESIGN 7.3: 送信境界マーカー方式)
    pub last_response: Option<String>,
    /// 応答の開始位置 (スクロールバック蓄積量)。u64::MAX = 未設定。
    ///
    /// 「最初に画面が動いた位置」ではなく「実行した位置」であることが大事。
    /// 貼り付けの表示や入力欄の描き直しも画面を動かすので、動いた位置から
    /// 取ると、答えではなく枠を掴む
    response_marker: AtomicU64,
    /// 応答を待っている間に画面の幅が狭まったか。
    ///
    /// 行番号は大きさを変えても動かないので、切り出す範囲は保てる。
    /// だが幅を狭めると vt100 が各行をその幅で切り捨てるため、
    /// 文章そのものが欠ける。こちらでは戻せないので、
    /// せめて欠けたかもしれないことが分かるようにしておく
    resized_while_waiting: AtomicBool,
    /// 実行した瞬間の可視画面 (上から順の各行)。
    ///
    /// 全画面TUI (Claude Code 等) はスクロールしないので行番号が進まず、
    /// 「実行してから書かれた行」を数えられない。代わりに実行前の画面と
    /// 見比べて、当時から同じ場所にあったもの (起動バナー、枠) を外す
    submitted_rows: Mutex<Vec<String>>,
    detector: Detector,
}

impl Tab {
    /// プロファイルは名前 (config指定) かコマンド名から都度解決する。
    /// 再起動時に再解決されるので、profiles/*.json の修正が即反映される
    fn resolve_profile(argv: &[String], spec: &Option<String>) -> Profile {
        match spec {
            Some(name) => crate::profile::load_by_name(name),
            None => crate::profile::load_for_command(argv.first().map(String::as_str).unwrap_or("")),
        }
    }

    pub fn spawn(
        title: String,
        argv: &[String],
        profile_spec: Option<String>,
        rows: u16,
        cols: u16,
        opts: TabOptions,
    ) -> Result<Self> {
        let profile = Self::resolve_profile(argv, &profile_spec);
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut cmd = build_command(argv);
        // 指定が無い/存在しない場合はアプリのフォルダで起動する
        // (存在しないフォルダを渡すと起動そのものが失敗するため)
        let cwd = opts
            .cwd
            .clone()
            .filter(|p| p.is_dir())
            .unwrap_or(std::env::current_dir()?);
        cmd.cwd(cwd);
        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let killer = child.clone_killer();

        let writer: PtyWriter = Arc::new(Mutex::new(pair.master.take_writer()?));
        let bell_count = Arc::new(AtomicU64::new(0));
        // 出力量の累計。INDEXの波形はこれの増分から描く
        // (画面ハッシュの変化だけだと「動いている量」が分からない)
        let bytes_out = Arc::new(AtomicU64::new(0));
        let parser: SharedParser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            opts.scrollback,
            QueryResponder {
                writer: Arc::clone(&writer),
                bell: Arc::clone(&bell_count),
            },
        )));
        let child_exited = Arc::new(AtomicBool::new(false));

        // PTY出力 → (必要なら文字コード変換) → vt100パーサ / セッションログ
        {
            let parser = Arc::clone(&parser);
            let counter = Arc::clone(&bytes_out);
            let mut reader = pair.master.try_clone_reader()?;
            let enc = opts.encoding;
            let mut log = opts
                .log
                .then(|| crate::session_log::SessionLog::open(std::path::Path::new("logs"), &title));
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let mut decoder = enc.map(|e| e.new_decoder());
                let mut text = String::new();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            counter.fetch_add(n as u64, Ordering::Relaxed);
                            let chunk: &[u8] = match decoder.as_mut() {
                                // Shift_JIS等はUTF-8に直してからパーサへ渡す
                                Some(d) => {
                                    text.clear();
                                    text.reserve(n * 3);
                                    let _ = d.decode_to_string(&buf[..n], &mut text, false);
                                    text.as_bytes()
                                }
                                None => &buf[..n],
                            };
                            if let Some(l) = log.as_mut() {
                                l.write(chunk);
                            }
                            // vt100 は幅を狭めた後の全角の扱いで落ちることがある
                            // (実測: 右端が全角のまま縮めて半角を書くと unwrap)。
                            // 落ちたら解析器を作り直して読み取りを続ける。
                            // ここで諦めると、そのタブは以後何も映さなくなる
                            let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                || {
                                    parser
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .process(chunk);
                                },
                            ));
                            if hit.is_err() {
                                // 壊れた状態のまま読み続けると、次の1文字でまた落ちる。
                                // 端末の完全リセットを流して、既知の状態へ戻す
                                let reset = std::panic::catch_unwind(
                                    std::panic::AssertUnwindSafe(|| {
                                        parser
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner())
                                            .process(b"\x1bc");
                                    }),
                                );
                                crate::append_hook_log(if reset.is_ok() {
                                    "画面の解析が壊れたので作り直しました (幅の変更と全角文字)"
                                } else {
                                    "画面の解析が壊れ、作り直しにも失敗しました"
                                });
                            }
                        }
                    }
                }
            });
        }
        // 子プロセス終了検知
        {
            let flag = Arc::clone(&child_exited);
            std::thread::spawn(move || {
                let _ = child.wait();
                flag.store(true, Ordering::SeqCst);
            });
        }

        Ok(Self {
            title,
            id: None,
            parser,
            writer,
            state: TabState::Wait,
            spinner_idx: 0,
            copy: None,
            chain_depth: 0,
            locked: false,
            auto_restart: false,
            needs_restart: false,
            depth: 0,
            argv: argv.to_vec(),
            profile_spec,
            opts,
            last_manual_ms: None,
            master: pair.master,
            killer,
            child_exited,
            bell_count,
            bytes_out,
            created: Instant::now(),
            prompted: AtomicBool::new(false),
            submitted_output: AtomicU64::new(0),
            submitted_screen: AtomicU64::new(0),
            saw_working: AtomicBool::new(false),
            last_resize_ms: AtomicU64::new(0),
            activity: [0; ACTIVITY_LEN],
            activity_mark: 0,
            last_hash: 0,
            last_change_ms: 0,
            last_response: None,
            response_marker: AtomicU64::new(u64::MAX),
            resized_while_waiting: AtomicBool::new(false),
            submitted_rows: Mutex::new(Vec::new()),
            detector: Detector::new(profile),
        })
    }

    /// 実行された入力の応答を待っているか
    pub fn was_prompted(&self) -> bool {
        self.prompted.load(Ordering::Relaxed)
    }

    /// 今の画面の中身 (最下部の飾りは除く)
    fn screen_fingerprint(&self) -> u64 {
        let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        screen_hash(p.screen(), self.detector.ignore_bottom_rows())
    }

    /// 「働き始めた表示を見たか」の生の値 (ログ用)
    pub fn saw_working_flag(&self) -> bool {
        self.saw_working.load(Ordering::Relaxed)
    }

    /// 実行が相手に届き、実際に応答が始まったか。
    ///
    /// AI CLIは働いている間それを画面に出す (「esc to interrupt」など)。
    /// 実行が届かなければ、貼り付けが入力欄に乗ったままで、その表示は出ない。
    /// 画面が変わったかどうかでは足りない ——
    /// 貼り付けが `[Pasted Content …]` に描き変わるだけでも画面は変わる。
    ///
    /// 作業中の表示を持たない相手 (素のシェル、プロファイル未設定) は、
    /// 実行後の描き直しが落ち着いてからの画面変化で代用する。
    ///
    /// プロファイルの有無で守りの強さが変わらないようにしてある。
    /// 以前は未設定なら判定が丸ごと弱くなり、貼り付けの描き直しだけで
    /// 「答えた」ことになっていた
    pub fn answered_since_submit(&self) -> bool {
        if self.detector.shows_working() {
            // 作業中を画面に出す相手なら、それが出たかどうかで判断する。
            // 画面が動いたかどうかより確かで、描き直しに騙されない
            return self.saw_working.load(Ordering::Relaxed);
        }
        // 出さない相手 (素のシェル、プロファイル未設定) は画面の変化で代用する。
        // 基準は実行を送った瞬間の画面で、実行は貼り付けの取り込みが
        // 終わってから送るので、この時点の画面は既に落ち着いている。
        // 届かなければ何も動かず、届けば答えが現れる
        self.screen_fingerprint() != self.submitted_screen.load(Ordering::Relaxed)
    }

    /// 応答を受け取り切ったので、次の実行を待つ状態に戻す
    pub fn finish_response(&mut self) {
        self.prompted.store(false, Ordering::Relaxed);
        self.response_marker.store(u64::MAX, Ordering::Relaxed);
    }

    /// 子プロセスへそのまま流すだけの入力 (マウス報告など)。
    /// 応答を求める入力ではないので prompted は立てない
    pub fn write_passthrough(&self, bytes: &[u8]) -> Result<()> {
        pty_write(&self.writer, bytes)
    }

    /// 入力を送る。実行を含むものだけが「応答を求めた」ことになる
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        if contains_submit(bytes) {
            self.prompted.store(true, Ordering::Relaxed);
            self.submitted_output
                .store(self.output_count(), Ordering::Relaxed);
            // 応答はここから始まる。これより前は指示であって答えではない。
            //
            // +1 は、実行がまさに今の行を書き終える動作だから。
            // カーソル行そのものを起点にすると、打ち込んだ指示の
            // 最後の1行 (折り返していればその後半だけ) を答えに含めてしまう
            self.response_marker
                .store(self.line_position() as u64 + 1, Ordering::Relaxed);
            self.submitted_screen
                .store(self.screen_fingerprint(), Ordering::Relaxed);
            self.saw_working.store(false, Ordering::Relaxed);
            self.resized_while_waiting.store(false, Ordering::Relaxed);
            *self.submitted_rows.lock().unwrap() = self.visible_rows();
        }
        // 相手がUTF-8以外なら、送る文字も変換する
        // (制御シーケンスはASCIIなのでそのまま通る)
        if let Some(enc) = self.opts.encoding {
            if let Ok(s) = std::str::from_utf8(bytes) {
                let (encoded, _, _) = enc.encode(s);
                return pty_write(&self.writer, &encoded);
            }
        }
        pty_write(&self.writer, bytes)
    }

    /// 再描画が届いているあいだか。
    ///
    /// 待つのは再描画そのものが終わるまでで、落ち着くまでではない。
    /// 長く止めると本物の応答の始まりを取りこぼす
    fn redrawing(&self) -> bool {
        const REDRAW_MS: u64 = 800;
        self.age_ms()
            .saturating_sub(self.last_resize_ms.load(Ordering::Relaxed))
            < REDRAW_MS
    }

    /// 相手が括弧貼り付けを理解すると宣言しているか。
    ///
    /// 端末の規格では、対応するアプリが自分で ESC[?2004h を送って申告する。
    /// 申告していない相手 (素のシェル) に目印付きで送ると、目印は無視され、
    /// 中の改行がそのまま実行になる。推測ではなく、この申告で判断する
    pub fn accepts_bracketed_paste(&self) -> bool {
        self.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste()
    }

    /// 応答を待つ間に幅が狭まったか (文章が欠けている恐れがある)
    pub fn resized_while_waiting(&self) -> bool {
        self.resized_while_waiting.load(Ordering::Relaxed)
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let narrower = {
            let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            let (_, old_cols) = p.screen().size();
            cols < old_cols
        };
        // 中身が失われるのは幅が狭まったときだけ。高さは行番号にも中身にも触らない
        if narrower && self.prompted.load(Ordering::Relaxed) {
            self.resized_while_waiting.store(true, Ordering::Relaxed);
        }
        self.last_resize_ms.store(self.age_ms(), Ordering::Relaxed);
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.parser.lock().unwrap_or_else(|e| e.into_inner()).screen_mut().set_size(rows, cols);
        Ok(())
    }

    pub fn exited(&self) -> bool {
        self.child_exited.load(Ordering::SeqCst)
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }

    /// 同じ設定でセッションを作り直す。
    /// 子プロセスの自己更新・SSH切断・クラッシュからの復帰に使う。
    /// ロック状態と階層表示は引き継ぎ、チェーン深度と履歴はリセットされる
    pub fn restart(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.kill();
        let mut fresh = Tab::spawn(
            self.title.clone(),
            &self.argv.clone(),
            self.profile_spec.clone(),
            rows,
            cols,
            self.opts.clone(),
        )?;
        fresh.locked = self.locked;
        fresh.depth = self.depth;
        fresh.auto_restart = self.auto_restart;
        fresh.id = self.id.clone();
        // 作り直したので、保留していた設定変更は反映済みになる
        *self = fresh;
        Ok(())
    }

    pub fn profile_name(&self) -> &str {
        self.detector.profile_name()
    }

    /// 起動条件の指紋。これが変わるとセッションの作り直しが必要
    pub fn signature(&self) -> String {
        signature_of(&self.argv, &self.opts)
    }

    /// 自動化から見たこのタブの見分け方
    pub fn key(&self) -> crate::hooks::TabKey {
        crate::hooks::TabKey {
            id: self.id.clone(),
            name: self.title.clone(),
        }
    }

    /// 再起動せずに反映できる設定を差し替える
    pub fn apply_live_config(&mut self, profile_spec: Option<String>, locked: bool, auto_restart: bool, depth: u16) {
        if self.profile_spec != profile_spec {
            self.profile_spec = profile_spec;
            self.detector = Detector::new(Self::resolve_profile(&self.argv, &self.profile_spec));
        }
        self.locked = locked;
        self.auto_restart = auto_restart;
        self.depth = depth;
    }

    /// 次回の再起動で使う起動条件を控えておく (再起動するまでは現行のまま動く)
    pub fn stage_restart_config(&mut self, argv: Vec<String>, opts: TabOptions) {
        self.argv = argv;
        self.opts = opts;
        self.needs_restart = true;
    }

    /// 200ms毎の状態判定 (非アクティブタブも含めて呼ぶこと)。
    /// 活動の有無は「画面内容の変化」で判定する (最下部のステータス行は除外)。
    /// 戻り値はフック発火用の (旧状態, 新状態)
    pub fn tick(&mut self, start: Instant) -> (TabState, TabState) {
        if self.exited() {
            let old = self.state;
            self.state = TabState::Exited;
            return (old, self.state);
        }
        let (screen_text, hash) = {
            let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            let screen = p.screen();
            (
                screen.contents(),
                screen_hash(screen, self.detector.ignore_bottom_rows()),
            )
        };
        let now = start.elapsed().as_millis() as u64;
        if hash != self.last_hash {
            self.last_hash = hash;
            // リサイズ後の描き直しは新しい出力ではないので、活動と数えない
            if !self.redrawing() {
                self.last_change_ms = now;
            }
        }
        let since = now.saturating_sub(self.last_change_ms);
        let old_state = self.state;
        self.state = self
            .detector
            .tick(&screen_text, since, self.bell_count.load(Ordering::Relaxed));
        if self.state == TabState::Busy {
            self.spinner_idx = self.spinner_idx.wrapping_add(1);
        }
        if self.detector.working_shown() {
            if !self.saw_working.swap(true, Ordering::Relaxed) {
                // 何を根拠に「働き始めた」と見たのかを残す。
                // 画面の飾りを拾っていた場合、ここに出る
                crate::append_hook_log(&format!(
                    "working tab? [{}] マッチ: {:?}",
                    self.detector.profile_name(),
                    self.detector.working_matched()
                ));
            }
        }
        self.sample_activity();

        // 応答キャプチャ (送信境界マーカー方式):
        // BUSY開始時点のスクロールバック蓄積量を境界として記録し、
        // DONEでその境界以降だけを抽出する (過去の応答は混ざらない)
        if old_state == TabState::Busy && self.state == TabState::Done {
            self.last_response = Some(self.capture_since_marker());
        }
        (old_state, self.state)
    }

    /// この tick の出力量を履歴に1つ積む。
    /// 生バイト数は桁が振れすぎるので、対数で 0..=7 段に潰す
    /// (「静か / ぽつぽつ / 流れている」が読めれば十分なので)
    fn sample_activity(&mut self) {
        let total = self.bytes_out.load(Ordering::Relaxed);
        let delta = total.saturating_sub(self.activity_mark);
        self.activity_mark = total;
        let level = match delta {
            0 => 0,
            1..=31 => 1,
            32..=127 => 2,
            128..=511 => 3,
            512..=2047 => 4,
            2048..=8191 => 5,
            8192..=32767 => 6,
            _ => 7,
        };
        self.activity.rotate_left(1);
        self.activity[ACTIVITY_LEN - 1] = level;
    }

    /// このAI固有の確認時間 (プロファイルに指定があれば)
    pub fn done_confirm_ms(&self) -> Option<u64> {
        self.detector.done_confirm_ms()
    }

    /// 直近の出力量 (古い→新しい、各 0..=7)
    pub fn activity(&self) -> &[u8] {
        &self.activity
    }

    /// 子プロセスが何か出力したか (起動して動き出したかの目安)
    pub fn had_output(&self) -> bool {
        self.output_count() > 0
    }

    /// PTYから読んだ累計バイト数。ある時点からの反応の有無を見るのに使う
    pub fn output_count(&self) -> u64 {
        self.bytes_out.load(Ordering::Relaxed)
    }

    /// このセッションを作ってからの経過ミリ秒
    pub fn age_ms(&self) -> u64 {
        self.created.elapsed().as_millis() as u64
    }

    /// 起動直後の自動化 (on_start) を流し込んでよい状態か。
    ///
    /// AI CLIは起動して入力欄を描き終わるまで入力を受け取らない。
    /// 出力が出て、かつ画面が落ち着いたところを「準備できた」とみなす。
    /// 何も出力しないプログラムのために時間切れも設ける
    pub fn ready_for_startup_hook(&self) -> bool {
        const GIVE_UP_MS: u64 = 15_000;
        (self.had_output() && self.state != TabState::Busy) || self.age_ms() > GIVE_UP_MS
    }

    /// これまでに書かれた行数の目安。
    ///
    /// 流れて消えた行数だけでは、画面に収まっている間ずっと 0 のままで
    /// 「どこから書かれたか」が分からない。画面内のカーソル位置を足して、
    /// 出力が進むほど増える値にする
    pub fn line_position(&self) -> usize {
        let mut p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let saved = p.screen().scrollback();
        p.screen_mut().set_scrollback(usize::MAX / 2);
        let scrolled = p.screen().scrollback();
        p.screen_mut().set_scrollback(saved);
        let (row, _) = p.screen().cursor_position();
        scrolled + row as usize
    }

    /// マーカー以降の新規出力をテキスト化する
    /// 試験から切り出しをそのまま覗くための入口
    #[cfg(test)]
    pub fn capture_for_probe(&self) -> String {
        self.capture_since_marker()
    }

    /// 今の可視画面を、上から順に1行ずつ
    fn visible_rows(&self) -> Vec<String> {
        let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let screen = p.screen();
        let (rows, cols) = screen.size();
        screen.rows(0, cols).take(rows as usize).collect()
    }

    /// 実行してから変わった行の範囲を返す (上からの行番号で lo..hi)。
    ///
    /// 見比べるのは実行の瞬間に自分で撮った画面なので、文言を決め打ちしない。
    /// CLI が見た目を変えても、日本語でも英語でも、そのまま効く。
    /// 上端は起動バナー、下端は入力欄の枠が外れる。
    ///
    /// 端から順に、食い違ったところで止める。真ん中は触らないので、
    /// 答えの中にたまたま実行前と同じ行があっても穴は空かない
    pub fn changed_span(before: &[String], now: &[String]) -> (usize, usize) {
        let same = |a: &String, b: &String| a.trim_end() == b.trim_end();
        let head = now.iter().zip(before).take_while(|(a, b)| same(a, b)).count();
        if head >= now.len() {
            // 何ひとつ変わっていない = 答えは届いていない
            return (0, 0);
        }
        // 行数が違うと下端どうしが対応しないので、そのときは上端だけで判断する
        let tail = if before.len() == now.len() {
            now.iter()
                .rev()
                .zip(before.iter().rev())
                .take_while(|(a, b)| same(a, b))
                .count()
        } else {
            0
        };
        (head, now.len().saturating_sub(tail).max(head))
    }

    /// 実行の瞬間から動いていない「貼り付いた枠」が、下端に何行あるか。
    ///
    /// カーソルより下は位置だけで枠と分かる。その内側は実行前の画面と
    /// 見比べる。入力欄そのもの (カーソル行) は実行で中身が消えるため
    /// 必ず食い違うが、これは答えではないので走査を止めさせない。
    ///
    /// Codex は例文をカーソル行に出す ("Implement {feature}")。
    /// スクロールする相手でも枠は貼り付いたままなので、同じ理屈で外せる
    fn pinned_rows(&self, rows: u16, cols: u16, cursor_row: u16, floor: usize) -> usize {
        let keep = (rows as usize).saturating_sub(floor);
        let mut now: Vec<String> = {
            let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            p.screen().rows(0, cols).take(keep).collect()
        };
        let mut before = self.submitted_rows.lock().unwrap().clone();
        before.truncate(keep);
        if let (Some(a), Some(b)) = (
            now.get_mut(cursor_row as usize),
            before.get(cursor_row as usize),
        ) {
            *a = b.clone();
        }
        let (_, end) = Self::changed_span(&before, &now);
        // end = 変わった行の下端。そこから下は貼り付いた枠
        (rows as usize).saturating_sub(end).max(floor)
    }

    fn capture_since_marker(&self) -> String {
        let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let (rows, cols) = p.screen().size();
        // カーソルは入力欄の中にある。その下にあるのは、答えではなく枠。
        // ヒント行 ("Use /skills to list available skills") や
        // ステータス行 ("gpt-5.5 medium  D:\\Test") がここに住んでいる
        let (cursor_row, _) = p.screen().cursor_position();
        if p.screen().alternate_screen() {
            // 全画面TUIはスクロールしないため可視画面のスナップショットで代替。
            // ただし実行前から出ていたもの (起動バナー等) は答えではない
            let (floor, _) = capture_range(rows, cursor_row, 0);
            let keep = (rows as usize).saturating_sub(floor);
            let now: Vec<String> = p.screen().rows(0, cols).take(keep).collect();
            let mut before = self.submitted_rows.lock().unwrap().clone();
            before.truncate(keep);
            // 上端は実行前から出ていたもの (起動バナー) を外す
            let (start, _) = Self::changed_span(&before, &now);
            drop(p);
            let lo = self.pinned_rows(rows, cols, cursor_row, floor);
            let hi = (rows as usize).saturating_sub(1).saturating_sub(start);
            if lo > hi {
                return String::new();
            }
            let mut p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            let text = extract_text(&mut p, lo, hi, cols);
            return text.trim_end().to_string();
        }
        drop(p);

        // 実行してから書かれた行だけを取る。
        // ここに画面の高さを足すと、必ず1画面ぶん (起動バナーや入力欄) が
        // 混ざり、答えの代わりに枠を渡すことになる
        let stored = self.response_marker.load(Ordering::Relaxed);
        let since = if stored == u64::MAX {
            rows.saturating_sub(1) as usize
        } else {
            self.line_position().saturating_sub(stored as usize)
        };
        let (floor, hi) = capture_range(rows, cursor_row, since);
        // 下端の枠は、スクロールしても貼り付いたまま残る
        let lo = self.pinned_rows(rows, cols, cursor_row, floor);
        if lo > hi {
            return String::new();
        }
        let mut p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let text = extract_text(&mut p, lo, hi, cols);
        text.trim_end().to_string()
    }
}



#[cfg(test)]
mod real_codex_probe {
    use super::{Tab, TabOptions};
    use std::io::Write as _;
    use std::time::{Duration, Instant};

    /// 本物の Codex に貼り付けて実行し、何が起きるかを全部書き出す。
    ///
    ///   cargo test probe_real_codex -- --ignored --nocapture
    ///
    /// 偽物で代用すると「こちらが思う振る舞い」しか確かめられないので、
    /// 実機で観測する
    #[test]
    #[ignore]
    fn probe_real_codex() {
        let dir = std::env::temp_dir().join("shikisha-codex-probe");
        let _ = std::fs::create_dir_all(&dir);
        let out_path = dir.join("probe.txt");
        let mut log = std::fs::File::create(&out_path).unwrap();

        let argv = vec!["codex".to_string()];
        let opts = TabOptions {
            cwd: Some(dir.clone()),
            ..TabOptions::default()
        };
        let mut t = Tab::spawn("codex".into(), &argv, Some("codex".into()), 30, 100, opts).unwrap();
        let start = Instant::now();

        let snap = |t: &mut Tab, log: &mut std::fs::File, phase: &str| {
            let (old, new) = t.tick(start);
            let screen = super::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen());
            let tail: Vec<&str> = screen
                .lines()
                .filter(|l| !l.trim().is_empty())
                .rev()
                .take(6)
                .collect();
            let _ = writeln!(
                log,
                "[{:>6}ms] {phase} 状態={}->{} prompted={} working見た={} マッチ={:?} 応答あり={} 出力={}\n    画面末尾: {:?}",
                start.elapsed().as_millis(),
                old.label(),
                new.label(),
                t.was_prompted(),
                t.saw_working_flag(),
                t.detector.working_matched(),
                t.answered_since_submit(),
                t.output_count(),
                tail
            );
        };

        // 起動を待つ。信頼確認が出たら答える
        let mut trusted = false;
        for _ in 0..80 {
            std::thread::sleep(Duration::from_millis(200));
            snap(&mut t, &mut log, "起動中");
            let screen = super::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen());
            if !trusted && screen.contains("Do you trust") {
                let _ = writeln!(log, "=== 信頼確認に 1 を返す ===");
                t.write_bytes(b"1\r").unwrap();
                trusted = true;
            }
            if trusted && screen.contains("Pasted") {
                break;
            }
        }
        // 落ち着くまで待つ
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(200));
            snap(&mut t, &mut log, "待機中");
        }
        let _ = writeln!(log, "=== 待機時の画面全体 ===\n{}",
                         super::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen()));

        // 利用者の事例と同じくらいの長さを貼り付ける
        let body = format!(
            "これはテストです。返事は OK の一言だけにしてください。{}",
            "あ".repeat(1900)
        );
        let bracketed = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste();
        let _ = writeln!(log, "=== 貼り付け ({}文字) 括弧付き貼り付け={} ===", body.chars().count(), bracketed);
        let mut bytes = Vec::new();
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(body.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
        } else {
            bytes.extend_from_slice(body.as_bytes());
        }
        t.write_bytes(&bytes).unwrap();

        // 貼り付けの取り込みが「終わる」まで待つ (始まるまで、ではない)
        let mut last = t.output_count();
        let mut quiet = 0;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(100));
            snap(&mut t, &mut log, "貼付後");
            let now = t.output_count();
            if now == last {
                quiet += 1;
                if quiet >= 4 {
                    break;
                }
            } else {
                quiet = 0;
                last = now;
            }
        }
        let _ = writeln!(log, "=== 貼り付けの取り込みが落ち着いた (出力={}) ===", t.output_count());

        let _ = writeln!(log, "=== 実行 (Enter) ===");
        t.write_bytes(b"\r").unwrap();

        // 実行後の様子を長めに追う
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(200));
            snap(&mut t, &mut log, "実行後");
        }

        let _ = writeln!(log, "=== 最終画面 ===\n{}", super::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen()));
        let _ = writeln!(log, "=== 取り込んだ応答 ===\n{:?}", t.last_response);
        t.kill();
        println!("書き出し: {}", out_path.display());
    }
}

#[cfg(test)]
mod layout_probe {
    use super::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    /// 起動直後の入力欄の形を、行番号とカーソル位置つきで書き出す。
    ///
    ///   cargo test layout_probe -- --ignored --nocapture
    ///
    /// 「カーソルより下を外す」で足りるのか、枠の上辺が残るのかを
    /// 本物で確かめる。推測で足すと、また効かない調整をすることになる
    #[test]
    #[ignore]
    fn probe_real_input_box_layout() {
        for cmd in ["codex", "claude"] {
            println!("\n================ {cmd} ================");
            let tab = match Tab::spawn(
                cmd.to_string(),
                &[cmd.to_string()],
                None,
                24,
                100,
                TabOptions::default(),
            ) {
                Ok(t) => t,
                Err(e) => {
                    println!("起動できず: {e}");
                    continue;
                }
            };
            // 枠を描き終えるまで待つ (出力が止まったら描き終わり)
            let start = Instant::now();
            let mut last = 0u64;
            let mut quiet = Instant::now();
            while start.elapsed() < Duration::from_secs(40) {
                std::thread::sleep(Duration::from_millis(200));
                let now = tab.output_count();
                if now != last {
                    last = now;
                    quiet = Instant::now();
                } else if last > 0 && quiet.elapsed() > Duration::from_secs(3) {
                    break;
                }
            }

            let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
            let screen = p.screen();
            let (rows, cols) = screen.size();
            let (cur_row, cur_col) = screen.cursor_position();
            println!("画面 {rows}行 x {cols}桁 / カーソル row={cur_row} col={cur_col}");
            println!("alternate_screen = {}", screen.alternate_screen());
            println!("カーソルより下: {} 行", rows - 1 - cur_row);
            println!("--- 全 {rows} 行 (深さ: 内容) ---");
            for r in (0..rows).rev() {
                let line = screen.rows(0, cols).nth(r as usize).unwrap_or_default();
                let depth = rows - 1 - r;
                let mark = if r == cur_row { " <== カーソル" } else { "" };
                println!("深さ{depth:>2} | {}{mark}", line.trim_end());
            }
        }
    }
}

#[cfg(test)]
mod capture_probe {
    use super::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    /// 本物に短い質問をして、切り出し結果を一字一句そのまま書き出す。
    ///
    ///   cargo test capture_probe -- --ignored --nocapture
    ///
    /// 知りたいのは「答えの本文が、範囲のどこから始まるか」。
    /// 枠の高さぶん頭が欠けているなら、下端を削るだけでは足りない
    #[test]
    #[ignore]
    fn probe_what_the_capture_actually_grabs() {
        let tab = Tab::spawn(
            "claude".into(),
            &["claude".to_string()],
            None,
            24,
            100,
            TabOptions::default(),
        )
        .expect("起動");

        // 枠を描き終えるまで待つ
        let quiet_for = |tab: &Tab, ms: u64, cap: u64| {
            let start = Instant::now();
            let mut last = 0u64;
            let mut quiet = Instant::now();
            while start.elapsed() < Duration::from_secs(cap) {
                std::thread::sleep(Duration::from_millis(200));
                let now = tab.output_count();
                if now != last {
                    last = now;
                    quiet = Instant::now();
                } else if last > 0 && quiet.elapsed() > Duration::from_millis(ms) {
                    return true;
                }
            }
            false
        };
        assert!(quiet_for(&tab, 3000, 60), "起動しない");

        // 括弧貼り付けで入れて、落ち着いてから実行 (本番と同じ順序)
        let q = "Reply with exactly three lines: AAA then BBB then CCC. Nothing else.";
        tab.write_passthrough(b"\x1b[200~").unwrap();
        tab.write_passthrough(q.as_bytes()).unwrap();
        tab.write_passthrough(b"\x1b[201~").unwrap();
        assert!(quiet_for(&tab, 600, 20), "貼り付けが落ち着かない");

        println!("=== 実行の直前 ===");
        {
            let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
            let (rows, _) = p.screen().size();
            let (cur, _) = p.screen().cursor_position();
            println!("rows={rows} cursor_row={cur} below={}", rows - 1 - cur);
        }
        println!("line_position = {}", tab.line_position());

        tab.write_bytes(b"\r").unwrap();
        assert!(quiet_for(&tab, 5000, 120), "答えが返らない");

        println!("=== 実行の直後 ===");
        {
            let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
            let (rows, _) = p.screen().size();
            let (cur, _) = p.screen().cursor_position();
            println!("rows={rows} cursor_row={cur} below={}", rows - 1 - cur);
        }
        println!("line_position = {}", tab.line_position());

        let got = tab.capture_for_probe();
        println!("\n=== 切り出し結果 ({} 行) ===", got.lines().count());
        for (i, l) in got.lines().enumerate() {
            println!("{i:>2} | {l}");
        }
        println!("=== ここまで ===");
        println!("AAA を含む: {}", got.contains("AAA"));
        println!("CCC を含む: {}", got.contains("CCC"));
    }
}

#[cfg(test)]
mod turns_probe {
    use super::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    /// 答えとして渡すのは、答えだけであること。本物の端末で確かめる。
    ///
    /// 利用者が見たのは、相手へ渡した文章の先頭に
    /// 「派閥として相手を１００文字以内で論破してください」だけが
    /// ぽつんと付いてくる現象。折り返した指示の後半だった。
    ///
    /// 実行はその行を書き終える動作なので、カーソル行を起点にすると
    /// 指示の最後の1行を巻き込む。2往復目以降でしか出ないので、
    /// 1回きりの試験では捕まらない
    #[test]
    fn the_instruction_is_not_sent_back_as_part_of_the_answer() {
        let mut tab = Tab::spawn(
            "cmd".into(),
            &["cmd.exe".to_string()],
            None,
            24,
            100,
            TabOptions::default(),
        )
        .expect("起動");

        let settle = |tab: &Tab| {
            let start = Instant::now();
            let mut last = 0u64;
            let mut quiet = Instant::now();
            while start.elapsed() < Duration::from_secs(20) {
                std::thread::sleep(Duration::from_millis(100));
                let now = tab.output_count();
                if now != last {
                    last = now;
                    quiet = Instant::now();
                } else if last > 0 && quiet.elapsed() > Duration::from_millis(700) {
                    return;
                }
            }
        };
        settle(&tab);

        // 100桁を超えて折り返す長さにする (利用者のプロンプトと同じ性質)
        let long = |n: usize| {
            format!(
                "echo TURN{n}-ドラクエ５のビアンカを妻にすべきかフローラを妻にすべきか議論をしてもらいますあなたはビアンカ派閥として相手を１００文字以内で論破してください-END{n}"
            )
        };

        for turn in 1..=3 {
            let text = long(turn);
            tab.write_passthrough(b"\x1b[200~").unwrap();
            tab.write_passthrough(text.as_bytes()).unwrap();
            tab.write_passthrough(b"\x1b[201~").unwrap();
            settle(&tab);

            let marker_before = tab.line_position();
            tab.write_bytes(b"\r").unwrap();
            settle(&tab);

            let got = tab.capture_for_probe();
            let _ = marker_before;
            assert!(
                got.contains(&format!("TURN{turn}-")) && got.contains(&format!("END{turn}")),
                "{turn}回目の答えが丸ごと入っていない: {got:?}"
            );
            assert!(
                got.trim_start().starts_with(&format!("TURN{turn}-")),
                "指示の折り返しの後半が頭に付いている: {got:?}"
            );
            for prev in 1..turn {
                assert!(
                    !got.contains(&format!("END{prev}")),
                    "{prev}回目の残りを拾っている: {got:?}"
                );
            }
            tab.finish_response();
        }
    }
}

#[cfg(test)]
mod paste_no_submit_probe {
    use super::{Tab, TabOptions};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    /// 括弧貼り付けだけを送ると、入力欄に入って止まること。
    ///
    /// 自動化から「下書きだけ入れて、送信は人が決める」を作るための土台。
    /// 実測: 短い本文はそのまま見え、長いと [Pasted text #1 +N lines] に畳まれる
    ///
    ///   cargo test paste_no_submit -- --ignored --nocapture
    ///
    /// APIは呼ばれないので消費はない。貼るだけで止まるかを実機で見る
    #[test]
    #[ignore]
    fn probe_paste_without_enter_stays_unsent() {
        let tab = Tab::spawn("claude".into(), &["claude".to_string()], None, 24, 100,
                             TabOptions::default()).expect("起動");
        let settle = |ms: u64, cap: u64| {
            let start = Instant::now();
            let (mut last, mut quiet) = (0u64, Instant::now());
            while start.elapsed() < Duration::from_secs(cap) {
                std::thread::sleep(Duration::from_millis(200));
                let n = tab.output_count();
                if n != last { last = n; quiet = Instant::now(); }
                else if last > 0 && quiet.elapsed() > Duration::from_millis(ms) { return; }
            }
        };
        settle(3000, 60);

        // Lua から作られるのと同じ形: ESC[200~ 本文 ESC[201~
        let body = "lp.html を読んでください。\n\n";
        let payload = format!("\x1b[200~{body}\x1b[201~");
        tab.write_bytes(payload.as_bytes()).unwrap();
        settle(1500, 20);

        // これが true になると、送っていないのに応答待ちが始まり、
        // on_done が空振りで撃たれる
        assert!(
            !tab.prompted.load(Ordering::Relaxed),
            "括弧貼り付けだけで送信扱いになっている"
        );
        let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
        let (rows, cols) = p.screen().size();
        println!("--- 下から8行 ---");
        for r in rows.saturating_sub(8)..rows {
            let l = p.screen().rows(0, cols).nth(r as usize).unwrap_or_default();
            println!("| {}", l.trim_end());
        }
    }
}

#[cfg(test)]
mod draft_target_tests {
    use super::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    fn settle(tab: &Tab, quiet_ms: u64, cap_s: u64) {
        let start = Instant::now();
        let (mut last, mut quiet) = (0u64, Instant::now());
        while start.elapsed() < Duration::from_secs(cap_s) {
            std::thread::sleep(Duration::from_millis(150));
            let n = tab.output_count();
            if n != last {
                last = n;
                quiet = Instant::now();
            } else if last > 0 && quiet.elapsed() > Duration::from_millis(quiet_ms) {
                return;
            }
        }
    }

    fn spawn(cmd: &str) -> Tab {
        Tab::spawn(
            cmd.into(),
            &[cmd.to_string()],
            None,
            24,
            100,
            TabOptions::default(),
        )
        .expect("起動")
    }

    /// 下書きをシェルへ置かないこと。
    ///
    /// 目印を理解しない相手に送ると、目印は無視され、中の改行が
    /// そのまま実行になる。実測 (cmd.exe): 目印で囲んだ `echo HELLO` に
    /// 復帰を付けたら、そのまま実行された。
    ///
    /// 見た目やプロファイル名で決めてはいけない。規格では、対応する
    /// アプリが自分で ESC[?2004h と申告する。それを読む
    #[test]
    fn a_shell_is_never_given_a_draft() {
        let tab = spawn("cmd.exe");
        settle(&tab, 700, 15);
        assert!(
            !tab.accepts_bracketed_paste(),
            "シェルを下書きの宛先と見なしている"
        );
    }

    /// AI CLI には置けること。
    ///
    ///   cargo test a_draft_reaches -- --ignored
    ///
    /// 実測: cmd.exe = false / powershell.exe = false / claude = true
    #[test]
    #[ignore]
    fn a_draft_reaches_an_ai_cli() {
        let tab = spawn("claude");
        settle(&tab, 2500, 60);
        assert!(
            tab.accepts_bracketed_paste(),
            "AI CLI が下書きを受け取れないことになっている"
        );
    }
}

#[cfg(test)]
mod resize_survival_tests {
    use super::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    fn settle(tab: &Tab, ms: u64) {
        let start = Instant::now();
        let (mut last, mut quiet) = (0u64, Instant::now());
        while start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(60));
            let n = tab.output_count();
            if n != last {
                last = n;
                quiet = Instant::now();
            } else if last > 0 && quiet.elapsed() > Duration::from_millis(ms) {
                return;
            }
        }
    }

    /// 幅を狭めても、画面が死なないこと。
    ///
    /// vt100 0.16.2 は「全角があるなら次のマスもある」と仮定して
    /// unwrap しており、右端が全角のまま狭めて半角を書くと落ちる
    /// (総当たりで 848 通り中 24 通り)。窓の幅を追いかける以上、
    /// 日本語を出しながら枠を引けば必ず通る道になる。
    ///
    /// 落ちた解析器は完全リセットで立て直す。錠前が毒されても
    /// 連鎖して死なない (毒された側を取り出して続ける)
    #[test]
    fn narrowing_the_window_does_not_kill_the_screen() {
        let tab = Tab::spawn(
            "cmd".into(),
            &["cmd.exe".to_string()],
            None,
            8,
            40,
            TabOptions::default(),
        )
        .expect("起動");
        settle(&tab, 500);

        for to in [20u16, 7, 5, 11, 60] {
            // 右端まで全角で埋める
            tab.write_passthrough("あいうえおかきくけこさしすせそたちつてと".as_bytes())
                .unwrap();
            settle(&tab, 300);
            tab.resize(8, to).unwrap();
            // 狭めた直後の半角文字が、落とす引き金だった。
            // 改行まで送って行を片付ける (残すと次のコマンドとくっつく)
            tab.write_passthrough(b"x\r").unwrap();
            settle(&tab, 300);

            // 生きていれば画面が読める (毒された錠前でも取り出せる)
            let text = {
                let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
                crate::tab::visible_text(p.screen())
            };
            assert!(
                text.lines().count() > 0,
                "幅 {to} で画面が読めなくなった"
            );
        }

        // 最後まで出力を受け取り続けていること
        tab.write_passthrough(b"echo ALIVE\r").unwrap();
        settle(&tab, 500);
        let text = {
            let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
            crate::tab::visible_text(p.screen())
        };
        assert!(
            text.contains("ALIVE"),
            "縮めた後に読み取りが止まっている: {text:?}"
        );
    }
}

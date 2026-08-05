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
    /// 人か自動化が、このタブに何か入力したか。
    ///
    /// 起動時のバナー出力だけでも画面は「動いて→止まる」ので、状態は必ず
    /// DONE を通る。応答を待っている相手が居ないのに応答完了として扱うと、
    /// 誰も聞いていない出力が自動化で転送されてしまう
    prompted: AtomicBool,
    /// 直近の出力量の履歴 (古い→新しい)。INDEXの波形用
    activity: [u8; ACTIVITY_LEN],
    /// 前回サンプル時点の累計バイト数
    activity_mark: u64,
    last_hash: u64,
    last_change_ms: u64,
    /// 最新応答のキャプチャ (DESIGN 7.3: 送信境界マーカー方式)
    pub last_response: Option<String>,
    /// BUSY遷移時のスクロールバック蓄積量 (応答の開始境界)
    response_marker: Option<usize>,
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
                            parser.lock().unwrap().process(chunk);
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
            last_resize_ms: AtomicU64::new(0),
            activity: [0; ACTIVITY_LEN],
            activity_mark: 0,
            last_hash: 0,
            last_change_ms: 0,
            last_response: None,
            response_marker: None,
            detector: Detector::new(profile),
        })
    }

    /// 誰かがこのタブに入力したか。応答完了として扱ってよいかの判定に使う
    pub fn was_prompted(&self) -> bool {
        self.prompted.load(Ordering::Relaxed)
    }

    /// 子プロセスへそのまま流すだけの入力 (マウス報告など)。
    /// 応答を求める入力ではないので prompted は立てない
    pub fn write_passthrough(&self, bytes: &[u8]) -> Result<()> {
        pty_write(&self.writer, bytes)
    }

    /// 入力を送る。ここを通ったものは「応答を求めた」とみなす
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        self.prompted.store(true, Ordering::Relaxed);
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

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.last_resize_ms.store(self.age_ms(), Ordering::Relaxed);
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.parser.lock().unwrap().screen_mut().set_size(rows, cols);
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
            let p = self.parser.lock().unwrap();
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
        self.sample_activity();

        // 応答キャプチャ (送信境界マーカー方式):
        // BUSY開始時点のスクロールバック蓄積量を境界として記録し、
        // DONEでその境界以降だけを抽出する (過去の応答は混ざらない)
        if self.state == TabState::Busy && old_state != TabState::Busy {
            self.response_marker = Some(self.scrollback_len());
        }
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

    /// 直近の出力量 (古い→新しい、各 0..=7)
    pub fn activity(&self) -> &[u8] {
        &self.activity
    }

    /// 子プロセスが何か出力したか (起動して動き出したかの目安)
    pub fn had_output(&self) -> bool {
        self.bytes_out.load(Ordering::Relaxed) > 0
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

    /// 現在のスクロールバック蓄積行数 (表示位置は変更しない)
    fn scrollback_len(&self) -> usize {
        let mut p = self.parser.lock().unwrap();
        let saved = p.screen().scrollback();
        p.screen_mut().set_scrollback(usize::MAX / 2);
        let max = p.screen().scrollback();
        p.screen_mut().set_scrollback(saved);
        max
    }

    /// マーカー以降の新規出力をテキスト化する
    fn capture_since_marker(&self) -> String {
        let mut p = self.parser.lock().unwrap();
        let (rows, cols) = p.screen().size();
        if p.screen().alternate_screen() {
            // 全画面TUIはスクロールしないため可視画面のスナップショットで代替
            return extract_text(&mut p, 0, rows.saturating_sub(1) as usize, cols);
        }
        let saved = p.screen().scrollback();
        p.screen_mut().set_scrollback(usize::MAX / 2);
        let now_len = p.screen().scrollback();
        p.screen_mut().set_scrollback(saved);
        let marker = self.response_marker.unwrap_or(now_len);
        let new_lines = now_len.saturating_sub(marker);
        extract_text(&mut p, 0, new_lines + rows.saturating_sub(1) as usize, cols)
    }
}


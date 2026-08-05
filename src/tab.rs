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

pub struct Tab {
    pub title: String,
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
    /// タブバーの表示インデント段数 (0 = 親)
    pub depth: u16,
    /// 再起動用に起動条件を保持する (プロセス終了後に同じ設定で再spawnできる)
    argv: Vec<String>,
    profile_spec: Option<String>,
    opts: TabOptions,
    /// 最後に人間が手動入力した時刻 (相対ms)。直後の自動送信をガードする
    pub last_manual_ms: u64,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child_exited: Arc<AtomicBool>,
    bell_count: Arc<AtomicU64>,
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
        cmd.cwd(std::env::current_dir()?);
        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let killer = child.clone_killer();

        let writer: PtyWriter = Arc::new(Mutex::new(pair.master.take_writer()?));
        let bell_count = Arc::new(AtomicU64::new(0));
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
            parser,
            writer,
            state: TabState::Wait,
            spinner_idx: 0,
            copy: None,
            chain_depth: 0,
            locked: false,
            auto_restart: false,
            depth: 0,
            argv: argv.to_vec(),
            profile_spec,
            opts,
            last_manual_ms: 0,
            master: pair.master,
            killer,
            child_exited,
            bell_count,
            last_hash: 0,
            last_change_ms: 0,
            last_response: None,
            response_marker: None,
            detector: Detector::new(profile),
        })
    }

    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
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

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
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
        *self = fresh;
        Ok(())
    }

    pub fn profile_name(&self) -> &str {
        self.detector.profile_name()
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
            self.last_change_ms = now;
        }
        let since = now.saturating_sub(self.last_change_ms);
        let old_state = self.state;
        self.state = self
            .detector
            .tick(&screen_text, since, self.bell_count.load(Ordering::Relaxed));
        if self.state == TabState::Busy {
            self.spinner_idx = self.spinner_idx.wrapping_add(1);
        }

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

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

pub struct Tab {
    pub title: String,
    pub parser: SharedParser,
    pub writer: PtyWriter,
    pub state: TabState,
    pub spinner_idx: usize,
    pub copy: Option<CopyState>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child_exited: Arc<AtomicBool>,
    bell_count: Arc<AtomicU64>,
    last_output_ms: Arc<AtomicU64>,
    detector: Detector,
}

impl Tab {
    pub fn spawn(
        title: String,
        argv: &[String],
        profile: Profile,
        rows: u16,
        cols: u16,
        start: Instant,
    ) -> Result<Self> {
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
        let last_output_ms = Arc::new(AtomicU64::new(0));
        let parser: SharedParser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            SCROLLBACK_LINES,
            QueryResponder {
                writer: Arc::clone(&writer),
                bell: Arc::clone(&bell_count),
            },
        )));
        let child_exited = Arc::new(AtomicBool::new(false));

        // PTY出力 → vt100パーサ (最終出力時刻も記録し、沈黙タイマーの信号にする)
        {
            let parser = Arc::clone(&parser);
            let last_output_ms = Arc::clone(&last_output_ms);
            let mut reader = pair.master.try_clone_reader()?;
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            last_output_ms
                                .store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
                            parser.lock().unwrap().process(&buf[..n]);
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
            master: pair.master,
            killer,
            child_exited,
            bell_count,
            last_output_ms,
            detector: Detector::new(profile),
        })
    }

    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
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

    pub fn profile_name(&self) -> &str {
        self.detector.profile_name()
    }

    /// 200ms毎の状態判定 (非アクティブタブも含めて呼ぶこと)
    pub fn tick(&mut self, start: Instant) {
        if self.exited() {
            self.state = TabState::Exited;
            return;
        }
        let screen_text = self.parser.lock().unwrap().screen().contents();
        let now = start.elapsed().as_millis() as u64;
        let since = now.saturating_sub(self.last_output_ms.load(Ordering::Relaxed));
        self.state = self
            .detector
            .tick(&screen_text, since, self.bell_count.load(Ordering::Relaxed));
        if self.state == TabState::Busy {
            self.spinner_idx = self.spinner_idx.wrapping_add(1);
        }
    }
}

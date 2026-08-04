//! ShikishaTerm-AI Phase 1 スパイク:
//! PTYで子プロセス(既定: PowerShell、引数で任意コマンド)を起動し、
//! tui-termでタブ内に端末画面を表示する。DESIGN.md 14章 Phase 1。
//!
//! 起動例:
//!   Shikisha-Term-AI.exe            # PowerShellをラップ
//!   Shikisha-Term-AI.exe claude     # Claude Codeをラップ
//!
//! 操作: Ctrl+B → q で終了（それ以外のキーは子プロセスへ透過）

mod detect;
mod profile;

use std::io::{Read as _, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use detect::{Detector, TabState};

use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Direction, Layout, Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_term::widget::PseudoTerminal;

const TAB_BAR_WIDTH: u16 = 18;
const STATUS_BAR_HEIGHT: u16 = 1;
const SCROLLBACK_LINES: usize = 5000;
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const NEON_GREEN: Color = Color::Rgb(57, 255, 20);
const NEON_YELLOW: Color = Color::Rgb(255, 234, 0);
const NEON_BLUE: Color = Color::Rgb(0, 170, 255);

type PtyWriter = Arc<Mutex<Box<dyn std::io::Write + Send>>>;

/// 子プロセスからの端末照会 (DSR/DA) への応答係。
/// ConPTY配下のプログラム (ssh等) はカーソル位置照会 `\x1b[6n` への応答を
/// 待ってブロックするため、本物のターミナルと同様にPTYへ書き戻す。
/// あわせてベル文字 (完了通知によく使われる) を数え、状態検出の信号にする。
struct QueryResponder {
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

fn main() -> Result<()> {
    let cmd_args: Vec<String> = std::env::args().skip(1).collect();
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let res = run(&mut terminal, cmd_args);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}

/// タブバー・枠線・ステータスバーを除いた、PTYに渡す端末サイズ (rows, cols)
fn pty_dims(size: Size) -> (u16, u16) {
    let cols = size.width.saturating_sub(TAB_BAR_WIDTH + 2).max(10);
    let rows = size
        .height
        .saturating_sub(STATUS_BAR_HEIGHT + 2)
        .max(3);
    (rows, cols)
}

/// ターミナルペインの内側 (枠線の内側) の矩形。pty_dimsと整合させること
fn pane_inner(size: Size) -> Rect {
    let (rows, cols) = pty_dims(size);
    Rect {
        x: TAB_BAR_WIDTH + 1,
        y: 1,
        width: cols,
        height: rows,
    }
}

fn in_rect(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

fn button_code(b: MouseButton) -> u16 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

/// 子プロセスがマウスレポートを要求している場合のイベント透過 (SGRエンコードのみ対応)
fn mouse_to_child_bytes(
    m: &MouseEvent,
    inner: Rect,
    mode: vt100::MouseProtocolMode,
    enc: vt100::MouseProtocolEncoding,
) -> Option<Vec<u8>> {
    use vt100::MouseProtocolMode as M;
    if matches!(mode, M::None) || !matches!(enc, vt100::MouseProtocolEncoding::Sgr) {
        return None;
    }
    if !in_rect(inner, m.column, m.row) {
        return None;
    }
    let x = m.column - inner.x + 1;
    let y = m.row - inner.y + 1;
    let (btn, press) = match m.kind {
        MouseEventKind::Down(b) => (button_code(b), true),
        MouseEventKind::Up(b) if !matches!(mode, M::Press) => (button_code(b), false),
        MouseEventKind::Drag(b) if matches!(mode, M::ButtonMotion | M::AnyMotion) => {
            (button_code(b) + 32, true)
        }
        MouseEventKind::Moved if matches!(mode, M::AnyMotion) => (32 + 3, true),
        MouseEventKind::ScrollUp => (64, true),
        MouseEventKind::ScrollDown => (65, true),
        _ => return None,
    };
    let suffix = if press { 'M' } else { 'm' };
    Some(format!("\x1b[<{btn};{x};{y}{suffix}").into_bytes())
}

fn run(terminal: &mut ratatui::DefaultTerminal, cmd_args: Vec<String>) -> Result<()> {
    let (rows, cols) = pty_dims(terminal.size()?);

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    let mut cmd = if cmd_args.is_empty() {
        CommandBuilder::new("powershell.exe")
    } else {
        let mut c = CommandBuilder::new(&cmd_args[0]);
        c.args(&cmd_args[1..]);
        c
    };
    cmd.cwd(std::env::current_dir()?);
    let session_title = if cmd_args.is_empty() {
        "LOCAL SHELL".to_string()
    } else {
        cmd_args[0].to_uppercase()
    };

    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    let mut killer = child.clone_killer();

    let start = Instant::now();
    let writer: PtyWriter = Arc::new(Mutex::new(pair.master.take_writer()?));
    let bell_count = Arc::new(AtomicU64::new(0));
    let last_output_ms = Arc::new(AtomicU64::new(0));
    let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
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
                        last_output_ms.store(start.elapsed().as_millis() as u64, Ordering::Relaxed);
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

    let mut prefix_active = false;
    let mut copy_state: Option<CopyState> = None;
    let mut flash: Option<String> = None;

    // 状態検出エンジン (プロファイルはコマンド名から選択)
    let profile_cmd = cmd_args.first().map(String::as_str).unwrap_or("powershell");
    let mut detector = Detector::new(profile::load_for_command(profile_cmd));
    let mut tab_state = TabState::Wait;
    let mut spinner_idx: usize = 0;
    let mut last_detect = Instant::now() - Duration::from_secs(1);

    loop {
        if child_exited.load(Ordering::SeqCst) {
            break;
        }

        // 200ms毎に画面・沈黙時間・ベルから状態を判定
        if last_detect.elapsed() >= Duration::from_millis(200) {
            last_detect = Instant::now();
            let screen_text = parser.lock().unwrap().screen().contents();
            let now = start.elapsed().as_millis() as u64;
            let since = now.saturating_sub(last_output_ms.load(Ordering::Relaxed));
            tab_state = detector.tick(&screen_text, since, bell_count.load(Ordering::Relaxed));
            if tab_state == TabState::Busy {
                spinner_idx = spinner_idx.wrapping_add(1);
            }
        }

        terminal.draw(|f| {
            draw(
                f,
                &parser,
                &session_title,
                prefix_active,
                copy_state.as_ref(),
                flash.as_deref(),
                tab_state,
                SPINNER[spinner_idx % SPINNER.len()],
                detector.profile_name(),
            );
        })?;

        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                flash = None;
                if prefix_active {
                    prefix_active = false;
                    match key.code {
                        KeyCode::Char('q') => break,
                        // Ctrl+B b で子プロセスに素のCtrl+Bを送る
                        KeyCode::Char('b') => pty_write(&writer, &[0x02])?,
                        // Ctrl+B [ でコピーモード (tmuxのコピーモード風)
                        KeyCode::Char('[') => {
                            copy_state = Some(CopyState {
                                cursor_row: pty_dims(terminal.size()?).0.saturating_sub(1),
                                anchor: None,
                            });
                        }
                        _ => {}
                    }
                } else if copy_state.is_some() {
                    let mut cs = copy_state.take().unwrap();
                    let (rows_v, cols_v) = pty_dims(terminal.size()?);
                    let mut p = parser.lock().unwrap();
                    let cur = p.screen().scrollback();
                    let mut keep = true;
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            p.screen_mut().set_scrollback(0);
                            keep = false;
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if cs.cursor_row > 0 {
                                cs.cursor_row -= 1;
                            } else {
                                p.screen_mut().set_scrollback(cur + 1);
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if cs.cursor_row + 1 < rows_v {
                                cs.cursor_row += 1;
                            } else {
                                p.screen_mut().set_scrollback(cur.saturating_sub(1));
                            }
                        }
                        KeyCode::PageUp => p.screen_mut().set_scrollback(cur + rows_v as usize),
                        KeyCode::PageDown => {
                            p.screen_mut().set_scrollback(cur.saturating_sub(rows_v as usize));
                        }
                        // 最古へ (実際の保持量にクランプされる)
                        KeyCode::Home | KeyCode::Char('g') => {
                            p.screen_mut().set_scrollback(usize::MAX / 2);
                        }
                        KeyCode::End | KeyCode::Char('G') => {
                            p.screen_mut().set_scrollback(0);
                            cs.cursor_row = rows_v.saturating_sub(1);
                        }
                        // 選択開始 / 解除
                        KeyCode::Char('v') | KeyCode::Char(' ') => {
                            cs.anchor = match cs.anchor {
                                Some(_) => None,
                                None => Some(abs_line(cur, rows_v, cs.cursor_row)),
                            };
                        }
                        // 選択範囲 (未選択ならカーソル行) をコピーして復帰
                        KeyCode::Char('y') | KeyCode::Enter => {
                            let here = abs_line(cur, rows_v, cs.cursor_row);
                            let (lo, hi) = match cs.anchor {
                                Some(a) => (a.min(here), a.max(here)),
                                None => (here, here),
                            };
                            let text = extract_text(&mut p, lo, hi, cols_v);
                            p.screen_mut().set_scrollback(0);
                            drop(p);
                            flash = Some(copy_to_clipboard(&text));
                            keep = false;
                        }
                        // 全履歴コピー
                        KeyCode::Char('a') => {
                            let text = extract_text(&mut p, 0, usize::MAX / 2, cols_v);
                            p.screen_mut().set_scrollback(0);
                            drop(p);
                            flash = Some(copy_to_clipboard(&text));
                            keep = false;
                        }
                        _ => {}
                    }
                    if keep {
                        copy_state = Some(cs);
                    }
                } else if key.code == KeyCode::Char('b')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    prefix_active = true;
                } else if let Some(bytes) = key_to_bytes(&key) {
                    pty_write(&writer, &bytes)?;
                }
            }
            Event::Paste(text) => pty_write(&writer, text.as_bytes())?,
            Event::Mouse(m) => {
                let inner = pane_inner(terminal.size()?);
                let in_pane = in_rect(inner, m.column, m.row);
                let row_in_pane = m
                    .row
                    .saturating_sub(inner.y)
                    .min(inner.height.saturating_sub(1));

                // コピーモード外で子プロセスがマウスを要求していれば透過する
                // (リモートのTUIアプリが自前でホイールスクロール等を処理できる)
                // Shift押下時は透過せず、常にこちらのコピー/ペースト操作を優先する
                if copy_state.is_none() && !m.modifiers.contains(KeyModifiers::SHIFT) {
                    let (mode, enc) = {
                        let p = parser.lock().unwrap();
                        (
                            p.screen().mouse_protocol_mode(),
                            p.screen().mouse_protocol_encoding(),
                        )
                    };
                    if !matches!(mode, vt100::MouseProtocolMode::None) {
                        if let Some(bytes) = mouse_to_child_bytes(&m, inner, mode, enc) {
                            pty_write(&writer, &bytes)?;
                        }
                        continue;
                    }
                }

                match m.kind {
                    // ホイール上: コピーモードへ入り過去へスクロール
                    MouseEventKind::ScrollUp if in_pane || copy_state.is_some() => {
                        if copy_state.is_none() {
                            copy_state = Some(CopyState {
                                cursor_row: row_in_pane,
                                anchor: None,
                            });
                        }
                        let mut p = parser.lock().unwrap();
                        let cur = p.screen().scrollback();
                        p.screen_mut().set_scrollback(cur + 3);
                    }
                    // ホイール下: 最下端まで戻ったら (未選択なら) ライブへ自動復帰
                    MouseEventKind::ScrollDown if copy_state.is_some() => {
                        let mut p = parser.lock().unwrap();
                        let cur = p.screen().scrollback();
                        let next = cur.saturating_sub(3);
                        p.screen_mut().set_scrollback(next);
                        drop(p);
                        if next == 0 && copy_state.as_ref().is_some_and(|c| c.anchor.is_none()) {
                            copy_state = None;
                        }
                    }
                    // 左クリック: コピーモード開始 + その行から選択開始
                    MouseEventKind::Down(MouseButton::Left) if in_pane => {
                        flash = None;
                        let offset = parser.lock().unwrap().screen().scrollback();
                        let anchor = abs_line(offset, inner.height, row_in_pane);
                        copy_state = Some(CopyState {
                            cursor_row: row_in_pane,
                            anchor: Some(anchor),
                        });
                    }
                    // ドラッグで選択範囲を拡張
                    MouseEventKind::Drag(MouseButton::Left) => {
                        if let Some(cs) = copy_state.as_mut() {
                            cs.cursor_row = row_in_pane;
                        }
                    }
                    // 左ボタン解放: 選択範囲を即クリップボードへ (PuTTY流の選択即コピー)
                    MouseEventKind::Up(MouseButton::Left) => {
                        let mut exit_copy = false;
                        if let Some(cs) = copy_state.as_mut() {
                            if let Some(anchor) = cs.anchor.take() {
                                let mut p = parser.lock().unwrap();
                                let cur = p.screen().scrollback();
                                let here = abs_line(
                                    cur,
                                    inner.height,
                                    cs.cursor_row.min(inner.height.saturating_sub(1)),
                                );
                                let (lo, hi) = (anchor.min(here), anchor.max(here));
                                let text = extract_text(&mut p, lo, hi, inner.width);
                                drop(p);
                                flash = Some(copy_to_clipboard(&text));
                                // ライブ位置での選択なら、そのまま通常操作へ戻る
                                exit_copy = cur == 0;
                            }
                        }
                        if exit_copy {
                            copy_state = None;
                        }
                    }
                    // 右クリック: クリップボードの内容をペースト (PuTTY流)
                    MouseEventKind::Down(MouseButton::Right) => {
                        if copy_state.take().is_some() {
                            parser.lock().unwrap().screen_mut().set_scrollback(0);
                        }
                        let bracketed = parser.lock().unwrap().screen().bracketed_paste();
                        flash = paste_clipboard(&writer, bracketed)?;
                    }
                    _ => {}
                }
            }
            Event::Resize(width, height) => {
                let (rows, cols) = pty_dims(Size { width, height });
                pair.master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;
                parser.lock().unwrap().screen_mut().set_size(rows, cols);
            }
            _ => {}
        }
    }

    let _ = killer.kill();
    Ok(())
}

fn pty_write(writer: &PtyWriter, bytes: &[u8]) -> Result<()> {
    let mut w = writer.lock().expect("pty writer lock");
    w.write_all(bytes)?;
    Ok(())
}

/// コピーモード (Ctrl+B [) の状態
struct CopyState {
    /// ペイン内のカーソル行 (0 = 最上行)
    cursor_row: u16,
    /// 選択開始位置 (画面最下行から数えた行数)。None = 未選択
    anchor: Option<usize>,
}

/// 画面最下行から数えた絶対行位置
fn abs_line(offset: usize, rows: u16, cursor_row: u16) -> usize {
    offset + rows.saturating_sub(1).saturating_sub(cursor_row) as usize
}

/// スクロールバック内の行範囲 (画面最下行からの行数 lo..=hi) をテキスト化する。
/// 折返し行は連結し、行末の空白は除去する。
fn extract_text<CB: vt100::Callbacks>(
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

fn copy_to_clipboard(text: &str) -> String {
    let lines = text.lines().count();
    match arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())) {
        Ok(()) => format!(">> COPIED {lines} LINES TO CLIPBOARD"),
        Err(e) => format!(">> COPY FAILED: {e}"),
    }
}

/// クリップボードの内容を子プロセスへペーストする。
/// 子がbracketed pasteモードなら \x1b[200~ ... \x1b[201~ で包む
fn paste_clipboard(writer: &PtyWriter, bracketed: bool) -> Result<Option<String>> {
    match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
        Ok(text) => {
            let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
            if bracketed {
                let mut bytes = b"\x1b[200~".to_vec();
                bytes.extend_from_slice(normalized.as_bytes());
                bytes.extend_from_slice(b"\x1b[201~");
                pty_write(writer, &bytes)?;
            } else {
                pty_write(writer, normalized.as_bytes())?;
            }
            Ok(None)
        }
        Err(e) => Ok(Some(format!(">> PASTE FAILED: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser_with_lines(rows: u16, cols: u16, n: usize) -> vt100::Parser {
        let mut p = vt100::Parser::new(rows, cols, 100);
        for i in 1..=n {
            p.process(format!("line{i}\r\n").as_bytes());
        }
        p
    }

    #[test]
    fn scrollback_view_shows_history() {
        let mut p = parser_with_lines(5, 20, 30);
        p.screen_mut().set_scrollback(10);
        let contents = p.screen().contents();
        assert!(
            contents.contains("line17"),
            "過去の行が見えるはず: {contents}"
        );
        assert!(
            !contents.contains("line30"),
            "最新行は画面外のはず: {contents}"
        );
    }

    #[test]
    fn extract_lines_from_scrollback() {
        let mut p = parser_with_lines(5, 20, 30);
        // 最下行(d=0)はプロンプト空行。d=1がline30、d=3がline28
        let text = extract_text(&mut p, 1, 3, 20);
        assert_eq!(text, "line28\nline29\nline30\n");
        // 抽出後はスクロール位置が復元される
        assert_eq!(p.screen().scrollback(), 0);
    }

    #[test]
    fn extract_joins_wrapped_lines() {
        // 5行画面: row0="abcdefghij"(折返し) row1="KLMNO" row2以降は空。
        // 画面最下行から数えると折返し行はd=4、続きはd=3
        let mut p = vt100::Parser::new(5, 10, 100);
        p.process(b"abcdefghijKLMNO\r\n");
        let text = extract_text(&mut p, 3, 4, 10);
        assert_eq!(text, "abcdefghijKLMNO\n");
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    f: &mut Frame,
    parser: &Arc<Mutex<vt100::Parser<QueryResponder>>>,
    session_title: &str,
    prefix_active: bool,
    copy: Option<&CopyState>,
    flash: Option<&str>,
    state: TabState,
    spinner: char,
    profile_name: &str,
) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(STATUS_BAR_HEIGHT)])
        .split(f.area());
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(TAB_BAR_WIDTH), Constraint::Min(1)])
        .split(outer[0]);

    // 状態インジケータ: 🟡BUSY(スピナー) 🟢DONE 🔵WAIT/QUESTION
    let (indicator, indicator_color) = match state {
        TabState::Busy => (spinner, NEON_YELLOW),
        TabState::Done => ('●', NEON_GREEN),
        TabState::Question => ('?', NEON_BLUE),
        TabState::Wait => ('●', NEON_BLUE),
    };
    let tabs = Paragraph::new(vec![
        Line::from(Span::styled("[≡] 0. INDEX", Style::default().fg(NEON_BLUE))),
        Line::from(vec![
            Span::styled(
                format!("[{indicator}] "),
                Style::default()
                    .fg(indicator_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("1. {session_title}"),
                Style::default().fg(NEON_GREEN).add_modifier(Modifier::BOLD),
            ),
        ]),
    ])
    .block(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(NEON_GREEN)),
    );
    f.render_widget(tabs, main[0]);

    let border_color = if copy.is_some() { NEON_YELLOW } else { NEON_GREEN };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            format!(" SESSION 1 :: {session_title} "),
            Style::default().fg(NEON_YELLOW),
        ));
    let inner = block.inner(main[1]);
    f.render_widget(block, main[1]);
    let scrollback_offset;
    let alt_screen;
    {
        let parser = parser.lock().unwrap();
        scrollback_offset = parser.screen().scrollback();
        alt_screen = parser.screen().alternate_screen();
        f.render_widget(PseudoTerminal::new(parser.screen()), inner);
    }

    // コピーモード: カーソル行と選択範囲をハイライト
    if let Some(cs) = copy {
        let rows_v = inner.height;
        let cursor_row = cs.cursor_row.min(rows_v.saturating_sub(1));
        let here = abs_line(scrollback_offset, rows_v, cursor_row);
        for r in 0..rows_v {
            let d = abs_line(scrollback_offset, rows_v, r);
            let in_selection = cs
                .anchor
                .is_some_and(|a| d >= a.min(here) && d <= a.max(here));
            let style = if r == cursor_row {
                Some(Style::default().bg(NEON_GREEN).fg(Color::Black))
            } else if in_selection {
                Some(Style::default().bg(Color::Rgb(0, 80, 40)))
            } else {
                None
            };
            if let Some(style) = style {
                f.buffer_mut().set_style(
                    ratatui::layout::Rect {
                        x: inner.x,
                        y: inner.y + r,
                        width: inner.width,
                        height: 1,
                    },
                    style,
                );
            }
        }
    }

    let status = if let Some(cs) = copy {
        let mode = if cs.anchor.is_some() { "SELECT" } else { "CURSOR" };
        let hist = if alt_screen {
            " | 履歴なし(全画面アプリ)"
        } else {
            ""
        };
        format!(
            " [COPY:{mode}] -{scrollback_offset} | 選択で自動コピー 右クリック:ペースト | v y a / Esc: LIVE{hist}"
        )
    } else if prefix_active {
        " [PREFIX] q: EXIT / [: COPY MODE / b: send Ctrl+B".to_string()
    } else if let Some(msg) = flash {
        format!(" {msg}")
    } else {
        format!(
            " PROFILE:{profile_name} [{}] | ドラッグ:コピー 右クリック:ペースト | Ctrl+B q: EXIT",
            state.label()
        )
    };
    let status_bg = if copy.is_some() { NEON_YELLOW } else { NEON_GREEN };
    f.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Black).bg(status_bg)),
        outer[1],
    );
}

/// crossterm KeyEvent → 子PTYへ送るバイト列 (VT100/xterm系エンコード)
fn key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::with_capacity(8);
    if key.modifiers.contains(KeyModifiers::ALT) {
        buf.push(0x1b);
    }
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let lower = c.to_ascii_lowercase();
                if lower.is_ascii_lowercase() {
                    buf.push((lower as u8) - b'a' + 1);
                } else {
                    return None;
                }
            } else {
                let mut tmp = [0u8; 4];
                buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
            }
        }
        KeyCode::Enter => buf.push(b'\r'),
        KeyCode::Backspace => buf.push(0x7f),
        KeyCode::Tab => buf.push(b'\t'),
        KeyCode::BackTab => buf.extend_from_slice(b"\x1b[Z"),
        KeyCode::Esc => buf.push(0x1b),
        KeyCode::Up => buf.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => buf.extend_from_slice(b"\x1b[B"),
        KeyCode::Right => buf.extend_from_slice(b"\x1b[C"),
        KeyCode::Left => buf.extend_from_slice(b"\x1b[D"),
        KeyCode::Home => buf.extend_from_slice(b"\x1b[H"),
        KeyCode::End => buf.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => buf.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => buf.extend_from_slice(b"\x1b[6~"),
        KeyCode::Insert => buf.extend_from_slice(b"\x1b[2~"),
        KeyCode::Delete => buf.extend_from_slice(b"\x1b[3~"),
        KeyCode::F(n) => match n {
            1 => buf.extend_from_slice(b"\x1bOP"),
            2 => buf.extend_from_slice(b"\x1bOQ"),
            3 => buf.extend_from_slice(b"\x1bOR"),
            4 => buf.extend_from_slice(b"\x1bOS"),
            5 => buf.extend_from_slice(b"\x1b[15~"),
            6 => buf.extend_from_slice(b"\x1b[17~"),
            7 => buf.extend_from_slice(b"\x1b[18~"),
            8 => buf.extend_from_slice(b"\x1b[19~"),
            9 => buf.extend_from_slice(b"\x1b[20~"),
            10 => buf.extend_from_slice(b"\x1b[21~"),
            11 => buf.extend_from_slice(b"\x1b[23~"),
            12 => buf.extend_from_slice(b"\x1b[24~"),
            _ => return None,
        },
        _ => return None,
    }
    Some(buf)
}

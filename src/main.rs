//! ShikishaTerm-AI Phase 1 スパイク:
//! PTYで子プロセス(既定: PowerShell、引数で任意コマンド)を起動し、
//! tui-termでタブ内に端末画面を表示する。DESIGN.md 14章 Phase 1。
//!
//! 起動例:
//!   Shikisha-Term-AI.exe            # PowerShellをラップ
//!   Shikisha-Term-AI.exe claude     # Claude Codeをラップ
//!
//! 操作: Ctrl+B → q で終了（それ以外のキーは子プロセスへ透過）

use std::io::{Read as _, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_term::widget::PseudoTerminal;

const TAB_BAR_WIDTH: u16 = 18;
const STATUS_BAR_HEIGHT: u16 = 1;
const SCROLLBACK_LINES: usize = 5000;

const NEON_GREEN: Color = Color::Rgb(57, 255, 20);
const NEON_YELLOW: Color = Color::Rgb(255, 234, 0);
const NEON_BLUE: Color = Color::Rgb(0, 170, 255);

fn main() -> Result<()> {
    let cmd_args: Vec<String> = std::env::args().skip(1).collect();
    let mut terminal = ratatui::init();
    let res = run(&mut terminal, cmd_args);
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

    let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK_LINES)));
    let child_exited = Arc::new(AtomicBool::new(false));

    // PTY出力 → vt100パーサ
    {
        let parser = Arc::clone(&parser);
        let mut reader = pair.master.try_clone_reader()?;
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => parser.lock().unwrap().process(&buf[..n]),
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

    let mut writer = pair.master.take_writer()?;
    let mut prefix_active = false;

    loop {
        if child_exited.load(Ordering::SeqCst) {
            break;
        }

        terminal.draw(|f| draw(f, &parser, &session_title, prefix_active))?;

        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if prefix_active {
                    prefix_active = false;
                    match key.code {
                        KeyCode::Char('q') => break,
                        // Ctrl+B b で子プロセスに素のCtrl+Bを送る
                        KeyCode::Char('b') => writer.write_all(&[0x02])?,
                        _ => {}
                    }
                } else if key.code == KeyCode::Char('b')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    prefix_active = true;
                } else if let Some(bytes) = key_to_bytes(&key) {
                    writer.write_all(&bytes)?;
                }
            }
            Event::Paste(text) => writer.write_all(text.as_bytes())?,
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

fn draw(
    f: &mut Frame,
    parser: &Arc<Mutex<vt100::Parser>>,
    session_title: &str,
    prefix_active: bool,
) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(STATUS_BAR_HEIGHT)])
        .split(f.area());
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(TAB_BAR_WIDTH), Constraint::Min(1)])
        .split(outer[0]);

    let tabs = Paragraph::new(vec![
        Line::from(Span::styled("[≡] 0. INDEX", Style::default().fg(NEON_BLUE))),
        Line::from(Span::styled(
            format!("[●] 1. {session_title}"),
            Style::default().fg(NEON_GREEN).add_modifier(Modifier::BOLD),
        )),
    ])
    .block(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(NEON_GREEN)),
    );
    f.render_widget(tabs, main[0]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(NEON_GREEN))
        .title(Span::styled(
            format!(" SESSION 1 :: {session_title} "),
            Style::default().fg(NEON_YELLOW),
        ));
    let inner = block.inner(main[1]);
    f.render_widget(block, main[1]);
    {
        let parser = parser.lock().unwrap();
        f.render_widget(PseudoTerminal::new(parser.screen()), inner);
    }

    let status = if prefix_active {
        " [PREFIX] q: EXIT / b: send Ctrl+B"
    } else {
        " KERNEL ACCESS GRANTED... PORTABLE_MODE_ON  |  Ctrl+B q: EXIT"
    };
    f.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Black).bg(NEON_GREEN)),
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

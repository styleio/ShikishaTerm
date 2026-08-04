//! ShikishaTerm-AI: ポータブル・マルチセッションAIオーケストレーションTUI
//!
//! Phase 3: マルチタブ + INDEXダッシュボード + config.json
//!
//! 起動:
//!   Shikisha-Term-AI.exe                 # config.jsonのタブ構成 (無ければPowerShell 1タブ)
//!   Shikisha-Term-AI.exe claude          # デバッグ用: 引数のコマンドを1タブで起動
//!
//! 操作 (プレフィックスキー Ctrl+B):
//!   Ctrl+B q      終了 / Ctrl+B 0-9 タブ切替 (0=INDEX) / Ctrl+B n/p 隣のタブ
//!   Ctrl+B [      コピーモード / Ctrl+B b 素のCtrl+Bを送信
//! マウス: ホイール=スクロール(コピーモード) / 左ドラッグ=選択即コピー / 右クリック=ペースト

mod config;
mod detect;
mod hooks;
mod profile;
mod tab;

use std::time::{Duration, Instant};

use anyhow::Result;
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

use detect::TabState;
use hooks::{Command, HookEngine, TabCtx};
use tab::{CopyState, Tab, extract_text};

const TAB_BAR_WIDTH: u16 = 18;
const STATUS_BAR_HEIGHT: u16 = 1;
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const NEON_GREEN: Color = Color::Rgb(57, 255, 20);
const NEON_YELLOW: Color = Color::Rgb(255, 234, 0);
const NEON_BLUE: Color = Color::Rgb(0, 170, 255);
const NEON_RED: Color = Color::Rgb(255, 70, 70);

fn main() -> Result<()> {
    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let res = run(&mut terminal);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}

/// タブバー・枠線・ステータスバーを除いた、PTYに渡す端末サイズ (rows, cols)
fn pty_dims(size: Size) -> (u16, u16) {
    let cols = size.width.saturating_sub(TAB_BAR_WIDTH + 2).max(10);
    let rows = size.height.saturating_sub(STATUS_BAR_HEIGHT + 2).max(3);
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

/// 画面最下行から数えた絶対行位置
fn abs_line(offset: usize, rows: u16, cursor_row: u16) -> usize {
    offset + rows.saturating_sub(1).saturating_sub(cursor_row) as usize
}

/// argvからタブ名を生成 ("ssh" → "SSH")
fn title_of(argv: &[String]) -> String {
    argv.first()
        .map(|c| {
            std::path::Path::new(c)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(c)
                .to_uppercase()
        })
        .unwrap_or_else(|| "SHELL".into())
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let cmd_args: Vec<String> = std::env::args().skip(1).collect();
    let start = Instant::now();
    let (rows, cols) = pty_dims(terminal.size()?);

    // タブ構成: CLI引数 (デバッグ用) > config.json > 既定 (PowerShell 1タブ)
    let cfg = if cmd_args.is_empty() {
        config::load()
    } else {
        None
    };
    let mut startup_errors: Vec<String> = Vec::new();
    let mut workspaces: Vec<config::Workspace> = Vec::new();
    if let Some(c) = &cfg {
        let (ws, errs) = c.resolve_workspaces();
        workspaces = ws;
        startup_errors.extend(errs);
    }

    let mut tabs: Vec<Tab> = Vec::new();
    let mut ws_index = 0usize;
    if !cmd_args.is_empty() {
        let profile = profile::load_for_command(&cmd_args[0]);
        tabs.push(Tab::spawn(
            title_of(&cmd_args),
            &cmd_args,
            profile,
            rows,
            cols,
        )?);
    } else if !workspaces.is_empty() {
        spawn_workspace(&workspaces[0], rows, cols, &mut tabs, &mut startup_errors);
    }
    if tabs.is_empty() && workspaces.is_empty() {
        let argv = vec!["powershell.exe".to_string()];
        tabs.push(Tab::spawn(
            "SHELL".into(),
            &argv,
            profile::load_for_command("powershell"),
            rows,
            cols,
        )?);
    }

    // Luaフックエンジン (config.jsonの "lua" 指定時のみ)
    let mut engine: Option<HookEngine> = cfg
        .as_ref()
        .and_then(|c| c.lua.as_deref())
        .and_then(|p| match HookEngine::load(&resolve_data_path(p)) {
            Ok(e) => Some(e),
            Err(e) => {
                startup_errors.push(format!("Lua: {e:#}"));
                None
            }
        });
    let max_chain = cfg.as_ref().and_then(|c| c.max_chain).unwrap_or(10);
    let mut auto_enabled = true;
    let mut started_fired = vec![false; tabs.len()];

    // 0 = INDEX、1.. = セッション
    let mut active: usize = if tabs.is_empty() { 0 } else { 1 };
    let mut prefix_active = false;
    let mut flash: Option<String> = startup_errors.first().map(|e| format!(">> 起動失敗 {e}"));
    let mut last_detect = Instant::now() - Duration::from_secs(1);
    // ワークスペースは仮想デスクトップ方式: 切替=非表示であって停止ではない。
    // 各ワークスペースのタブ群を保持し、初回アクティブ化時に起動する
    let mut ws_tabs: Vec<Vec<Tab>> = Vec::new();
    if !workspaces.is_empty() {
        ws_tabs.push(std::mem::take(&mut tabs));
        for _ in 1..workspaces.len() {
            ws_tabs.push(Vec::new());
        }
        tabs = std::mem::take(&mut ws_tabs[0]);
    }
    let mut ws_open = false;
    let mut help_open = false;

    loop {
        // 200ms毎に全タブの状態を判定 (非アクティブタブの完了もINDEXに反映される)
        if last_detect.elapsed() >= Duration::from_millis(200) {
            last_detect = Instant::now();
            let mut transitions = Vec::with_capacity(tabs.len());
            for (i, t) in tabs.iter_mut().enumerate() {
                let (old, new) = t.tick(start);
                transitions.push((i + 1, old, new));
            }

            // フック発火 → wait中コルーチン再開 → 積まれた操作の実行
            if let Some(eng) = engine.as_mut() {
                if auto_enabled {
                    for (i, fired) in started_fired.iter_mut().enumerate() {
                        if !*fired {
                            *fired = true;
                            eng.fire("on_start", &tab_ctx(&tabs[i], i + 1), None);
                        }
                    }
                    for &(idx, old, new) in &transitions {
                        if old == new {
                            continue;
                        }
                        let ctx = tab_ctx(&tabs[idx - 1], idx);
                        match new {
                            TabState::Busy => eng.fire("on_busy", &ctx, None),
                            TabState::Done if old == TabState::Busy => {
                                eng.fire("on_done", &ctx, None);
                            }
                            TabState::Question => {
                                let screen =
                                    tabs[idx - 1].parser.lock().unwrap().screen().contents();
                                eng.fire("on_question", &ctx, Some(&screen));
                            }
                            TabState::Exited => eng.fire("on_exit", &ctx, None),
                            _ => {}
                        }
                    }
                    if eng.has("on_tick") {
                        for (i, t) in tabs.iter().enumerate() {
                            eng.fire("on_tick", &tab_ctx(t, i + 1), Some(t.state.label()));
                        }
                    }
                    eng.tick_pending(&|idx| {
                        tabs.get(idx.wrapping_sub(1))
                            .map(|t| t.parser.lock().unwrap().screen().contents())
                    });
                }
                let cmds = eng.drain_commands();
                if !cmds.is_empty() {
                    let now_ms = start.elapsed().as_millis() as u64;
                    exec_commands(cmds, &mut tabs, max_chain, auto_enabled, now_ms, &mut flash);
                }
            }
        }

        let ui = Ui {
            active,
            prefix_active,
            auto: engine.as_ref().map(|_| auto_enabled),
            ws_names: workspaces.iter().map(|w| w.name.clone()).collect(),
            ws_index,
            ws_open,
            help_open,
        };
        terminal.draw(|f| draw(f, &tabs, &ui, flash.as_deref()))?;

        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                flash = None;
                // オーバーレイ (ヘルプ / ワークスペース一覧) が最優先
                if help_open {
                    help_open = false;
                    continue;
                }
                if ws_open {
                    match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => ws_open = false,
                        KeyCode::Char(c @ '1'..='9') => {
                            let n = c as usize - '1' as usize;
                            if n < workspaces.len() {
                                switch_workspace(
                                    n,
                                    &mut ws_index,
                                    &mut tabs,
                                    &mut ws_tabs,
                                    &workspaces,
                                    &mut active,
                                    rows,
                                    cols,
                                    &mut startup_errors,
                                    &mut started_fired,
                                );
                            }
                            ws_open = false;
                        }
                        _ => {}
                    }
                    continue;
                }
                if prefix_active {
                    prefix_active = false;
                    match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char(c @ '0'..='9') => {
                            let n = c as usize - '0' as usize;
                            if n <= tabs.len() {
                                active = n;
                            }
                        }
                        KeyCode::Char('n') => {
                            active = if active >= tabs.len() { 0 } else { active + 1 };
                        }
                        KeyCode::Char('p') => {
                            active = if active == 0 { tabs.len() } else { active - 1 };
                        }
                        // Ctrl+B b で子プロセスに素のCtrl+Bを送る
                        KeyCode::Char('b') => {
                            if let Some(t) = session_mut(&mut tabs, active) {
                                t.write_bytes(&[0x02])?;
                            }
                        }
                        // Ctrl+B l 入力ロック切替 / w ワークスペース一覧 / ? ヘルプ
                        KeyCode::Char('l') => {
                            if let Some(t) = session_mut(&mut tabs, active) {
                                t.locked = !t.locked;
                                flash = Some(
                                    if t.locked {
                                        ">> このタブをロックしました (Ctrl+B l で解除)"
                                    } else {
                                        ">> ロック解除"
                                    }
                                    .to_string(),
                                );
                            }
                        }
                        KeyCode::Char('w') => {
                            if workspaces.len() > 1 {
                                ws_open = true;
                            }
                        }
                        KeyCode::Char('W') => {
                            if workspaces.len() > 1 {
                                let next = (ws_index + 1) % workspaces.len();
                                switch_workspace(
                                    next,
                                    &mut ws_index,
                                    &mut tabs,
                                    &mut ws_tabs,
                                    &workspaces,
                                    &mut active,
                                    rows,
                                    cols,
                                    &mut startup_errors,
                                    &mut started_fired,
                                );
                            }
                        }
                        KeyCode::Char('?') => help_open = true,
                        // Ctrl+B a 自動化ON/OFF、Ctrl+B x 緊急停止
                        KeyCode::Char('a') => {
                            auto_enabled = !auto_enabled;
                            flash = Some(
                                if auto_enabled { ">> AUTO: ON" } else { ">> AUTO: OFF" }
                                    .to_string(),
                            );
                        }
                        KeyCode::Char('x') => {
                            auto_enabled = false;
                            flash =
                                Some(">> EMERGENCY STOP: 全自動化停止 (Ctrl+B aで再開)".to_string());
                        }
                        // Ctrl+B c で最新キャプチャ応答をクリップボードへ
                        KeyCode::Char('c') => {
                            if let Some(t) = session_mut(&mut tabs, active) {
                                flash = Some(match &t.last_response {
                                    Some(r) if !r.trim().is_empty() => copy_to_clipboard(r),
                                    _ => ">> NO CAPTURED RESPONSE".to_string(),
                                });
                            }
                        }
                        // Ctrl+B [ でコピーモード (tmuxのコピーモード風)
                        KeyCode::Char('[') => {
                            let rows = pty_dims(terminal.size()?).0;
                            if let Some(t) = session_mut(&mut tabs, active) {
                                t.copy = Some(CopyState {
                                    cursor_row: rows.saturating_sub(1),
                                    anchor: None,
                                });
                            }
                        }
                        _ => {}
                    }
                } else if key.code == KeyCode::Char('b')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    prefix_active = true;
                } else if active == 0 {
                    // INDEX = ホーム画面: 数字でタブ切替、英字でメニュー実行
                    match key.code {
                        KeyCode::Char(c @ '0'..='9') => {
                            let n = c as usize - '0' as usize;
                            if n <= tabs.len() {
                                active = n;
                            }
                        }
                        KeyCode::Char('?') | KeyCode::Char('h') => help_open = true,
                        KeyCode::Char('w') => {
                            if workspaces.len() > 1 {
                                ws_open = true;
                            }
                        }
                        KeyCode::Char('e') | KeyCode::Char('k') => {
                            flash = Some(">> 設定GUI / パスワード変更は Phase 5 で実装".to_string());
                        }
                        KeyCode::Char('q') => break,
                        _ => {}
                    }
                } else {
                    let size = terminal.size()?;
                    let now_ms = start.elapsed().as_millis() as u64;
                    let mut locked_hit = false;
                    if let Some(t) = session_mut(&mut tabs, active) {
                        if t.copy.is_some() {
                            handle_copy_key(t, &key, size, &mut flash)?;
                        } else if t.locked {
                            // ソフトロック: 閲覧・コピーはできるが入力は無視
                            locked_hit = true;
                        } else if let Some(bytes) = key_to_bytes(&key) {
                            // 手動入力: チェーン(透明のボール)をリセットし、
                            // 直後の自動送信をガードする
                            t.chain_depth = 0;
                            t.last_manual_ms = now_ms;
                            t.write_bytes(&bytes)?;
                        }
                    }
                    if locked_hit {
                        flash = Some(
                            ">> 🔒 ロック中です (Ctrl+B l または 🔒クリックで解除)".to_string(),
                        );
                    }
                }
            }
            Event::Paste(text) => {
                let now_ms = start.elapsed().as_millis() as u64;
                if let Some(t) = session_mut(&mut tabs, active) {
                    if !t.locked {
                        t.chain_depth = 0;
                        t.last_manual_ms = now_ms;
                        t.write_bytes(text.as_bytes())?;
                    }
                }
            }
            Event::Mouse(m) => {
                let size = terminal.size()?;
                let now_ms = start.elapsed().as_millis() as u64;
                if help_open {
                    if matches!(m.kind, MouseEventKind::Down(_)) {
                        help_open = false;
                    }
                    continue;
                }
                // ワークスペースのドロップダウン (左バー最上部)
                if ws_open {
                    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                        if m.column < TAB_BAR_WIDTH && m.row >= 1 {
                            let n = (m.row - 1) as usize;
                            if n < workspaces.len() {
                                switch_workspace(
                                    n,
                                    &mut ws_index,
                                    &mut tabs,
                                    &mut ws_tabs,
                                    &workspaces,
                                    &mut active,
                                    rows,
                                    cols,
                                    &mut startup_errors,
                                    &mut started_fired,
                                );
                            }
                        }
                        ws_open = false;
                    }
                    continue;
                }
                // 左バー最上部のワークスペース名クリックで一覧を開く
                if workspaces.len() > 1
                    && matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
                    && m.column < TAB_BAR_WIDTH
                    && m.row == 0
                {
                    ws_open = true;
                    continue;
                }
                let ws_offset = if workspaces.len() > 1 { 1 } else { 0 };
                handle_mouse(
                    &mut tabs,
                    &mut active,
                    m,
                    size,
                    now_ms,
                    ws_offset,
                    &mut flash,
                )?;
            }
            Event::Resize(width, height) => {
                let (rows, cols) = pty_dims(Size { width, height });
                for t in &tabs {
                    let _ = t.resize(rows, cols);
                }
            }
            _ => {}
        }
    }

    for t in tabs.iter_mut() {
        t.kill();
    }
    Ok(())
}

/// ワークスペースのタブ群を起動する (初回アクティブ化時に呼ぶ)
fn spawn_workspace(
    ws: &config::Workspace,
    rows: u16,
    cols: u16,
    tabs: &mut Vec<Tab>,
    errors: &mut Vec<String>,
) {
    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        if argv.is_empty() {
            continue;
        }
        let profile = match &ft.cfg.profile {
            Some(name) => profile::load_by_name(name),
            None => profile::load_for_command(&argv[0]),
        };
        let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
        match Tab::spawn(title.clone(), &argv, profile, rows, cols) {
            Ok(mut tab) => {
                tab.locked = ft.cfg.locked;
                tab.depth = ft.depth;
                tabs.push(tab);
            }
            Err(e) => errors.push(format!("{title}: {e}")),
        }
    }
}

/// ワークスペース切替 (仮想デスクトップ方式)。
/// 切替は非表示化であって停止ではない — 裏に回ったタブも動き続ける。
/// 未起動のワークスペースはこのタイミングで初回起動する
#[allow(clippy::too_many_arguments)]
fn switch_workspace(
    to: usize,
    ws_index: &mut usize,
    tabs: &mut Vec<Tab>,
    ws_tabs: &mut [Vec<Tab>],
    workspaces: &[config::Workspace],
    active: &mut usize,
    rows: u16,
    cols: u16,
    errors: &mut Vec<String>,
    started_fired: &mut Vec<bool>,
) {
    if to == *ws_index || to >= workspaces.len() {
        return;
    }
    ws_tabs[*ws_index] = std::mem::take(tabs);
    *ws_index = to;
    *tabs = std::mem::take(&mut ws_tabs[to]);
    if tabs.is_empty() {
        spawn_workspace(&workspaces[to], rows, cols, tabs, errors);
    }
    started_fired.resize(tabs.len(), false);
    *active = if tabs.is_empty() { 0 } else { 1 };
}

fn session_mut(tabs: &mut [Tab], active: usize) -> Option<&mut Tab> {
    if active == 0 {
        None
    } else {
        tabs.get_mut(active - 1)
    }
}

/// exe隣 (ポータブル配置) を優先してデータファイルのパスを解決する
fn resolve_data_path(p: &str) -> std::path::PathBuf {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
    {
        let cand = dir.join(p);
        if cand.exists() {
            return cand;
        }
    }
    std::path::PathBuf::from(p)
}

fn tab_ctx(t: &Tab, index: usize) -> TabCtx {
    TabCtx {
        index,
        name: t.title.clone(),
        state: t.state.label().to_string(),
        profile: t.profile_name().to_string(),
        output: t.last_response.clone().unwrap_or_default(),
        chain_depth: t.chain_depth,
        locked: t.locked,
    }
}

/// 手動入力直後は自動送信を控える猶予 (打鍵の混線防止)
const MANUAL_GUARD_MS: u64 = 5000;

fn append_hook_log(msg: &str) {
    let _ = std::fs::create_dir_all("logs");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("logs/hooks.log")
    {
        use std::io::Write as _;
        let _ = writeln!(f, "{msg}");
    }
}

/// Luaフックが積んだ操作依頼を実行する。
/// 自動送信はチェーン深度 (透明のボール) を継承し、上限で止める
fn exec_commands(
    cmds: Vec<Command>,
    tabs: &mut [Tab],
    max_chain: u32,
    auto_enabled: bool,
    now_ms: u64,
    flash: &mut Option<String>,
) {
    for cmd in cmds {
        match cmd {
            Command::Log(msg) => append_hook_log(&msg),
            Command::Notify { dest, text } => {
                append_hook_log(&format!("NOTIFY[{dest}] {text}"));
                *flash = Some(format!(">> NOTIFY[{dest}] {text}"));
            }
            Command::SendKeys { target, keys } => {
                if !auto_enabled {
                    continue;
                }
                if let Some(t) = tabs.get(target.wrapping_sub(1)) {
                    if now_ms.saturating_sub(t.last_manual_ms) < MANUAL_GUARD_MS {
                        continue;
                    }
                    let _ = t.write_bytes(keys.as_bytes());
                }
            }
            Command::SendPrompt {
                target,
                text,
                origin,
            } => {
                if !auto_enabled {
                    continue;
                }
                let depth = tabs
                    .get(origin.wrapping_sub(1))
                    .map(|t| t.chain_depth)
                    .unwrap_or(0)
                    + 1;
                if depth > max_chain {
                    *flash = Some(format!(">> 自動チェーン上限({max_chain})に達したため停止"));
                    append_hook_log(&format!("chain limit ({max_chain}): tab{origin} -> tab{target}"));
                    continue;
                }
                let Some(t) = tabs.get_mut(target.wrapping_sub(1)) else {
                    continue;
                };
                if now_ms.saturating_sub(t.last_manual_ms) < MANUAL_GUARD_MS {
                    *flash = Some(">> 手動操作中のため自動送信をスキップ".to_string());
                    continue;
                }
                t.chain_depth = depth;
                let bracketed = t.parser.lock().unwrap().screen().bracketed_paste();
                let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
                let mut bytes = Vec::new();
                if bracketed {
                    bytes.extend_from_slice(b"\x1b[200~");
                    bytes.extend_from_slice(normalized.as_bytes());
                    bytes.extend_from_slice(b"\x1b[201~");
                } else {
                    bytes.extend_from_slice(normalized.as_bytes());
                }
                bytes.push(b'\r');
                let _ = t.write_bytes(&bytes);
                append_hook_log(&format!("auto-send tab{origin} -> tab{target} (depth {depth})"));
            }
        }
    }
}

/// コピーモード中のキー操作
fn handle_copy_key(t: &mut Tab, key: &KeyEvent, size: Size, flash: &mut Option<String>) -> Result<()> {
    let (rows_v, cols_v) = pty_dims(size);
    let Some(mut cs) = t.copy.take() else {
        return Ok(());
    };
    let mut p = t.parser.lock().unwrap();
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
            *flash = Some(copy_to_clipboard(&text));
            t.copy = None;
            return Ok(());
        }
        // 全履歴コピー
        KeyCode::Char('a') => {
            let text = extract_text(&mut p, 0, usize::MAX / 2, cols_v);
            p.screen_mut().set_scrollback(0);
            drop(p);
            *flash = Some(copy_to_clipboard(&text));
            t.copy = None;
            return Ok(());
        }
        _ => {}
    }
    if keep {
        t.copy = Some(cs);
    }
    Ok(())
}

/// マウス操作: タブバークリック切替 / ホイールスクロール / 選択即コピー / 右クリックペースト
#[allow(clippy::too_many_arguments)]
fn handle_mouse(
    tabs: &mut [Tab],
    active: &mut usize,
    m: MouseEvent,
    size: Size,
    now_ms: u64,
    ws_offset: u16,
    flash: &mut Option<String>,
) -> Result<()> {
    // タブバークリックで切替 (ws_offset行目=INDEX、以降=セッション)
    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
        if m.column < TAB_BAR_WIDTH {
            if m.row < ws_offset {
                return Ok(());
            }
            let r = (m.row - ws_offset) as usize;
            if r >= 1 && r <= tabs.len() {
                // 行末の🔒アイコンをクリックしたらロック切替 (マウスだけで操作可能)
                let t = &mut tabs[r - 1];
                if m.column >= TAB_BAR_WIDTH.saturating_sub(3) {
                    t.locked = !t.locked;
                    *flash = Some(
                        if t.locked {
                            ">> 🔒 ロックしました"
                        } else {
                            ">> ロック解除"
                        }
                        .to_string(),
                    );
                    return Ok(());
                }
            }
            if r <= tabs.len() {
                *active = r;
            }
            return Ok(());
        }
    }

    let inner = pane_inner(size);
    let in_pane = in_rect(inner, m.column, m.row);
    let Some(t) = session_mut(tabs, *active) else {
        return Ok(());
    };
    let row_in_pane = m
        .row
        .saturating_sub(inner.y)
        .min(inner.height.saturating_sub(1));

    // コピーモード外で子プロセスがマウスを要求していれば透過する
    // (リモートのTUIアプリが自前でホイールスクロール等を処理できる)
    // Shift押下時は透過せず、常にこちらのコピー/ペースト操作を優先する
    if t.copy.is_none() && !m.modifiers.contains(KeyModifiers::SHIFT) {
        let (mode, enc) = {
            let p = t.parser.lock().unwrap();
            (
                p.screen().mouse_protocol_mode(),
                p.screen().mouse_protocol_encoding(),
            )
        };
        if !matches!(mode, vt100::MouseProtocolMode::None) {
            if let Some(bytes) = mouse_to_child_bytes(&m, inner, mode, enc) {
                t.write_bytes(&bytes)?;
            }
            return Ok(());
        }
    }

    match m.kind {
        // ホイール上: コピーモードへ入り過去へスクロール
        MouseEventKind::ScrollUp if in_pane || t.copy.is_some() => {
            if t.copy.is_none() {
                t.copy = Some(CopyState {
                    cursor_row: row_in_pane,
                    anchor: None,
                });
            }
            let mut p = t.parser.lock().unwrap();
            let cur = p.screen().scrollback();
            p.screen_mut().set_scrollback(cur + 3);
        }
        // ホイール下: 最下端まで戻ったら (未選択なら) ライブへ自動復帰
        MouseEventKind::ScrollDown if t.copy.is_some() => {
            let mut p = t.parser.lock().unwrap();
            let cur = p.screen().scrollback();
            let next = cur.saturating_sub(3);
            p.screen_mut().set_scrollback(next);
            drop(p);
            if next == 0 && t.copy.as_ref().is_some_and(|c| c.anchor.is_none()) {
                t.copy = None;
            }
        }
        // ロック中タブでも閲覧・コピーはできる (右クリックのペーストのみ抑止)
        MouseEventKind::Down(MouseButton::Right) if t.locked => {
            *flash = Some(">> 🔒 ロック中のためペーストできません".to_string());
        }
        // 左クリック: コピーモード開始 + その行から選択開始
        MouseEventKind::Down(MouseButton::Left) if in_pane => {
            *flash = None;
            let offset = t.parser.lock().unwrap().screen().scrollback();
            let anchor = abs_line(offset, inner.height, row_in_pane);
            t.copy = Some(CopyState {
                cursor_row: row_in_pane,
                anchor: Some(anchor),
            });
        }
        // ドラッグで選択範囲を拡張
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(cs) = t.copy.as_mut() {
                cs.cursor_row = row_in_pane;
            }
        }
        // 左ボタン解放: 選択範囲を即クリップボードへ (PuTTY流の選択即コピー)
        MouseEventKind::Up(MouseButton::Left) => {
            let mut exit_copy = false;
            if let Some(cs) = t.copy.as_mut() {
                if let Some(anchor) = cs.anchor.take() {
                    let mut p = t.parser.lock().unwrap();
                    let cur = p.screen().scrollback();
                    let here = abs_line(
                        cur,
                        inner.height,
                        cs.cursor_row.min(inner.height.saturating_sub(1)),
                    );
                    let (lo, hi) = (anchor.min(here), anchor.max(here));
                    let text = extract_text(&mut p, lo, hi, inner.width);
                    drop(p);
                    *flash = Some(copy_to_clipboard(&text));
                    // ライブ位置での選択なら、そのまま通常操作へ戻る
                    exit_copy = cur == 0;
                }
            }
            if exit_copy {
                t.copy = None;
            }
        }
        // 右クリック: クリップボードの内容をペースト (PuTTY流)
        MouseEventKind::Down(MouseButton::Right) => {
            if t.copy.take().is_some() {
                t.parser.lock().unwrap().screen_mut().set_scrollback(0);
            }
            t.chain_depth = 0;
            t.last_manual_ms = now_ms;
            *flash = paste_clipboard(t)?;
        }
        _ => {}
    }
    Ok(())
}

fn indicator(t: &Tab) -> (char, Color) {
    match t.state {
        TabState::Busy => (SPINNER[t.spinner_idx % SPINNER.len()], NEON_YELLOW),
        TabState::Done => ('●', NEON_GREEN),
        TabState::Question => ('?', NEON_BLUE),
        TabState::Wait => ('●', NEON_BLUE),
        TabState::Exited => ('✖', NEON_RED),
    }
}

/// 描画に必要なUI状態
struct Ui {
    active: usize,
    prefix_active: bool,
    auto: Option<bool>,
    ws_names: Vec<String>,
    ws_index: usize,
    ws_open: bool,
    help_open: bool,
}

fn draw(f: &mut Frame, tabs: &[Tab], ui: &Ui, flash: Option<&str>) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(STATUS_BAR_HEIGHT)])
        .split(f.area());
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(TAB_BAR_WIDTH), Constraint::Min(1)])
        .split(outer[0]);

    let mut lines: Vec<Line> = Vec::new();
    let multi_ws = ui.ws_names.len() > 1;
    // ワークスペースが複数ある時だけ、左バー最上部に現在地を出す
    // (高さは1行のみ消費。クリックでドロップダウン)
    if multi_ws {
        let name = ui
            .ws_names
            .get(ui.ws_index)
            .map(String::as_str)
            .unwrap_or("-");
        lines.push(Line::from(Span::styled(
            format!("[▼] {name}"),
            Style::default()
                .fg(Color::Black)
                .bg(NEON_YELLOW)
                .add_modifier(Modifier::BOLD),
        )));
    }

    if ui.ws_open {
        // ドロップダウン展開中はタブ一覧の代わりにワークスペース一覧を出す
        for (i, name) in ui.ws_names.iter().enumerate() {
            let style = if i == ui.ws_index {
                Style::default().fg(Color::Black).bg(NEON_YELLOW)
            } else {
                Style::default().fg(NEON_YELLOW)
            };
            lines.push(Line::from(Span::styled(
                format!(" {}. {name}", i + 1),
                style,
            )));
        }
    } else {
        let index_style = if ui.active == 0 {
            Style::default().fg(Color::Black).bg(NEON_BLUE)
        } else {
            Style::default().fg(NEON_BLUE)
        };
        lines.push(Line::from(Span::styled("[≡] 0. INDEX", index_style)));
        for (i, t) in tabs.iter().enumerate() {
            let (ind, ind_color) = indicator(t);
            let title_style = if ui.active == i + 1 {
                Style::default()
                    .fg(Color::Black)
                    .bg(NEON_GREEN)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(NEON_GREEN).add_modifier(Modifier::BOLD)
            };
            // 子タブは "└" とインデントで階層表示 (転送関係はLuaが決める)
            let prefix = if t.depth > 0 {
                format!("{}└", " ".repeat((t.depth as usize - 1) * 1))
            } else {
                String::new()
            };
            let label = format!("{prefix}{}. {}", i + 1, t.title);
            let width = TAB_BAR_WIDTH as usize - 4 - 3;
            let label: String = label.chars().take(width).collect();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{ind}] "),
                    Style::default().fg(ind_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(label, title_style),
                Span::styled(
                    if t.locked { " 🔒" } else { "   " },
                    Style::default().fg(NEON_YELLOW),
                ),
            ]));
        }
    }
    let tabs_widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(NEON_GREEN)),
    );
    f.render_widget(tabs_widget, main[0]);

    if ui.active == 0 {
        draw_index(f, tabs, main[1], outer[1], flash, ui);
    } else if let Some(t) = tabs.get(ui.active - 1) {
        draw_session(f, t, main[1], outer[1], flash, ui);
    }

    if ui.help_open {
        draw_help(f, f.area());
    }
}

/// ヘルプオーバーレイ (どこからでも Ctrl+B ? / INDEXで ?)
fn draw_help(f: &mut Frame, area: Rect) {
    let w = 62.min(area.width.saturating_sub(4));
    let h = 18.min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(ratatui::widgets::Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(NEON_YELLOW))
        .title(Span::styled(
            " HELP ",
            Style::default().fg(Color::Black).bg(NEON_YELLOW),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let text = vec![
        Line::from(" Ctrl+B q      終了"),
        Line::from(" Ctrl+B 0-9    タブ切替 (0=INDEX)   n/p 隣のタブ"),
        Line::from(" Ctrl+B w / W  ワークスペース一覧 / 次へ"),
        Line::from(" Ctrl+B l      入力ロック切替 (🔒クリックでも可)"),
        Line::from(" Ctrl+B [      コピーモード  c 最新応答をコピー"),
        Line::from(" Ctrl+B a / x  自動化ON/OFF / 緊急停止"),
        Line::from(" Ctrl+B b      子プロセスへ Ctrl+B を送る"),
        Line::default(),
        Line::from(" マウス:"),
        Line::from("  ホイール    スクロール(コピーモード)"),
        Line::from("  左ドラッグ  選択して離すとコピー"),
        Line::from("  右クリック  ペースト"),
        Line::from("  タブ名/🔒   クリックで切替 / ロック解除"),
        Line::default(),
        Line::from(Span::styled(
            " 何かキーを押すと閉じます",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    f.render_widget(Paragraph::new(text), inner);
}

fn auto_label(auto: Option<bool>) -> &'static str {
    match auto {
        Some(true) => "AUTO:ON | ",
        Some(false) => "AUTO:OFF | ",
        None => "",
    }
}

/// INDEX = ホーム画面: セッション一覧 + メニュー
fn draw_index(
    f: &mut Frame,
    tabs: &[Tab],
    area: Rect,
    status_area: Rect,
    flash: Option<&str>,
    ui: &Ui,
) {
    let title = match ui.ws_names.get(ui.ws_index) {
        Some(n) if ui.ws_names.len() > 1 => format!(" ACTIVE SESSION MAP :: {n} "),
        _ => " ACTIVE SESSION MAP ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(NEON_BLUE))
        .title(Span::styled(title, Style::default().fg(NEON_YELLOW)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = vec![
        Line::from(Span::styled(
            format!(" {:<3} {:<18} {:<10} {}", "NO", "NAME", "STATE", "PROFILE"),
            Style::default().fg(NEON_BLUE).add_modifier(Modifier::BOLD),
        )),
        Line::default(),
    ];
    for (i, t) in tabs.iter().enumerate() {
        let (ind, color) = indicator(t);
        let name = format!(
            "{}{}{}",
            "  ".repeat(t.depth as usize),
            t.title,
            if t.locked { " 🔒" } else { "" }
        );
        lines.push(Line::from(vec![
            Span::styled(format!(" {ind} "), Style::default().fg(color)),
            Span::raw(format!("{}. ", i + 1)),
            Span::styled(
                format!("{name:<18}"),
                Style::default().fg(NEON_GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{:<10}", t.state.label()), Style::default().fg(color)),
            Span::raw(t.profile_name().to_string()),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        " ── MENU ────────────────────────────",
        Style::default().fg(NEON_BLUE),
    )));
    let menu = [
        ("[数字]", "タブへ切替 (タブ名クリックでも可)"),
        ("[w]", "ワークスペース切替"),
        ("[e]", "設定を編集 (ブラウザ)"),
        ("[k]", "マスターパスワード変更"),
        ("[?]", "ヘルプ / キー一覧"),
        ("[q]", "終了"),
    ];
    for (key, desc) in menu {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {key:<7}"),
                Style::default().fg(NEON_YELLOW).add_modifier(Modifier::BOLD),
            ),
            Span::raw(desc),
        ]));
    }
    f.render_widget(Paragraph::new(lines), inner);

    let status = flash.map(|m| format!(" {m}")).unwrap_or_else(|| {
        format!(
            " {}INDEX | メニューはキー押下で実行 / Ctrl+B q: EXIT",
            auto_label(ui.auto)
        )
    });
    f.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Black).bg(NEON_BLUE)),
        status_area,
    );
}

/// セッションペイン: 子端末の描画 + コピーモードハイライト + IMEカーソル + ステータス
fn draw_session(
    f: &mut Frame,
    t: &Tab,
    area: Rect,
    status_area: Rect,
    flash: Option<&str>,
    ui: &Ui,
) {
    let border_color = if t.copy.is_some() {
        NEON_YELLOW
    } else if t.locked {
        NEON_BLUE
    } else {
        NEON_GREEN
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            format!(
                " SESSION :: {} {}",
                t.title,
                if t.locked { "🔒 LOCKED " } else { "" }
            ),
            Style::default().fg(NEON_YELLOW),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let scrollback_offset;
    let alt_screen;
    {
        let parser = t.parser.lock().unwrap();
        let screen = parser.screen();
        scrollback_offset = screen.scrollback();
        alt_screen = screen.alternate_screen();
        f.render_widget(PseudoTerminal::new(screen), inner);

        // IMEの変換ウィンドウ・未確定文字列はホスト端末の実カーソル位置に
        // 表示されるため、子端末のカーソル位置に実カーソルを重ねておく。
        // これが無いと日本語入力の変換候補が画面のあちこちに飛ぶ
        if t.copy.is_none() && scrollback_offset == 0 && !screen.hide_cursor() {
            let (crow, ccol) = screen.cursor_position();
            if crow < inner.height && ccol < inner.width {
                f.set_cursor_position((inner.x + ccol, inner.y + crow));
            }
        }
    }

    // コピーモード: カーソル行と選択範囲をハイライト
    if let Some(cs) = &t.copy {
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
                    Rect {
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

    let status = if let Some(cs) = &t.copy {
        let mode = if cs.anchor.is_some() { "SELECT" } else { "CURSOR" };
        let hist = if alt_screen {
            " | 履歴なし(全画面アプリ)"
        } else {
            ""
        };
        format!(
            " [COPY:{mode}] -{scrollback_offset} | 選択で自動コピー 右クリック:ペースト | v y a / Esc: LIVE{hist}"
        )
    } else if ui.prefix_active {
        " [PREFIX] q:終了 0-9:タブ w:WS l:ロック [:コピー c:応答 a/x:自動 ?:ヘルプ".to_string()
    } else if let Some(msg) = flash {
        format!(" {msg}")
    } else if t.locked {
        " 🔒 LOCKED — 入力は無効です (Ctrl+B l または 🔒クリックで解除)".to_string()
    } else {
        format!(
            " {}PROFILE:{} [{}] | ドラッグ:コピー 右クリック:ペースト | Ctrl+B ?: ヘルプ",
            auto_label(ui.auto),
            t.profile_name(),
            t.state.label()
        )
    };
    let status_bg = if t.copy.is_some() { NEON_YELLOW } else { NEON_GREEN };
    f.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Black).bg(status_bg)),
        status_area,
    );
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
fn paste_clipboard(t: &Tab) -> Result<Option<String>> {
    match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
        Ok(text) => {
            let bracketed = t.parser.lock().unwrap().screen().bracketed_paste();
            let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
            if bracketed {
                let mut bytes = b"\x1b[200~".to_vec();
                bytes.extend_from_slice(normalized.as_bytes());
                bytes.extend_from_slice(b"\x1b[201~");
                t.write_bytes(&bytes)?;
            } else {
                t.write_bytes(normalized.as_bytes())?;
            }
            Ok(None)
        }
        Err(e) => Ok(Some(format!(">> PASTE FAILED: {e}"))),
    }
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

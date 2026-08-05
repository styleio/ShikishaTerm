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

mod caps;
mod config;
mod crypto;
mod detect;
mod hooks;
mod i18n;
mod netaddr;
mod notify;
mod profile;
mod remote;
mod session_log;
mod tab;
mod watch;
mod webui;

use std::sync::{Arc, Mutex};
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
use unicode_width::UnicodeWidthStr as _;

const TAB_BAR_MIN: u16 = 10;
const TAB_BAR_MAX: u16 = 40;
const STATUS_BAR_HEIGHT: u16 = 1;
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

const NEON_GREEN: Color = Color::Rgb(57, 255, 20);
const NEON_YELLOW: Color = Color::Rgb(255, 234, 0);
const NEON_BLUE: Color = Color::Rgb(0, 170, 255);
const NEON_RED: Color = Color::Rgb(255, 70, 70);

/// 異常終了の理由を残す。TUIは画面を占有するため、
/// パニックメッセージが見えないまま消えてしまうのを防ぐ
fn install_crash_log() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let where_ = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        append_hook_log(&format!("!!! 異常終了 {where_}: {info}"));
        let _ = std::fs::create_dir_all("logs");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("logs/crash.log")
        {
            use std::io::Write as _;
            let _ = writeln!(f, "{where_}: {info}");
        }
        prev(info);
    }));
}

fn main() -> Result<()> {
    install_crash_log();
    // 表示言語を決める (設定 → OS の順。翻訳が無ければ英語)
    i18n::init(
        config::load().and_then(|c| c.language).as_deref(),
        &[config_file_dir(), std::path::PathBuf::from(".")],
    );
    // 設定だけ開くモード (TUIを起動せずブラウザで設定を編集する)
    if std::env::args().nth(1).as_deref() == Some("--settings") {
        // 本体が動いていなくてもQRを出せる (接続先は設定から都度組み立てる)
        let info = Arc::new(Mutex::new(webui::RemoteInfo::default()));
        let web = webui::WebUi::start_with(config::config_file_path(), info)?;
        println!("{}", i18n::tp("msg.settings_opened", &[("url", &web.url)]));
        open_browser(&web.url);
        println!("{}", i18n::t("msg.settings_wait"));
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        web.shutdown();
        return Ok(());
    }

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let res = run(&mut terminal);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    res
}

/// タブバー・枠線・ステータスバーを除いた、PTYに渡す端末サイズ (rows, cols)
fn pty_dims(size: Size, tab_w: u16) -> (u16, u16) {
    let cols = size.width.saturating_sub(tab_w + 2).max(10);
    let rows = size.height.saturating_sub(STATUS_BAR_HEIGHT + 2).max(3);
    (rows, cols)
}

/// ターミナルペインの内側 (枠線の内側) の矩形。pty_dimsと整合させること
fn pane_inner(size: Size, tab_w: u16) -> Rect {
    let (rows, cols) = pty_dims(size, tab_w);
    Rect {
        x: tab_w + 1,
        y: 1,
        width: cols,
        height: rows,
    }
}

/// タブ名に合わせたタブバー幅の自動算出。
/// "[x] 12. タブ名 🔒" が収まる幅 (全角考慮) を求め、範囲内に収める
fn auto_tab_width(tabs: &[Tab]) -> u16 {
    let longest = tabs
        .iter()
        .map(|t| {
            // インジケータ4桁 + "N. " + 名前 + インデント + 錠2桁 + 枠線1桁
            4 + 4 + t.title.width() as u16 + t.depth + 2 + 1
        })
        .max()
        .unwrap_or(TAB_BAR_MIN);
    longest.clamp(TAB_BAR_MIN, TAB_BAR_MAX)
}

fn in_rect(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

/// 表示幅 (全角=2桁) で切り詰める。日本語タブ名がはみ出さないように
fn truncate_width(s: &str, max: u16) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max as usize {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

/// セッション見出しの文言と、その中で錠アイコンが始まる表示位置。
/// 描画とクリック判定の両方で使い、位置ズレが起きないようにする
fn session_title(t: &Tab) -> (String, String, u16) {
    let head = format!(" {} :: {} ", i18n::t("tui.session"), t.title);
    let offset = head.width() as u16;
    let lock = if t.locked {
        "🔒 LOCKED ".to_string()
    } else {
        "🔓 UNLOCK ".to_string()
    };
    (head, lock, offset)
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

/// マスターパスワードの入力モーダル。入力は伏字表示。
/// Escでキャンセル (None)。パスワードはTUI内で完結させ、ブラウザには出さない
fn prompt_password(
    terminal: &mut ratatui::DefaultTerminal,
    title: &str,
    note: &str,
) -> Result<Option<String>> {
    let mut input = String::new();
    loop {
        terminal.draw(|f| {
            let area = f.area();
            let w = 56.min(area.width.saturating_sub(4));
            let rect = Rect {
                x: area.x + (area.width.saturating_sub(w)) / 2,
                y: area.y + area.height / 3,
                width: w,
                height: 7,
            };
            f.render_widget(ratatui::widgets::Clear, rect);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(NEON_YELLOW))
                .title(Span::styled(
                    format!(" {title} "),
                    Style::default().fg(Color::Black).bg(NEON_YELLOW),
                ));
            let inner = block.inner(rect);
            f.render_widget(block, rect);
            let text = vec![
                Line::from(Span::styled(note, Style::default().fg(Color::DarkGray))),
                Line::default(),
                Line::from(vec![
                    Span::styled("  > ", Style::default().fg(NEON_GREEN)),
                    Span::styled(
                        "*".repeat(input.chars().count()),
                        Style::default().fg(NEON_GREEN),
                    ),
                    Span::styled("_", Style::default().fg(NEON_GREEN)),
                ]),
                Line::default(),
                Line::from(Span::styled(
                    format!("  {}", i18n::t("prompt.password.keys")),
                    Style::default().fg(Color::DarkGray),
                )),
            ];
            f.render_widget(Paragraph::new(text), inner);
        })?;

        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Enter => return Ok(Some(input)),
                KeyCode::Esc => return Ok(None),
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        input.push(c);
                    }
                }
                _ => {}
            }
        }
    }
}

/// マスターパスワードの設定・変更・解除 (INDEXメニュー [k])
fn manage_master_password(
    terminal: &mut ratatui::DefaultTerminal,
    cfg: Option<&config::Config>,
    password: &mut Option<String>,
) -> Result<String> {
    let Some(path) = cfg.and_then(|c| c.secrets_path()) else {
        return Ok(i18n::t("msg.password.no_secrets"));
    };
    if !path.exists() {
        return Ok(i18n::tp("msg.password.missing", &[("path", &path.display().to_string())]));
    }
    let text = std::fs::read_to_string(&path)?;

    if crypto::is_encrypted(&text) {
        // 変更 or 解除
        let Some(old) = prompt_password(
            terminal,
            &i18n::t("prompt.password.current"),
            &i18n::t("prompt.password.current_note"),
        )?
        else {
            return Ok(i18n::t("msg.password.cancelled"));
        };
        let env: crypto::Envelope = serde_json::from_str(&text)?;
        let plain = match crypto::decrypt(&env, &old) {
            Ok(p) => p,
            Err(e) => return Ok(format!(">> {e}")),
        };
        let Some(new) = prompt_password(
            terminal,
            &i18n::t("prompt.password.new"),
            &i18n::t("prompt.password.new_note"),
        )? else {
            return Ok(i18n::t("msg.password.cancelled"));
        };
        if new.is_empty() {
            crypto::write_atomic(&path, &plain)?;
            *password = None;
            return Ok(i18n::t("msg.password.removed"));
        }
        let confirm = prompt_password(terminal, &i18n::t("prompt.password.confirm"), "")?;
        if confirm.as_deref() != Some(new.as_str()) {
            return Ok(i18n::t("msg.password.mismatch"));
        }
        crypto::write_atomic(&path, &serde_json::to_string_pretty(&crypto::encrypt(&plain, &new)?)?)?;
        *password = Some(new);
        Ok(i18n::t("msg.password.changed"))
    } else {
        // 新規設定
        let Some(new) = prompt_password(
            terminal,
            &i18n::t("prompt.password.set"),
            &i18n::t("prompt.password.set_note"),
        )? else {
            return Ok(i18n::t("msg.password.cancelled"));
        };
        if new.is_empty() {
            return Ok(i18n::t("msg.password.empty"));
        }
        let confirm = prompt_password(terminal, &i18n::t("prompt.password.confirm"), "")?;
        if confirm.as_deref() != Some(new.as_str()) {
            return Ok(i18n::t("msg.password.mismatch"));
        }
        crypto::encrypt_file(&path, &new)?;
        *password = Some(new);
        Ok(i18n::t("msg.password.encrypted"))
    }
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let cmd_args: Vec<String> = std::env::args().skip(1).collect();
    let start = Instant::now();
    // 幅はconfig指定 → 無ければタブ名から自動算出 (タブ起動後に確定)
    let mut tab_w = 18u16;
    let (mut rows, mut cols) = pty_dims(terminal.size()?, tab_w);

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
        tabs.push(Tab::spawn(
            title_of(&cmd_args),
            &cmd_args,
            None,
            rows,
            cols,
            tab::TabOptions::default(),
        )?);
    } else if !workspaces.is_empty() {
        spawn_workspace(&workspaces[0], rows, cols, &mut tabs, &mut startup_errors);
    }
    // 設定がまだ無い = 初回起動。何をすればいいか分からないまま
    // シェルが1つ開くだけ、という体験にならないよう案内する
    let first_run = cmd_args.is_empty() && cfg.is_none();
    if tabs.is_empty() && workspaces.is_empty() {
        let argv = vec!["powershell.exe".to_string()];
        tabs.push(Tab::spawn(
            "SHELL".into(),
            &argv,
            None,
            rows,
            cols,
            tab::TabOptions::default(),
        )?);
    }

    // タブ名が出揃ってから幅を確定し、PTYサイズを合わせ直す
    tab_w = match cfg.as_ref().and_then(|c| c.tab_bar_width) {
        Some(w) => w.clamp(TAB_BAR_MIN, TAB_BAR_MAX),
        None => auto_tab_width(&tabs),
    };
    (rows, cols) = pty_dims(terminal.size()?, tab_w);
    for t in &tabs {
        let _ = t.resize(rows, cols);
    }

    // Luaフックエンジンはワークスペース単位 (共有変数もその中で共有される)。
    // 未使用のワークスペースは作らず、切替時に必要なら生成する
    let mut max_chain = cfg.as_ref().and_then(|c| c.max_chain).unwrap_or(10);
    // secretsが暗号化されていれば起動時にマスターパスワードを尋ねる
    let mut password: Option<String> = None;
    if let Some(path) = cfg.as_ref().and_then(|c| c.secrets_path()) {
        if std::fs::read_to_string(&path)
            .map(|t| crypto::is_encrypted(&t))
            .unwrap_or(false)
        {
            for attempt in 1..=3 {
                let note = if attempt == 1 {
                    i18n::t("prompt.password.note")
                } else {
                    i18n::t("prompt.password.retry")
                };
                match prompt_password(terminal, &i18n::t("prompt.password.title"), &note)? {
                    Some(pw) => {
                        let ok = std::fs::read_to_string(&path)
                            .ok()
                            .and_then(|t| serde_json::from_str::<crypto::Envelope>(&t).ok())
                            .map(|env| crypto::decrypt(&env, &pw).is_ok())
                            .unwrap_or(false);
                        if ok {
                            password = Some(pw);
                            break;
                        }
                    }
                    // キャンセル時は秘密情報なしで続行 (通知だけが使えない)
                    None => {
                        startup_errors
                            .push(i18n::t("prompt.password.skipped"));
                        break;
                    }
                }
            }
        }
    }

    // 通知先 (Slack / Telegram)。Luaはここに登録された宛先にしか送れない
    let mut notifier = match cfg.as_ref() {
        Some(c) => {
            let (dests, err) = c.resolve_notify(password.as_deref());
            if let Some(e) = err {
                startup_errors.push(e);
            }
            notify::Notifier::new(dests)
        }
        None => notify::Notifier::new(Default::default()),
    };
    // 自動化に与える能力 (既定は空)。設定ファイルにだけ書ける玄人向け機能
    let mut caps: hooks::Caps = std::rc::Rc::new(match cfg.as_ref() {
        Some(c) => caps::Capabilities::new(
            c.capabilities.clone(),
            config_file_dir(),
            c.resolve_tokens(password.as_deref()),
        ),
        None => caps::Capabilities::disabled(),
    });
    let mut engines: Vec<Option<HookEngine>> = (0..workspaces.len().max(1)).map(|_| None).collect();
    engines[0] = build_engine(cfg.as_ref(), workspaces.first(), &mut startup_errors, &caps);
    let mut engine = engines[0].take();

    // リモートUI (スマホ等から監視・指示する)。設定で有効にしたときだけ待ち受ける。
    // 状況は設定画面にも渡し、QRコードをブラウザで見られるようにする
    let remote_info: Arc<Mutex<webui::RemoteInfo>> = Arc::new(Mutex::new(Default::default()));
    let mut remote_ui = start_remote(cfg.as_ref(), password.as_deref(), &mut startup_errors);
    publish_remote(&remote_info, &remote_ui);

    let mut auto_enabled = true;
    let mut started_fired = vec![false; tabs.len()];

    // 0 = INDEX、1.. = セッション。初回はINDEX(案内のある画面)から始める
    let mut active: usize = if tabs.is_empty() || first_run { 0 } else { 1 };
    let mut prefix_active = false;
    let mut flash: Option<String> = startup_errors.first().map(|e| i18n::tp("msg.startup_failed", &[("error", e)]));
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
    // 設定ファイルの変更監視 (保存したら再起動なしで反映する)
    let mut watcher = watch::Watcher::new(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
    let mut cfg = cfg;

    let mut ws_open = false;
    let mut help_open = false;
    let mut qr_open = false;
    // タブバー境界線のドラッグ中フラグ (マウスで幅を調整できる)
    let mut dragging_divider = false;
    // 設定Web GUI (INDEXの [e] で起動、アプリ終了時に停止)
    let mut web: Option<webui::WebUi> = None;
    let config_file = config::config_file_path();

    loop {
        // 設定が保存されたら読み直して反映する (アプリの再起動は不要)
        if watcher.changed() {
            if let Some(newcfg) = config::load() {
                let (new_ws, errs) = newcfg.resolve_workspaces();
                startup_errors.extend(errs);
                // 表示中のワークスペースへ即反映し、他は切替時に反映する
                let target = new_ws
                    .iter()
                    .position(|w| Some(&w.name) == workspaces.get(ws_index).map(|w| &w.name))
                    .unwrap_or(0);
                let mut msg = i18n::t("msg.config_reloaded");
                if let Some(w) = new_ws.get(target) {
                    msg = apply_ws_config(&mut tabs, w, rows, cols, &mut startup_errors);
                    ws_index = target;
                }
                // 他のワークスペースは作り直しに任せる (裏で動いているタブは触らない)
                ws_tabs.resize_with(new_ws.len().max(1), Vec::new);
                workspaces = new_ws;
                max_chain = newcfg.max_chain.unwrap_or(10);
                if let Some(w) = newcfg.tab_bar_width {
                    let w = w.clamp(TAB_BAR_MIN, TAB_BAR_MAX);
                    if w != tab_w {
                        tab_w = w;
                        (rows, cols) = pty_dims(terminal.size()?, tab_w);
                        for t in &tabs {
                            let _ = t.resize(rows, cols);
                        }
                    }
                }
                // 通知先・能力・自動化スクリプトを作り直す
                let (dests, err) = newcfg.resolve_notify(password.as_deref());
                if let Some(e) = err {
                    startup_errors.push(e);
                }
                notifier = notify::Notifier::new(dests);
                caps = std::rc::Rc::new(caps::Capabilities::new(
                    newcfg.capabilities.clone(),
                    config_file_dir(),
                    newcfg.resolve_tokens(password.as_deref()),
                ));
                engine = build_engine(
                    Some(&newcfg),
                    workspaces.get(ws_index),
                    &mut startup_errors,
                    &caps,
                );
                started_fired.clear();
                started_fired.resize(tabs.len(), false);
                if active > tabs.len() {
                    active = if tabs.is_empty() { 0 } else { 1 };
                }
                // リモートUIの設定変更を反映する (有効化/無効化もここで効く)
                let mut remote_changed: Option<String> = None;
                let want = newcfg.remote.clone();
                let now = cfg.as_ref().map(|c| c.remote.clone()).unwrap_or_default();
                if (want.enabled, &want.bind, want.port, want.allow_public)
                    != (now.enabled, &now.bind, now.port, now.allow_public)
                {
                    if let Some(r) = &remote_ui {
                        r.shutdown();
                    }
                    remote_ui = start_remote(Some(&newcfg), password.as_deref(), &mut startup_errors);
                    publish_remote(&remote_info, &remote_ui);
                    remote_changed = Some(if remote_ui.is_some() {
                        i18n::t("msg.remote_enabled")
                    } else {
                        i18n::t("msg.remote_stopped")
                    });
                }
                cfg = Some(newcfg);
                watcher.retarget(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
                flash = Some(match remote_changed {
                    Some(m) => format!(">> {m}"),
                    None => format!(">> {msg}"),
                });
            }
        }

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
                // ループ中から現在の状態を読めるようにする (shikisha.state)
                eng.set_states(
                    tabs.iter()
                        .map(|t| (t.key(), t.state.label().to_string()))
                        .collect(),
                );
                // 終了したタブで待機中のループは破棄する (無限ループを残さない)
                for &(idx, old, new) in &transitions {
                    if new == TabState::Exited && old != TabState::Exited {
                        eng.cancel_tab(idx);
                    }
                }
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
                        // 再起動したら on_start をやり直す (SSH再接続後のresume自動化)
                        if new != TabState::Exited && old == TabState::Exited {
                            if let Some(f) = started_fired.get_mut(idx - 1) {
                                *f = false;
                            }
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
                    eng.tick_pending(&|idx| {
                        tabs.get(idx.wrapping_sub(1))
                            .map(|t| t.parser.lock().unwrap().screen().contents())
                    });
                }
                let cmds = eng.drain_commands();
                if !cmds.is_empty() {
                    let now_ms = start.elapsed().as_millis() as u64;
                    exec_commands(
                        cmds,
                        &mut tabs,
                        max_chain,
                        auto_enabled,
                        now_ms,
                        rows,
                        cols,
                        &notifier,
                        &mut flash,
                    );
                }
            }

            // リモートUIへ現在の状況を渡し、届いた操作を実行する
            if let Some(r) = remote_ui.as_ref() {
                *r.snapshot.lock().unwrap() = remote::Snapshot {
                    workspace: workspaces
                        .get(ws_index)
                        .map(|w| w.name.clone())
                        .unwrap_or_default(),
                    auto_enabled,
                    tabs: tabs
                        .iter()
                        .enumerate()
                        .map(|(i, t)| remote::RemoteTab {
                            index: i + 1,
                            name: t.title.clone(),
                            state: t.state.label().to_string(),
                            locked: t.locked,
                            output: trim_for_phone(
                                &t.last_response.clone().unwrap_or_default(),
                                200,
                            ),
                            screen: trim_for_phone(
                                &t.parser.lock().unwrap().screen().contents(),
                                200,
                            ),
                        })
                        .collect(),
                };
                let now_ms = start.elapsed().as_millis() as u64;
                while let Ok(cmd) = r.rx.try_recv() {
                    match cmd {
                        // 遠隔からの入力は人間の操作として扱う
                        // (自動チェーンをリセットし、ロック中は拒否する)
                        remote::RemoteCmd::Send { tab, text } => {
                            if let Some(t) = tabs.get_mut(tab.wrapping_sub(1)) {
                                if t.locked {
                                    continue;
                                }
                                t.chain_depth = 0;
                                t.last_manual_ms = now_ms;
                                let bracketed =
                                    t.parser.lock().unwrap().screen().bracketed_paste();
                                let body = text.replace("\r\n", "\r").replace('\n', "\r");
                                let mut bytes = Vec::new();
                                if bracketed {
                                    bytes.extend_from_slice(b"\x1b[200~");
                                    bytes.extend_from_slice(body.as_bytes());
                                    bytes.extend_from_slice(b"\x1b[201~");
                                } else {
                                    bytes.extend_from_slice(body.as_bytes());
                                }
                                bytes.push(b'\r');
                                let _ = t.write_bytes(&bytes);
                                append_hook_log(&format!("remote送信 tab{tab}"));
                            }
                        }
                        remote::RemoteCmd::Keys { tab, keys } => {
                            if let Some(t) = tabs.get_mut(tab.wrapping_sub(1)) {
                                if t.locked {
                                    continue;
                                }
                                t.chain_depth = 0;
                                t.last_manual_ms = now_ms;
                                let _ = t.write_bytes(keys.as_bytes());
                            }
                        }
                        remote::RemoteCmd::SetAuto(on) => {
                            auto_enabled = on;
                            if !on {
                                if let Some(eng) = engine.as_mut() {
                                    eng.cancel_all();
                                }
                            }
                            flash = Some(i18n::t(if on {
                                "msg.remote_auto_on"
                            } else {
                                "msg.remote_auto_off"
                            }));
                        }
                    }
                }
            }

            // auto_restart: 終了したタブを自動で復帰させる
            for (i, t) in tabs.iter_mut().enumerate() {
                if t.state == TabState::Exited && t.auto_restart {
                    match t.restart(rows, cols) {
                        Ok(()) => {
                            append_hook_log(&format!("auto-restart tab{}", i + 1));
                            flash = Some(i18n::tp("msg.restarted", &[("name", &t.title)]));
                        }
                        Err(e) => flash = Some(i18n::tp("msg.restart_failed", &[("error", &e.to_string())])),
                    }
                }
            }
        }

        let ui = Ui {
            tab_w,
            first_run,
            active,
            prefix_active,
            auto: engine.as_ref().map(|_| auto_enabled),
            ws_names: workspaces.iter().map(|w| w.name.clone()).collect(),
            ws_index,
            ws_open,
            help_open,
            qr: if qr_open { remote_ui.as_ref().map(|r| r.url.clone()) } else { None },
            remote_on: remote_ui.is_some(),
        };
        terminal.draw(|f| draw(f, &tabs, &ui, flash.as_deref()))?;

        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                flash = None;
                // オーバーレイ (ヘルプ / QR / ワークスペース一覧) が最優先
                if help_open {
                    help_open = false;
                    continue;
                }
                if qr_open {
                    qr_open = false;
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
                                    cfg.as_ref(),
                                    &mut engine,
                                    &mut engines,
                                    &caps,
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
                        // Ctrl+B r このタブを再起動 (終了・切断からの復帰)
                        KeyCode::Char('r') => {
                            if let Some(eng) = engine.as_mut() {
                                eng.cancel_tab(active);
                            }
                            if let Some(t) = session_mut(&mut tabs, active) {
                                flash = Some(match t.restart(rows, cols) {
                                    Ok(()) => i18n::tp("msg.restarted", &[("name", &t.title)]),
                                    Err(e) => i18n::tp("msg.restart_failed", &[("error", &e.to_string())]),
                                });
                            }
                        }
                        // Ctrl+B l 入力ロック切替 / w ワークスペース一覧 / ? ヘルプ
                        KeyCode::Char('l') => {
                            if let Some(t) = session_mut(&mut tabs, active) {
                                t.locked = !t.locked;
                                flash = Some(i18n::t(if t.locked {
                                    "msg.lock_on"
                                } else {
                                    "msg.lock_off"
                                }));
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
                                    cfg.as_ref(),
                                    &mut engine,
                                    &mut engines,
                                    &caps,
                                );
                            }
                        }
                        KeyCode::Char('?') => help_open = true,
                        // Ctrl+B a 自動化ON/OFF、Ctrl+B x 緊急停止
                        KeyCode::Char('a') => {
                            auto_enabled = !auto_enabled;
                            flash = Some(i18n::t(if auto_enabled {
                                "msg.auto_on"
                            } else {
                                "msg.auto_off"
                            }));
                        }
                        KeyCode::Char('x') => {
                            auto_enabled = false;
                            // 待機中のループも全て破棄する (再開時に蘇らせない)
                            if let Some(eng) = engine.as_mut() {
                                eng.cancel_all();
                            }
                            flash =
                                Some(i18n::t("msg.emergency_stop"));
                        }
                        // Ctrl+B c で最新キャプチャ応答をクリップボードへ
                        KeyCode::Char('c') => {
                            if let Some(t) = session_mut(&mut tabs, active) {
                                flash = Some(match &t.last_response {
                                    Some(r) if !r.trim().is_empty() => copy_to_clipboard(r),
                                    _ => i18n::t("msg.no_response"),
                                });
                            }
                        }
                        // Ctrl+B [ でコピーモード (tmuxのコピーモード風)
                        KeyCode::Char('[') => {
                            let rows = pty_dims(terminal.size()?, tab_w).0;
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
                        // スマホから繋ぐためのQRコードを出す
                        KeyCode::Char('i') => {
                            if remote_ui.is_some() {
                                qr_open = true;
                            } else {
                                flash = Some(
                                    i18n::t("msg.remote_disabled"),
                                );
                            }
                        }
                        KeyCode::Char('w') => {
                            if workspaces.len() > 1 {
                                ws_open = true;
                            }
                        }
                        KeyCode::Char('r') => {
                            let mut msgs = Vec::new();
                            for t in tabs.iter_mut().filter(|t| t.state == TabState::Exited) {
                                match t.restart(rows, cols) {
                                    Ok(()) => msgs.push(t.title.clone()),
                                    Err(e) => msgs.push(format!("{}(失敗:{e})", t.title)),
                                }
                            }
                            flash = Some(if msgs.is_empty() {
                                i18n::t("msg.restart_none")
                            } else {
                                i18n::tp("msg.restarted_list", &[("names", &msgs.join(", "))])
                            });
                        }
                        // 通知先の疎通確認 (フックを待たずに設定を検証できる)
                        KeyCode::Char('t') => {
                            flash = Some(if notifier.is_empty() {
                                i18n::t("msg.notify_none")
                            } else {
                                notifier.send_all("ShikishaTerm-AI: テスト通知")
                            });
                        }
                        // マスターパスワードの設定・変更・解除 (TUI内で完結)
                        KeyCode::Char('k') => {
                            flash = Some(manage_master_password(
                                terminal,
                                cfg.as_ref(),
                                &mut password,
                            )?);
                        }
                        // 設定GUI: ローカルWebサーバーを起動してブラウザで開く
                        KeyCode::Char('e') => {
                            flash = Some(match web.as_ref() {
                                Some(w) => {
                                    open_browser(&w.url);
                                    i18n::tp("msg.settings_opened", &[("url", &w.url)])
                                }
                                None => match webui::WebUi::start_with(config_file.clone(), Arc::clone(&remote_info)) {
                                    Ok(w) => {
                                        open_browser(&w.url);
                                        let msg = i18n::tp("msg.settings_opened", &[("url", &w.url)]);
                                        web = Some(w);
                                        msg
                                    }
                                    Err(e) => i18n::tp("msg.settings_failed", &[("error", &e.to_string())]),
                                },
                            });
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
                            handle_copy_key(t, &key, size, tab_w, &mut flash)?;
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
                            i18n::t("msg.locked"),
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
                        if m.column < tab_w && m.row >= 1 {
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
                                    cfg.as_ref(),
                                    &mut engine,
                                    &mut engines,
                                    &caps,
                                );
                            }
                        }
                        ws_open = false;
                    }
                    continue;
                }
                // タブバーの境界線をドラッグして幅を調整する
                match m.kind {
                    MouseEventKind::Down(MouseButton::Left) if m.column == tab_w - 1 => {
                        dragging_divider = true;
                        continue;
                    }
                    MouseEventKind::Drag(MouseButton::Left) if dragging_divider => {
                        let new_w = (m.column + 1).clamp(TAB_BAR_MIN, TAB_BAR_MAX);
                        if new_w != tab_w {
                            tab_w = new_w;
                            (rows, cols) = pty_dims(terminal.size()?, tab_w);
                            for t in &tabs {
                                let _ = t.resize(rows, cols);
                            }
                        }
                        continue;
                    }
                    MouseEventKind::Up(MouseButton::Left) if dragging_divider => {
                        dragging_divider = false;
                        flash = Some(i18n::tp(
                            "msg.tabbar_width",
                            &[("width", &tab_w.to_string())],
                        ));
                        continue;
                    }
                    _ => {}
                }
                // 左バー最上部のワークスペース名クリックで一覧を開く
                if workspaces.len() > 1
                    && matches!(m.kind, MouseEventKind::Down(MouseButton::Left))
                    && m.column < tab_w
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
                    tab_w,
                    rows,
                    cols,
                    &mut flash,
                )?;
            }
            Event::Resize(width, height) => {
                (rows, cols) = pty_dims(Size { width, height }, tab_w);
                for t in &tabs {
                    let _ = t.resize(rows, cols);
                }
            }
            _ => {}
        }
    }

    if let Some(w) = &web {
        w.shutdown();
    }
    if let Some(r) = &remote_ui {
        r.shutdown();
    }
    for t in tabs.iter_mut() {
        t.kill();
    }
    Ok(())
}

/// 既定のブラウザでURLを開く
fn open_browser(url: &str) {
    // cmd の start はURL内の & を分割してしまうため、空タイトル引数の後に渡す
    let _ = std::process::Command::new("cmd")
        .args(["/c", "start", "", url])
        .spawn();
}

/// Luaフックを3階層 (基本 > ワークスペース > タブ) で読み込む。
/// フックの引き当ては「より具体的な方が勝つ」ので、タブ用スクリプトが
/// 定義していないフックだけがワークスペース・基本へフォールバックする
fn build_engine(
    cfg: Option<&config::Config>,
    ws: Option<&config::Workspace>,
    errors: &mut Vec<String>,
    caps: &hooks::Caps,
) -> Option<HookEngine> {
    let base = cfg.and_then(|c| c.automation_path());
    let ws_lua = ws.and_then(|w| w.automation.clone());
    let tab_luas: Vec<(usize, String)> = ws
        .map(|w| {
            w.tabs
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.cfg.automation_path().map(|p| (i + 1, p)))
                .collect()
        })
        .unwrap_or_default();
    if base.is_none() && ws_lua.is_none() && tab_luas.is_empty() {
        return None;
    }

    let mut engine = match HookEngine::with_caps(hooks::Caps::clone(caps)) {
        Ok(e) => e,
        Err(e) => {
            errors.push(format!("Lua: {e:#}"));
            return None;
        }
    };
    let load = |engine: &mut HookEngine, path: &str, errors: &mut Vec<String>| -> Option<usize> {
        match engine.load_path(&resolve_data_path(path)) {
            Ok(id) => Some(id),
            Err(e) => {
                errors.push(format!("Lua({path}): {e:#}"));
                None
            }
        }
    };
    if let Some(p) = &base {
        if let Some(id) = load(&mut engine, p, errors) {
            engine.set_base(id);
        }
    }
    if let Some(p) = &ws_lua {
        if let Some(id) = load(&mut engine, p, errors) {
            engine.set_workspace(id);
        }
    }
    for (idx, p) in &tab_luas {
        if let Some(id) = load(&mut engine, p, errors) {
            engine.set_tab(*idx, id);
        }
    }
    (!engine.is_empty()).then_some(engine)
}

/// タブ設定を作り直したTabOptionsに変換する
fn tab_options(cfg: &config::TabConfig) -> tab::TabOptions {
    tab::TabOptions {
        // 相対指定は設定ファイルの場所を基準にする (フォルダごと持ち運べる)
        cwd: cfg.cwd.as_ref().map(|c| {
            let p = std::path::PathBuf::from(c);
            if p.is_absolute() {
                p
            } else {
                config_file_dir().join(p)
            }
        }),
        scrollback: cfg.scrollback.unwrap_or(tab::SCROLLBACK_LINES),
        encoding: tab::TabOptions::encoding_from_name(cfg.encoding.as_deref()),
        log: cfg.log,
    }
}

/// 設定変更を、起動中のタブ群へ反映する。
/// 反映できるものは即座に、セッションの作り直しが要るものは保留して印を付ける
/// (実行中のAIを勝手に切らないため)。戻り値は利用者への報告メッセージ
fn apply_ws_config(
    tabs: &mut Vec<Tab>,
    ws: &config::Workspace,
    rows: u16,
    cols: u16,
    errors: &mut Vec<String>,
) -> String {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut staged = 0usize;

    // 設定に無くなったタブを閉じる (GUIで削除された = 明示的な指示)
    let wanted: Vec<String> = ws
        .tabs
        .iter()
        .map(|f| {
            f.cfg
                .name
                .clone()
                .unwrap_or_else(|| title_of(&f.cfg.command.argv()))
        })
        .collect();
    tabs.retain_mut(|t| {
        if wanted.contains(&t.title) {
            true
        } else {
            t.kill();
            removed += 1;
            false
        }
    });

    // 既存タブの更新と、新規タブの追加
    let mut ordered: Vec<Tab> = Vec::with_capacity(ws.tabs.len());
    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        if argv.is_empty() {
            continue;
        }
        let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
        let opts = tab_options(&ft.cfg);
        match tabs.iter().position(|t| t.title == title) {
            Some(i) => {
                let mut t = tabs.remove(i);
                t.apply_live_config(
                    ft.cfg.profile.clone(),
                    ft.cfg.locked,
                    ft.cfg.auto_restart,
                    ft.depth,
                );
                // コマンド・文字コード・行数の変更は作り直しが必要
                if t.signature() != tab::signature_of(&argv, &opts) {
                    t.stage_restart_config(argv.clone(), opts);
                    staged += 1;
                }
                ordered.push(t);
            }
            None => match Tab::spawn(title.clone(), &argv, ft.cfg.profile.clone(), rows, cols, opts) {
                Ok(mut t) => {
                    t.locked = ft.cfg.locked;
                    t.auto_restart = ft.cfg.auto_restart;
                    t.depth = ft.depth;
                    t.id = ft.cfg.id.clone();
                    ordered.push(t);
                    added += 1;
                }
                Err(e) => errors.push(format!("{title}: {e}")),
            },
        }
    }
    // 設定に載っていない残りは閉じる
    for mut t in tabs.drain(..) {
        t.kill();
        removed += 1;
    }
    *tabs = ordered;

    let mut parts = vec![i18n::t("msg.config_reloaded")];
    if added > 0 {
        parts.push(i18n::tp("msg.config_added", &[("n", &added.to_string())]));
    }
    if removed > 0 {
        parts.push(i18n::tp("msg.config_removed", &[("n", &removed.to_string())]));
    }
    if staged > 0 {
        parts.push(i18n::tp("msg.config_needs_restart", &[("n", &staged.to_string())]));
    }
    parts.join(" / ")
}

/// ワークスペースのタブ群を起動する (初回アクティブ化時に呼ぶ)
fn spawn_workspace(
    ws: &config::Workspace,
    rows: u16,
    cols: u16,
    tabs: &mut Vec<Tab>,
    errors: &mut Vec<String>,
) {
    // 呼び名が重複していると自動化の送信先が定まらないので知らせる
    let dups = config::duplicate_keys(ws);
    if !dups.is_empty() {
        errors.push(format!(
            "タブ名が重複しています ({}) — 自動化から指すにはIDを設定してください",
            dups.join(", ")
        ));
    }
    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        if argv.is_empty() {
            continue;
        }
        let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
        match Tab::spawn(
            title.clone(),
            &argv,
            ft.cfg.profile.clone(),
            rows,
            cols,
            tab_options(&ft.cfg),
        ) {
            Ok(mut tab) => {
                tab.locked = ft.cfg.locked;
                tab.auto_restart = ft.cfg.auto_restart;
                tab.depth = ft.depth;
                tab.id = ft.cfg.id.clone();
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
    cfg: Option<&config::Config>,
    engine: &mut Option<HookEngine>,
    engines: &mut [Option<HookEngine>],
    caps: &hooks::Caps,
) {
    if to == *ws_index || to >= workspaces.len() {
        return;
    }
    ws_tabs[*ws_index] = std::mem::take(tabs);
    // Lua環境はワークスペース毎に保持する (共有変数が切替で失われない)
    engines[*ws_index] = engine.take();
    *ws_index = to;
    *tabs = std::mem::take(&mut ws_tabs[to]);
    if tabs.is_empty() {
        spawn_workspace(&workspaces[to], rows, cols, tabs, errors);
    }
    *engine = match engines[to].take() {
        Some(e) => Some(e),
        None => build_engine(cfg, workspaces.get(to), errors, caps),
    };
    started_fired.clear();
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

/// スマホへ送る画面テキストを整える。
/// 端末の空行がそのままだと本文が見えなくなるので末尾を落とし、
/// 通信量のために行数も抑える
fn trim_for_phone(s: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map(|i| i + 1)
        .unwrap_or(0);
    let start = end.saturating_sub(max_lines);
    lines[start..end]
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// リモートUIのトークンを決める。
/// secretsにあればそれを使い、無ければ .remote-token に保存して使い回す
/// (毎回変わるとスマホを繋ぎ直すことになり、QRも設定画面から出せない)
pub fn remote_token(cfg: &config::Config, password: Option<&str>) -> String {
    if let Some(t) = cfg.remote_token(password) {
        return t;
    }
    let path = config_file_dir().join(".remote-token");
    if let Ok(t) = std::fs::read_to_string(&path) {
        let t = t.trim().to_string();
        if t.len() >= 16 {
            return t;
        }
    }
    let t = random_hex(24);
    let _ = crypto::write_atomic(&path, &t);
    t
}

/// 設定に従ってリモートUIを開始する (無効なら None)
fn start_remote(
    cfg: Option<&config::Config>,
    password: Option<&str>,
    errors: &mut Vec<String>,
) -> Option<remote::RemoteUi> {
    let c = cfg.filter(|c| c.remote.enabled)?;
    match netaddr::resolve_bind(&c.remote.bind, c.remote.allow_public) {
        Ok((ip, note)) => {
            let token = remote_token(c, password);
            match remote::RemoteUi::start(ip, c.remote.port, token) {
                Ok(mut r) => {
                    if let Some(n) = &note {
                        errors.push(n.clone());
                    }
                    r.note = note;
                    Some(r)
                }
                Err(e) => {
                    errors.push(format!("リモートUI: {e}"));
                    None
                }
            }
        }
        Err(e) => {
            errors.push(format!("リモートUI: {e}"));
            None
        }
    }
}

/// 設定画面がQRコードを出せるよう、現在の待ち受け状況を渡す
fn publish_remote(info: &Arc<Mutex<webui::RemoteInfo>>, ui: &Option<remote::RemoteUi>) {
    let mut i = info.lock().unwrap();
    match ui {
        Some(r) => {
            i.running = true;
            i.url = r.url.clone();
            i.note = r.note.clone().unwrap_or_default();
        }
        None => *i = Default::default(),
    }
}

/// 16進のランダム文字列 (リモートUIのトークン用)
fn random_hex(bytes: usize) -> String {
    use rand::TryRng as _;
    let mut buf = vec![0u8; bytes];
    if rand::rngs::SysRng.try_fill_bytes(&mut buf).is_err() {
        return "shikisha-fallback-token".into();
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// 設定ファイルのあるフォルダ (相対パスの基準)
fn config_file_dir() -> std::path::PathBuf {
    config::config_file_path()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
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

pub fn append_hook_log(msg: &str) {
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
#[allow(clippy::too_many_arguments)]
fn exec_commands(
    cmds: Vec<Command>,
    tabs: &mut [Tab],
    max_chain: u32,
    auto_enabled: bool,
    now_ms: u64,
    rows: u16,
    cols: u16,
    notifier: &notify::Notifier,
    flash: &mut Option<String>,
) {
    // タブ名でも指定できるようにする (番号は並べ替えで変わるため)
    let keys: Vec<hooks::TabKey> = tabs.iter().map(|t| t.key()).collect();
    let index_of = |r: &hooks::TabRef| r.resolve(&keys);
    for cmd in cmds {
        match cmd {
            Command::Log(msg) => append_hook_log(&msg),
            Command::Restart { target } => {
                let Some(target) = index_of(&target) else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                    continue;
                };
                if let Some(t) = tabs.get_mut(target.wrapping_sub(1)) {
                    match t.restart(rows, cols) {
                        Ok(()) => {
                            append_hook_log(&format!("restart tab{target} (lua)"));
                            *flash = Some(i18n::tp("msg.restarted", &[("name", &t.title)]));
                        }
                        Err(e) => *flash = Some(i18n::tp("msg.restart_failed", &[("error", &e.to_string())])),
                    }
                }
            }
            Command::Notify { dest, text } => {
                append_hook_log(&format!("NOTIFY[{dest}] {text}"));
                *flash = Some(notifier.send(&dest, &text));
            }
            Command::SendKeys { target, keys } => {
                if !auto_enabled {
                    continue;
                }
                let Some(target) = index_of(&target) else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                    continue;
                };
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
                let Some(target) = index_of(&target) else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                    append_hook_log(&format!("送信先が見つかりません: {target:?}"));
                    continue;
                };
                let depth = tabs
                    .get(origin.wrapping_sub(1))
                    .map(|t| t.chain_depth)
                    .unwrap_or(0)
                    + 1;
                if depth > max_chain {
                    *flash = Some(i18n::tp("msg.chain_limit", &[("max", &max_chain.to_string())]));
                    append_hook_log(&format!("chain limit ({max_chain}): tab{origin} -> tab{target}"));
                    continue;
                }
                let Some(t) = tabs.get_mut(target.wrapping_sub(1)) else {
                    continue;
                };
                if now_ms.saturating_sub(t.last_manual_ms) < MANUAL_GUARD_MS {
                    *flash = Some(i18n::t("msg.manual_guard"));
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
fn handle_copy_key(
    t: &mut Tab,
    key: &KeyEvent,
    size: Size,
    tab_w: u16,
    flash: &mut Option<String>,
) -> Result<()> {
    let (rows_v, cols_v) = pty_dims(size, tab_w);
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
    tab_w: u16,
    rows: u16,
    cols: u16,
    flash: &mut Option<String>,
) -> Result<()> {
    // セッション見出しの錠アイコン (枠の上辺) クリックでロック切替
    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
        if m.row == 0 && m.column >= tab_w {
            if let Some(t) = session_mut(tabs, *active) {
                let (_, lock, offset) = session_title(t);
                let lo = tab_w + 1 + offset;
                if m.column >= lo && m.column < lo + lock.width() as u16 {
                    t.locked = !t.locked;
                    *flash = Some(i18n::t(if t.locked {
                        "msg.lock_on"
                    } else {
                        "msg.lock_off"
                    }));
                    return Ok(());
                }
            }
        }
    }

    // タブバークリックで切替 (ws_offset行目=INDEX、以降=セッション)
    if let MouseEventKind::Down(MouseButton::Left) = m.kind {
        if m.column < tab_w {
            if m.row < ws_offset {
                return Ok(());
            }
            let r = (m.row - ws_offset) as usize;
            if r >= 1 && r <= tabs.len() {
                let t = &mut tabs[r - 1];
                // 終了したタブは ✖ インジケータのクリックで再起動
                if t.state == TabState::Exited && m.column < 3 {
                    *flash = Some(match t.restart(rows, cols) {
                        Ok(()) => i18n::tp("msg.restarted", &[("name", &t.title)]),
                        Err(e) => i18n::tp("msg.restart_failed", &[("error", &e.to_string())]),
                    });
                    return Ok(());
                }
                // 行末の🔒アイコン (右端の枠線1桁を除く2桁) でロック切替
                if m.column >= tab_w.saturating_sub(3) {
                    t.locked = !t.locked;
                    *flash = Some(i18n::t(if t.locked {
                        "msg.lock_on"
                    } else {
                        "msg.lock_off"
                    }));
                    return Ok(());
                }
            }
            if r <= tabs.len() {
                *active = r;
            }
            return Ok(());
        }
    }

    let inner = pane_inner(size, tab_w);
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
            *flash = Some(i18n::t("msg.locked_paste"));
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
    tab_w: u16,
    /// 設定がまだ無い初回起動 (INDEXに案内を出す)
    first_run: bool,
    active: usize,
    prefix_active: bool,
    auto: Option<bool>,
    ws_names: Vec<String>,
    ws_index: usize,
    ws_open: bool,
    help_open: bool,
    /// QRコード表示中なら、その接続URL
    qr: Option<String>,
    /// リモートUIが待ち受け中か (常時わかるように表示する)
    remote_on: bool,
}

fn draw(f: &mut Frame, tabs: &[Tab], ui: &Ui, flash: Option<&str>) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(STATUS_BAR_HEIGHT)])
        .split(f.area());
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(ui.tab_w), Constraint::Min(1)])
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
        lines.push(Line::from(Span::styled(format!("[≡] 0. {}", i18n::t("tui.index")), index_style)));
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
                format!("{}└", " ".repeat(t.depth as usize - 1))
            } else {
                String::new()
            };
            // 右端1桁は枠線。錠アイコン(全角2桁)とインジケータ4桁を除いた幅に収める
            const LOCK_W: u16 = 2;
            let avail = ui.tab_w.saturating_sub(1 + 4 + LOCK_W).max(1);
            let label = truncate_width(&format!("{prefix}{}. {}", i + 1, t.title), avail);
            let pad = avail as usize - label.width();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("[{ind}] "),
                    Style::default().fg(ind_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("{label}{}", " ".repeat(pad)), title_style),
                Span::styled(
                    if t.needs_restart {
                        "⟳ "
                    } else if t.locked {
                        "🔒"
                    } else {
                        "  "
                    },
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
    if let Some(url) = &ui.qr {
        draw_qr(f, f.area(), url);
    }
}

/// スマホから繋ぐためのQRコード。URLを手入力させないための表示
fn draw_qr(f: &mut Frame, area: Rect, url: &str) {
    let lines = netaddr::qr_lines(url);
    let w = (lines.first().map(|l| l.chars().count()).unwrap_or(30) as u16 + 4)
        .min(area.width.saturating_sub(2));
    let h = (lines.len() as u16 + 6).min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(ratatui::widgets::Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(NEON_YELLOW))
        .title(Span::styled(
            format!(" {} ", i18n::t("tui.qr.title")),
            Style::default().fg(Color::Black).bg(NEON_YELLOW),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut text: Vec<Line> = lines
        .into_iter()
        .map(|l| Line::from(Span::styled(l, Style::default().fg(Color::White))))
        .collect();
    text.push(Line::default());
    text.push(Line::from(Span::styled(
        url.to_string(),
        Style::default().fg(NEON_BLUE),
    )));
    text.push(Line::from(Span::styled(
        i18n::t("tui.qr.hint"),
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(Paragraph::new(text), inner);
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
            format!(" {} ", i18n::t("tui.help.title")),
            Style::default().fg(Color::Black).bg(NEON_YELLOW),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let keys = [
        "tui.help.quit", "tui.help.tabs", "tui.help.ws", "tui.help.lock",
        "tui.help.restart", "tui.help.copy", "tui.help.auto", "tui.help.raw",
    ];
    let mouse = [
        "tui.help.mouse.wheel", "tui.help.mouse.drag", "tui.help.mouse.right",
        "tui.help.mouse.tab", "tui.help.mouse.divider",
    ];
    let mut text: Vec<Line> = keys
        .iter()
        .map(|k| Line::from(format!(" {}", i18n::t(k))))
        .collect();
    text.push(Line::default());
    text.push(Line::from(format!(" {}", i18n::t("tui.help.mouse"))));
    text.extend(mouse.iter().map(|k| Line::from(format!(" {}", i18n::t(k)))));
    text.push(Line::default());
    text.push(Line::from(Span::styled(
        format!(" {}", i18n::t("tui.help.close")),
        Style::default().fg(Color::DarkGray),
    )));
    f.render_widget(Paragraph::new(text), inner);
}

fn auto_label(auto: Option<bool>) -> &'static str {
    match auto {
        Some(true) => "AUTO:ON | ",
        Some(false) => "AUTO:OFF | ",
        None => "",
    }
}

/// 遠隔操作を受け付けている間は、忘れないよう常に表示する
fn remote_label(on: bool) -> &'static str {
    if on { "REMOTE:ON | " } else { "" }
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
        Some(n) if ui.ws_names.len() > 1 => format!(" {} :: {n} ", i18n::t("tui.index")),
        _ => " ACTIVE SESSION MAP ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(NEON_BLUE))
        .title(Span::styled(title, Style::default().fg(NEON_YELLOW)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    // 初回起動: 何をすればいいか分からないまま終わらせない
    if ui.first_run {
        lines.push(Line::from(Span::styled(
            format!(" {}", i18n::t("tui.welcome.title")),
            Style::default().fg(NEON_YELLOW).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled(
                " [e] ",
                Style::default().fg(Color::Black).bg(NEON_GREEN).add_modifier(Modifier::BOLD),
            ),
            Span::raw(i18n::t("tui.welcome.line1")),
        ]));
        lines.push(Line::from(i18n::t("tui.welcome.line2")));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            i18n::t("tui.welcome.line3"),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::default());
    }
    lines.push(Line::from(Span::styled(
        format!(
            " {:<3} {:<18} {:<10} {}",
            i18n::t("tui.col.no"),
            i18n::t("tui.col.name"),
            i18n::t("tui.col.state"),
            i18n::t("tui.col.profile")
        ),
        Style::default().fg(NEON_BLUE).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::default());
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
            Span::styled(format!("{:<10}", t.state.display()), Style::default().fg(color)),
            Span::raw(t.profile_name().to_string()),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!(" ── {} ──────────────────────", i18n::t("tui.menu")),
        Style::default().fg(NEON_BLUE),
    )));
    let menu = [
        ("[1-9]", i18n::t("tui.menu.tabs")),
        ("[r]", i18n::t("tui.menu.restart")),
        ("[w]", i18n::t("tui.menu.workspace")),
        ("[t]", i18n::t("tui.menu.notify")),
        ("[i]", i18n::t("tui.menu.phone")),
        ("[e]", i18n::t("tui.menu.settings")),
        ("[k]", i18n::t("tui.menu.password")),
        ("[?]", i18n::t("tui.menu.help")),
        ("[q]", i18n::t("tui.menu.quit")),
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
            " {}{}{}",
            remote_label(ui.remote_on),
            auto_label(ui.auto),
            i18n::t("tui.index.hint")
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
    // 見出しの錠アイコンはクリックでロック切替できる (マウスだけで操作可能)
    let (head, lock, _) = session_title(t);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::styled(head, Style::default().fg(NEON_YELLOW)),
            Span::styled(
                lock,
                Style::default()
                    .fg(Color::Black)
                    .bg(if t.locked { NEON_BLUE } else { NEON_GREEN })
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
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
        let mode = i18n::t(if cs.anchor.is_some() {
            "tui.status.copy.select"
        } else {
            "tui.status.copy.cursor"
        });
        let hist = if alt_screen {
            &i18n::t("tui.status.copy.nohistory")
        } else {
            ""
        };
        i18n::tp(
            "tui.status.copy",
            &[
                ("mode", &mode),
                ("offset", &scrollback_offset.to_string()),
                ("hist", hist),
            ],
        )
    } else if ui.prefix_active {
        i18n::t("tui.status.prefix")
    } else if let Some(msg) = flash {
        format!(" {msg}")
    } else if t.needs_restart {
        format!(" {}", i18n::t("tui.status.needs_restart"))
    } else if t.state == TabState::Exited {
        format!(" {}", i18n::t("tui.status.exited"))
    } else if t.locked {
        format!(" {}", i18n::t("tui.status.locked"))
    } else {
        format!(
            " {}{}{}",
            remote_label(ui.remote_on),
            auto_label(ui.auto),
            i18n::tp(
                "tui.status.normal",
                &[("profile", t.profile_name()), ("state", &t.state.display())]
            )
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
        Ok(()) => i18n::tp("msg.copied", &[("lines", &lines.to_string())]),
        Err(e) => i18n::tp("msg.copy_failed", &[("error", &e.to_string())]),
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
        Err(e) => Ok(Some(i18n::tp("msg.paste_failed", &[("error", &e.to_string())]))),
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

    fn workspace_from(json: &str) -> config::Workspace {
        let cfg: config::Config = serde_json::from_str(json).unwrap();
        cfg.resolve_workspaces().0.into_iter().next().unwrap()
    }

    /// 初回起動でINDEXに案内が出ること (何をすればいいか分からないまま終わらせない)
    #[test]
    fn first_run_shows_how_to_open_settings() {
        use ratatui::backend::TestBackend;
        let argv = vec!["cmd.exe".to_string()];
        let mut tabs = vec![
            Tab::spawn("SHELL".into(), &argv, None, 20, 100, tab::TabOptions::default()).unwrap(),
        ];
        let ui = Ui {
            tab_w: 18,
            first_run: true,
            active: 0,
            prefix_active: false,
            auto: None,
            ws_names: vec![],
            ws_index: 0,
            ws_open: false,
            help_open: false,
            qr: None,
            remote_on: false,
        };
        let mut term = ratatui::Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &tabs, &ui, None)).unwrap();
        // 全角文字は2セルを占め、2セル目が空になるため空白を落として比較する
        let screen: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        tabs[0].kill();
        // テストでは初期化していないので基準の英語が出る
        assert!(screen.contains("Welcome"), "初回の案内が出る: {screen}");
        assert!(screen.contains("settingsscreen"), "設定の開き方が示される");
    }

    #[test]
    fn phone_view_drops_trailing_blank_lines() {
        // 端末の空行をそのまま送ると、スマホでは本文が見えなくなる
        let screen = "hello\nworld\n\n\n\n\n";
        assert_eq!(trim_for_phone(screen, 200), "hello\nworld");
        // 長すぎる場合は末尾だけ送る
        let long: String = (1..=300).map(|i| format!("line{i}\n")).collect();
        let out = trim_for_phone(&long, 10);
        assert_eq!(out.lines().count(), 10);
        assert!(out.ends_with("line300"));
        assert_eq!(trim_for_phone("   \n\n", 200), "");
    }

    #[test]
    fn tab_starts_in_the_configured_folder() {
        let dir = std::env::temp_dir().join("shikisha-cwd-test");
        std::fs::create_dir_all(&dir).unwrap();
        let opts = tab::TabOptions {
            cwd: Some(dir.clone()),
            ..Default::default()
        };
        let argv = vec!["cmd.exe".to_string(), "/c".into(), "cd".into()];
        let mut t = Tab::spawn("cwd".into(), &argv, None, 10, 60, opts).unwrap();
        // cmd の "cd" は現在のフォルダを表示する
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let screen = t.parser.lock().unwrap().screen().contents();
        t.kill();
        assert!(
            screen.contains("shikisha-cwd-test"),
            "指定した作業フォルダで起動する: {screen}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_folder_falls_back_instead_of_failing_to_start() {
        let opts = tab::TabOptions {
            cwd: Some(std::path::PathBuf::from("Z:/does/not/exist")),
            ..Default::default()
        };
        let argv = vec!["cmd.exe".to_string()];
        // 存在しないフォルダでもセッションは起動する (起動失敗より復帰しやすい)
        let mut t = Tab::spawn("fallback".into(), &argv, None, 10, 60, opts)
            .expect("存在しないフォルダでも起動できる");
        t.kill();
    }

    #[test]
    fn hot_reload_applies_changes_without_restarting_untouched_tabs() {
        let ws0 = workspace_from(
            r#"{"workspaces":[{"name":"T","tabs":[
                {"name":"one","command":"cmd.exe"},
                {"name":"two","command":"cmd.exe"}
            ]}]}"#,
        );
        let mut tabs = Vec::new();
        let mut errs = Vec::new();
        spawn_workspace(&ws0, 24, 80, &mut tabs, &mut errs);
        assert_eq!(tabs.len(), 2, "{errs:?}");
        let one_before = tabs[0].signature();

        // one: ロックを付ける(即時反映) / two: 削除 / three: 追加
        let ws1 = workspace_from(
            r#"{"workspaces":[{"name":"T","tabs":[
                {"name":"one","command":"cmd.exe","locked":true},
                {"name":"three","command":"cmd.exe"}
            ]}]}"#,
        );
        let msg = apply_ws_config(&mut tabs, &ws1, 24, 80, &mut errs);

        assert_eq!(
            tabs.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
            vec!["one", "three"],
            "設定の順序どおりに並ぶ"
        );
        assert!(tabs[0].locked, "ロックは再起動なしで反映される");
        assert!(!tabs[0].needs_restart, "起動条件が同じなら再起動不要");
        assert_eq!(tabs[0].signature(), one_before, "既存セッションは維持される");
        assert!(msg.contains("added 1") && msg.contains("stopped 1"), "{msg}");

        // 文字コードの変更は作り直しが必要なので、保留して印を付ける
        let ws2 = workspace_from(
            r#"{"workspaces":[{"name":"T","tabs":[
                {"name":"one","command":"cmd.exe","encoding":"shift_jis"},
                {"name":"three","command":"cmd.exe"}
            ]}]}"#,
        );
        let msg2 = apply_ws_config(&mut tabs, &ws2, 24, 80, &mut errs);
        assert!(tabs[0].needs_restart, "要再起動の印が付く");
        assert!(msg2.contains("1 need a restart"), "{msg2}");

        for t in tabs.iter_mut() {
            t.kill();
        }
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


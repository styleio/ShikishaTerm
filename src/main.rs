//! SHIKISHA-TERM: ポータブル・マルチセッションAIオーケストレーションTUI
//!
//! Phase 3: マルチタブ + INDEXダッシュボード + config.json
//!
//! 起動:
//!   SHIKISHA-TERM.exe                 # config.jsonのタブ構成 (無ければPowerShell 1タブ)
//!   SHIKISHA-TERM.exe claude          # デバッグ用: 引数のコマンドを1タブで起動
//!
//! 操作 (プレフィックスキー Ctrl+B):
//!   Ctrl+B q      終了 / Ctrl+B 0-9 タブ切替 (0=INDEX) / Ctrl+B n/p 隣のタブ
//!   Ctrl+B [      コピーモード / Ctrl+B b 素のCtrl+Bを送信
//! マウス: ホイール=スクロール(コピーモード) / 左ドラッグ=選択即コピー / 右クリック=ペースト

mod ball;
mod browser;
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
mod shell;
mod tab;
mod uistate;
mod watch;
mod winmode;
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

// 黒地に彩度100%の純色を並べると、道具ではなく「侵入されている画面」に見える。
// 彩度を落として少し温度を持たせると、同じ配置のまま印象が変わる。
//
// 青だけは動かさない。ロゴの #00AAFF と同じ値で、枠も見出しもワードマークも
// ロゴと地続きになる
/// 動いているもの (実行中のタブ、選択中の行)
const NEON_GREEN: Color = Color::Rgb(74, 222, 128);
/// 目を向けてほしいもの (見出し、注意書き)
const NEON_YELLOW: Color = Color::Rgb(255, 200, 87);
/// 枠と見出し。ロゴと同じ青
const NEON_BLUE: Color = Color::Rgb(0, 170, 255);
/// 止まっている・失敗している
const NEON_RED: Color = Color::Rgb(255, 107, 107);

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
    // 窓モードの試作。自前の窓にターミナルを描く。
    // 確かめたいのは日本語入力と描画速度で、そこが通らなければこの道は選べない
    if std::env::args().nth(1).as_deref() == Some("--window") {
        let rest: Vec<String> = std::env::args().skip(2).collect();
        return winmode::run(&rest);
    }
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
    // どのビルドを動かしているかを常に見えるようにする。
    // 「直したのに直らない」の原因が古い実行ファイルだったことが何度かあった
    let _ = execute!(
        std::io::stdout(),
        ratatui::crossterm::terminal::SetTitle(format!(
            "SHIKISHA-TERM  build {}  ({})",
            env!("BUILD_TIME"),
            env!("BUILD_REV")
        ))
    );
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
/// 表示幅で右詰めする。`format!("{:<n}")` は文字数で数えるため、
/// 日本語のように1文字が2桁を占める名前が入るとカラムがずれる
fn pad_width(s: &str, w: u16) -> String {
    let t = truncate_width(s, w);
    let pad = (w as usize).saturating_sub(t.width());
    format!("{t}{}", " ".repeat(pad))
}

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

/// 描画先。ターミナルにも、自前の窓にも描ける。
///
/// ループの側は「どちらに描いているか」を知らない。
/// 知らせると、片方だけの分岐がループ中に増えていく
enum Surface<'a> {
    /// 今までどおり、動かしている端末へ描く
    Term(&'a mut ratatui::DefaultTerminal),
}

impl Surface<'_> {
    fn size(&self) -> Result<ratatui::layout::Size> {
        match self {
            Surface::Term(t) => Ok(t.size()?),
        }
    }

    /// 端末そのものを要る処理のための取り出し口 (パスワード入力など)。
    ///
    /// 窓では別の出し方が要るので、そのとき None になる。
    /// そうしておけば「ここは別の道が必要」がコンパイラから見える
    fn term_mut(&mut self) -> Option<&mut ratatui::DefaultTerminal> {
        match self {
            Surface::Term(t) => Some(t),
        }
    }

    fn draw(
        &mut self,
        tabs: &[Tab],
        ui: &Ui,
        flash: Option<&str>,
        hits: &mut Vec<HitBox>,
    ) -> Result<()> {
        match self {
            Surface::Term(t) => {
                t.draw(|f| draw(f, tabs, ui, flash, hits))?;
                Ok(())
            }
        }
    }
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let cmd_args: Vec<String> = std::env::args().skip(1).collect();
    let start = Instant::now();
    // 幅はconfig指定 → 無ければタブ名から自動算出 (タブ起動後に確定)
    let mut tab_w = 18u16;
    let mut surface = Surface::Term(terminal);
    let (mut rows, mut cols) = pty_dims(surface.size()?, tab_w);

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
    let remembered = config::load_last_workspace();
    let mut ws_index = starting_workspace(
        cfg.as_ref().and_then(|c| c.restore_workspace).unwrap_or(true),
        remembered.as_deref(),
        &workspaces.iter().map(|w| w.name.clone()).collect::<Vec<_>>(),
    );
    if let Some(w) = workspaces.get(ws_index) {
        // どこから始まったかは、あとで「なぜこの画面なのか」を追う手がかりになる
        append_hook_log(&format!(
            "起動: ワークスペース「{}」({})",
            w.name,
            match remembered.as_deref() {
                Some(r) if r == w.name => "前回の続き",
                _ => "先頭",
            }
        ));
    }
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
    (rows, cols) = pty_dims(surface.size()?, tab_w);
    for t in &tabs {
        let _ = t.resize(rows, cols);
    }

    // Luaフックエンジンはワークスペース単位 (共有変数もその中で共有される)。
    // 未使用のワークスペースは作らず、切替時に必要なら生成する
    let mut max_chain = cfg.as_ref().and_then(|c| c.max_chain).unwrap_or(10);
    let mut done_confirm_ms = cfg
        .as_ref()
        .and_then(|c| c.done_confirm_ms)
        .unwrap_or(profile::DEFAULT_DONE_CONFIRM_MS);
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
                let Some(term) = surface.term_mut() else {
                    break;
                };
                match prompt_password(term, &i18n::t("prompt.password.title"), &note)? {
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
            c.browser_overlay.unwrap_or(true),
        ),
        None => caps::Capabilities::disabled(),
    });
    let mut engines: Vec<Option<HookEngine>> = (0..workspaces.len().max(1)).map(|_| None).collect();
    engines[0] = build_engine(cfg.as_ref(), workspaces.first(), &mut startup_errors, &caps);
    if let Some(w) = workspaces.first() {
        open_declared_browsers(w, &caps, &mut startup_errors);
    }
    let mut engine = engines[0].take();

    // リモートUI (スマホ等から監視・指示する)。設定で有効にしたときだけ待ち受ける。
    // 状況は設定画面にも渡し、QRコードをブラウザで見られるようにする
    let remote_info: Arc<Mutex<webui::RemoteInfo>> = Arc::new(Mutex::new(Default::default()));
    let mut remote_ui = start_remote(cfg.as_ref(), password.as_deref(), &mut startup_errors);
    publish_remote(&remote_info, &remote_ui);

    // マウスを有効にした直後のコンソール入力モード。
    // 子プロセスに崩されたらこれに戻す (ensure_mouse_capture)
    let console_mode = console_input_mode();

    let mut auto_enabled = true;
    let mut started_fired = vec![false; tabs.len()];
    // 自動チェーンの「透明のボール」。今どのタブが仕事を持っているかを表示に使う
    let mut ball = ball::Ball::default();
    // 送信した本文に対して、あとから実行(改行)を送る予約
    let mut pending_submit: Vec<PendingSubmit> = Vec::new();
    // 応答完了と見えたタブと、それを確定させる時刻。
    // 途中の息継ぎで撃たないよう、静かなまま保っていることを確かめてから撃つ
    let mut pending_done: Vec<(usize, u64)> = Vec::new();
    // ボールを追って画面を切り替えるか
    let mut follow_ball = cfg.as_ref().and_then(|c| c.follow_ball).unwrap_or(true);
    // 人が最後に画面を触った時刻。直後は追従しない
    let mut view_touched_ms: u64 = 0;
    // 追従で切り替えた先。同じ場所へ何度も飛ばさないために覚えておく
    let mut followed: usize = 0;
    // INDEXで押せる場所。毎フレーム描画時に作り直す
    let mut hits: Vec<HitBox> = Vec::new();
    let mut hover: Option<Hit> = None;

    // 0 = INDEX、1.. = セッション。初回はINDEX(案内のある画面)から始める
    let mut active: usize = if tabs.is_empty() || first_run { 0 } else { 1 };
    let mut prefix_active = false;
    // 重ねたブラウザを今見せているか。出しっぱなしだと
    // ターミナルがずっと隠れてしまうので、既定は隠す
    let mut browser_shown = false;
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
                follow_ball = newcfg.follow_ball.unwrap_or(true);
                done_confirm_ms = newcfg
                    .done_confirm_ms
                    .unwrap_or(profile::DEFAULT_DONE_CONFIRM_MS);
                if let Some(w) = newcfg.tab_bar_width {
                    let w = w.clamp(TAB_BAR_MIN, TAB_BAR_MAX);
                    if w != tab_w {
                        tab_w = w;
                        (rows, cols) = pty_dims(surface.size()?, tab_w);
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
                    newcfg.browser_overlay.unwrap_or(true),
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
                let now_ms = start.elapsed().as_millis() as u64;
                if auto_enabled {
                    for (i, fired) in started_fired.iter_mut().enumerate() {
                        // 起動直後に送ると、AI CLIが入力欄を描く前なので捨てられる。
                        // 準備できるまで待ってから流し込む
                        if !*fired && tabs[i].ready_for_startup_hook() {
                            *fired = true;
                            eng.fire("on_start", &tab_ctx(&tabs[i], i + 1), None);
                        }
                    }
                    for &(idx, old, new) in &transitions {
                        if old == new {
                            continue;
                        }
                        let t = &tabs[idx - 1];
                        append_hook_log(&format!(
                            "状態 tab{idx} {}->{} [{}] prompted={} working={} 応答あり={} 実行待ち={}",
                            old.label(),
                            new.label(),
                            t.profile_name(),
                            t.was_prompted(),
                            t.saw_working_flag(),
                            t.answered_since_submit(),
                            pending_submit.iter().any(|p| p.tab == idx)
                        ));
                    }

                    // 続きが始まったら、完了の確定待ちは取り消す
                    for &(idx, _, new) in &transitions {
                        if new == TabState::Busy || new == TabState::Exited {
                            pending_done.retain(|&(t, _)| t != idx);
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
                        // 起動時のバナー出力だけでも画面は動いて止まるので、
                        // どのタブも必ず一度 DONE を通る。誰も何も聞いていない
                        // その出力を応答として転送しないよう、入力があった後だけ扱う
                        // 実行(改行)がまだ届いていないタブは、貼り付けが置かれた
                        // だけの状態。静かになっても、それは応答ではない
                        let submitting = pending_submit.iter().any(|p| p.tab == idx);
                        // 実行のあとに何も出ていないなら、届かなかったということ。
                        // 貼り付けが見えているだけの画面を応答と読まない
                        let answering = tabs[idx - 1].was_prompted()
                            && !submitting
                            && tabs[idx - 1].answered_since_submit();
                        match new {
                            TabState::Busy if answering => eng.fire("on_busy", &ctx, None),
                            TabState::Done if old == TabState::Busy && !answering => {
                                append_hook_log(&format!(
                                    "done無視 tab{idx} [{}] prompted={} submitting={} answered={}",
                                    tabs[idx - 1].profile_name(),
                                    tabs[idx - 1].was_prompted(),
                                    submitting,
                                    tabs[idx - 1].answered_since_submit()
                                ));
                            }
                            TabState::Done if answering && old == TabState::Busy => {
                                append_hook_log(&format!(
                                    "done確認待ち tab{idx} [{}]",
                                    tabs[idx - 1].profile_name()
                                ));
                                // ここでは撃たない。AIの出力は途中で息継ぎをするので、
                                // 静かになっただけでは終わったと言えない
                                // AI固有の指定があればそちら、無ければ基本設定
                                let wait = tabs[idx - 1].done_confirm_ms().unwrap_or(done_confirm_ms);
                                let at = now_ms + wait;
                                pending_done.retain(|&(t, _)| t != idx);
                                pending_done.push((idx, at));
                            }
                            TabState::Question => {
                                let screen =
                                    tabs[idx - 1].parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents();
                                eng.fire("on_question", &ctx, Some(&screen));
                            }
                            TabState::Exited => eng.fire("on_exit", &ctx, None),
                            _ => {}
                        }
                    }
                    // 静かなまま保ったものだけを、本当の完了として撃つ
                    let (ready, waiting): (Vec<_>, Vec<_>) =
                        pending_done.iter().partition(|&&(_, at)| now_ms >= at);
                    pending_done = waiting;
                    for (idx, _) in ready {
                        if let Some(t) = tabs.get_mut(idx.wrapping_sub(1)) {
                            if t.state != TabState::Done {
                                continue;
                            }
                            // 一度の実行に一度の応答。次を待つには次の実行が要る
                            t.finish_response();
                        }
                        let ctx = tab_ctx(&tabs[idx - 1], idx);
                        // 幅を狭めると vt100 が各行をその幅で切り捨てるので、
                        // 応答を待つ間に狭めていると文章が欠けている。
                        // 戻せないが、黙って欠けたものを渡すよりは残す
                        if tabs[idx - 1].resized_while_waiting() {
                            append_hook_log(&format!(
                                "警告 tab{idx}: 応答中に画面幅が狭まりました。\
                                 端末が行を切り詰めるため、応答が欠けている恐れがあります"
                            ));
                        }
                        append_hook_log(&format!(
                            "on_done発火 tab{idx}: 応答 {}文字: {}",
                            ctx.output.chars().count(),
                            log_excerpt(&ctx.output, 100)
                        ));
                        eng.fire("on_done", &ctx, None);
                    }

                    eng.tick_pending(&|idx| {
                        tabs.get(idx.wrapping_sub(1))
                            .map(|t| t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents())
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
                        &mut ball,
                        &mut pending_submit,
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
                    cols,
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
                            // 見た目を運ぶので contents() ではなく行単位で取る
                            screen: trim_for_phone(
                                &tab::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen()),
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
                                t.last_manual_ms = Some(now_ms);
                                let seen = t.output_count();
                                write_prompt(t, &text);
                                pending_submit.push(PendingSubmit::new(tab, seen, now_ms));
                                append_hook_log(&format!(
                                    "remote送信 tab{tab}: {}",
                                    log_excerpt(&text, 120)
                                ));
                            }
                        }
                        remote::RemoteCmd::Keys { tab, keys } => {
                            if let Some(t) = tabs.get_mut(tab.wrapping_sub(1)) {
                                if t.locked {
                                    continue;
                                }
                                t.chain_depth = 0;
                                t.last_manual_ms = Some(now_ms);
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

        // 予約しておいた実行(改行)を、相手が貼り付けを描いてから送る
        if !pending_submit.is_empty() {
            let now_ms = start.elapsed().as_millis() as u64;
            pending_submit.retain_mut(|p| {
                let Some(t) = tabs.get(p.tab.wrapping_sub(1)) else {
                    return false;
                };
                if !p.ready(t.output_count(), now_ms) {
                    return true;
                }
                let settled = now_ms < p.give_up;
                let _ = t.write_bytes(b"\r");
                append_hook_log(&format!(
                    "submit tab{} ({})",
                    p.tab,
                    if settled { "取り込み完了後" } else { "落ち着かないまま送信" }
                ));
                false
            });
        }

        // 子プロセスがコンソールを崩していたら戻す。判定は GetConsoleMode 1回だけ
        ensure_mouse_capture(console_mode);

        // ボールが渡った先へ画面を移す。
        // 人が画面を触った直後は従わない (読んでいる最中に飛ばされないように)
        {
            let now_ms = start.elapsed().as_millis() as u64;
            if let Some(to) = follow_target(
                follow_ball,
                ball.holder,
                followed,
                tabs.len(),
                now_ms,
                view_touched_ms,
            ) {
                followed = to;
                if active != to {
                    // 「渡ったのに画面が動かない」を追えるようにしておく。
                    // 一度これを整理で消して、まさにその調査で困った
                    append_hook_log(&format!("追従 tab{active} -> tab{to}"));
                    active = to;
                }
            }
        }

        // 人間が入力すると chain_depth が0に戻る。ボールもそれに追従させる
        // (リセット箇所を増やさずに済むよう、持ち主の側から確認する)
        // 人待ちのボールはここで消さない。連鎖は終わっていても
        // 仕事は holder にある。人が触ったら touched 側で消える
        if ball.holder > 0
            && !ball.awaiting_human
            && !tabs
                .get(ball.holder - 1)
                .map(|t| t.chain_depth > 0)
                .unwrap_or(false)
        {
            ball.reset();
        }
        ball.clamp_to(tabs.len());

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
            ball,
            max_chain,
            now_ms: start.elapsed().as_millis() as u64,
            hover,
        };
        surface.draw(&tabs, &ui, flash.as_deref(), &mut hits)?;
        // 重ねているブラウザを、ターミナルの動きに付いていかせる。
        // 所有関係で最小化と重なり順はOSが見てくれるが、位置だけは追う必要がある
        if browser_shown {
            caps.browsers_fit(true);
        }

        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        let ev = event::read()?;

        // マウスが乗っている行を覚えて、押せる場所が見て分かるようにする
        if let Event::Mouse(m) = &ev {
            hover = hit_at(&hits, m.row, m.column);
        } else if matches!(ev, Event::Key(_)) {
            hover = None;
        }

        // INDEXのメニューはクリックでもキーと同じ動作にする。
        // 分岐を増やすとキー操作と挙動がずれていくので、キー入力に翻訳して合流させる
        let ev = match &ev {
            Event::Mouse(m) if matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) => {
                match hit_at(&hits, m.row, m.column) {
                    // メニューはキー入力に翻訳して、キー操作と処理を共有する
                    Some(Hit::Key(c)) => {
                        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
                    }
                    Some(Hit::Tab(n)) => {
                        if n <= tabs.len() {
                            active = n;
                            view_touched_ms = start.elapsed().as_millis() as u64;
                        }
                        continue;
                    }
                    Some(Hit::Index) => {
                        active = 0;
                        view_touched_ms = start.elapsed().as_millis() as u64;
                        continue;
                    }
                    Some(Hit::Lock(n)) => {
                        if let Some(t) = tabs.get_mut(n - 1) {
                            t.locked = !t.locked;
                            flash = Some(i18n::t(if t.locked { "msg.lock_on" } else { "msg.lock_off" }));
                        }
                        continue;
                    }
                    Some(Hit::Restart(n)) => {
                        if let Some(t) = tabs.get_mut(n - 1) {
                            flash = Some(match t.restart(rows, cols) {
                                Ok(()) => i18n::tp("msg.restarted", &[("name", &t.title)]),
                                Err(e) => i18n::tp("msg.restart_failed", &[("error", &e.to_string())]),
                            });
                        }
                        continue;
                    }
                    Some(Hit::Workspace) => {
                        ws_open = !ws_open;
                        continue;
                    }
                    Some(Hit::WorkspaceItem(n)) => {
                        switch_workspace(
                            n, &mut ws_index, &mut tabs, &mut ws_tabs, &workspaces,
                            &mut active, rows, cols, &mut startup_errors,
                            &mut started_fired, cfg.as_ref(), &mut engine, &mut engines, &caps,
                        );
                        ws_open = false;
                        continue;
                    }
                    Some(Hit::EmergencyStop) => {
                        auto_enabled = !auto_enabled;
                        if !auto_enabled {
                            // 送信予約が残っていると、止めた後に改行だけ届いてしまう
                            pending_submit.clear();
                            // 待機中のループも破棄する (Ctrl+B x と同じ)
                            if let Some(e) = engine.as_mut() {
                                e.cancel_all();
                            }
                        }
                        flash = Some(i18n::t(if auto_enabled {
                            "msg.auto_on"
                        } else {
                            "msg.emergency_stop"
                        }));
                        continue;
                    }
                    Some(Hit::Divider) | None => ev,
                }
            }
            _ => ev,
        };

        match ev {
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
                                view_touched_ms = start.elapsed().as_millis() as u64;
                            }
                        }
                        KeyCode::Char('n') => {
                            active = if active >= tabs.len() { 0 } else { active + 1 };
                            view_touched_ms = start.elapsed().as_millis() as u64;
                        }
                        KeyCode::Char('p') => {
                            active = if active == 0 { tabs.len() } else { active - 1 };
                            view_touched_ms = start.elapsed().as_millis() as u64;
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
                        // Ctrl+B o 重ねたブラウザの出し入れ。
                        // w も b も既に埋まっている (数えて選んだ)
                        KeyCode::Char('o') if caps.has_browser() => {
                            browser_shown = !browser_shown;
                            caps.browsers_fit(browser_shown);
                            flash = Some(i18n::t(if browser_shown {
                                "msg.browser_shown"
                            } else {
                                "msg.browser_hidden"
                            }));
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
                            // 送信予約が残っていると、止めた後に改行だけ届いてしまう
                            pending_submit.clear();
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
                            let rows = pty_dims(surface.size()?, tab_w).0;
                            if let Some(t) = session_mut(&mut tabs, active) {
                                t.copy = Some(CopyState {
                                    cursor_row: rows.saturating_sub(1),
                                    anchor: None,
                                    dragged: false,
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
                                notifier.send_all("SHIKISHA-TERM: テスト通知")
                            });
                        }
                        // マスターパスワードの設定・変更・解除 (TUI内で完結)
                        KeyCode::Char('k') => {
                            let Some(term) = surface.term_mut() else {
                                continue;
                            };
                            flash = Some(manage_master_password(
                                term,
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
                    let size = surface.size()?;
                    let now_ms = start.elapsed().as_millis() as u64;
                    let mut locked_hit = false;
                    if let Some(t) = session_mut(&mut tabs, active) {
                        if t.copy.is_some() {
                            handle_copy_key(t, &key, size, tab_w, &mut flash)?;
                        } else if t.locked {
                            // ソフトロック: 閲覧・コピーはできるが入力は無視
                            locked_hit = true;
                        } else if let Some(bytes) = key_to_bytes(&key) {
                            // 手動入力は連鎖を切る。ただし下書きを受け取った
                            // タブへの入力だけは切らない。あれは乗っ取りではなく
                            // 参加で、書き足して流すところまでが一連の流れ
                            if ball.awaiting_human && ball.holder == active {
                                ball.awaiting_human = false;
                            } else {
                                t.chain_depth = 0;
                            }
                            t.last_manual_ms = Some(now_ms);
                            view_touched_ms = now_ms;
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
                        t.last_manual_ms = Some(now_ms);
                        t.write_bytes(text.as_bytes())?;
                    }
                }
            }
            Event::Mouse(m) => {
                let size = surface.size()?;
                let now_ms = start.elapsed().as_millis() as u64;
                if help_open {
                    if matches!(m.kind, MouseEventKind::Down(_)) {
                        help_open = false;
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
                            (rows, cols) = pty_dims(surface.size()?, tab_w);
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
                handle_mouse(&mut tabs, &mut active, m, size, now_ms, tab_w, &mut flash)?;
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
/// 子プロセスに自分のコンソールを渡さない。
///
/// コンソールを継承した子 (特に cmd.exe) は入力モードから ENABLE_MOUSE_INPUT を
/// 落とす。実測で 0x1f7 -> 0x1e7。こうなるとマウスが一切効かなくなり、
/// 「設定画面を開いて戻ったらタブが押せない」という形で表面化する
pub fn detach_console(cmd: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

/// 本文を送ってから実行(改行)を送るまでの、最低限の間。
/// 相手が本当に処理し終える時間は機種にも負荷にも依るので、これは下限でしかない
const SUBMIT_FLOOR_MS: u64 = 100;
/// 貼り付けの取り込みが終わったとみなす無出力時間。
///
/// 「反応が始まった」ではなく「終わった」を待つ。長い貼り付けは描画が
/// 何往復も続くので、始まった時点で改行を送ると取り込み中に届いて捨てられる
/// (実測: 約600文字なら通り、約1900文字で落ちる)
const SUBMIT_QUIET_MS: u64 = 400;
/// 相手が返し続けて落ち着かないときに、それでも実行を送るまでの上限
const SUBMIT_GIVE_UP_MS: u64 = 8_000;

/// 本文を送ったあと、実行(改行)を送る予約。
///
/// まとめて1回で書くと、AI CLIが貼り付けを取り込む前に改行が届いて捨てられる。
/// かといって固定の待ち時間にすると、それは「相手が何秒で処理するか」の当て推量で、
/// 機種・負荷・本文の長さのどれかが変われば破綻する。
/// 相手が貼り付けを描いた (= 出力を返した) ことを合図にする
struct PendingSubmit {
    tab: usize,
    /// 最後に見た累計出力量。増えている間は、まだ取り込み中
    seen: u64,
    /// 出力が止まってからの起点 (None = まだ止まっていない)
    quiet_since: Option<u64>,
    /// 早すぎる送信を防ぐ下限時刻
    not_before: u64,
    /// 落ち着かないときに諦めて送る時刻
    give_up: u64,
}

impl PendingSubmit {
    fn new(tab: usize, seen: u64, now_ms: u64) -> Self {
        Self {
            tab,
            seen,
            quiet_since: None,
            not_before: now_ms + SUBMIT_FLOOR_MS,
            give_up: now_ms + SUBMIT_GIVE_UP_MS,
        }
    }

    /// 今このタブへ実行(改行)を送ってよいか。
    ///
    /// 待つのは「反応が始まった」ではなく「取り込みが終わった」。
    /// 長い貼り付けは描画が何往復も続くので、始まった時点で送ると
    /// 取り込み中に届いて捨てられる (実測: 約600文字は通り、約1900文字で落ちた)
    fn ready(&mut self, output_count: u64, now_ms: u64) -> bool {
        if output_count != self.seen {
            // まだ取り込み中。止まってからの計測はやり直す
            self.seen = output_count;
            self.quiet_since = None;
        } else if self.quiet_since.is_none() {
            self.quiet_since = Some(now_ms);
        }
        if now_ms < self.not_before {
            return false;
        }
        let settled = self
            .quiet_since
            .is_some_and(|q| now_ms.saturating_sub(q) >= SUBMIT_QUIET_MS);
        settled || now_ms >= self.give_up
    }
}

/// プロンプトへ本文だけを送る。実行は呼び出し側が少し遅らせて送る
fn write_prompt(t: &Tab, text: &str) {
    let bracketed = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste();
    let body = text.replace("\r\n", "\r").replace('\n', "\r");
    let mut bytes = Vec::new();
    if bracketed {
        // 括弧付き貼り付けに対応していれば、複数行でも1回の入力として渡る
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(body.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
    } else {
        bytes.extend_from_slice(body.as_bytes());
    }
    let _ = t.write_bytes(&bytes);
}

fn open_browser(url: &str) {
    // cmd の start はURL内の & を分割してしまうため、空タイトル引数の後に渡す
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/c", "start", "", url])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = detach_console(&mut cmd).spawn();
}

/// コンソール入力モード。子プロセスに崩されたかの判定に使う
fn console_input_mode() -> Option<u32> {
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE};
    unsafe {
        let mut mode = 0u32;
        (GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode) != 0).then_some(mode)
    }
}

/// 崩されていたら crossterm に設定し直させる。
/// 予防 (detach_console) をすり抜ける経路が残っていても、ここで復帰できる
fn ensure_mouse_capture(expected: Option<u32>) {
    let (Some(want), Some(now)) = (expected, console_input_mode()) else {
        return;
    };
    if want != now {
        let _ = execute!(std::io::stdout(), EnableMouseCapture);
    }
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
        // ブラウザは子プロセスではないので、ここでは立てない
        // (open_declared_browsers が窓を開く)
        if config::browser_url_of(&argv).is_some() {
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
/// 設定で宣言されたブラウザを開く。
///
/// 1台開けなくても他は動かす。ブラウザが立たないことは、
/// ワークスペース全体を止める理由にならない
fn open_declared_browsers(ws: &config::Workspace, caps: &hooks::Caps, errors: &mut Vec<String>) {
    for b in &ws.browsers {
        if let Err(e) = caps.browser_open(&b.id, &b.url) {
            errors.push(format!("ブラウザ {}: {e:#}", b.id));
        }
    }
    // タブとして「browser https://...」と書かれたものも同じ扱い。
    // 自動化から指す名前は、そのタブのID (無ければ表示名)
    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        let Some(url) = config::browser_url_of(&argv) else {
            continue;
        };
        let name = ft
            .cfg
            .id
            .clone()
            .or_else(|| ft.cfg.name.clone())
            .unwrap_or_else(|| "browser".into());
        if let Err(e) = caps.browser_open(&name, &url) {
            errors.push(format!("ブラウザ {name}: {e:#}"));
        }
    }
}

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
    config::save_last_workspace(&workspaces[to].name);
    *tabs = std::mem::take(&mut ws_tabs[to]);
    if tabs.is_empty() {
        spawn_workspace(&workspaces[to], rows, cols, tabs, errors);
        open_declared_browsers(&workspaces[to], caps, errors);
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

/// ボールを追って移るべき画面。移らないなら None。
///
/// 人が画面を触った直後は従わない。読んでいる最中に飛ばされるのが
/// いちばん困るので、手を出したらしばらく黙る
/// どのワークスペースから始めるか。
///
/// 覚えるのは番号ではなく名前。番号は並べ替えや追加でずれるので、
/// 「昨日の続き」のつもりが別のものになる。
/// 見つからなければ先頭に落とす (消した・改名した場合)
fn starting_workspace(enabled: bool, last: Option<&str>, names: &[String]) -> usize {
    if !enabled {
        return 0;
    }
    last.and_then(|want| names.iter().position(|n| n == want))
        .unwrap_or(0)
}

fn follow_target(
    enabled: bool,
    holder: usize,
    already: usize,
    tab_count: usize,
    now_ms: u64,
    view_touched_ms: u64,
) -> Option<usize> {
    if !enabled || holder == 0 || holder == already || holder > tab_count {
        return None;
    }
    (now_ms.saturating_sub(view_touched_ms) >= FOLLOW_GUARD_MS).then_some(holder)
}

/// 人が画面を触ってから、自動追従を再開するまでの間。
///
/// 読んでいる最中に勝手に飛ばされるのが一番困るので、
/// 手を出したらしばらく黙って従う
const FOLLOW_GUARD_MS: u64 = 8_000;

/// 人間が触った直後か。一度も触られていなければ false。
///
/// ここを「時刻0 = 触られた」と扱うと、アプリ起動からガード時間のあいだ
/// 自動送信が丸ごと捨てられる (起動時の自動化が動かない原因になっていた)
fn touched_recently(t: &Tab, now_ms: u64) -> bool {
    t.last_manual_ms
        .is_some_and(|m| now_ms.saturating_sub(m) < MANUAL_GUARD_MS)
}

/// ログ用に1行へ潰した抜粋。全文だと読めないので頭だけ残す
fn log_excerpt(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = one.chars().take(max).collect();
    if one.chars().count() > max {
        out.push('…');
    }
    out
}

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
    ball: &mut ball::Ball,
    pending_submit: &mut Vec<PendingSubmit>,
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
                    if touched_recently(t, now_ms) {
                        continue;
                    }
                    let _ = t.write_bytes(keys.as_bytes());
                }
            }
            Command::DraftPrompt {
                target,
                text,
                origin,
            } => {
                if !auto_enabled {
                    continue;
                }
                let Some(idx) = index_of(&target) else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                    continue;
                };
                let depth = tabs
                    .get(origin.wrapping_sub(1))
                    .map(|t| t.chain_depth)
                    .unwrap_or(0)
                    + 1;
                if depth > max_chain {
                    *flash = Some(i18n::t("msg.chain_limit"));
                    append_hook_log(&format!(
                        "chain limit ({max_chain}): 下書き tab{origin} -> tab{idx}"
                    ));
                    continue;
                }
                if let Some(t) = tabs.get_mut(idx.wrapping_sub(1)) {
                    if touched_recently(t, now_ms) {
                        continue;
                    }
                    // 目印を理解しない相手 (素のシェル) に同じものを送ると、
                    // 目印は無視され、中の改行がそのまま実行になる。
                    // 黙って改行を落とすより、断って理由を残す方がいい
                    if !t.accepts_bracketed_paste() {
                        let msg = i18n::tp("msg.draft_unsupported", &[("tab", &t.title)]);
                        append_hook_log(&msg);
                        *flash = Some(msg);
                        continue;
                    }
                    // 実行(改行)は送らない。人が書き足して自分で送る
                    let mut bytes = Vec::with_capacity(text.len() + 12);
                    bytes.extend_from_slice(b"\x1b[200~");
                    bytes.extend_from_slice(text.as_bytes());
                    bytes.extend_from_slice(b"\x1b[201~");
                    let _ = t.write_bytes(&bytes);
                    // 人も輪の一部。書き足して流せば連鎖は続くので、
                    // 深さは自動送信と同じに数える
                    t.chain_depth = depth;
                    ball.draft(origin, idx, depth, now_ms);
                    append_hook_log(&format!(
                        "下書き tab{origin} -> tab{idx} (depth {depth}): {}",
                        log_excerpt(&text, 60)
                    ));
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
                if touched_recently(t, now_ms) {
                    *flash = Some(i18n::t("msg.manual_guard"));
                    continue;
                }
                t.chain_depth = depth;
                let seen = t.output_count();
                write_prompt(t, &text);
                pending_submit.push(PendingSubmit::new(target, seen, now_ms));
                append_hook_log(&format!("貼り付け tab{target} ({}文字)", text.chars().count()));
                ball.throw(origin, target, depth, now_ms);
                append_hook_log(&format!(
                    "auto-send tab{origin} -> tab{target} (depth {depth}): {}",
                    log_excerpt(&text, 120)
                ));
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
    let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
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
    tab_w: u16,
    flash: &mut Option<String>,
) -> Result<()> {
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
            let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
            (
                p.screen().mouse_protocol_mode(),
                p.screen().mouse_protocol_encoding(),
            )
        };
        if !matches!(mode, vt100::MouseProtocolMode::None) {
            if let Some(bytes) = mouse_to_child_bytes(&m, inner, mode, enc) {
                // マウス報告は応答を求める入力ではない
                t.write_passthrough(&bytes)?;
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
                    dragged: false,
                });
            }
            let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
            let cur = p.screen().scrollback();
            p.screen_mut().set_scrollback(cur + 3);
        }
        // ホイール下: 最下端まで戻ったら (未選択なら) ライブへ自動復帰
        MouseEventKind::ScrollDown if t.copy.is_some() => {
            let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
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
            let offset = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().scrollback();
            let anchor = abs_line(offset, inner.height, row_in_pane);
            t.copy = Some(CopyState {
                cursor_row: row_in_pane,
                anchor: Some(anchor),
                dragged: false,
            });
        }
        // ドラッグで選択範囲を拡張
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(cs) = t.copy.as_mut() {
                if cs.cursor_row != row_in_pane {
                    cs.dragged = true;
                }
                cs.cursor_row = row_in_pane;
            }
        }
        // 左ボタン解放: 選択範囲を即クリップボードへ (PuTTY流の選択即コピー)
        MouseEventKind::Up(MouseButton::Left) => {
            let mut exit_copy = false;
            if let Some(cs) = t.copy.as_mut() {
                // 動かしていないなら選択ではない。置くだけのつもりのクリックで
                // クリップボードを奪わない (貼り付ける直前に消えると実害が大きい)
                if !cs.dragged {
                    cs.anchor = None;
                    exit_copy = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().scrollback() == 0;
                } else if let Some(anchor) = cs.anchor.take() {
                    let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
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
                t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen_mut().set_scrollback(0);
            }
            t.chain_depth = 0;
            t.last_manual_ms = Some(now_ms);
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

/// INDEXで押せる場所。描画時に記録し、クリック判定はこれだけを見る。
/// レイアウトを二重に計算しないので、表示とクリック位置がずれない
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Hit {
    /// そのタブへ切り替える (1始まり)
    Tab(usize),
    /// INDEXへ戻る
    Index,
    /// INDEXのメニュー行。同じ文字のキーを押したのと同じ扱いにする
    Key(char),
    /// 錠アイコン: 入力ロックの切替
    Lock(usize),
    /// ✖ / ⟳ アイコン: セッションの再起動
    Restart(usize),
    /// 左バー最上部のワークスペース名: 一覧を開く
    Workspace,
    /// ワークスペース一覧の項目
    WorkspaceItem(usize),
    /// タブバーの幅を変える境界線
    Divider,
    /// 全自動化の緊急停止 / 再開 (ステータス行の右端に常設)
    EmergencyStop,
}

/// 押せる場所 (画面上の絶対座標)。描画しながら記録する
struct HitBox {
    y: u16,
    x0: u16,
    x1: u16,
    hit: Hit,
}

/// その座標にある押せる場所を探す。
/// 後から記録したものを優先する (錠アイコンはタブ行の上に重なっているため)
fn hit_at(hits: &[HitBox], row: u16, col: u16) -> Option<Hit> {
    hits.iter()
        .rev()
        .find(|h| h.y == row && col >= h.x0 && col < h.x1)
        .map(|h| h.hit)
}

/// ボールのレーンが使う幅 (記号1桁 + 余白1桁)
const LANE_W: u16 = 2;
/// 自動チェーンのボール。状態インジケータの `●` と紛れないよう別の字にする
const BALL: char = '◉';

/// 連鎖の深さを色にする。上限に近づくほど熱くなる
fn heat_color(heat: f32) -> Color {
    if heat >= 0.8 {
        NEON_RED
    } else if heat >= 0.5 {
        NEON_YELLOW
    } else {
        NEON_GREEN
    }
}

/// 左バーのレーンに描く1コマ。`row` は 0=INDEX(人間)、1.. はタブ番号。
/// 飛行中は経路を線で残すので、静止画でも「どこからどこへ」が読める
fn lane_cell(ui: &Ui, row: usize) -> Span<'static> {
    let blank = Span::raw("  ");
    let hot = heat_color(ui.ball.heat(ui.max_chain));
    let glyph = |c: char, style: Style| Span::styled(format!("{c} "), style);
    match ui.ball.phase(ui.now_ms) {
        ball::Phase::Idle => blank,
        ball::Phase::Held { at } if at == row => glyph(BALL, Style::default().fg(hot)),
        ball::Phase::Caught { at } if at == row => glyph(
            BALL,
            Style::default().fg(Color::Black).bg(hot).add_modifier(Modifier::BOLD),
        ),
        ball::Phase::Flying { from, to, progress } => {
            let pos = from as f32 + (to as f32 - from as f32) * progress;
            if pos.round() as usize == row {
                return glyph(BALL, Style::default().fg(hot).add_modifier(Modifier::BOLD));
            }
            if row == to {
                let head = if to > from { '▼' } else { '▲' };
                return glyph(head, Style::default().fg(hot));
            }
            let (lo, hi) = if from < to { (from, to) } else { (to, from) };
            if row >= lo && row <= hi {
                return glyph('│', Style::default().fg(Color::DarkGray));
            }
            blank
        }
        _ => blank,
    }
}

/// 左バー上部の連鎖カウンタ。自動チェーンが動いている間だけ出す。
/// バーはINDEX側にあるので、狭い左バーでは数字だけにする
fn chain_gauge_line(ui: &Ui, width: u16) -> Option<Line<'static>> {
    if ui.ball.phase(ui.now_ms) == ball::Phase::Idle {
        return None;
    }
    let hot = heat_color(ui.ball.heat(ui.max_chain));
    let text = format!(" ⟲ {} {}/{}", i18n::t("tui.chain"), ui.ball.depth, ui.max_chain);
    Some(Line::from(Span::styled(
        pad_width(&text, width.saturating_sub(1)),
        Style::default().fg(hot).add_modifier(Modifier::BOLD),
    )))
}

/// INDEX上部の1行。連鎖ゲージと自動化の状態を並べる。
/// チェーンが動いていないときは、動いていないと分かる形で出す
fn chain_header(ui: &Ui, width: u16) -> Line<'static> {
    let heat = ui.ball.heat(ui.max_chain);
    let running = ui.ball.phase(ui.now_ms) != ball::Phase::Idle;
    let hot = if running { heat_color(heat) } else { Color::DarkGray };
    let label = i18n::t("tui.chain");
    let count = if running {
        format!("{}/{}", ui.ball.depth, ui.max_chain)
    } else {
        format!("—/{}", ui.max_chain)
    };
    // 区切りの "|" はステータス行用なので、ここでは落として並べる
    let auto = format!(
        "  {}{}",
        auto_label(ui.auto).trim_end_matches(['|', ' ']),
        if ui.remote_on { "  REMOTE:ON" } else { "" }
    );
    // 残った幅をゲージに使う。" ⟲ " と数字の前後の空白まで数に入れないと枠からはみ出す
    let fixed = format!(" ⟲ {label}  {count}{auto}").width();
    let bar_w = (width as usize).saturating_sub(fixed);
    let filled = if running { (bar_w as f32 * heat).round() as usize } else { 0 };
    Line::from(vec![
        Span::styled(format!(" ⟲ {label} "), Style::default().fg(hot).add_modifier(Modifier::BOLD)),
        Span::styled("━".repeat(filled), Style::default().fg(hot)),
        Span::styled(
            "━".repeat(bar_w.saturating_sub(filled)),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(format!(" {count}"), Style::default().fg(hot).add_modifier(Modifier::BOLD)),
        Span::styled(auto, Style::default().fg(NEON_BLUE)),
    ])
}

/// ステータス行を描く。右端に緊急停止ボタンを常設する。
///
/// INDEXでもセッション画面でも同じ位置にあることが大事で、
/// 慌てているときに探させない。連鎖ゲージの隣に置く案もあったが、
/// あれは連鎖中しか出ないので、いざという時に無いことがある
fn draw_status(
    f: &mut Frame,
    area: Rect,
    text: &str,
    bg: Color,
    ui: &Ui,
    hits: &mut Vec<HitBox>,
) {
    // 自動化そのものが無い構成では出さない (押しても意味が無いため)
    let Some(on) = ui.auto else {
        f.render_widget(
            Paragraph::new(text.to_string()).style(Style::default().fg(Color::Black).bg(bg)),
            area,
        );
        return;
    };

    let label = if on {
        format!(" ■ {} ", i18n::t("tui.stop"))
    } else {
        format!(" ▶ {} ", i18n::t("tui.resume"))
    };
    let btn_w = label.width() as u16;
    let left_w = area.width.saturating_sub(btn_w);
    hits.push(HitBox {
        y: area.y,
        x0: area.x + left_w,
        x1: area.x + area.width,
        hit: Hit::EmergencyStop,
    });

    // 止められる状態のときだけ赤くする。停止中は静かに再開ボタンとして残す
    let btn_style = if !on {
        Style::default().fg(Color::Black).bg(Color::DarkGray).add_modifier(Modifier::BOLD)
    } else if ui.hover == Some(Hit::EmergencyStop) {
        Style::default().fg(NEON_YELLOW).bg(NEON_RED).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Black).bg(NEON_RED).add_modifier(Modifier::BOLD)
    };
    f.render_widget(
        Paragraph::new(pad_width(text, left_w))
            .style(Style::default().fg(Color::Black).bg(bg)),
        Rect { width: left_w, ..area },
    );
    f.render_widget(
        Paragraph::new(label).style(btn_style),
        Rect { x: area.x + left_w, width: btn_w, ..area },
    );
}

/// 出力量 0..=7 を波形の1文字にする
fn spark(level: u8) -> char {
    const BARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    BARS[(level as usize).min(7)]
}

/// INDEXのカード間のすき間に描く連結線。`below` の行の下のすき間を表す
fn lane_gap(ui: &Ui, below: usize) -> Span<'static> {
    if let ball::Phase::Flying { from, to, .. } = ui.ball.phase(ui.now_ms) {
        let (lo, hi) = if from < to { (from, to) } else { (to, from) };
        if below >= lo && below < hi {
            let hot = heat_color(ui.ball.heat(ui.max_chain));
            let c = if to > from { '│' } else { '│' };
            return Span::styled(format!("{c} "), Style::default().fg(hot));
        }
    }
    Span::raw("  ")
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
    /// 自動チェーンの現在地 (透明のボールを見えるようにしたもの)
    ball: ball::Ball,
    /// チェーン上限。ボールの色が上限にどれだけ近いかを表す
    max_chain: u32,
    /// 描画時刻 (相対ms)。ボールのアニメ進行に使う
    now_ms: u64,
    /// マウスが乗っているINDEXの行 (押せる場所が分かるように色を変える)
    hover: Option<Hit>,
}

fn draw(f: &mut Frame, tabs: &[Tab], ui: &Ui, flash: Option<&str>, hits: &mut Vec<HitBox>) {
    hits.clear();
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
        hits.push(HitBox { y: main[0].y, x0: 0, x1: ui.tab_w - 1, hit: Hit::Workspace });
        let bg = if ui.hover == Some(Hit::Workspace) { NEON_GREEN } else { NEON_YELLOW };
        lines.push(Line::from(Span::styled(
            pad_width(&format!("[▼] {name}"), ui.tab_w - 1),
            Style::default().fg(Color::Black).bg(bg).add_modifier(Modifier::BOLD),
        )));
    }

    if ui.ws_open {
        // ドロップダウン展開中はタブ一覧の代わりにワークスペース一覧を出す
        for (i, name) in ui.ws_names.iter().enumerate() {
            let hit = Hit::WorkspaceItem(i);
            hits.push(HitBox {
                y: main[0].y + lines.len() as u16,
                x0: 0,
                x1: ui.tab_w - 1,
                hit,
            });
            let style = if i == ui.ws_index || ui.hover == Some(hit) {
                Style::default().fg(Color::Black).bg(NEON_YELLOW)
            } else {
                Style::default().fg(NEON_YELLOW)
            };
            lines.push(Line::from(Span::styled(
                pad_width(&format!(" {}. {name}", i + 1), ui.tab_w - 1),
                style,
            )));
        }
    } else {
        let index_style = if ui.active == 0 {
            Style::default().fg(Color::Black).bg(NEON_BLUE)
        } else {
            Style::default().fg(NEON_BLUE)
        };
        // 自動チェーンが動いている間だけ、上部に連鎖の深さを出す
        // (上限に近づくと色が変わるので、暴走対策が効いている様子が見える)
        if let Some(line) = chain_gauge_line(ui, ui.tab_w) {
            lines.push(line);
        }
        hits.push(HitBox {
            y: main[0].y + lines.len() as u16,
            x0: 0,
            x1: ui.tab_w - 1,
            hit: Hit::Index,
        });
        let index_style = if ui.hover == Some(Hit::Index) && ui.active != 0 {
            Style::default().fg(Color::Black).bg(NEON_BLUE)
        } else {
            index_style
        };
        lines.push(Line::from(vec![
            lane_cell(ui, 0),
            Span::styled(format!("[≡] 0. {}", i18n::t("tui.index")), index_style),
        ]));
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
            // 右端1桁は枠線。錠アイコン(全角2桁)・インジケータ4桁・
            // ボールのレーンを除いた幅に収める
            const LOCK_W: u16 = 2;
            let avail = ui.tab_w.saturating_sub(1 + LANE_W + 4 + LOCK_W).max(1);
            let label = truncate_width(&format!("{prefix}{}. {}", i + 1, t.title), avail);
            let pad = avail as usize - label.width();

            // 押せる場所を、実際に描く位置から作る。
            // 手計算した固定値にすると、レーンを足したときのように黙ってずれる
            let y = main[0].y + lines.len() as u16;
            let ind_x = LANE_W;
            let icon_x = ui.tab_w - 1 - LOCK_W;
            hits.push(HitBox { y, x0: 0, x1: ui.tab_w - 1, hit: Hit::Tab(i + 1) });
            // 終了・要再起動のときだけ、インジケータが再起動ボタンになる
            let restartable = t.state == TabState::Exited || t.needs_restart;
            if restartable {
                hits.push(HitBox {
                    y,
                    x0: ind_x,
                    x1: ind_x + 4,
                    hit: Hit::Restart(i + 1),
                });
            }
            hits.push(HitBox {
                y,
                x0: icon_x,
                x1: icon_x + LOCK_W,
                hit: Hit::Lock(i + 1),
            });

            let hovered_row = ui.hover == Some(Hit::Tab(i + 1));
            let title_style = if hovered_row && ui.active != i + 1 {
                Style::default().fg(Color::Black).bg(NEON_GREEN).add_modifier(Modifier::BOLD)
            } else {
                title_style
            };
            let ind_style = if ui.hover == Some(Hit::Restart(i + 1)) {
                Style::default().fg(Color::Black).bg(ind_color).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(ind_color).add_modifier(Modifier::BOLD)
            };
            let icon = if t.needs_restart {
                "⟳ "
            } else if t.locked {
                "🔒"
            } else if hovered_row || ui.hover == Some(Hit::Lock(i + 1)) {
                // 何も無い所は押せると分からないので、マウスが来たら鍵を出す
                "🔓"
            } else {
                "  "
            };
            let icon_style = if ui.hover == Some(Hit::Lock(i + 1)) {
                Style::default().fg(Color::Black).bg(NEON_YELLOW).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(NEON_YELLOW)
            };
            lines.push(Line::from(vec![
                lane_cell(ui, i + 1),
                Span::styled(format!("[{ind}] "), ind_style),
                Span::styled(format!("{label}{}", " ".repeat(pad)), title_style),
                Span::styled(icon, icon_style),
            ]));
        }
    }
    for y in main[0].y..main[0].y + main[0].height {
        hits.push(HitBox { y, x0: ui.tab_w - 1, x1: ui.tab_w, hit: Hit::Divider });
    }
    let border = if ui.hover == Some(Hit::Divider) { NEON_YELLOW } else { NEON_GREEN };
    let tabs_widget = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(border)),
    );
    f.render_widget(tabs_widget, main[0]);

    if ui.active == 0 {
        draw_index(f, tabs, main[1], outer[1], flash, ui, hits);
    } else if let Some(t) = tabs.get(ui.active - 1) {
        draw_session(f, t, main[1], outer[1], flash, ui, hits);
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
/// ブロック文字のワードマーク (3行)。1文字ぶんの幅は不揃いなので、
/// 右端の余白まで含めて数えず、実際の文字幅で測る
const WORDMARK: [&str; 3] = [
    "█▀▀ █ █ █ █ █ █ █▀▀ █ █ █▀█    ▀█▀ █▀▀ █▀█ █▄█",
    "▀▀█ █▀█ █ █▀▄ █ ▀▀█ █▀█ █▀█ ▀▀  █  █▀▀ █▀▄ █ █",
    "▀▀▀ ▀ ▀ ▀ ▀ ▀ ▀ ▀▀▀ ▀ ▀ ▀ ▀     ▀  ▀▀▀ ▀ ▀ ▀ ▀",
];

/// 1行に落としたときの表記
const WORDMARK_SMALL: &str = "◢◤ SHIKISHA-TERM";

/// 名前をどう出すか。画面に入らないなら小さくし、それも入らないなら出さない。
///
/// 入らないものを無理に描くと折り返して崩れ、名前どころか画面が壊れて見える。
/// 縦も見るのは、タブが多いときに名前で一覧を押し出さないため
pub fn wordmark_lines(width: u16, height: u16) -> Vec<String> {
    let need = WORDMARK
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as u16;
    // 左の余白1桁ぶんを足して測る。ぴったりだと枠に触れて窮屈に見える
    if width >= need + 2 && height >= 12 {
        return WORDMARK.iter().map(|l| format!(" {l}")).collect();
    }
    if width >= WORDMARK_SMALL.chars().count() as u16 + 2 {
        return vec![format!(" {WORDMARK_SMALL}")];
    }
    Vec::new()
}

fn draw_index(
    f: &mut Frame,
    tabs: &[Tab],
    area: Rect,
    status_area: Rect,
    flash: Option<&str>,
    ui: &Ui,
    hits: &mut Vec<HitBox>,
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
    // 名前を画面の中に置く。ターミナルのタイトルバーは
    // フォーカスモードでも切り抜きでも消えるので、あてにしない
    let mark = wordmark_lines(inner.width, inner.height);
    if !mark.is_empty() {
        // 枠に触れさせない。詰まっていると窮屈に見える
        lines.push(Line::default());
        for l in mark {
            lines.push(Line::from(Span::styled(
                l,
                Style::default().fg(NEON_BLUE).add_modifier(Modifier::BOLD),
            )));
        }
        lines.push(Line::default());
    }
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
    // 稼働盤: 何が動いていて、誰が仕事を持っているかを一望する。
    // 光っているものは全部実データ (状態・出力量・連鎖の深さ) にしてある
    lines.push(Line::from(Span::styled(
        format!(" build {}  ({})", env!("BUILD_TIME"), env!("BUILD_REV")),
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(chain_header(ui, inner.width));
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!(
            "    {:<3} {:<16} {:<10} {:<10} {}",  // 見出しは半角のみなので固定幅でよい
            i18n::t("tui.col.no"),
            i18n::t("tui.col.name"),
            i18n::t("tui.col.state"),
            i18n::t("tui.col.profile"),
            i18n::t("tui.col.activity")
        ),
        Style::default().fg(NEON_BLUE).add_modifier(Modifier::BOLD),
    )));
    for (i, t) in tabs.iter().enumerate() {
        let (ind, color) = indicator(t);
        let name = format!(
            "{}{}{}",
            "  ".repeat(t.depth as usize),
            t.title,
            if t.locked { " 🔒" } else { "" }
        );
        let wave: String = t.activity().iter().map(|l| spark(*l)).collect();
        let hit = Hit::Tab(i + 1);
        hits.push(HitBox {
            y: inner.y + lines.len() as u16,
            x0: inner.x,
            x1: inner.x + inner.width,
            hit,
        });
        let name_style = if ui.hover == Some(hit) {
            Style::default().fg(Color::Black).bg(NEON_GREEN).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(NEON_GREEN).add_modifier(Modifier::BOLD)
        };
        lines.push(Line::from(vec![
            lane_cell(ui, i + 1),
            Span::styled("▌", Style::default().fg(color)),
            Span::styled(format!("{ind} "), Style::default().fg(color)),
            Span::raw(format!("{:<3}", format!("{}.", i + 1))),
            Span::styled(pad_width(&name, 16), name_style),
            Span::styled(pad_width(&t.state.display(), 10), Style::default().fg(color)),
            Span::raw(pad_width(t.profile_name(), 10)),
            Span::styled(wave, Style::default().fg(color)),
        ]));
        if i + 1 < tabs.len() {
            lines.push(Line::from(lane_gap(ui, i + 1)));
        }
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
        // "[r]" のように1文字のものだけ押せる ("[1-9]" は説明であって行き先が無い)
        let hit = key
            .strip_prefix('[')
            .and_then(|k| k.strip_suffix(']'))
            .filter(|k| k.chars().count() == 1)
            .and_then(|k| k.chars().next())
            .map(Hit::Key);
        let y = inner.y + lines.len() as u16;
        if let Some(hit) = hit {
            hits.push(HitBox { y, x0: inner.x, x1: inner.x + inner.width, hit });
        }
        if hit.is_some() && ui.hover == hit {
            lines.push(Line::from(Span::styled(
                pad_width(&format!(" {key:<7}{desc}"), inner.width),
                Style::default().fg(Color::Black).bg(NEON_YELLOW).add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {key:<7}"),
                    Style::default().fg(NEON_YELLOW).add_modifier(Modifier::BOLD),
                ),
                Span::raw(desc),
            ]));
        }
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
    draw_status(f, status_area, &status, NEON_BLUE, ui, hits);
}

/// セッションペイン: 子端末の描画 + コピーモードハイライト + IMEカーソル + ステータス
fn draw_session(
    f: &mut Frame,
    t: &Tab,
    area: Rect,
    status_area: Rect,
    flash: Option<&str>,
    ui: &Ui,
    hits: &mut Vec<HitBox>,
) {
    let border_color = if t.copy.is_some() {
        NEON_YELLOW
    } else if t.locked {
        NEON_BLUE
    } else {
        NEON_GREEN
    };
    // 見出しの錠アイコンはクリックでロック切替できる (マウスだけで操作可能)
    let (head, lock, offset) = session_title(t);
    let lock_x = area.x + 1 + offset;
    hits.push(HitBox {
        y: area.y,
        x0: lock_x,
        x1: lock_x + lock.width() as u16,
        hit: Hit::Lock(ui.active),
    });
    let lock_bg = if ui.hover == Some(Hit::Lock(ui.active)) {
        NEON_YELLOW
    } else if t.locked {
        NEON_BLUE
    } else {
        NEON_GREEN
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(vec![
            Span::styled(head, Style::default().fg(NEON_YELLOW)),
            Span::styled(
                lock,
                Style::default().fg(Color::Black).bg(lock_bg).add_modifier(Modifier::BOLD),
            ),
        ]));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let scrollback_offset;
    let alt_screen;
    {
        let parser = t.parser.lock().unwrap_or_else(|e| e.into_inner());
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
    draw_status(f, status_area, &status, status_bg, ui, hits);
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
            let bracketed = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste();
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

    /// 画面に出た文字を1つの文字列にする (空白は落として位置ずれの影響を消す)
    fn screen_text(term: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    fn test_ui(active: usize, ball: ball::Ball, now_ms: u64) -> Ui {
        Ui {
            tab_w: 22,
            first_run: false,
            active,
            prefix_active: false,
            auto: Some(true),
            ws_names: vec![],
            ws_index: 0,
            ws_open: false,
            help_open: false,
            qr: None,
            remote_on: false,
            ball,
            max_chain: 10,
            now_ms,
            hover: None,
        }
    }

    /// ボールが「今どのタブにあるか」と「どこから飛んできたか」が画面に出ること。
    /// 静止画でも経路が読めることが狙いなので、飛行中の連結線も確認する
    #[test]
    fn the_chain_ball_is_visible_and_shows_its_path() {
        use ratatui::backend::TestBackend;
        let argv = vec!["cmd.exe".to_string()];
        let mut tabs: Vec<Tab> = (1..=3)
            .map(|i| {
                Tab::spawn(format!("T{i}"), &argv, None, 20, 100, tab::TabOptions::default()).unwrap()
            })
            .collect();
        let mut term = ratatui::Terminal::new(TestBackend::new(100, 30)).unwrap();

        // 誰も自動で動いていないうちはボールを出さない (常時ちらつかせない)
        let ui = test_ui(0, ball::Ball::default(), 0);
        term.draw(|f| draw(f, &tabs, &ui, None, &mut Vec::new())).unwrap();
        let idle = screen_text(&term);
        assert!(!idle.contains(BALL), "静止中はボールを出さない: {idle}");
        assert!(idle.contains("—/10"), "連鎖していないことが分かる: {idle}");

        // タブ1がタブ3へ投げた直後: 飛行中の経路が見える
        let mut b = ball::Ball::default();
        b.throw(1, 3, 2, 1_000);
        let ui = test_ui(0, b, 1_050);
        term.draw(|f| draw(f, &tabs, &ui, None, &mut Vec::new())).unwrap();
        let flying = screen_text(&term);
        assert!(flying.contains(BALL), "ボールが見える: {flying}");
        assert!(flying.contains('│'), "経路が線で残る: {flying}");
        assert!(flying.contains("2/10"), "連鎖の深さが出る: {flying}");

        // 着弾後は持ち主のところに落ち着く
        let ui = test_ui(0, b, 1_000 + 2_000);
        term.draw(|f| draw(f, &tabs, &ui, None, &mut Vec::new())).unwrap();
        assert!(screen_text(&term).contains(BALL), "保持中も見える");

        for t in tabs.iter_mut() {
            t.kill();
        }
    }

    /// 波形は飾りではなく出力量なので、何も出ていなければ底ばいであること
    #[test]
    fn activity_wave_reflects_real_output() {
        let argv = vec!["cmd.exe".to_string()];
        let mut t =
            Tab::spawn("SHELL".into(), &argv, None, 20, 100, tab::TabOptions::default()).unwrap();
        assert_eq!(t.activity().len(), tab::ACTIVITY_LEN);
        assert!(t.activity().iter().all(|l| *l == 0), "起動直後は無音");

        // 出力があったあとにtickすると、直近のコマが立ち上がる
        t.write_bytes(b"echo hello\r").unwrap();
        let start = Instant::now();
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(25));
            t.tick(start);
            if *t.activity().last().unwrap() > 0 {
                break;
            }
        }
        assert!(
            t.activity().iter().any(|l| *l > 0),
            "出力があれば波形が立つ: {:?}",
            t.activity()
        );
        t.kill();
    }

    /// 送信は「本文を入れる」と「実行する」の2段階であること。
    ///
    /// まとめて1回で書くと、AI CLIの入力欄が貼り付けを処理しきる前に改行が届き、
    /// 本文だけ入って実行されない状態になる (スマホからの送信で実際に起きた)
    #[test]
    fn a_prompt_is_typed_first_and_submitted_after() {
        let argv = vec!["cmd.exe".to_string()];
        let mut t =
            Tab::spawn("shell".into(), &argv, None, 20, 60, tab::TabOptions::default()).unwrap();

        let screen = |t: &Tab| tab::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen());
        let has_line = |t: &Tab, want: &str| {
            screen(t).lines().any(|l| l.trim() == want)
        };
        let wait_for = |t: &Tab, want: &str| {
            for _ in 0..60 {
                if has_line(t, want) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };

        // プロンプトが出るまで待つ
        for _ in 0..60 {
            if screen(&t).contains('>') {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        write_prompt(&t, "echo shikisha-ok");
        std::thread::sleep(Duration::from_millis(400));
        assert!(
            screen(&t).contains("echo shikisha-ok"),
            "本文は入力欄に入る: {}",
            screen(&t)
        );
        assert!(
            !has_line(&t, "shikisha-ok"),
            "まだ実行はされていない: {}",
            screen(&t)
        );

        // 予約されていた実行が届く
        t.write_bytes(b"\r").unwrap();
        assert!(wait_for(&t, "shikisha-ok"), "実行される: {}", screen(&t));

        t.kill();
    }

    /// 「人間が触った直後は自動送信しない」保護が、起動直後に誤作動しないこと。
    ///
    /// 触られた時刻を 0 で初期化していたため、アプリ起動からガード時間のあいだ
    /// 「たった今触った」と誤認し、起動時の自動化が黙って捨てられていた
    #[test]
    fn an_untouched_tab_is_not_mistaken_for_one_just_typed_into() {
        let argv = vec!["cmd.exe".to_string()];
        let mut t =
            Tab::spawn("T".into(), &argv, None, 20, 60, tab::TabOptions::default()).unwrap();

        // 起動直後: まだ誰も触っていないので、いつ聞かれても保護は働かない
        assert!(!touched_recently(&t, 0), "起動した瞬間");
        assert!(!touched_recently(&t, 1_000), "1秒後");
        assert!(
            !touched_recently(&t, MANUAL_GUARD_MS - 1),
            "ガード時間の内側でも、触られていなければ送ってよい"
        );

        // 人間が触ったらガードが効く
        t.last_manual_ms = Some(10_000);
        assert!(touched_recently(&t, 10_000), "触った直後");
        assert!(
            touched_recently(&t, 10_000 + MANUAL_GUARD_MS - 1),
            "ガード時間内はまだ効く"
        );
        assert!(
            !touched_recently(&t, 10_000 + MANUAL_GUARD_MS),
            "時間が過ぎたら解ける"
        );

        t.kill();
    }

    /// 実行(改行)は、貼り付けの取り込みが「終わって」から送ること。
    ///
    /// 「始まった」時点で送ると、長い貼り付けでは取り込み中に届いて捨てられる。
    /// 実測では約600文字なら通り、約1900文字で落ちた
    #[test]
    fn the_enter_waits_for_the_paste_to_finish_being_taken_in() {
        let mut p = PendingSubmit::new(1, 100, 1_000);

        // 出力が動いている間は、いくら経っても送らない
        assert!(!p.ready(100, 1_000), "送った瞬間");
        assert!(!p.ready(200, 1_100), "反応が始まっただけでは送らない");
        assert!(!p.ready(300, 2_000), "まだ増えている");
        assert!(!p.ready(400, 3_000), "まだ増えている");

        // 止まってから、静かな時間が続いて初めて送る
        assert!(!p.ready(400, 3_100), "止まった直後はまだ");
        assert!(!p.ready(400, 3_100 + SUBMIT_QUIET_MS - 1), "静かな時間が足りない");
        assert!(p.ready(400, 3_100 + SUBMIT_QUIET_MS), "落ち着いたら送る");

        // 途中で再開したら測り直す。
        // 静止の起点は「止まった瞬間」ではなく「止まっていると最初に気づいた時刻」
        let mut p = PendingSubmit::new(1, 0, 0);
        assert!(!p.ready(0, 100), "静かだがまだ足りない");
        assert!(!p.ready(50, 200), "再開したので測り直す");
        assert!(!p.ready(50, 300), "ここで改めて静止を観測");
        assert!(!p.ready(50, 300 + SUBMIT_QUIET_MS - 1), "測り直し中");
        assert!(p.ready(50, 300 + SUBMIT_QUIET_MS), "改めて落ち着いた");

        // 落ち着かないままでも、上限に達したら送る
        let mut p = PendingSubmit::new(1, 0, 0);
        let mut out = 0;
        for t in (100..SUBMIT_GIVE_UP_MS).step_by(100) {
            out += 1;
            assert!(!p.ready(out, t), "増え続けている間は待つ ({t}ms)");
        }
        out += 1;
        assert!(p.ready(out, SUBMIT_GIVE_UP_MS), "上限に達したら送る");
    }

    /// ボールを追って画面が移ること、そして人の操作を邪魔しないこと。
    #[test]
    fn the_view_follows_the_ball_but_yields_to_the_person() {
        let g = FOLLOW_GUARD_MS;

        // ボールが渡った先へ移る
        assert_eq!(follow_target(true, 2, 1, 3, g, 0), Some(2));
        // 同じ場所へは何度も飛ばさない
        assert_eq!(follow_target(true, 2, 2, 3, g, 0), None);
        // 誰も持っていなければ動かない
        assert_eq!(follow_target(true, 0, 1, 3, g, 0), None);
        // 居ないタブへは行かない (ワークスペース切替の直後など)
        assert_eq!(follow_target(true, 5, 1, 3, g, 0), None);
        // 設定で切っていれば動かない
        assert_eq!(follow_target(false, 2, 1, 3, g, 0), None);

        // 人が画面を触った直後は従わない (読んでいる最中に飛ばさない)
        assert_eq!(follow_target(true, 2, 1, 3, 1_000, 1_000), None);
        assert_eq!(follow_target(true, 2, 1, 3, 1_000 + g - 1, 1_000), None);
        // 時間が経てば再び従う
        assert_eq!(follow_target(true, 2, 1, 3, 1_000 + g, 1_000), Some(2));
    }

    /// 緊急停止は、どの画面にいても同じ場所にあること。
    /// 慌てているときに探させないのが目的なので、位置が動いたら意味が無い
    #[test]
    fn the_emergency_stop_is_always_in_the_same_corner() {
        use ratatui::backend::TestBackend;
        let argv = vec!["cmd.exe".to_string()];
        let mut tabs = vec![
            Tab::spawn("T1".into(), &argv, None, 20, 100, tab::TabOptions::default()).unwrap(),
        ];
        let mut term = ratatui::Terminal::new(TestBackend::new(100, 30)).unwrap();

        let draw_with = |term: &mut ratatui::Terminal<TestBackend>, active, auto| {
            let mut hits: Vec<HitBox> = Vec::new();
            let mut ui = test_ui(active, ball::Ball::default(), 0);
            ui.auto = auto;
            term.draw(|f| draw(f, &tabs, &ui, None, &mut hits)).unwrap();
            hits.into_iter().find(|h| h.hit == Hit::EmergencyStop)
        };

        // INDEXでもセッション画面でも、右下の同じ位置に出る
        let on_index = draw_with(&mut term, 0, Some(true)).expect("INDEXに出る");
        let on_session = draw_with(&mut term, 1, Some(true)).expect("セッション画面にも出る");
        assert_eq!(
            (on_index.y, on_index.x0, on_index.x1),
            (on_session.y, on_session.x0, on_session.x1),
            "画面が変わっても位置は動かない"
        );
        assert_eq!(on_index.x1, 100, "右端まで届く");
        assert_eq!(on_index.y, 29, "最下行にある");
        assert_eq!(
            hit_at(&[on_index], 29, 99),
            Some(Hit::EmergencyStop),
            "右下の隅を押せる"
        );

        // 停止中は再開ボタンとして残る (戻り方が分からなくならないように)
        assert!(draw_with(&mut term, 0, Some(false)).is_some(), "停止中も押せる");
        // 自動化が無い構成では出さない
        assert!(draw_with(&mut term, 0, None).is_none(), "自動化が無ければ出さない");

        for t in tabs.iter_mut() {
            t.kill();
        }
    }

    /// 単クリックではクリップボードを奪わないこと。
    ///
    /// 選択は行単位なので、クリックしただけでも「1行選んだ」形になる。
    /// そのままコピーすると、貼り付けようとしていた中身が消える
    /// (Codexのプロンプトをクリックしてplaceholderがコピーされていた)
    #[test]
    fn a_click_without_dragging_leaves_the_clipboard_alone() {
        use ratatui::crossterm::event::MouseEvent;

        let argv = vec!["cmd.exe".to_string()];
        let mut tabs = vec![
            Tab::spawn("T".into(), &argv, None, 20, 60, tab::TabOptions::default()).unwrap(),
        ];
        let mut active = 1usize;
        let size = Size { width: 100, height: 30 };
        let tab_w = 20u16;
        let inner = pane_inner(size, tab_w);

        let at = |kind, row: u16| MouseEvent {
            kind,
            column: inner.x + 3,
            row,
            modifiers: KeyModifiers::NONE,
        };
        let mut flash = None;

        // 押して、動かさずに離す
        handle_mouse(&mut tabs, &mut active, at(MouseEventKind::Down(MouseButton::Left), inner.y + 2),
                     size, 0, tab_w, &mut flash).unwrap();
        assert!(tabs[0].copy.is_some(), "押した時点では選択の準備に入る");
        handle_mouse(&mut tabs, &mut active, at(MouseEventKind::Up(MouseButton::Left), inner.y + 2),
                     size, 0, tab_w, &mut flash).unwrap();
        assert!(
            flash.is_none(),
            "クリックしただけでコピーしている: {flash:?}"
        );
        assert!(tabs[0].copy.is_none(), "クリックのあとは通常操作へ戻る");

        // 押して、動かしてから離す
        handle_mouse(&mut tabs, &mut active, at(MouseEventKind::Down(MouseButton::Left), inner.y + 2),
                     size, 0, tab_w, &mut flash).unwrap();
        handle_mouse(&mut tabs, &mut active, at(MouseEventKind::Drag(MouseButton::Left), inner.y + 5),
                     size, 0, tab_w, &mut flash).unwrap();
        handle_mouse(&mut tabs, &mut active, at(MouseEventKind::Up(MouseButton::Left), inner.y + 5),
                     size, 0, tab_w, &mut flash).unwrap();
        assert!(flash.is_some(), "ドラッグして選んだらコピーする");

        for t in tabs.iter_mut() {
            t.kill();
        }
    }

    /// 押せる場所が、実際に描いた位置に記録されること。
    /// 座標を別に計算し直すとレイアウト変更で黙ってずれる (レーン追加で実際にずれた)
    #[test]
    fn clickable_regions_follow_what_was_drawn() {
        use ratatui::backend::TestBackend;
        let argv = vec!["cmd.exe".to_string()];
        let mut tabs: Vec<Tab> = (1..=2)
            .map(|i| {
                Tab::spawn(format!("T{i}"), &argv, None, 20, 100, tab::TabOptions::default()).unwrap()
            })
            .collect();
        let mut hits: Vec<HitBox> = Vec::new();
        let ui = test_ui(0, ball::Ball::default(), 0);
        let mut term = ratatui::Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| draw(f, &tabs, &ui, None, &mut hits)).unwrap();

        let find = |hit: Hit| hits.iter().find(|h| h.hit == hit);
        let all = |hit: Hit| hits.iter().filter(|h| h.hit == hit).count();

        assert!(find(Hit::Index).is_some(), "INDEXへ戻る行が押せる");
        for n in 1..=2 {
            // 左バーとINDEXの一覧、どちらからでもタブへ行ける
            assert_eq!(all(Hit::Tab(n)), 2, "タブ{n}は2箇所から押せる");
            let lock = find(Hit::Lock(n)).expect("錠アイコンが押せる");
            assert_eq!(
                lock.x1,
                ui.tab_w - 1,
                "錠アイコンは枠線のすぐ左に来る (描画位置と一致していること)"
            );
        }
        for key in ['r', 'w', 't', 'i', 'e', 'k', '?', 'q'] {
            assert!(find(Hit::Key(key)).is_some(), "[{key}] が押せない");
        }
        assert!(
            !hits.iter().any(|h| matches!(h.hit, Hit::Key(c) if c.is_ascii_digit())),
            "説明だけの行 ([1-9]) は押せないままにする"
        );

        // 動いているタブに再起動ボタンは出さない (押せてしまうと事故になる)
        assert!(find(Hit::Restart(1)).is_none(), "動作中は再起動ボタンを出さない");
        tabs[0].needs_restart = true;
        term.draw(|f| draw(f, &tabs, &ui, None, &mut hits)).unwrap();
        let restart = hits
            .iter()
            .find(|h| h.hit == Hit::Restart(1))
            .expect("要再起動なら押せる");
        assert!(restart.x0 >= LANE_W, "ボールのレーンの右側にある");

        // 座標の引き当て
        let q = hits.iter().find(|h| h.hit == Hit::Key('q')).unwrap();
        assert_eq!(hit_at(&hits, q.y, q.x0), Some(Hit::Key('q')));
        assert_eq!(hit_at(&hits, q.y, q.x1 - 1), Some(Hit::Key('q')), "行の右端まで押せる");
        assert_eq!(hit_at(&hits, q.y, q.x0.saturating_sub(1)), None, "枠の外は押せない");
        assert_eq!(hit_at(&hits, q.y + 100, q.x0), None, "別の行は反応しない");
        // 錠アイコンはタブ行に重なっている。狭い方が勝たないと押せない
        let lock = hits.iter().find(|h| h.hit == Hit::Lock(2)).unwrap();
        assert_eq!(hit_at(&hits, lock.y, lock.x0), Some(Hit::Lock(2)), "重なりは錠が優先");

        // ホバーは色だけ変える
        let plain = { term.draw(|f| draw(f, &tabs, &ui, None, &mut hits)).unwrap(); screen_text(&term) };
        let mut ui2 = test_ui(0, ball::Ball::default(), 0);
        ui2.hover = Some(Hit::Key('q'));
        let hovered = { term.draw(|f| draw(f, &tabs, &ui2, None, &mut hits)).unwrap(); screen_text(&term) };
        assert_eq!(plain, hovered, "文字は変えず色だけ変える");
        assert!(
            term.backend().buffer().content().iter().any(|c| c.bg != ratatui::style::Color::Reset),
            "ホバー行に背景色が付く"
        );

        for t in tabs.iter_mut() {
            t.kill();
        }
    }

    /// 目視確認用 (cargo test preview_ops_board -- --nocapture --ignored)
    #[test]
    #[ignore]
    fn preview_ops_board() {
        use ratatui::backend::TestBackend;
        let argv = vec!["cmd.exe".to_string()];
        let names = ["実装", "検査", "サーバー"];
        let mut tabs: Vec<Tab> = names
            .iter()
            .map(|n| Tab::spawn(n.to_string(), &argv, None, 20, 100, tab::TabOptions::default()).unwrap())
            .collect();
        tabs[2].locked = true;
        let start = Instant::now();
        for _ in 0..12 {
            std::thread::sleep(Duration::from_millis(40));
            for t in tabs.iter_mut() { t.tick(start); }
        }
        let mut b = ball::Ball::default();
        b.throw(1, 2, 3, 1_000);
        for (title, now, hover) in [
            ("--- 停止ボタン (通常) ---", 3_000u64, None),
            ("--- 停止ボタン (ホバー) ---", 3_000, Some(Hit::EmergencyStop)),
        ] {
            let mut ui = test_ui(0, b, now);
            ui.hover = hover;
            let mut term = ratatui::Terminal::new(TestBackend::new(96, 26)).unwrap();
            term.draw(|f| draw(f, &tabs, &ui, None, &mut Vec::new())).unwrap();
            println!("{title}");
            let buf = term.backend().buffer();
            for y in 0..buf.area.height {
                let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
                // 反転表示は文字ダンプに出ないので、背景色が付いた行に印を出す
                let lit = (0..buf.area.width)
                    .any(|x| buf[(x, y)].bg != ratatui::style::Color::Reset);
                println!("{}|{}|", if lit { "*" } else { " " }, row.trim_end());
            }
        }
        for t in tabs.iter_mut() { t.kill(); }
    }

    /// 初回起動でINDEXに案内が出ること (何をすればいいか分からないまま終わらせない)
    /// 起動したら、前に開いていたワークスペースから始めること。
    ///
    /// 毎回先頭から始まると、試したいものが2つ目にあるだけで、
    /// 起動のたびに切り替える手間が要る。デバッグ中はそれを何十回も繰り返す
    #[test]
    fn it_opens_where_you_left_off() {
        let names: Vec<String> = ["指揮者", "たまごカート編集部", "検証"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        assert_eq!(
            starting_workspace(true, Some("たまごカート編集部"), &names),
            1,
            "前に開いていたものに戻らない"
        );

        // 番号ではなく名前で覚えるので、並べ替えても追いかける
        let reordered: Vec<String> = ["検証", "たまごカート編集部", "指揮者"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            starting_workspace(true, Some("たまごカート編集部"), &reordered),
            1
        );
        assert_eq!(
            starting_workspace(true, Some("指揮者"), &reordered),
            2,
            "並べ替えで別のワークスペースを開いている"
        );

        // 消した・改名した・記憶が無い・切ってある → 先頭
        assert_eq!(starting_workspace(true, Some("消えた"), &names), 0);
        assert_eq!(starting_workspace(true, None, &names), 0);
        assert_eq!(starting_workspace(false, Some("検証"), &names), 0, "切ってある");
        assert_eq!(starting_workspace(true, Some("指揮者"), &[]), 0, "空でも落ちない");
    }

    /// 名前は画面に入るときだけ出すこと。
    ///
    /// TUI はターミナルの中で動くので、ウィンドウのタイトルは
    /// フォーカスモードにすれば消えるし、切り抜かれても消える。
    /// 画面の中に無いと、GIF が README から離れた時点で
    /// 何のソフトか分からなくなる
    #[test]
    fn the_name_shrinks_rather_than_breaking_the_layout() {
        let wide = wordmark_lines(100, 30);
        assert_eq!(wide.len(), 3, "広ければワードマーク");
        for l in &wide {
            assert!(
                l.chars().count() <= 100,
                "枠からはみ出している: {} 桁",
                l.chars().count()
            );
        }

        // 幅が足りなければ1行に落とす (折り返して崩れるより小さく出す)
        let narrow = wordmark_lines(30, 30);
        assert_eq!(narrow.len(), 1);
        assert!(narrow[0].contains("SHIKISHA-TERM"));

        // 縦が足りないときも1行。タブの一覧を名前で押し出さない
        let short = wordmark_lines(100, 8);
        assert_eq!(short.len(), 1, "低い画面では一覧を優先する");

        // どちらも足りなければ出さない
        assert!(wordmark_lines(10, 30).is_empty());
        assert!(wordmark_lines(0, 0).is_empty());
    }

    /// ワードマークの3行は同じ幅であること (揃っていないと文字が崩れて見える)
    #[test]
    fn the_wordmark_rows_line_up() {
        let w: Vec<usize> = WORDMARK.iter().map(|l| l.chars().count()).collect();
        assert!(
            w.iter().all(|n| *n == w[0]),
            "行ごとに幅が違う: {w:?}"
        );
    }


    /// 稼働盤が、書き直さずに窓へ出せること。
    ///
    /// INDEXもタブバーもボールも draw() の中にある。HTMLで書き直すと
    /// 以後すべての変更を2回書くことになる (スマホ表示で一度そうなり、
    /// 片方だけ壊れた)。出力を変換すれば、足した機能は自動で付いてくる
    #[test]
    fn the_board_renders_into_the_window_without_rewriting_it() {
        use ratatui::backend::TestBackend;
        let mut b = crate::ball::Ball::default();
        b.throw(1, 2, 3, 1_000);
        let ui = test_ui(0, b, 3_000);
        let mut term = ratatui::Terminal::new(TestBackend::new(96, 26)).unwrap();
        term.draw(|f| draw(f, &[], &ui, None, &mut Vec::new())).unwrap();

        let html = crate::winmode::buffer_html(term.backend().buffer());

        // ワードマークも枠も、そのまま出ている
        assert!(html.contains("█"), "ワードマークが出ていない");
        assert!(html.contains("CHAIN"), "連鎖の表示が出ていない");
        // 色が付いている (灰色一色ではない)
        assert!(html.contains("color:#"), "色が付いていない");
        assert!(html.matches("<span").count() > 5, "まとまりが少なすぎる");
        // 26行ぶんある
        assert_eq!(html.lines().count(), 26, "行数が合わない");
        // 画面の中身がHTMLとして解釈されない
        assert!(!html.contains("<script>"), "生のタグが混ざっている");
    }

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
            ball: ball::Ball::default(),
            max_chain: 10,
            now_ms: 0,
            hover: None,
        };
        let mut term = ratatui::Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| draw(f, &tabs, &ui, None, &mut Vec::new())).unwrap();
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
        let screen = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents();
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


#[cfg(test)]
mod console_mode_tests {
    use std::process::Command;

    /// コンソールが無ければ None (CIやパイプ経由では取れない)
    fn input_mode() -> Option<u32> {
        super::console_input_mode()
    }

    /// 子プロセスを起動してもマウス入力が生き残ること。
    ///
    /// コンソールを継承した cmd.exe は ENABLE_MOUSE_INPUT を落とす (実測 0x1f7 -> 0x1e7)。
    /// これを踏むと設定画面を開いて戻った瞬間からマウスが効かなくなる。
    ///
    /// 注意: コンソールに繋がっていないと検証できないので、その場合は飛ばす。
    /// 手元で確認するには、コンソールのあるウィンドウでテストバイナリを直接実行する
    #[test]
    fn spawning_a_child_keeps_mouse_input_alive() {
        let Some(before) = input_mode() else {
            eprintln!("コンソールが無いので検証を省略");
            return;
        };

        // stdio は継承したまま試す。null にすると stdio 側だけで防げてしまい、
        // detach_console が壊れても気づけないテストになる
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "exit"]);
        let _ = super::detach_console(&mut cmd).spawn().unwrap().wait();

        assert_eq!(
            before,
            input_mode().unwrap(),
            "子プロセスの起動でコンソール入力モードが変わった (マウスが死ぬ)"
        );
    }
}



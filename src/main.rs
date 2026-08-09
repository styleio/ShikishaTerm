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

// 画面は自前の窓に描く。黒いコンソールは要らないので、Windowsに用意させない。
// (端末で使う --settings だけは、そのとき自分で1つ開く)
#![windows_subsystem = "windows"]

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
mod update;
mod watch;
mod ws;
mod webui;
mod wspack;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};


use detect::TabState;
use hooks::{Command, HookEngine, TabCtx};
use tab::{CopyState, Tab, extract_text};
use unicode_width::UnicodeWidthStr as _;

const TAB_BAR_MIN: u16 = 10;
const TAB_BAR_MAX: u16 = 40;
const STATUS_BAR_HEIGHT: u16 = 1;


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
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(config::logs_dir().join("crash.log"))
        {
            use std::io::Write as _;
            let _ = writeln!(f, "{where_}: {info}");
        }
        prev(info);
    }));
}

fn main() -> Result<()> {
    install_crash_log();
    // 旧配置 (ルート直下の config.json) を config フォルダへ移す (一度だけ)。
    // 読み込みより前に済ませないと、移行前の空設定で起動してしまう
    config::migrate_legacy_config();
    // WebView2 のユーザーデータ (Cookie・キャッシュ等) の置き場を設定から決める。
    // 既定はローカル (%LOCALAPPDATA%) — Drive同期フォルダに置くとキャッシュが
    // 延々と同期され通知や衝突を招くため。最初のWebViewが作られる前に指定する
    unsafe {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", config::browser_data_dir());
    }
    // 表示言語を決める (設定 → OS の順。翻訳が無ければ英語)
    i18n::init(
        config::load().and_then(|c| c.language).as_deref(),
        &[config_file_dir(), std::path::PathBuf::from(".")],
    );
    // 設定だけ開くモード (本体を起動せずブラウザで設定を編集する)
    if std::env::args().nth(1).as_deref() == Some("--settings") {
        // ここは文字で話すモードなので、話す場所を確保する
        open_console();
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
    // 画面中継の自己検証: ブラウザを開き、CDPのフレームを1枚保存して終わる。
    // 保存した画像を見れば、非表示のWebViewでもフレームが出るかが分かる
    if std::env::args().nth(1).as_deref() == Some("--cast-test") {
        open_console();
        let url = std::env::args().nth(2).unwrap_or_else(|| "https://example.com/".into());
        return cast_test(&url);
    }

    // 叩いたら窓が出る。ランチャを挟むのは、挟む理由があるときだけでいい
    run_in_window()
}

/// `--cast-test <url>`: ブラウザを開いて画面中継を始め、最初のフレームを
/// logs/cast-test.jpg に保存して終わる。人の目が要らない自己検証用
fn cast_test(url: &str) -> Result<()> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;

    println!("開いています: {url}");
    let browser = browser::Browser::spawn(url, "cast-test")?;
    browser.screencast(None, true)?;
    println!("中継開始。フレームを待っています (最長20秒)...");

    let status = config::logs_dir().join("cast-test.txt");
    // 最初のフレームは描画前で真っ白になりがち。数秒ためて「最後の1枚」を保存する
    let settle = Instant::now() + Duration::from_secs(5);
    let mut last: Option<(Vec<u8>, u32, u32)> = None;
    let mut count = 0u32;
    loop {
        for ev in browser.drain() {
            if let browser::Ev::Frame { data, w, h, .. } = ev {
                let bytes = b64
                    .decode(data.as_bytes())
                    .map_err(|e| anyhow::anyhow!("フレームのbase64が不正: {e}"))?;
                last = Some((bytes, w, h));
                count += 1;
            }
        }
        if Instant::now() >= settle {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    match last {
        Some((bytes, w, h)) => {
            let path = config::logs_dir().join("cast-test.jpg");
            std::fs::write(&path, &bytes)?;
            let msg = format!(
                "OK: {} ({}x{}, {} bytes, {} frames)\n",
                path.display(),
                w,
                h,
                bytes.len(),
                count
            );
            let _ = std::fs::write(&status, &msg);
            print!("保存: {msg}");
        }
        None => {
            let _ = std::fs::write(&status, "TIMEOUT: no frame in 5s\n");
            println!("フレームが来ませんでした (非表示WebViewでは中継が止まる可能性)");
        }
    }
    Ok(())
}

/// 画面の大きさ。幅と高さだけあればいい
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u16,
    pub height: u16,
}

/// 画面上の矩形
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// タブバー・枠線・ステータスバーを除いた、PTYに渡す端末サイズ (rows, cols)
fn pty_dims(size: Size, tab_w: u16) -> (u16, u16) {
    let cols = size.width.saturating_sub(tab_w + 2).max(10);
    let rows = size.height.saturating_sub(STATUS_BAR_HEIGHT + 2).max(3);
    (rows, cols)
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


/// マスターパスワードの設定・変更・解除 (INDEXメニュー [k])
fn manage_master_password(
    surface: &mut WinSurface,
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
        let Some(old) = surface.ask_password(&i18n::t("prompt.password.current"),
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
        let Some(new) = surface.ask_password(&i18n::t("prompt.password.new"),
            &i18n::t("prompt.password.new_note"),
        )? else {
            return Ok(i18n::t("msg.password.cancelled"));
        };
        if new.is_empty() {
            crypto::write_atomic(&path, &plain)?;
            *password = None;
            return Ok(i18n::t("msg.password.removed"));
        }
        let confirm = surface.ask_password(&i18n::t("prompt.password.confirm"), "")?;
        if confirm.as_deref() != Some(new.as_str()) {
            return Ok(i18n::t("msg.password.mismatch"));
        }
        crypto::write_atomic(&path, &serde_json::to_string_pretty(&crypto::encrypt(&plain, &new)?)?)?;
        *password = Some(new);
        Ok(i18n::t("msg.password.changed"))
    } else {
        // 新規設定
        let Some(new) = surface.ask_password(&i18n::t("prompt.password.set"),
            &i18n::t("prompt.password.set_note"),
        )? else {
            return Ok(i18n::t("msg.password.cancelled"));
        };
        if new.is_empty() {
            return Ok(i18n::t("msg.password.empty"));
        }
        let confirm = surface.ask_password(&i18n::t("prompt.password.confirm"), "")?;
        if confirm.as_deref() != Some(new.as_str()) {
            return Ok(i18n::t("msg.password.mismatch"));
        }
        crypto::encrypt_file(&path, &new)?;
        *password = Some(new);
        Ok(i18n::t("msg.password.encrypted"))
    }
}

/// 自前の窓へ描くときの持ち物
struct WinSurface {
    win: std::rc::Rc<crate::browser::Browser>,
    rows: u16,
    cols: u16,
    /// 直前に送った状態。変わったときだけ送る
    last: Option<crate::uistate::UiState>,
    last_screen: String,
    /// 中身の領域 (x, y, 幅, 高さ)。ブラウザを置く場所
    area: (i32, i32, i32, i32),
    /// 窓から来た意図を、ループが読む形に直したもの。
    /// ループは端末のキー操作しか知らないので、そこへ寄せる
    pending: std::collections::VecDeque<Event>,
    /// 置いたページの帯で、人が「終わった」を押したもの
    presses: Vec<String>,
    /// 読み込みが終わったページ (呼び名, URL, 参照先まで揃ったか)
    loads: Vec<(String, String, bool)>,
    /// ホイールで頼まれた遡り (正 = 過去へ)
    scrolls: Vec<(i32, u16, u16)>,
    /// 上のバーで頼まれた移動
    gos: Vec<crate::browser::Go>,
    /// 聞いておいた居場所の答え (窓の中での名前, URL, 戻れる, 進める)
    wheres: Vec<(String, String, bool, bool)>,
    /// ブラウザごとの「通信中か」。読み込み開始で真、終了で偽。
    /// 上のバーの進捗表示に使う (呼び名 = 見せているブラウザの鍵)
    loading: std::collections::HashMap<String, bool>,
    /// 中継画面のフレーム (JPEGのバイト列)。ループがスマホへ配る
    frames: Vec<Vec<u8>>,
    /// 窓が閉じた。描く先が無くなったので、ループは畳むしかない
    closed: bool,
}

impl WinSurface {
    /// 外から届いた操作を、窓の打鍵と同じ列に入れる。
    /// スマホから来ても、ループから見れば区別が無い
    fn inject(&mut self, ev: Event) {
        self.pending.push_back(ev);
    }

    /// 帯のボタンが押されたページの名前を引き取る。
    /// 窓の報告は1本しかないので、受けるのはここだけにする
    fn take_presses(&mut self) -> Vec<String> {
        std::mem::take(&mut self.presses)
    }

    /// 読み込みが終わったページを引き取る (呼び名, URL, 揃ったか)
    fn take_loads(&mut self) -> Vec<(String, String, bool)> {
        std::mem::take(&mut self.loads)
    }

    /// 上のバーで頼まれた移動を引き取る
    fn take_gos(&mut self) -> Vec<crate::browser::Go> {
        std::mem::take(&mut self.gos)
    }

    /// ホイールの合図を引き取る (目盛りの数, 指していた行, 桁)
    fn take_scrolls(&mut self) -> Vec<(i32, u16, u16)> {
        std::mem::take(&mut self.scrolls)
    }

    /// 居場所の答えを引き取る
    fn take_wheres(&mut self) -> Vec<(String, String, bool, bool)> {
        std::mem::take(&mut self.wheres)
    }
    /// 今そのブラウザが読み込み中か。分からなければ (静止していれば) 偽
    fn loading_of(&self, key: &str) -> bool {
        self.loading.get(key).copied().unwrap_or(false)
    }

    /// たまった中継フレームを引き取る (ループがスマホへ配る)
    fn take_frames(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.frames)
    }

    fn take_events(&mut self, active_tab: Option<&Tab>) {
        use crate::browser::Ev;
        for ev in self.win.drain() {
            match ev {
                Ev::Resize { rows, cols, area } => {
                    self.rows = rows;
                    self.cols = cols;
                    self.area = area;
                    self.pending.push_back(Event::Resize(cols, rows));
                }
                Ev::JsError { msg } => {
                    crate::append_hook_log(&format!("画面の失敗: {msg}"));
                }
                // 窓が閉じた。ここで畳まないと、描く先を失ったまま
                // 誰にも見えないプロセスが残り、待ち受けの口を握り続ける
                Ev::Closed => self.closed = true,
                // 上のバーが押された。宛先は「今見ているページ」なので
                // ループが決める (バーは1枚しか出ていない)
                Ev::Go { go } => self.gos.push(go),
                Ev::Scroll { by, row, col } => self.scrolls.push((by, row, col)),
                Ev::Where {
                    from: Some(name),
                    url,
                    can_back,
                    can_forward,
                } => self.wheres.push((name, url, can_back, can_forward)),
                // 置いたページの帯が押された = 人が自分の番を終えた。
                // 誰が押したかは、報告に付いてくる名前でしか分からない
                Ev::Button { from: Some(name) } => self.presses.push(name),
                // 置いたページの読み込みが終わった (移動のたびに来る)
                Ev::Ready {
                    from: Some(name),
                    url,
                    complete,
                } => self.loads.push((name, url, complete)),
                // ブラウザが読み込みを始めた/終えた。見せている側の進捗表示に使う
                Ev::Loading {
                    from: Some(name),
                    busy,
                } => {
                    self.loading.insert(name, busy);
                }
                // クリップボードは端末側と同じ扱いにする
                Ev::Copy { text } => {
                    if let Ok(mut c) = arboard::Clipboard::new() {
                        let _ = c.set_text(text);
                    }
                }
                Ev::Paste => {
                    if let Some(t) = active_tab {
                        let _ = paste_clipboard(t);
                    }
                }
                // 中継フレーム。base64をほどいてバイト列で溜め、ループがスマホへ配る
                Ev::Frame { data, .. } => {
                    use base64::Engine as _;
                    if let Ok(bytes) =
                        base64::engine::general_purpose::STANDARD.decode(data.as_bytes())
                    {
                        self.frames.push(bytes);
                    }
                }
                // 残りは打鍵に直せるもの。直し方は keys_for に1つだけ置く
                other => {
                    for e in keys_for(&other) {
                        self.pending.push_back(e);
                    }
                }
            }
        }
    }
}

/// 画面からの意図を、ループが既に知っている打鍵に直す。
///
/// 窓もスマホも同じページを使う。直し方が2か所にあると、
/// 同じ押下がどちらから来たかで別の意味になる日が来る。
/// 打鍵に直せない意図 (読み込み完了・大きさの変更など) は空を返す
fn keys_for(ev: &crate::browser::Ev) -> Vec<Event> {
    use crate::browser::Ev;
    let plain = |c: char| Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    let prefixed = |c: char| {
        vec![
            Event::Key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            plain(c),
        ]
    };
    match ev {
        // 「このタブを見たい」は Ctrl+B の数字と同じこと
        Ev::Select { tab } if *tab <= 9 => {
            prefixed(char::from_digit(*tab as u32, 10).unwrap_or('0'))
        }
        // タブバーの + は、どのタブを見ていても効くよう前置キー付きにする
        Ev::AddTab => prefixed('t'),
        // 盤面のメニューは INDEX を見ているときの素の打鍵。
        // 前置キーを付けると、同じ文字が両方にあるものだけが効く
        Ev::Menu { key } => key.chars().next().map(plain).map(|k| vec![k]).unwrap_or_default(),
        Ev::Stop => prefixed('x'),
        Ev::Key { text, named, ctrl } => {
            if let Some(n) = named {
                named_key(n)
                    .map(|code| vec![Event::Key(KeyEvent::new(code, KeyModifiers::NONE))])
                    .unwrap_or_default()
            } else if let Some(c) = ctrl.as_ref().and_then(|s| s.chars().next()) {
                vec![Event::Key(KeyEvent::new(
                    KeyCode::Char(c),
                    KeyModifiers::CONTROL,
                ))]
            } else if let Some(t) = text {
                t.chars().map(plain).collect()
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// 名前で送られてきた制御キーを、端末のキー種別に直す
fn named_key(n: &str) -> Option<KeyCode> {
    Some(match n {
        "enter" => KeyCode::Enter,
        "bs" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "esc" => KeyCode::Esc,
        "del" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "right" => KeyCode::Right,
        "left" => KeyCode::Left,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" => KeyCode::PageUp,
        "pgdn" => KeyCode::PageDown,
        _ => {
            let f = n.strip_prefix('f')?.parse::<u8>().ok()?;
            (1..=12).contains(&f).then_some(KeyCode::F(f))?
        }
    })
}

/// URLに載っている文字を戻す (%xx と +)。
/// 小さな用途なので、依存を増やさずここで解く
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(b[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 自前の窓を開いて、同じループをその上で回す
fn run_in_window() -> Result<()> {
    // 外皮を配る。file:// は wry のIPCで落ちるので、ローカルHTTPで出す
    let server = tiny_http::Server::http("127.0.0.1:0")
        .map_err(|e| anyhow::anyhow!("ローカルサーバーを開けません: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| anyhow::anyhow!("ポートが取れません"))?
        .port();
    let page = shell::page("");
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let url = req.url().to_string();
            // QRの絵は netaddr が作る。ページ側で作り直さない
            let (body, mime) = if url.starts_with("/qr.svg") {
                let want = url
                    .split_once("u=")
                    .map(|(_, v)| percent_decode(v))
                    .unwrap_or_default();
                (netaddr::qr_svg(&want, 6), "image/svg+xml")
            } else {
                (page.clone(), "text/html; charset=utf-8")
            };
            let r = tiny_http::Response::from_string(body).with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], mime.as_bytes())
                    .expect("header"),
            );
            let _ = req.respond(r);
        }
    });

    let win = std::rc::Rc::new(browser::Browser::spawn(
        &format!("http://127.0.0.1:{port}/"),
        "SHIKISHA-TERM",
    )?);
    run(WinSurface {
        win,
        rows: 40,
        cols: 120,
        last: None,
        last_screen: String::new(),
        area: (0, 0, 0, 0),
        pending: std::collections::VecDeque::new(),
        presses: Vec::new(),
        loads: Vec::new(),
        scrolls: Vec::new(),
        gos: Vec::new(),
        wheres: Vec::new(),
        loading: std::collections::HashMap::new(),
        frames: Vec::new(),
        closed: false,
    })
}

/// 今の状態を、見た目を持たない形にまとめる
fn ui_state_of(tabs: &[Tab], ui: &Ui, flash: Option<&str>) -> crate::uistate::UiState {
    crate::uistate::UiState {
        workspace: ui
            .ws_names
            .get(ui.ws_index)
            .cloned()
            .unwrap_or_default(),
        workspaces: ui.ws_names.clone(),
        ws_index: ui.ws_index,
        active: ui.active,
        auto_enabled: ui.auto.unwrap_or(true),
        remote_on: ui.remote_on,
        first_run: ui.first_run,
        // 設定に書いた順のまま並べる。
        // セッションとブラウザを分けて並べると、1番目に書いたブラウザが
        // 後ろに回る
        tabs: ui
            .panes
            .iter()
            .enumerate()
            .filter_map(|(i, p)| match p {
                Pane::Session(s) => tabs.get(*s).map(|t| crate::uistate::TabState::of(i + 1, t)),
                Pane::Browser { key, name } => {
                    Some(crate::uistate::TabState::browser(i + 1, key, name))
                }
            })
            .collect(),
        // ボールはセッションの番号で動く。見せるのは画面の番号
        ball: crate::uistate::BallState::of(&ui.ball, ui.max_chain, ui.now_ms),
        flash: flash.map(str::to_string),
        help_open: ui.help_open,
        ws_open: ui.ws_open,
        qr: ui.qr.clone(),
        nav: ui.nav.clone(),
        scrolled: ui.scrolled,
        build: format!("build {}  ({})", env!("BUILD_TIME"), env!("BUILD_REV")),
    }
}

impl WinSurface {
    fn size(&self) -> Result<Size> {
        Ok(Size { width: self.cols, height: self.rows })
    }

    /// 次の操作を待つ。窓からの意図は、ループが既に知っている
    /// キー操作に直して渡してある
    fn poll(&mut self, timeout: Duration, active_tab: Option<&Tab>) -> Result<Option<Event>> {
        self.take_events(active_tab);
        if self.closed {
            return Ok(None);
        }
        if let Some(e) = self.pending.pop_front() {
            return Ok(Some(e));
        }
        std::thread::sleep(timeout);
        Ok(None)
    }

    /// ブラウザを置く先。窓の中に置くと、位置も重なり順もOSが見てくれる
    fn host(&self) -> Option<(std::rc::Rc<crate::browser::Browser>, (i32, i32, i32, i32))> {
        Some((std::rc::Rc::clone(&self.win), self.area))
    }

    /// パスワードを聞く。スマホには出さない (ページ側が出さない)
    fn ask_password(&mut self, title: &str, note: &str) -> Result<Option<String>> {
        let _ = self.win.eval(&format!(
            "return window.__password({},{});",
            serde_json::to_string(title).unwrap_or_default(),
            serde_json::to_string(note).unwrap_or_default()
        ));
        // 人が入力し終えるまで待つ。急かす理由がない
        self.win.wait_password(Duration::from_secs(600))
    }

    fn draw(&mut self, tabs: &[Tab], ui: &Ui, flash: Option<&str>) -> Result<()> {
        {
            {
                let w = &mut *self;
                let state = ui_state_of(tabs, ui, flash);
                if w.last.as_ref() != Some(&state) {
                    let json = serde_json::to_string(&state).unwrap_or_default();
                    if w.last.is_none() {
                        crate::append_hook_log(&format!(
                            "状態を送る: タブ{} 「{}」 {}文字",
                            state.tabs.len(),
                            state.workspace,
                            json.len()
                        ));
                    }
                    let _ = w.win.eval(&format!(
                        "return window.__state({});",
                        serde_json::to_string(&json).unwrap_or_default()
                    ));
                    w.last = Some(state);
                }
                // ターミナルの中身は、見ているタブのぶんだけ送る
                if let Some(t) = session_at(&ui.panes, ui.active).and_then(|i| tabs.get(i)) {
                    let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                    let s = p.screen();
                    let html = crate::shell::screen_html(s);
                    if html != w.last_screen {
                        w.last_screen = html.clone();
                        let _ = w.win.eval(&format!(
                            "return window.__screen({});",
                            serde_json::to_string(&html).unwrap_or_default()
                        ));
                    }
                    let (r, c) = s.cursor_position();
                    let on = !s.hide_cursor();
                    let _ = w
                        .win
                        .eval(&format!("return window.__cursor({r},{c},{on});"));
                }
                Ok(())
            }
        }
    }
}

fn run(mut surface: WinSurface) -> Result<()> {
    // モード指定は起動するコマンドではない。
    // 外すのを忘れると `--window` という名前のプログラムを探しに行く
    let cmd_args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !matches!(a.as_str(), "--settings"))
        .collect();
    let start = Instant::now();
    // 幅はconfig指定 → 無ければタブ名から自動算出 (タブ起動後に確定)
    let mut tab_w = 18u16;
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
    } else if let Some(w) = workspaces.get(ws_index) {
        // 前回の続きから始めるなら、起動するのもそのワークスペース。
        // ここを先頭に決め打つと、名前だけ復元されて中身が違う画面になる
        spawn_workspace(w, rows, cols, &mut tabs, &mut startup_errors);
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
                match surface.ask_password(&i18n::t("prompt.password.title"), &note)? {
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
    let caps: hooks::Caps = std::rc::Rc::new(match cfg.as_ref() {
        Some(c) => caps::Capabilities::new(
            c.capabilities.clone(),
            config_file_dir(),
            c.resolve_tokens(password.as_deref()),
        ),
        None => caps::Capabilities::disabled(),
    });
    let mut engines: Vec<Option<HookEngine>> = (0..workspaces.len().max(1)).map(|_| None).collect();
    // 窓を持っているなら、ブラウザはその中に置く
    caps.set_host(surface.host());
    caps.set_workspace(ws_index);
    if let Some(w) = workspaces.get(ws_index) {
        engines[ws_index] = build_engine(cfg.as_ref(), Some(w), &mut startup_errors, &caps);
        open_declared_browsers(w, &caps, &mut startup_errors);
    } else {
        engines[0] = build_engine(cfg.as_ref(), None, &mut startup_errors, &caps);
    }
    let slot = ws_index.min(engines.len().saturating_sub(1));
    let mut engine = engines[slot].take();

    // リモートUI (スマホ等から監視・指示する)。設定で有効にしたときだけ待ち受ける。
    // 状況は設定画面にも渡し、QRコードをブラウザで見られるようにする
    let remote_info: Arc<Mutex<webui::RemoteInfo>> = Arc::new(Mutex::new(Default::default()));
    let mut remote_ui = start_remote(cfg.as_ref(), password.as_deref(), &mut startup_errors);
    publish_remote(&remote_info, &remote_ui);

    // 今どこへ焦点を移してあるか。None = まだ一度も移していない
    let mut focused: Option<Option<String>> = None;

    // 見ているページの居場所 (窓の中での名前, URL, 戻れるか, 進めるか)。
    // 窓しか知らないので、聞いて控える
    let mut where_now: Option<(String, String, bool, bool)> = None;
    let mut asked_where_ms: u64 = 0;

    let mut auto_enabled = true;
    let mut started_fired = vec![false; tabs.len()];
    // 自動チェーンの「透明のボール」。今どのタブが仕事を持っているかを表示に使う
    let mut ball = ball::Ball::default();
    // 相手がまだ受け取れない受け渡しを預かる場所
    let mut waiting: Vec<Waiting> = Vec::new();
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

    // 0 = INDEX、1.. = セッション。初回はINDEX(案内のある画面)から始める
    let mut active: usize = if tabs.is_empty() || first_run { 0 } else { 1 };
    let mut prefix_active = false;
    // 直前に描いた状態。スマホへはこれを渡す (組み立てる場所を1つに保つ)
    let mut last_ui_state: Option<crate::uistate::UiState> = None;
    // 重ねたブラウザを今見せているか。出しっぱなしだと
    // ターミナルがずっと隠れてしまうので、既定は隠す
    let mut flash: Option<String> = startup_errors.first().map(|e| i18n::tp("msg.startup_failed", &[("error", e)]));
    let mut last_detect = Instant::now() - Duration::from_secs(1);
    // いま画面中継しているブラウザ (見ている人がいる間だけ流す)
    let mut casting: Option<String> = None;
    // ワークスペースは仮想デスクトップ方式: 切替=非表示であって停止ではない。
    // 各ワークスペースのタブ群を保持し、初回アクティブ化時に起動する
    // 起動した分は tabs が持っている。棚は残りのワークスペースの数だけ空けておく
    let mut ws_tabs: Vec<Vec<Tab>> = Vec::new();
    ws_tabs.resize_with(workspaces.len(), Vec::new);
    // 設定ファイルの変更監視 (保存したら再起動なしで反映する)
    let mut watcher = watch::Watcher::new(watch::watch_targets(cfg.as_ref(), &config::config_file_path()));
    // 新しい版が出ていないか裏で一度だけ確かめる (知らせるだけで、更新はしない)
    let update_rx = update::spawn_check();
    let mut cfg = cfg;

    let mut ws_open = false;
    let mut help_open = false;
    let mut qr_open = false;
    // タブバー境界線のドラッグ中フラグ (マウスで幅を調整できる)
    // 設定Web GUI (INDEXの [e] で起動、アプリ終了時に停止)
    let mut web: Option<webui::WebUi> = None;
    let config_file = config::config_file_path();

    loop {
        // 画面に並ぶもの。設定に書いた順。
        // 押せる番号の上限は、セッションの数だけでは足りない
        let hosted = caps.hosted_names();
        let titles: Vec<&str> = tabs.iter().map(|t| t.title.as_str()).collect();
        let layout = panes_of(workspaces.get(ws_index), &titles, &hosted);
        let panes = layout.len();
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
                    // ブラウザも設定に揃える。足したものは開き、消したものは閉じ、
                    // バーと帯は出し直す。開き直さないと反映されない、では
                    // 設定を触った意味がない (既に開いてあるページには触らない)
                    open_declared_browsers(w, &caps, &mut startup_errors);
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
                // 設定から来るものだけを差し替える。作り直すと、窓の中に
                // 置いたページを誰も知らなくなり、消せないまま画面に残る
                // (設定を保存した瞬間に設定画面が居座り、タブが効かなくなった)
                caps.set_config(
                    newcfg.capabilities.clone(),
                    newcfg.resolve_tokens(password.as_deref()),
                );
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

        // 更新の知らせ。他の表示を潰さないよう、画面が空いたときに出す
        if flash.is_none() {
            if let Ok(v) = update_rx.try_recv() {
                flash = Some(i18n::tp("msg.update_available", &[("version", &v)]));
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
                        eng.cancel_tab(pane_at(&layout, idx));
                    }
                }
                let now_ms = start.elapsed().as_millis() as u64;
                if auto_enabled {
                    for (i, fired) in started_fired.iter_mut().enumerate() {
                        // 起動直後に送ると、AI CLIが入力欄を描く前なので捨てられる。
                        // 準備できるまで待ってから流し込む
                        if !*fired && tabs[i].ready_for_startup_hook() {
                            *fired = true;
                            eng.fire(
                                "on_start",
                                &tab_ctx(&tabs[i], pane_at(&layout, i + 1)),
                                None,
                            );
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
                        let ctx = tab_ctx(&tabs[idx - 1], pane_at(&layout, idx));
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
                        let ctx = tab_ctx(&tabs[idx - 1], pane_at(&layout, idx));
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

                    // 自動化が指すのは画面の番号。中身はセッションが持っている
                    eng.tick_pending(&|pane| {
                        session_at(&layout, pane)
                            .and_then(|i| tabs.get(i))
                            .map(|t| t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().contents())
                    });
                }
                let cmds = eng.drain_commands();
                if !cmds.is_empty() {
                    let now_ms = start.elapsed().as_millis() as u64;
                    exec_commands(
                        cmds,
                        &mut tabs,
                        &layout,
                        max_chain,
                        auto_enabled,
                        now_ms,
                        rows,
                        cols,
                        &notifier,
                        &mut flash,
                        &mut ball,
                        &mut pending_submit,
                        &mut waiting,
                    );
                }
            }

            // リモートUIへ現在の状況を渡し、届いた操作を実行する
            if let Some(r) = remote_ui.as_ref() {
                *r.snapshot.lock().unwrap() = remote::Snapshot {
                    // 描画のときに作ったものを渡す。ここでは ui がまだ無く、
                    // 作り直すと「状態を組み立てる場所」が2つになる
                    ui: last_ui_state.clone(),
                    screen_html: tabs
                        .get(session_at(&layout, active).unwrap_or(usize::MAX))
                        .map(|t| {
                            let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
                            shell::screen_html(p.screen())
                        })
                        .unwrap_or_default(),
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

        // リモート操作とフレーム配信は毎イテレーション処理する (200ms待つと
        // 指の軌跡が固まって届き、スワイプの再現が壊れる)
        if let Some(r) = remote_ui.as_ref() {
            let now_ms = start.elapsed().as_millis() as u64;
            // いま見ているブラウザ (Injectの宛先・中継の対象)
            let shown_browser = match layout.get(active.wrapping_sub(1)) {
                Some(Pane::Browser { key, .. }) => Some(key.clone()),
                _ => None,
            };
            while let Ok(cmd) = r.rx.try_recv() {
                match cmd {
                    // 遠隔からの入力は人間の操作として扱う
                    // (自動チェーンをリセットし、ロック中は拒否する)
                    remote::RemoteCmd::Send { tab, text } => {
                        if let Some(t) = session_at(&layout, tab).and_then(|i| tabs.get_mut(i)) {
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
                        if let Some(t) = session_at(&layout, tab).and_then(|i| tabs.get_mut(i)) {
                            if t.locked {
                                continue;
                            }
                            t.chain_depth = 0;
                            t.last_manual_ms = Some(now_ms);
                            let _ = t.write_bytes(keys.as_bytes());
                        }
                    }
                    // 中継画面への入力は、見ているブラウザへ本物の入力として注入する
                    remote::RemoteCmd::Ui(crate::browser::Ev::Inject { input, .. }) => {
                        if let Some(key) = &shown_browser {
                            let _ = caps.browser_inject(key, input);
                        }
                    }
                    // その他の画面操作は、窓から来たものと同じ打鍵に直す
                    remote::RemoteCmd::Ui(ev) => {
                        for e in keys_for(&ev) {
                            surface.inject(e);
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
            // 溜まった中継フレームは最新の1枚だけ配る (古いものは捨てる)。
            // 送り手が速いときに回線とスマホを溢れさせない。常に一番新しい絵を見せる
            if let Some(jpeg) = surface.take_frames().pop() {
                r.push_frame(jpeg);
            }
            // 見ているブラウザに視聴者がいれば中継、いなければ止める
            let want = if r.has_frame_clients() {
                shown_browser
            } else {
                None
            };
            if want != casting {
                if let Some(old) = &casting {
                    let _ = caps.browser_screencast(old, false);
                }
                if let Some(new) = &want {
                    let _ = caps.browser_screencast(new, true);
                }
                casting = want;
            } else if let Some(key) = &casting {
                // 対象は同じでも、新しい視聴者が入ったら今の画面を1枚出す。
                // 静止したページだと、変化待ちのままいつまでも空になるため
                if r.take_keyframe_request() {
                    let _ = caps.browser_screencast(key, true);
                }
            }
        }

        // 預かっている受け渡しを、相手が受け取れるようになったら流す。
        // 諦めた分も黙って消さない。消えたことが見えないのが一番困る
        if !waiting.is_empty() {
            let now_ms = start.elapsed().as_millis() as u64;
            let keys = pane_keys(&layout, &tabs);
            let mut ready: Vec<Command> = Vec::new();
            let mut keep: Vec<Waiting> = Vec::new();
            for w in std::mem::take(&mut waiting) {
                let can = target_of(&w.cmd)
                    .and_then(|r| r.resolve(&keys))
                    .and_then(|p| session_at(&layout, p))
                    .and_then(|i| tabs.get(i))
                    .map(ready_to_receive)
                    .unwrap_or(false);
                if can {
                    ready.push(w.cmd);
                } else if now_ms >= w.give_up_ms {
                    let to = target_of(&w.cmd);
                    append_hook_log(&format!("受け取れないまま時間切れ: {to:?}"));
                    flash = Some(i18n::tp(
                        "msg.handoff_timeout",
                        &[("target", &format!("{to:?}"))],
                    ));
                } else {
                    keep.push(w);
                }
            }
            waiting = keep;
            if !ready.is_empty() {
                exec_commands(
                    ready,
                    &mut tabs,
                    &layout,
                    max_chain,
                    auto_enabled,
                    now_ms,
                    rows,
                    cols,
                    &notifier,
                    &mut flash,
                    &mut ball,
                    &mut pending_submit,
                    &mut waiting,
                );
            }
        }

        // 予約しておいた実行(改行)を、相手が貼り付けを描いてから送る
        if !pending_submit.is_empty() {
            let now_ms = start.elapsed().as_millis() as u64;
            pending_submit.retain_mut(|p| {
                let Some(t) = session_at(&layout, p.tab).and_then(|i| tabs.get(i)) else {
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

        // ボールが渡った先へ画面を移す。
        // 人が画面を触った直後は従わない (読んでいる最中に飛ばされないように)
        {
            let now_ms = start.elapsed().as_millis() as u64;
            if let Some(to) = follow_target(
                follow_ball,
                ball.holder,
                followed,
                // 数えるのは画面に並んでいる数。セッションの数で数えると、
                // ブラウザを挟んだ分だけ後ろのタブが「無い番号」に見え、
                // そこへ渡ったボールに永久に追従しない
                layout.len(),
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
            && !session_at(&layout, ball.holder)
                .and_then(|i| tabs.get(i))
                .map(|t| t.chain_depth > 0)
                .unwrap_or(false)
        {
            ball.reset();
        }
        ball.clamp_to(layout.len());

        // 見ているブラウザの上に出す操作。
        //
        // 出す・出さないは設定かLuaが決め、押せる・押せないは窓が答える。
        // 答えは遅れて届くので、届くまでは押せない顔で出しておく
        let drawn_ms = start.elapsed().as_millis() as u64;
        let showing = match layout.get(active.wrapping_sub(1)) {
            Some(Pane::Browser { key, .. }) => Some(key.clone()),
            _ => None,
        };
        let nav = showing.as_deref().and_then(|key| {
            let spec = caps.nav_of(key)?;
            let w = where_now.as_ref().filter(|w| w.0 == key);
            Some(crate::uistate::NavState {
                back: spec.back,
                forward: spec.forward,
                reload: spec.reload,
                edit: spec.url,
                can_back: w.is_some_and(|w| w.2),
                can_forward: w.is_some_and(|w| w.3),
                at: w.map(|w| w.1.clone()).unwrap_or_default(),
                loading: surface.loading_of(key),
            })
        });
        // 居場所は窓しか知らない。出しているときだけ、ほどよい間隔で聞く。
        // 履歴から戻ったページは読み込みを名乗らないことがあるので、
        // 「読み込んだら聞く」だけでは戻るボタンが古いままになる
        if let (Some(key), true) = (
            &showing,
            nav.is_some() && drawn_ms.saturating_sub(asked_where_ms) >= WHERE_EVERY_MS,
        ) {
            asked_where_ms = drawn_ms;
            let _ = caps.browser_where(key);
        }

        let ui = Ui {
            first_run,
            active,
            auto: engine.as_ref().map(|_| auto_enabled),
            ws_names: workspaces.iter().map(|w| w.name.clone()).collect(),
            ws_index,
            ws_open,
            help_open,
            qr: if qr_open { remote_ui.as_ref().map(|r| r.url.clone()) } else { None },
            remote_on: remote_ui.is_some(),
            nav,
            scrolled: session_at(&layout, active)
                .and_then(|i| tabs.get(i))
                .map(|t| {
                    t.parser
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .screen()
                        .scrollback()
                })
                .unwrap_or(0),
            ball,
            max_chain,
            now_ms: start.elapsed().as_millis() as u64,
            panes: layout.clone(),
        };
        last_ui_state = Some(ui_state_of(&tabs, &ui, flash.as_deref()));
        surface.draw(&tabs, &ui, flash.as_deref())?;
        // 窓の大きさは変わる。渡し直さないと、置いたページは前の大きさのまま
        caps.set_area(surface.area);
        // 選ばれている1枚だけを、ターミナルの中身の位置に置く。
        // 所有関係で最小化と重なり順はOSが見てくれるが、位置だけは追う必要がある
        // キーボードの焦点を、見えているものへ移す。
        //
        // ページの中の焦点 (activeElement) と、OSが見ている焦点は別物。
        // 窓ができた直後はOS側が定まっておらず、打鍵は届くのに
        // 日本語の変換窓だけが画面の隅に出た (窓を少しでも動かすと直る、が目印)。
        // 見ているものが変わるたびに、こちらから移し直す
        {
            let want = match layout.get(active.wrapping_sub(1)) {
                Some(Pane::Browser { key, .. }) => Some(key.clone()),
                _ => None,
            };
            if focused.as_ref() != Some(&want) {
                focused = Some(want.clone());
                match &want {
                    Some(name) => {
                        let _ = caps.browser_focus(name);
                    }
                    None => {
                        let _ = surface.win.focus(None);
                    }
                }
            }
        }
        caps.show_only(match layout.get(active.wrapping_sub(1)) {
            Some(Pane::Browser { key, .. }) => Some(key.as_str()),
            _ => None,
        });
        // 帯のボタンが押されたことを預ける。
        // 窓の報告を受けられるのは本体だけなので、ここを通す
        for child in surface.take_presses() {
            caps.note_press(&child);
            // 窓の中での名前を呼び名へ戻す。戻せないのは別のワークスペースの分
            let Some(name) = caps.name_of_child(&child) else {
                continue;
            };
            append_hook_log(&format!("帯を押した {name}"));
            if !auto_enabled {
                flash = Some(i18n::t("msg.press_auto_off"));
                continue;
            }
            let Some((eng, page)) = engine
                .as_mut()
                .zip(page_ctx(&layout, &name, String::new(), true))
            else {
                continue;
            };
            // 受ける先が無いのに押せる顔で出ていると、壊れたようにしか見えない
            if !eng.has_page_hook("on_press", page.index) {
                flash = Some(i18n::tp("msg.press_nowhere", &[("name", &page.name)]));
                append_hook_log("on_press が書かれていないので何もしない");
                continue;
            }
            eng.fire_page("on_press", &page);
        }
        // ホイールを回した。見えているタブだけが動く
        for (by, row, col) in surface.take_scrolls() {
            if by == 0 {
                continue;
            }
            if let Some(t) = session_at(&layout, active).and_then(|i| tabs.get(i)) {
                scroll_by(t, by, row, col);
                // 遡っている間に画面が飛ぶと、読んでいるものを見失う
                view_touched_ms = start.elapsed().as_millis() as u64;
            }
        }

        // 上のバーが押された。宛先は今見ているページ (バーは1枚しか出ていない)。
        // 連鎖の深さには触らない。数えるのは他のタブへ渡ったときだけ
        for go in surface.take_gos() {
            let Some(Pane::Browser { key, .. }) = layout.get(active.wrapping_sub(1)) else {
                continue;
            };
            // 出していない操作は受け付けない。画面に無いものが効くのはおかしい
            let Some(spec) = caps.nav_of(key) else {
                continue;
            };
            use crate::browser::Go;
            let allowed = match &go {
                Go::Back => spec.back,
                Go::Forward => spec.forward,
                Go::Reload => spec.reload,
                Go::To(_) => spec.url,
            };
            if !allowed {
                continue;
            }
            // 人が打った文字は、開いてよい行き先かを見てから渡す
            let go = match go {
                Go::To(raw) => match crate::browser::openable(&raw) {
                    Some(u) => Go::To(u),
                    None => {
                        flash = Some(i18n::tp("msg.nav.bad_url", &[("url", raw.trim())]));
                        continue;
                    }
                },
                other => other,
            };
            append_hook_log(&format!("移動 {key}: {go:?}"));
            let _ = caps.browser_go(key, go);
            // 動いた直後は居場所が変わる。次の描画で聞き直させる
            asked_where_ms = 0;
        }
        // 答えは窓の中での名前で返る。人の呼び名に戻してから控える
        for (child, url, can_back, can_forward) in surface.take_wheres() {
            if let Some(name) = caps.name_of_child(&child) {
                where_now = Some((name, url, can_back, can_forward));
            }
        }

        // 読み込みが終わったページ。移動のたびに来る
        for (child, url, complete) in surface.take_loads() {
            let Some(name) = caps.name_of_child(&child) else {
                continue;
            };
            append_hook_log(&format!(
                "読み込み {name}: {url} ({})",
                if complete { "全部" } else { "DOMまで" }
            ));
            if auto_enabled {
                if let (Some(eng), Some(page)) =
                    (engine.as_mut(), page_ctx(&layout, &name, url, complete))
                {
                    eng.fire_page("on_load", &page);
                }
            }
        }

        let polled = surface.poll(
            Duration::from_millis(16),
            session_at(&layout, active).and_then(|i| tabs.get(i)),
        )?;
        // 窓が無くなったら、Ctrl+B q と同じところへ落ちる。
        // 片付けは1か所だけにしておきたい
        if surface.closed {
            break;
        }
        let Some(ev) = polled else {
            continue;
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
                            if n <= panes {
                                active = n;
                                view_touched_ms = start.elapsed().as_millis() as u64;
                            }
                        }
                        KeyCode::Char('n') => {
                            active = if active >= panes { 0 } else { active + 1 };
                            view_touched_ms = start.elapsed().as_millis() as u64;
                        }
                        KeyCode::Char('p') => {
                            active = if active == 0 { panes } else { active - 1 };
                            view_touched_ms = start.elapsed().as_millis() as u64;
                        }
                        // Ctrl+B b で子プロセスに素のCtrl+Bを送る
                        KeyCode::Char('b') => {
                            if let Some(t) = session_mut(&mut tabs, &layout, active) {
                                t.write_bytes(&[0x02])?;
                            }
                        }
                        // Ctrl+B r このタブを再起動 (終了・切断からの復帰)
                        KeyCode::Char('r') => {
                            if let Some(eng) = engine.as_mut() {
                                eng.cancel_tab(active);
                            }
                            if let Some(t) = session_mut(&mut tabs, &layout, active) {
                                flash = Some(match t.restart(rows, cols) {
                                    Ok(()) => i18n::tp("msg.restarted", &[("name", &t.title)]),
                                    Err(e) => i18n::tp("msg.restart_failed", &[("error", &e.to_string())]),
                                });
                            }
                        }
                        // Ctrl+B l 入力ロック切替 / w ワークスペース一覧 / ? ヘルプ
                        KeyCode::Char('l') => {
                            if let Some(t) = session_mut(&mut tabs, &layout, active) {
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
                        // Ctrl+B t 設定画面を「タブ追加」の状態で開く (タブバーの + が送る)。
                        // nonce を変えないと、2度目の押下で同じURLに戻れず何も起きない
                        KeyCode::Char('t') => {
                            let query = format!(
                                "&addtab={ws_index}&nonce={}",
                                start.elapsed().as_millis()
                            );
                            flash = Some(
                                match open_settings(
                                    &mut web,
                                    &config_file,
                                    &remote_info,
                                    &caps,
                                    &query,
                                ) {
                                    Ok(()) => {
                                        active = layout.len() + 1;
                                        i18n::t("msg.settings_here")
                                    }
                                    Err(e) => i18n::tp(
                                        "msg.settings_failed",
                                        &[("error", &e.to_string())],
                                    ),
                                },
                            );
                        }
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
                            if let Some(t) = session_mut(&mut tabs, &layout, active) {
                                flash = Some(match &t.last_response {
                                    Some(r) if !r.trim().is_empty() => copy_to_clipboard(r),
                                    _ => i18n::t("msg.no_response"),
                                });
                            }
                        }
                        // Ctrl+B [ でコピーモード (tmuxのコピーモード風)
                        KeyCode::Char('[') => {
                            let rows = pty_dims(surface.size()?, tab_w).0;
                            if let Some(t) = session_mut(&mut tabs, &layout, active) {
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
                    // INDEX = ホーム画面: 数字でタブ切替、英字でメニュー実行。
                    // ここで受ける文字は MENU_KEYS と揃っている必要がある
                    // (盤面が出しているのに、押しても何も起きない、を防ぐ)
                    match key.code {
                        KeyCode::Char(c @ '0'..='9') => {
                            let n = c as usize - '0' as usize;
                            if n <= panes {
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
                            flash = Some(manage_master_password(
                                &mut surface,
                                cfg.as_ref(),
                                &mut password,
                            )?);
                        }
                        // 設定: 自分の窓の中で開く。
                        // 外のブラウザに投げると、どの窓が誰のものか分からなくなる
                        KeyCode::Char('e') => {
                            flash = Some(
                                match open_settings(&mut web, &config_file, &remote_info, &caps, "")
                                {
                                    Ok(()) => {
                                        // 開いたら、そのタブへ移る。
                                        // 出したのに見えていない状態を作らない
                                        // 並びは次の描画で組み直される。
                                        // ここでは今の並びの後ろに付く分を指す
                                        active = layout.len() + 1;
                                        i18n::t("msg.settings_here")
                                    }
                                    Err(e) => i18n::tp(
                                        "msg.settings_failed",
                                        &[("error", &e.to_string())],
                                    ),
                                },
                            );
                        }
                        KeyCode::Char('q') => break,
                        _ => {}
                    }
                    // INDEX-ここまで (盤面が出すキーを、ここが受けているか試験が見る)
                } else {
                    let size = surface.size()?;
                    let now_ms = start.elapsed().as_millis() as u64;
                    let mut locked_hit = false;
                    if let Some(t) = session_mut(&mut tabs, &layout, active) {
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
                            // 打った文字は一番下に出る。遡ったままだと見えない
                            to_live(t);
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
                if let Some(t) = session_mut(&mut tabs, &layout, active) {
                    if !t.locked {
                        t.chain_depth = 0;
                        t.last_manual_ms = Some(now_ms);
                        to_live(t);
                        t.write_bytes(text.as_bytes())?;
                    }
                }
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

/// 子プロセスに窓を出させない。
///
/// cmd.exe のようなコンソールのアプリは、黙って起動すると黒い窓を出す。
/// ブラウザを開くたびに一瞬ちらつくので、最初から出させない
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

/// 設定画面を窓の中に置くときの名前。
/// 綴りがずれると別のブラウザとして扱われ、2枚目が開く
const SETTINGS_TAB: &str = "settings";

/// ホイール1目盛りぶんの合図を、端末の作法で書き出す。
///
/// 全画面のプログラムは自分の中身を自分で巻き戻すので、こちらが
/// 履歴を持っていても意味がない。回したことを伝えるのが正しい。
/// 番号は決まりごと: 64 が上、65 が下
fn wheel_bytes(up: bool, row: u16, col: u16, enc: vt100::MouseProtocolEncoding) -> Vec<u8> {
    let button = if up { 64 } else { 65 };
    // 画面の左上は 1,1 (0始まりではない)
    let (x, y) = (col.saturating_add(1), row.saturating_add(1));
    match enc {
        vt100::MouseProtocolEncoding::Sgr => {
            format!("\x1b[<{button};{x};{y}M").into_bytes()
        }
        vt100::MouseProtocolEncoding::Utf8 => {
            let mut out = b"\x1b[M".to_vec();
            for v in [button + 32, x + 32, y + 32] {
                let mut buf = [0u8; 4];
                out.extend_from_slice(
                    char::from_u32(v as u32).unwrap_or(' ').encode_utf8(&mut buf).as_bytes(),
                );
            }
            out
        }
        // 昔の書き方は1バイトずつ。223 より先は表せない
        _ => {
            let b = |v: u16| (v.min(223) as u8).saturating_add(32);
            vec![0x1b, b'[', b'M', b(button), b(x), b(y)]
        }
    }
}

/// 遡った先。正が過去。0 より手前 (未来) は無い
fn scrolled_to(cur: usize, by: i32) -> usize {
    if by > 0 {
        cur.saturating_add(by as usize)
    } else {
        cur.saturating_sub(by.unsigned_abs() as usize)
    }
}

/// ホイールを回した。
///
/// 相手がマウスを見ているなら、回したことをそのまま渡す。全画面の
/// プログラムは自分の中身を自分で巻き戻すので、こちらの履歴には何も無い。
/// 見ていないなら (素のシェル等)、こちらが持っている履歴を遡る。
/// `by` は目盛りの数で、正が過去
fn scroll_by(t: &Tab, by: i32, row: u16, col: u16) {
    let (wants_mouse, enc) = {
        let p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
        let s = p.screen();
        (
            s.mouse_protocol_mode() != vt100::MouseProtocolMode::None,
            s.mouse_protocol_encoding(),
        )
    };
    if wants_mouse {
        let mut bytes = Vec::new();
        for _ in 0..by.unsigned_abs().min(16) {
            bytes.extend_from_slice(&wheel_bytes(by > 0, row, col, enc));
        }
        let _ = t.write_bytes(&bytes);
        return;
    }
    // 1目盛りで3行。端末の作法に合わせる
    let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
    let next = scrolled_to(p.screen().scrollback(), by.saturating_mul(3));
    p.screen_mut().set_scrollback(next);
}

/// 今の画面へ戻す。
///
/// 打った文字は画面の一番下に出る。遡ったまま打つと、
/// 自分が打っているところが見えない
fn to_live(t: &Tab) {
    let mut p = t.parser.lock().unwrap_or_else(|e| e.into_inner());
    if p.screen().scrollback() != 0 {
        p.screen_mut().set_scrollback(0);
    }
}

/// 「今どこに居るか」を窓に聞く間隔。
///
/// 押せる・押せないの見た目がこれだけ遅れる。毎フレーム聞くほどの
/// ものではなく、人が気づくほど遅れてもいけない
const WHERE_EVERY_MS: u64 = 400;

fn open_browser(url: &str) {
    // cmd の start はURL内の & を分割してしまうため、空タイトル引数の後に渡す
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/c", "start", "", url])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = detach_console(&mut cmd).spawn();
}

/// 文字で話す場所を用意する。
///
/// この実行ファイルは窓のアプリなので、Windowsはコンソールを付けてくれない。
/// 呼んだ相手が端末なら、そこへお邪魔する。そうでなければ自分で1つ開く。
/// 既に繋がっているなら何もしない (どちらも失敗するだけで済む)
fn open_console() {
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole,
    };
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            AllocConsole();
        }
    }
}

/// Luaフックを3階層 (基本 > ワークスペース > タブ) で読み込む。
/// フックの引き当ては「より具体的な方が勝つ」ので、タブ用スクリプトが
/// 定義していないフックだけがワークスペース・基本へフォールバックする
/// タブごとの自動化を、画面の番号で並べる。
///
/// 番号は画面に出ているものと同じにする。人が押す番号、スクリプトが
/// 指す番号、ボールが飛ぶ番号が別々だと、誰にも追えなくなる。
///
/// 並べ替えられても、設定を読み直せば付け直される。
/// どこにも覚えさせないので、ずれたままにならない
fn automation_by_pane(ws: &config::Workspace) -> Vec<(usize, String)> {
    let mut pane = 0;
    let mut out = Vec::new();
    for t in &ws.tabs {
        // コマンドが空の行は、画面にも並ばない
        if t.cfg.command.argv().is_empty() {
            continue;
        }
        pane += 1;
        if let Some(p) = t.cfg.automation_path() {
            out.push((pane, p));
        }
    }
    out
}

fn build_engine(
    cfg: Option<&config::Config>,
    ws: Option<&config::Workspace>,
    errors: &mut Vec<String>,
    caps: &hooks::Caps,
) -> Option<HookEngine> {
    let base = cfg.and_then(|c| c.automation_path());
    let ws_lua = ws.and_then(|w| w.automation.clone());
    let tab_luas: Vec<(usize, String)> = ws.map(automation_by_pane).unwrap_or_default();
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
    // 既に開いてあるものは触らない。開き直すと読み込みからやり直しになり、
    // 設定を保存するたびに見ていたページが消える
    let open_now = caps.hosted_names();
    let already = |name: &str| open_now.iter().any(|n| n == name);
    for b in &ws.browsers {
        if already(&b.id) {
            caps.note_declared(&b.id);
            continue;
        }
        match caps.browser_open(&b.id, &b.url) {
            Ok(()) => caps.note_declared(&b.id),
            Err(e) => errors.push(format!("ブラウザ {}: {e:#}", b.id)),
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
        if !already(&name) {
            if let Err(e) = caps.browser_open(&name, &url) {
                errors.push(format!("ブラウザ {name}: {e:#}"));
                continue;
            }
        }
        caps.note_declared(&name);
    }
    apply_browser_chrome(ws, caps);
}

/// ページの上のバーと下の帯を、設定のとおりに出し直す。
///
/// 開いたときだけでなく、設定を読み直したときにも通る。
/// ここを通さないと、チェックを入れても再起動するまで出てこない
fn apply_browser_chrome(ws: &config::Workspace, caps: &hooks::Caps) {
    // 設定から消えたブラウザは閉じる。置いたままにすると、
    // 「設定に無いページ」として一覧の後ろに現れ直す
    let declared: Vec<String> = ws
        .browsers
        .iter()
        .map(|b| b.id.clone())
        .chain(ws.tabs.iter().filter_map(|ft| {
            let argv = ft.cfg.command.argv();
            config::browser_url_of(&argv)?;
            Some(
                ft.cfg
                    .id
                    .clone()
                    .or_else(|| ft.cfg.name.clone())
                    .unwrap_or_else(|| "browser".into()),
            )
        }))
        .collect();
    for gone in caps.keep_only_declared(&declared) {
        append_hook_log(&format!("設定から消えたので閉じました: {gone}"));
    }

    for ft in &ws.tabs {
        let argv = ft.cfg.command.argv();
        if config::browser_url_of(&argv).is_none() {
            continue;
        }
        let name = ft
            .cfg
            .id
            .clone()
            .or_else(|| ft.cfg.name.clone())
            .unwrap_or_else(|| "browser".into());
        // 外したなら消す。設定を戻したのに残っていては直せない。
        // Luaから出したものも、設定を保存した時点の指定に揃える
        match ft.cfg.nav {
            Some(nav) => {
                let _ = caps.browser_nav(&name, nav);
            }
            None => {
                let _ = caps.browser_unnav(&name);
            }
        }
        match &ft.cfg.ask {
            Some(ask) => {
                let label = if ask.label.trim().is_empty() {
                    i18n::t("tui.ask.label")
                } else {
                    ask.label.clone()
                };
                let _ = caps.browser_ask(&name, &ask.text, &label);
            }
            None => {
                let _ = caps.browser_unask(&name);
            }
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
        // ブラウザは子プロセスではない。窓の中にページを置くだけなので、
        // ここで立てようとすると「browser という名前の実行ファイルが無い」と
        // 言われて、起動のたびに身に覚えのない失敗が出る
        // (open_declared_browsers が開く)
        if config::browser_url_of(&argv).is_some() {
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
    // 呼び名はワークスペースの中でだけ通じる。
    // 置いたページも、いま見ている分だけがタブに並ぶ
    caps.set_workspace(to);
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

/// 画面に並ぶもの。設定に書いた順のまま。
///
/// セッションとブラウザを別々に持っているのは中の都合で、
/// 設定を書いた人には関係がない
#[derive(Clone, Debug, PartialEq)]
enum Pane {
    /// tabs の何番目か (0始まり)
    Session(usize),
    /// 窓の中に置いたページ
    Browser {
        /// 自動化から指す名前 (ID、無ければ表示名)。窓に置くときの名前でもある
        key: String,
        /// 人が読む名前
        name: String,
    },
}

/// 設定に書いた順で、画面に並ぶものを組み立てる。
///
/// 設定に無いもの (自動化が後から開いたブラウザ、引数で立てたタブ) は
/// 後ろに付ける。書いていないものの位置は決めようがない
fn panes_of(ws: Option<&config::Workspace>, titles: &[&str], hosted: &[String]) -> Vec<Pane> {
    let mut out: Vec<Pane> = Vec::new();
    let mut used_tabs = vec![false; titles.len()];
    let mut used_web: Vec<&str> = Vec::new();
    if let Some(ws) = ws {
        for ft in &ws.tabs {
            let argv = ft.cfg.command.argv();
            if argv.is_empty() {
                continue;
            }
            if config::browser_url_of(&argv).is_some() {
            let key = ft
                    .cfg
                    .id
                    .clone()
                    .or_else(|| ft.cfg.name.clone())
                    .unwrap_or_else(|| "browser".into());
                // 開けていなくても位置は持つ。開いた順で番号が動くと、
                // スクリプトの指す先が走るたびに変わる
                if let Some(h) = hosted.iter().find(|h| **h == key) {
                    used_web.push(h);
                }
                let name = ft.cfg.name.clone().unwrap_or_else(|| key.clone());
                out.push(Pane::Browser { key, name });
                continue;
            }
            let title = ft.cfg.name.clone().unwrap_or_else(|| title_of(&argv));
            // 同じ名前が並んでいても、書いた順に1つずつ対応させる
            let found = titles
                .iter()
                .enumerate()
                .find(|(i, t)| **t == title && !used_tabs[*i])
                .map(|(i, _)| i);
            if let Some(i) = found {
                used_tabs[i] = true;
                out.push(Pane::Session(i));
            }
        }
    }
    // 設定に書かれていないもの
    for (i, used) in used_tabs.iter().enumerate() {
        if !used {
            out.push(Pane::Session(i));
        }
    }
    // 設定に無いもの (自動化が後から開いた分、設定画面など) は名前がそれだけ
    for h in hosted {
        if !used_web.iter().any(|u| u == h) {
            out.push(Pane::Browser {
                key: h.clone(),
                name: h.clone(),
            });
        }
    }
    out
}

/// 画面の番号 (1始まり) から、セッションの居場所を引く
fn session_at(panes: &[Pane], active: usize) -> Option<usize> {
    match panes.get(active.checked_sub(1)?)? {
        Pane::Session(i) => Some(*i),
        Pane::Browser { .. } => None,
    }
}

/// セッションの居場所から、画面の番号 (1始まり) を引く。
/// ボールはセッションの番号で動くので、見せるときにここを通す
fn pane_at(panes: &[Pane], session: usize) -> usize {
    panes
        .iter()
        .position(|p| *p == Pane::Session(session.wrapping_sub(1)))
        .map(|i| i + 1)
        .unwrap_or(0)
}

/// いま見ているセッション。ブラウザを見ているなら None
fn session_mut<'a>(tabs: &'a mut [Tab], panes: &[Pane], active: usize) -> Option<&'a mut Tab> {
    let i = session_at(panes, active)?;
    tabs.get_mut(i)
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
/// secretsにあればそれを使い、無ければ data\remote-token に保存して使い回す
/// (毎回変わるとスマホを繋ぎ直すことになり、QRも設定画面から出せない)
pub fn remote_token(cfg: &config::Config, password: Option<&str>) -> String {
    if let Some(t) = cfg.remote_token(password) {
        return t;
    }
    let path = config::state_path("remote-token");
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

/// ポータブル配置のルート (相対パスの基準。exe と各フォルダが並ぶ場所)
fn config_file_dir() -> std::path::PathBuf {
    config::root_dir()
}

/// 設定画面を自分の窓の中に開く。立ち上げるのは1度だけで、2度目からは同じ場所へ戻る。
/// `query` はURLに付け足す追加の指示 ("&addtab=0" など。無指定は "")
fn open_settings(
    web: &mut Option<webui::WebUi>,
    config_file: &std::path::Path,
    remote_info: &Arc<Mutex<webui::RemoteInfo>>,
    caps: &hooks::Caps,
    query: &str,
) -> Result<()> {
    let url = match web.as_ref() {
        Some(w) => w.url.clone(),
        None => {
            let w = webui::WebUi::start_with(config_file.to_path_buf(), Arc::clone(remote_info))?;
            let u = w.url.clone();
            *web = Some(w);
            u
        }
    };
    caps.browser_open(SETTINGS_TAB, &format!("{url}{query}"))
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

/// 置いたページの様子を、画面の並びから組み立てる。
/// 並びに無いページ (閉じた後など) には None
fn page_ctx(
    panes: &[Pane],
    key: &str,
    url: String,
    complete: bool,
) -> Option<hooks::PageCtx> {
    panes.iter().enumerate().find_map(|(i, p)| match p {
        Pane::Browser { key: k, name } if k == key => Some(hooks::PageCtx {
            index: i + 1,
            id: k.clone(),
            name: name.clone(),
            url: url.clone(),
            complete,
        }),
        _ => None,
    })
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
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(config::logs_dir().join("hooks.log"))
    {
        use std::io::Write as _;
        let _ = writeln!(f, "{msg}");
    }
}

/// 画面の並びどおりの呼び名の一覧。
///
/// 指す先は画面の並びで数える。名前でも番号でも同じものを指す
/// (番号は並べ替えで変わるので、書くときは名前を勧める)
fn pane_keys(panes: &[Pane], tabs: &[Tab]) -> Vec<hooks::TabKey> {
    panes
        .iter()
        .map(|p| match p {
            Pane::Session(i) => tabs.get(*i).map(|t| t.key()).unwrap_or_default(),
            // ブラウザも呼び名 (ID) が先。表示名でも引けるようにする
            Pane::Browser { key, name } => hooks::TabKey {
                id: Some(key.clone()),
                name: name.clone(),
            },
        })
        .collect()
}

/// まだ渡せない受け渡し。相手が入力を受け取れるようになったら実行する。
///
/// 渡す先が起動しきっていないことは珍しくない。捨てられたことは
/// 誰にも見えないので、こちらで持っておく
struct Waiting {
    cmd: Command,
    /// これを過ぎたら諦める。持ち続けても、いつか誰も覚えていない
    give_up_ms: u64,
}

/// 相手が受け取れるようになるまで待てる受け渡しか。
///
/// 待てるのは「渡す」ものだけ。再起動や通知は相手の準備と関係がない
fn can_wait(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::SendPrompt { .. } | Command::DraftPrompt { .. }
    )
}

/// その受け渡しの宛先
fn target_of(cmd: &Command) -> Option<&hooks::TabRef> {
    match cmd {
        Command::SendPrompt { target, .. } | Command::DraftPrompt { target, .. } => Some(target),
        _ => None,
    }
}

/// 渡す相手が入力を受け取れる状態か
fn ready_to_receive(t: &Tab) -> bool {
    t.ready_for_startup_hook()
}

/// 諦めるまでの間。これより長く持っていても、書いた人はもう見ていない
const WAIT_FOR_TAB_MS: u64 = 30_000;

/// Luaフックが積んだ操作依頼を実行する。
/// 自動送信はチェーン深度 (透明のボール) を継承し、上限で止める
#[allow(clippy::too_many_arguments)]
fn exec_commands(
    cmds: Vec<Command>,
    tabs: &mut [Tab],
    panes: &[Pane],
    max_chain: u32,
    auto_enabled: bool,
    now_ms: u64,
    rows: u16,
    cols: u16,
    notifier: &notify::Notifier,
    flash: &mut Option<String>,
    ball: &mut ball::Ball,
    pending_submit: &mut Vec<PendingSubmit>,
    waiting: &mut Vec<Waiting>,
) {
    let keys = pane_keys(panes, tabs);
    let index_of = |r: &hooks::TabRef| r.resolve(&keys);
    // 画面の番号から、タブ配列の居場所へ。ブラウザなら None
    let session_of = |pane: usize| session_at(panes, pane);
    for cmd in cmds {
        // 渡す相手がまだ入力を受け取れないなら、預かって後で渡す。
        // ここで流すと黙って捨てられ、書いた人には何も見えない
        if can_wait(&cmd) {
            let not_yet = target_of(&cmd)
                .and_then(index_of)
                .and_then(session_of)
                .and_then(|i| tabs.get(i))
                .map(|t| !ready_to_receive(t))
                .unwrap_or(false);
            if not_yet {
                if let Some(t) = target_of(&cmd) {
                    append_hook_log(&format!("受け取れるまで待つ: {t:?}"));
                }
                waiting.push(Waiting {
                    cmd,
                    give_up_ms: now_ms + WAIT_FOR_TAB_MS,
                });
                continue;
            }
        }
        match cmd {
            Command::Log(msg) => append_hook_log(&msg),
            Command::Restart { target } => {
                let Some(target) = index_of(&target) else {
                    *flash = Some(i18n::tp("msg.tab_not_found", &[("target", &format!("{target:?}"))]));
                    continue;
                };
                if let Some(t) = session_of(target).and_then(|i| tabs.get_mut(i)) {
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
                if let Some(t) = session_of(target).and_then(|i| tabs.get(i)) {
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
                let depth = session_of(origin)
                    .and_then(|i| tabs.get(i))
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
                if let Some(t) = session_of(idx).and_then(|i| tabs.get_mut(i)) {
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
                let depth = session_of(origin)
                    .and_then(|i| tabs.get(i))
                    .map(|t| t.chain_depth)
                    .unwrap_or(0)
                    + 1;
                if depth > max_chain {
                    *flash = Some(i18n::tp("msg.chain_limit", &[("max", &max_chain.to_string())]));
                    append_hook_log(&format!("chain limit ({max_chain}): tab{origin} -> tab{target}"));
                    continue;
                }
                let Some(t) = session_of(target).and_then(|i| tabs.get_mut(i)) else {
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












/// 描画に必要なUI状態
struct Ui {
    /// 設定がまだ無い初回起動 (INDEXに案内を出す)
    first_run: bool,
    active: usize,
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
    /// 画面に並ぶもの。設定に書いた順
    panes: Vec<Pane>,
    /// 見ているブラウザの上に出す操作 (None = 出さない)
    nav: Option<crate::uistate::NavState>,
    /// 今の画面から何行遡って見ているか (0 = 今)
    scrolled: usize,
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


    /// 盤面が出しているメニューのキーを、INDEX が全部受けること。
    ///
    /// 出しているのに受け手が無いと、押しても何も起きない。
    /// 落ちも警告も出ないので、押した人が気づくしかない。
    ///
    /// 実際に `e`(設定) `i`(QR) `t`(通知) がそうだった。
    /// 前置キー付きで送っていたため、たまたま両方に同じ文字がある
    /// `?` `w` `r` だけが効き、半分動くので原因が見えなかった
    #[test]
    fn every_key_the_board_offers_is_answered_on_index() {
        let src = include_str!("main.rs");
        // INDEX の分岐だけを切り出す
        let head = "// INDEX = ホーム画面";
        let from = src.find(head).expect("INDEX の分岐が見つからない");
        // 分岐の終わりは、印を置いてある。
        // 文字数で切ると足りず、括弧で探すと途中の入れ子に当たる
        let len = src[from..]
            .find("INDEX-ここまで")
            .expect("INDEX の分岐の終わりの印が無い");
        let body = &src[from..from + len];

        for (key, _) in crate::shell::MENU {
            let want = format!("KeyCode::Char('{key}')");
            assert!(
                body.contains(&want),
                "盤面は {key} を出しているのに、INDEX に受け手が無い"
            );
        }
    }

    /// タブバーの + は、どのタブを見ていても効くよう前置キー付きで届くこと
    #[test]
    fn the_add_tab_button_arrives_prefixed() {
        let evs = super::keys_for(&crate::browser::Ev::AddTab);
        assert_eq!(evs.len(), 2, "前置キー + 本体の2打鍵");
        let Event::Key(k) = &evs[0] else { panic!("前置キーが打鍵でない") };
        assert_eq!(k.code, KeyCode::Char('b'));
        assert!(k.modifiers.contains(KeyModifiers::CONTROL));
        let Event::Key(k) = &evs[1] else { panic!("本体が打鍵でない") };
        assert_eq!(k.code, KeyCode::Char('t'));
        assert!(k.modifiers.is_empty());
    }

    /// 盤面のメニューは、前置キーの付かない打鍵として届くこと。
    ///
    /// Ctrl+B を付けると、前置キー側の表に同じ文字があるものだけが効く
    #[test]
    fn a_menu_press_arrives_as_a_plain_key() {
        for (key, _) in crate::shell::MENU {
            let evs = super::keys_for(&crate::browser::Ev::Menu {
                key: key.to_string(),
            });
            assert_eq!(evs.len(), 1, "{key}: 打鍵が1つでない");
            let Event::Key(k) = &evs[0] else {
                panic!("{key}: 打鍵になっていない")
            };
            assert_eq!(k.code, KeyCode::Char(key.chars().next().unwrap()));
            assert!(
                k.modifiers.is_empty(),
                "{key}: 前置キーが付いている ({:?})",
                k.modifiers
            );
        }
    }

    /// 渡す相手が受け取れないとき、預かること。
    ///
    /// AI CLI は起動してすぐ入力欄を描かない。その前に流し込むと
    /// 黙って捨てられ、書いた人には「動いていない」としか見えない。
    ///
    /// 待てるのは「渡す」ものだけ。再起動や通知は相手の準備と関係がない
    #[test]
    fn only_a_handoff_waits_for_the_other_side() {
        use hooks::{Command, TabRef};
        let draft = Command::DraftPrompt {
            target: TabRef::Name("ai".into()),
            text: "x".into(),
            origin: 1,
        };
        let send = Command::SendPrompt {
            target: TabRef::Name("ai".into()),
            text: "x".into(),
            origin: 1,
        };
        assert!(can_wait(&draft) && can_wait(&send), "渡すものが待てない");
        assert_eq!(target_of(&draft).map(|t| format!("{t:?}")).as_deref(),
                   Some("Name(\"ai\")"));

        for other in [
            Command::Restart { target: TabRef::Index(1) },
            Command::Notify { dest: "slack".into(), text: "x".into() },
            Command::Log("x".into()),
            Command::SendKeys { target: TabRef::Index(1), keys: "y".into() },
        ] {
            assert!(!can_wait(&other), "待つ必要のないものを預かっている: {other:?}");
        }
    }

    /// ブラウザのフックへ渡す様子が、画面の並びから作られること。
    ///
    /// 番号は人が押す番号と同じ。名前は人が読む方で、
    /// 自動化から指す呼び名とは別
    #[test]
    fn a_page_knows_its_number_and_both_of_its_names() {
        let layout = vec![
            Pane::Browser { key: "html".into(), name: "HTML解析".into() },
            Pane::Session(0),
        ];
        let page = page_ctx(&layout, "html", "https://example.com/".into(), true)
            .expect("並びにあるのに見つからない");
        assert_eq!(page.index, 1, "画面の番号と違う");
        assert_eq!(page.id, "html", "自動化から指す呼び名が違う");
        assert_eq!(page.name, "HTML解析", "人が読む名前が出ていない");
        assert!(page.complete);

        // 並びに無いページ (閉じた後など) には渡さない
        assert!(page_ctx(&layout, "shop", String::new(), true).is_none());
    }

    /// 自動化の割り当てが、画面の番号で並ぶこと。
    ///
    /// 人が押す番号、スクリプトが指す番号、ボールが飛ぶ番号は
    /// 同じでなければ追えない。番号はどこにも覚えさせず、
    /// 設定を読むたびに付け直す (並べ替えられても、ずれたままにならない)
    #[test]
    fn the_scripts_are_numbered_the_way_the_screen_is() {
        let ws = ws_from(&[
            ("HTML解析", "html", "browser https://example.com/"),
            ("エンジニア", "ai", "claude"),
        ]);
        let mut ws = ws;
        ws.tabs[0].cfg.automation = Some("scripts/html".into());
        ws.tabs[1].cfg.automation = Some("scripts/ai".into());

        let got = automation_by_pane(&ws);
        // 画面の番号で並ぶ。ブラウザが1番、claude が2番
        assert_eq!(
            got,
            vec![(1, "scripts/html".to_string()), (2, "scripts/ai".to_string())],
            "割り当てがずれている"
        );
    }

    /// 画面の並びが、設定に書いた順であること。
    ///
    /// セッションとブラウザは別々に持っている。並べるときにその都合を
    /// 出すと、1番目に書いたブラウザが後ろに回る。実際そうなっていて、
    /// 「順番的にはHTMLが1番なのになんで2番目になっているの？」となった
    #[test]
    fn the_order_on_screen_is_the_order_in_the_settings() {
        let ws = ws_from(&[
            ("HTML解析", "html", "browser https://example.com/"),
            ("エンジニア", "ai", "claude"),
        ]);
        let tabs = ["エンジニア"];
        let hosted = vec!["html".to_string()];

        let panes = panes_of(Some(&ws), &tabs, &hosted);
        assert_eq!(
            panes,
            vec![Pane::Browser { key: "html".into(), name: "HTML解析".into() }, Pane::Session(0)],
            "設定の順に並んでいない"
        );
        // 画面の番号から、セッションを引き当てられること
        assert_eq!(session_at(&panes, 1), None, "1番はブラウザのはず");
        assert_eq!(session_at(&panes, 2), Some(0));
        // ボールはセッションの番号で動く。見せるのは画面の番号
        assert_eq!(pane_at(&panes, 1), 2);
    }

    fn ws_from(rows: &[(&str, &str, &str)]) -> config::Workspace {
        let tabs = rows
            .iter()
            .map(|(name, id, cmd)| {
                config::FlatTab {
                    cfg: config::TabConfig {
                        name: Some(name.to_string()),
                        id: Some(id.to_string()),
                        command: config::CommandSpec::Line(cmd.to_string()),
                        ..Default::default()
                    },
                    depth: 0,
                }
            })
            .collect();
        config::Workspace {
            name: "試験".into(),
            tabs,
            automation: None,
            browsers: Vec::new(),
        }
    }

    /// まだ開けていないブラウザも、設定に書いた位置を保つこと。
    ///
    /// 開いた順で番号が動くと、スクリプトが指す先が走るたびに変わる。
    /// 開けなかったことは状態で見せればいい
    #[test]
    fn a_browser_keeps_its_place_even_before_it_opens() {
        let ws = ws_from(&[
            ("HTML解析", "html", "browser https://example.com/"),
            ("エンジニア", "ai", "claude"),
        ]);
        let tabs = ["エンジニア"];
        let panes = panes_of(Some(&ws), &tabs, &[]);
        assert_eq!(
            panes,
            vec![Pane::Browser { key: "html".into(), name: "HTML解析".into() }, Pane::Session(0)],
            "開く前だと番号がずれる"
        );
    }

    /// 設定に書いていないものは後ろに付くこと。
    /// 自動化が後から開いたブラウザや、引数で立てたタブの居場所は決めようがない
    #[test]
    fn what_the_settings_do_not_mention_goes_last() {
        let ws = ws_from(&[("エンジニア", "ai", "claude")]);
        let tabs = ["エンジニア", "あとから"];
        let hosted = vec!["settings".to_string()];
        let panes = panes_of(Some(&ws), &tabs, &hosted);
        assert_eq!(
            panes,
            vec![
                Pane::Session(0),
                Pane::Session(1),
                Pane::Browser { key: "settings".into(), name: "settings".into() }
            ]
        );
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

    /// ホイールで遡り、打てば今へ戻ること。
    ///
    /// 遡ったまま打つと、打った文字は画面の一番下に出るので見えない。
    /// 「打ったのに何も出ない」に見える
    #[test]
    fn the_wheel_goes_back_and_typing_comes_home() {
        assert_eq!(scrolled_to(0, 3), 3, "遡れていない");
        assert_eq!(scrolled_to(3, -1), 2);
        // 行き過ぎても今より先へは行かない
        assert_eq!(scrolled_to(2, -100), 0);
        assert_eq!(scrolled_to(0, -1), 0);
        // 一番奥へ (実際に持っている量は端末側が抑える)
        assert_eq!(scrolled_to(5, i32::MAX), 5 + i32::MAX as usize);

        // 打ったら今へ戻る。遡ったままだと、打った文字は
        // 画面の一番下に出るので見えない
        let mut p = vt100::Parser::new(3, 20, 100);
        p.process(b"1\r\n2\r\n3\r\n4\r\n5\r\n6\r\n");
        p.screen_mut().set_scrollback(2);
        assert_eq!(p.screen().scrollback(), 2);
        p.screen_mut().set_scrollback(scrolled_to(2, i32::MIN));
        assert_eq!(p.screen().scrollback(), 0, "今へ戻らない");
    }

    /// 全画面のプログラムには、回したことをそのまま渡すこと。
    ///
    /// あちらは自分の中身を自分で巻き戻すので、こちらが履歴を持っていても
    /// 何も無い (代替画面には遡る先が無い)。Claude Code がこれに当たる
    #[test]
    fn a_full_screen_program_is_told_that_the_wheel_turned() {
        use vt100::MouseProtocolEncoding as E;
        // 今どきの書き方。64が上、65が下、位置は1始まり
        assert_eq!(wheel_bytes(true, 0, 0, E::Sgr), b"\x1b[<64;1;1M".to_vec());
        assert_eq!(wheel_bytes(false, 4, 9, E::Sgr), b"\x1b[<65;10;5M".to_vec());
        // 昔の書き方は1バイトずつ (32を足す)
        assert_eq!(
            wheel_bytes(true, 0, 0, E::Default),
            vec![0x1b, b'[', b'M', 96, 33, 33]
        );
    }

    /// ブラウザを挟んだ並びでも、ボールを追えること。
    ///
    /// ボールは画面の番号で動く。数える方をセッションの数にすると、
    /// ブラウザの枚数だけ後ろの番号が「無いタブ」に見えて、
    /// そこへ渡ったボールには二度と追従しない。
    /// (解析=1 ブラウザ / AI=2 セッション の並びで、AIへ渡しても画面が動かなかった)
    #[test]
    fn a_browser_in_the_row_does_not_hide_the_tabs_behind_it() {
        let panes = vec![
            Pane::Browser { key: "html".into(), name: "解析".into() },
            Pane::Session(0),
        ];
        // セッションは1つだけ。数える先を間違えると 2 > 1 で弾かれる
        assert_eq!(
            follow_target(true, 2, 0, panes.len(), FOLLOW_GUARD_MS, 0),
            Some(2),
            "ブラウザの後ろのタブへ追従できていない"
        );
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


    /// ワードマークの3行は同じ幅であること (揃っていないと文字が崩れて見える)
    #[test]
    fn the_wordmark_rows_line_up() {
        let w: Vec<usize> = WORDMARK.iter().map(|l| l.chars().count()).collect();
        assert!(
            w.iter().all(|n| *n == w[0]),
            "行ごとに幅が違う: {w:?}"
        );
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
mod shutdown_tests {
    /// 窓のアプリとして作られていること。
    ///
    /// これが無いと Windows が黒いコンソールを一緒に開く。
    /// 画面は自前の窓に描いているので、その窓には何も映らない
    #[test]
    fn the_exe_asks_windows_for_no_console() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("#![windows_subsystem = \"windows\"]"),
            "コンソールが付いてくる"
        );
    }

    /// 窓が閉じたら終わること。
    ///
    /// 閉じても回り続けると、誰にも見えないプロセスが残る。
    /// それが待ち受けの口を握ったままなので、次の起動が
    /// 「アドレスは既に使用中」で失敗する。
    ///
    /// 打鍵に直せない報告は keys_for が捨てるので、閉じたことは
    /// そこを通せない。ループが直に見るしかない
    #[test]
    fn closing_the_window_ends_the_run() {
        use crate::browser::Ev;
        assert!(
            super::keys_for(&Ev::Closed).is_empty(),
            "閉じたことを打鍵として扱っている"
        );
        let src = include_str!("main.rs");
        assert!(
            src.contains("Ev::Closed => self.closed = true"),
            "窓が閉じた報告を受けていない"
        );
        // 改行の書き方は環境で変わるので、行ごとに見る
        let mut lines = src.lines().map(str::trim);
        assert!(
            lines.any(|l| l == "if surface.closed {") && lines.next() == Some("break;"),
            "閉じてもループが終わらない"
        );
    }
}



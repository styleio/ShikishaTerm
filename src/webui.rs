//! 設定用ローカルWeb GUI。DESIGN.md 5.5 / 10.2章。
//!
//! セキュリティ:
//!   - 127.0.0.1 のみにバインド (外部からは到達不能、ファイアウォール警告も出ない)
//!   - ランダムポート + 起動毎のワンタイムトークン
//!   - トークンはURLとリクエストヘッダの両方で検証し、同一PCの他プロセスや
//!     悪意あるWebページ (CSRF / DNS rebinding) からの設定API操作を防ぐ
//!   - Hostヘッダを127.0.0.1系に限定 (DNS rebinding対策)
//!   - マスターパスワードはここでは扱わない (TUI内で完結させる)

use std::io::Cursor;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use rand::TryRng as _;
use tiny_http::{Header, Response, Server};

/// 設定画面へ渡すリモートUIの状況 (本体が更新する)
#[derive(Default, Clone)]
pub struct RemoteInfo {
    pub running: bool,
    pub url: String,
    /// 有効にできない・注意が要る場合の説明
    pub note: String,
}

pub struct WebUi {
    pub url: String,
    stop: Arc<AtomicBool>,
}

impl WebUi {
    /// 設定ファイルを編集するローカルサーバーを起動する。
    /// config_path は編集対象 (通常 config.json)。
    /// password は本体が握っているマスターパスワードの共有 (秘密の暗号化に使う)。
    /// **ページにもネットワークにも出さない**。同一プロセスのサーバ側でだけ使う
    pub fn start_with(
        config_path: std::path::PathBuf,
        remote: Arc<std::sync::Mutex<RemoteInfo>>,
        password: Arc<std::sync::Mutex<Option<String>>>,
    ) -> Result<Self> {
        let token = random_token()?;
        let server = Server::http("127.0.0.1:0")
            .map_err(|e| anyhow::anyhow!("ローカルサーバーを起動できません: {e}"))?;
        let port = server
            .server_addr()
            .to_ip()
            .context("ポート取得に失敗")?
            .port();
        let url = format!("http://127.0.0.1:{port}/?token={token}");
        let stop = Arc::new(AtomicBool::new(false));

        {
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Err(e) = handle(req, &token, &config_path, &remote, &password) {
                        crate::append_hook_log(&format!("WebUI: {e}"));
                    }
                }
            });
        }
        Ok(Self { url, stop })
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 24];
    rand::rngs::SysRng
        .try_fill_bytes(&mut bytes)
        .map_err(|e| anyhow::anyhow!("トークン生成に失敗: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// 定数時間比較 (トークンの総当たりを情報漏洩なく弾く)
fn token_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn header_value(req: &tiny_http::Request, name: &'static str) -> String {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_default()
}

fn query_token(url: &str) -> String {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix("token="))
        .unwrap_or("")
        .to_string()
}

/// ?file=... のパスを安全に解決する。
/// 設定ファイルと同じディレクトリ配下の .json だけを許可し、
/// 絶対パス・親ディレクトリ参照 (..) は拒否する (パストラバーサル対策)
fn safe_workspace_path(
    url: &str,
    config_path: &std::path::Path,
) -> Option<std::path::PathBuf> {
    let raw = url
        .split_once('?')?
        .1
        .split('&')
        .find_map(|kv| kv.strip_prefix("file="))?;
    let decoded = percent_decode(raw);
    let rel = std::path::Path::new(&decoded);
    if rel.is_absolute() {
        return None;
    }
    if rel
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir | std::path::Component::Prefix(_)))
    {
        return None;
    }
    if rel.extension().and_then(|e| e.to_str()) != Some("json") {
        return None;
    }
    let base = config_path.parent()?;
    Some(base.join(rel))
}

/// 自動化フォルダのパスを安全に解決する (拡張子チェックなし版)
fn safe_dir_path(url: &str, config_path: &std::path::Path) -> Option<std::path::PathBuf> {
    let raw = url
        .split_once('?')?
        .1
        .split('&')
        .find_map(|kv| kv.strip_prefix("dir="))?;
    let decoded = percent_decode(raw);
    let rel = std::path::Path::new(&decoded);
    if rel.is_absolute() || decoded.is_empty() {
        return None;
    }
    if rel.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::Prefix(_)
        )
    }) {
        return None;
    }
    Some(config_path.parent()?.join(rel))
}

/// マニュアルはexeに埋め込む。どこから起動しても必ず参照でき、
/// 配布物にドキュメントを同梱し忘れても壊れない。
/// 翻訳を exe に同梱したい場合はここに1行足す (無くても docs/ に置けば読まれる)
const EMBEDDED_MANUALS: &[(&str, &str)] = &[
    ("en", include_str!("../docs/AUTOMATION.md")),
    ("ja", include_str!("../docs/AUTOMATION.ja.md")),
];

/// 隣に置かれたファイルがあればそちらを優先する (利用者が加筆できるように)。
/// 言語版 (AUTOMATION.<コード>.md) → 英語 → 埋め込み の順に探す
fn load_manual(config_path: &std::path::Path) -> String {
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(d) = config_path.parent() {
        dirs.push(d.to_path_buf());
    }
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
    {
        dirs.push(d);
    }
    dirs.push(std::path::PathBuf::from("."));

    let lang = crate::i18n::lang();
    let mut names = Vec::new();
    if lang != "en" {
        names.push(format!("AUTOMATION.{lang}.md"));
    }
    names.push("AUTOMATION.md".to_string());

    for name in &names {
        for d in &dirs {
            for rel in [d.join("docs").join(name), d.join(name)] {
                if let Ok(s) = std::fs::read_to_string(rel) {
                    if !s.trim().is_empty() {
                        return s;
                    }
                }
            }
        }
    }
    let embedded = |code: &str| EMBEDDED_MANUALS.iter().find(|(c, _)| *c == code).map(|(_, m)| *m);
    embedded(&lang).or_else(|| embedded("en")).unwrap_or_default().to_string()
}

/// 表示すべきリモートUIの状況を求める。
/// 本体が待ち受け中ならその情報を、そうでなければ設定から接続先を組み立てる
/// (設定だけを開いた場合や、有効にした直後でもQRを出せるようにするため)
fn effective_remote(shared: &Arc<std::sync::Mutex<RemoteInfo>>) -> RemoteInfo {
    let info = shared.lock().unwrap().clone();
    if info.running && !info.url.is_empty() {
        return info;
    }
    let Some(c) = crate::config::load().filter(|c| c.remote.enabled) else {
        return RemoteInfo::default();
    };
    match crate::netaddr::resolve_bind(&c.remote.bind, c.remote.allow_public) {
        Ok((ip, _)) => RemoteInfo {
            running: true,
            url: format!(
                "http://{ip}:{}/?t={}",
                c.remote.port,
                crate::remote_token(&c, None)
            ),
            note: crate::i18n::t("settings.phone.only_while_running"),
        },
        Err(e) => RemoteInfo {
            running: false,
            url: String::new(),
            note: e,
        },
    }
}

/// 自プロセスのダイアログが現れたら前面に持ち上げる。
/// Windowsはバックグラウンドのプロセスが勝手に前面へ出るのを禁じているため、
/// 最前面属性を付けてブラウザの後ろに隠れないようにする
#[cfg(windows)]
fn raise_own_dialog() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    type BOOL = i32;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
        SetForegroundWindow, SetWindowPos, SwitchToThisWindow, HWND_TOPMOST, SWP_NOMOVE,
        SWP_NOSIZE, SWP_SHOWWINDOW,
    };

    struct Found {
        pid: u32,
        hwnd: HWND,
    }

    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let found = unsafe { &mut *(lparam as *mut Found) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != found.pid || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        // 標準のダイアログはクラス名 "#32770"。コンソールは対象外
        let mut buf = [0u16; 64];
        let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
        let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        if class == "#32770" {
            found.hwnd = hwnd;
            return 0;
        }
        1
    }

    let pid = std::process::id();
    std::thread::spawn(move || {
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut found = Found {
                pid,
                hwnd: std::ptr::null_mut(),
            };
            unsafe { EnumWindows(Some(cb), &mut found as *mut Found as LPARAM) };
            if !found.hwnd.is_null() {
                unsafe {
                    // 最前面属性は付けたままにする。閉じるまでの間だけなので、
                    // これを外すとブラウザの後ろに隠れてしまう
                    SetWindowPos(
                        found.hwnd,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                    );
                    BringWindowToTop(found.hwnd);
                    // 前面化はバックグラウンドプロセスだと拒否されることがあるため、
                    // Alt+Tab相当の切り替えも併用する
                    SwitchToThisWindow(found.hwnd, 1);
                    SetForegroundWindow(found.hwnd);
                }
                break;
            }
        }
    });
}

#[cfg(not(windows))]
fn raise_own_dialog() {}

/// 選ばれたパスを設定に書く形にする。
/// 設定フォルダ配下なら相対パスにして、フォルダごと持ち運べる状態を保つ
fn display_path(path: &std::path::Path, config_path: &std::path::Path) -> String {
    config_path
        .parent()
        .and_then(|base| path.strip_prefix(base).ok())
        .map(|rel| rel.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
        .replace('\\', "/")
}

/// 最初に開く場所。鍵なら ~/.ssh、フォルダなら設定のある場所
fn default_pick_dir(kind: &str, config_path: &std::path::Path) -> Option<std::path::PathBuf> {
    if kind == "key" {
        if let Some(home) = std::env::var_os("USERPROFILE") {
            let ssh = std::path::PathBuf::from(&home).join(".ssh");
            if ssh.is_dir() {
                return Some(ssh);
            }
            return Some(std::path::PathBuf::from(home));
        }
    }
    config_path.parent().map(std::path::Path::to_path_buf)
}

/// 設定画面から読み書きするイベントファイル。
/// セッションのものと、ブラウザのものの両方
const EVENT_FILES: [&str; 8] = [
    "on_start",
    "on_done",
    "on_question",
    "on_exit",
    "on_busy",
    "on_load",
    "on_press",
    "_shared",
];

/// ローカルにインストール済みのAI CLIをワンショットで実行し、Luaコードを生成させる。
/// APIキーは不要 (利用者のサブスク認証をそのまま使う)。
/// 生成結果は必ず画面に表示し、利用者が承認するまで保存しない
/// 対応するAI CLI (名前, 非対話実行の引数, 表示名)
const AI_ENGINES: [(&str, &[&str], &str); 3] = [
    ("claude", &["-p"], "Claude Code"),
    ("codex", &["exec"], "Codex CLI"),
    ("gemini", &["-p"], "Gemini CLI"),
];

/// タブ構成を説明文にする。AIに送信先の番号を分からせるために渡す
/// (利用者はタブ名で指示できるべきなので)
fn describe_tabs(parsed: &serde_json::Value) -> String {
    let Some(tabs) = parsed.get("tabs").and_then(|v| v.as_array()) else {
        return String::new();
    };
    if tabs.is_empty() {
        return String::new();
    }
    let me = parsed.get("self").and_then(|v| v.as_u64()).unwrap_or(0);
    let mut s = crate::i18n::t("ai.tabs.header");
    s.push('\n');
    for t in tabs {
        let i = t.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            s.push_str(&format!("{i}. {name}"));
        } else {
            // IDがある場合はそちらで指させる (タブ名を変えても壊れない)
            s.push_str(&format!(
                "{i}. {name}{}",
                crate::i18n::tp("ai.tabs.id", &[("id", id)])
            ));
        }
        if i == me {
            s.push_str(&crate::i18n::t("ai.tabs.self"));
        }
        s.push('\n');
    }
    s
}

fn generate_with_local_ai(
    event: &str,
    want: &str,
    layout: &str,
    engine: Option<&str>,
    config_path: &std::path::Path,
) -> Result<String> {
    if want.trim().is_empty() {
        anyhow::bail!("{}", crate::i18n::t("automation.editor.want"));
    }
    // マニュアルを仕様書としてAIに渡す (独自APIは学習データに無いため)
    let manual = load_manual(config_path);

    // 会話文を返させないため、出力形式をマーカーで固定する
    let prompt = crate::i18n::tp(
        "ai.prompt",
        &[
            ("event", event),
            ("want", want),
            ("layout", layout),
            ("manual", &manual),
        ],
    );

    let (cmd, args) = pick_local_ai(engine)?;
    let mut spawner = std::process::Command::new(&cmd);
    spawner
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // ここもコンソールを継承するとマウスが死ぬ (open_browser と同じ理由)
    let mut child = crate::detach_console(&mut spawner)
        .spawn()
        .with_context(|| crate::i18n::tp("ai.err.cannot_run", &[("cmd", &cmd)]))?;
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().context("stdinを開けません")?;
        stdin.write_all(prompt.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        anyhow::bail!(
            "{}",
            crate::i18n::tp(
                "ai.err.failed",
                &[("cmd", &cmd), ("error", String::from_utf8_lossy(&out.stderr).trim())]
            )
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    extract_lua(&text)
}

/// AIの出力から <<<LUA ... >>> の中身を取り出す。
/// マーカーが無ければコードフェンスを剥がして返し、それも無ければエラーにする
/// (会話文をそのままコードとして保存してしまわないため)
fn extract_lua(text: &str) -> Result<String> {
    if let Some((_, rest)) = text.split_once("<<<LUA") {
        let body = rest.split_once(">>>").map(|(b, _)| b).unwrap_or(rest);
        return Ok(body.trim().to_string());
    }
    let stripped = strip_code_fence(text);
    // コードらしさの最低条件: shikisha.* か tab. を含むこと
    if stripped.contains("shikisha.") || stripped.contains("tab.") {
        return Ok(stripped);
    }
    anyhow::bail!(
        "{}",
        crate::i18n::tp(
            "ai.err.no_code",
            &[("reply", &text.trim().chars().take(120).collect::<String>())]
        )
    )
}

/// 使うAI CLIを決める。指定が無ければ claude → codex → gemini の順で最初に見つかったもの
fn pick_local_ai(want: Option<&str>) -> Result<(String, Vec<String>)> {
    for (name, args, _) in AI_ENGINES {
        if want.is_some_and(|w| w != name) {
            continue;
        }
        if let Some(path) = crate::tab::resolve_command(name) {
            let p = path.to_string_lossy().to_string();
            // .cmd/.bat は cmd.exe 経由でないと起動できない
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            return Ok(if matches!(ext.as_deref(), Some("cmd") | Some("bat")) {
                let mut a = vec!["/c".to_string(), p];
                a.extend(args.iter().map(|s| s.to_string()));
                ("cmd.exe".to_string(), a)
            } else {
                (p, args.iter().map(|s| s.to_string()).collect())
            });
        }
    }
    match want {
        Some(w) => anyhow::bail!("{w} が見つかりません"),
        None => anyhow::bail!(
            "AIコマンドが見つかりません。Claude Code / Codex CLI / Gemini CLI の\
             いずれかをインストールすると、この機能が使えます"
        ),
    }
}

/// AIが付けがちなコードフェンスを取り除く
fn strip_code_fence(s: &str) -> String {
    let t = s.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t.to_string();
    };
    let rest = rest.strip_prefix("lua").unwrap_or(rest);
    rest.trim_start_matches('\n')
        .rsplit_once("```")
        .map(|(body, _)| body)
        .unwrap_or(rest)
        .trim()
        .to_string()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// JSONで応答する
fn json_resp(v: serde_json::Value) -> Response<Cursor<Vec<u8>>> {
    Response::from_string(v.to_string()).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap(),
    )
}

/// 秘密ファイルのパス。設定に指定があればそれ、無ければ config.json の隣の secrets.json
fn secrets_file(config_path: &std::path::Path) -> std::path::PathBuf {
    crate::config::load()
        .and_then(|c| c.secrets_path())
        .unwrap_or_else(|| {
            let mut p = config_path.to_path_buf();
            p.set_file_name("secrets.json");
            p
        })
}

fn handle(
    req: tiny_http::Request,
    token: &str,
    config_path: &std::path::Path,
    remote: &Arc<std::sync::Mutex<RemoteInfo>>,
    password: &Arc<std::sync::Mutex<Option<String>>>,
) -> Result<()> {
    // DNS rebinding対策: Hostは必ずループバックであること
    let host = header_value(&req, "Host");
    let host_ok = host.starts_with("127.0.0.1:") || host.starts_with("localhost:");
    // トークンはURLか X-Token ヘッダのどちらかで一致すればよい
    let supplied = {
        let h = header_value(&req, "X-Token");
        if h.is_empty() {
            query_token(req.url())
        } else {
            h
        }
    };
    if !host_ok || !token_eq(&supplied, token) {
        return req
            .respond(Response::from_string("forbidden").with_status_code(403))
            .map_err(Into::into);
    }

    let method = req.method().as_str().to_string();
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    match (method.as_str(), path.as_str()) {
        ("GET", "/") => {
            let html = crate::i18n::render(PAGE)
                .replace("__TOKEN__", token)
                .replace("__DICT__", &crate::i18n::dict_json());
            let resp = Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            );
            req.respond(resp)?;
        }
        // 書き方の説明 (GUIから開ける。ファイルを探させない)
        ("GET", "/help") => {
            let md = load_manual(config_path);
            let html = crate::i18n::render(HELP_PAGE)
                .replace("__MD__", &serde_json::to_string(&md)?);
            let resp = Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            );
            req.respond(resp)?;
        }
        ("GET", "/api/config") => {
            let text = std::fs::read_to_string(config_path).unwrap_or_else(|_| "{}".into());
            let resp = Response::from_string(text).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                    .unwrap(),
            );
            req.respond(resp)?;
        }
        // ワークスペース定義ファイル (外部ファイル参照) の読み書き
        ("GET", "/api/workspace") => {
            let Some(p) = safe_workspace_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            let text = std::fs::read_to_string(&p).unwrap_or_else(|_| r#"{"tabs":[]}"#.into());
            let resp = Response::from_string(text).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..])
                    .unwrap(),
            );
            req.respond(resp)?;
        }
        ("POST", "/api/workspace") => {
            let Some(p) = safe_workspace_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(_) => {
                    if let Some(dir) = p.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    crate::crypto::write_atomic(&p, &body)?;
                    req.respond(Response::from_string(r#"{"ok":true}"#))?;
                }
                Err(e) => {
                    let msg = serde_json::json!({ "ok": false, "error": e.to_string() });
                    req.respond(Response::from_string(msg.to_string()).with_status_code(400))?;
                }
            }
        }
        // ── 秘密 (GitHub Secrets 相当) ────────────────────
        // マスターパスワードはページにもネットワークにも出さない。
        // 一覧はキーと説明だけを返し、値は決して返さない
        ("GET", "/api/secrets") => {
            let path = secrets_file(config_path);
            let pw = password.lock().unwrap().clone();
            let encrypted = std::fs::read_to_string(&path)
                .map(|t| crate::crypto::is_encrypted(&t))
                .unwrap_or(false);
            let (mode, items): (&str, Vec<serde_json::Value>) = if !path.exists() {
                ("empty", Vec::new())
            } else if encrypted && pw.is_none() {
                // 暗号化されていてパスワードが無ければ、一覧すら出せない
                ("locked", Vec::new())
            } else {
                match crate::config::list_secrets(&path, pw.as_deref()) {
                    Ok(list) => (
                        if encrypted { "encrypted" } else { "plaintext" },
                        list.into_iter()
                            .map(|(k, d)| serde_json::json!({ "key": k, "description": d }))
                            .collect(),
                    ),
                    Err(_) => ("locked", Vec::new()),
                }
            };
            req.respond(json_resp(serde_json::json!({
                "mode": mode,
                "has_password": pw.is_some(),
                "secrets": items,
            })))?;
        }
        ("POST", "/api/secrets/set") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let s = |k| p.get(k).and_then(|v| v.as_str()).unwrap_or("");
            let (key, desc, value) = (s("key").trim(), s("description"), s("value"));
            let path = secrets_file(config_path);
            let pw = password.lock().unwrap().clone();
            let resp = if value.is_empty() {
                serde_json::json!({ "ok": false, "error": "値が空です" })
            } else {
                match crate::config::upsert_secret(&path, pw.as_deref(), key, desc, value) {
                    Ok(()) => serde_json::json!({ "ok": true }),
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                }
            };
            req.respond(json_resp(resp))?;
        }
        ("POST", "/api/secrets/delete") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let key = p.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let path = secrets_file(config_path);
            let pw = password.lock().unwrap().clone();
            let resp = match crate::config::delete_secret(&path, pw.as_deref(), key) {
                Ok(()) => serde_json::json!({ "ok": true }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            };
            req.respond(json_resp(resp))?;
        }
        // ワークスペース1つを、スクリプトごと1枚のファイルに書き出す。
        // 番号で指すのは保存済みの設定。画面の編集中の姿ではない
        ("POST", "/api/workspace/export") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let index = p.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let resp = match crate::wspack::pack(config_path, index) {
                Ok((name, text)) => {
                    raise_own_dialog();
                    let picked = rfd::FileDialog::new()
                        .set_title(crate::i18n::t("settings.ws.export.title"))
                        .set_file_name(&name)
                        .add_filter(crate::i18n::t("settings.ws.file_kind"), &["json"])
                        .set_directory(
                            config_path.parent().unwrap_or(std::path::Path::new(".")),
                        )
                        .save_file();
                    match picked {
                        // 選ばれた場所へ書く。ここは利用者が指した先なので設定フォルダの外でよい
                        Some(path) => match crate::crypto::write_atomic(&path, &text) {
                            Ok(()) => serde_json::json!({
                                "ok": true,
                                "path": path.display().to_string(),
                            }),
                            Err(e) => {
                                serde_json::json!({ "ok": false, "error": e.to_string() })
                            }
                        },
                        None => serde_json::json!({ "ok": false, "cancelled": true }),
                    }
                }
                Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
            };
            req.respond(
                Response::from_string(resp.to_string()).with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            )?;
        }
        // 書き出したファイルを取り込む。設定にワークスペースが1つ増える
        ("POST", "/api/workspace/import") => {
            raise_own_dialog();
            let picked = rfd::FileDialog::new()
                .set_title(crate::i18n::t("settings.ws.import.title"))
                .add_filter(crate::i18n::t("settings.ws.file_kind"), &["json"])
                .set_directory(config_path.parent().unwrap_or(std::path::Path::new(".")))
                .pick_file();
            let resp = match picked {
                Some(path) => match std::fs::read_to_string(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|t| crate::wspack::unpack(config_path, &t))
                {
                    Ok(placed) => serde_json::json!({
                        "ok": true,
                        "name": placed.name,
                        "files": placed.files,
                        "moved": placed.moved.iter()
                            .map(|(f, t)| serde_json::json!([f, t]))
                            .collect::<Vec<_>>(),
                    }),
                    Err(e) => serde_json::json!({ "ok": false, "error": format!("{e:#}") }),
                },
                None => serde_json::json!({ "ok": false, "cancelled": true }),
            };
            req.respond(
                Response::from_string(resp.to_string()).with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            )?;
        }
        // 自動化 (イベント別ファイル) の読み書き
        ("GET", "/api/automation") => {
            let Some(dir) = safe_dir_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            let mut map = serde_json::Map::new();
            for name in EVENT_FILES {
                let f = dir.join(format!("{name}.lua"));
                let body = std::fs::read_to_string(&f).unwrap_or_default();
                map.insert(name.to_string(), serde_json::Value::String(body));
            }
            let resp = Response::from_string(serde_json::Value::Object(map).to_string())
                .with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                );
            req.respond(resp)?;
        }
        ("POST", "/api/automation") => {
            let Some(dir) = safe_dir_path(req.url(), config_path) else {
                return req
                    .respond(Response::from_string("bad path").with_status_code(400))
                    .map_err(Into::into);
            };
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let parsed: serde_json::Value = serde_json::from_str(&body)?;
            std::fs::create_dir_all(&dir)?;
            for name in EVENT_FILES {
                let Some(code) = parsed.get(name).and_then(|v| v.as_str()) else {
                    continue;
                };
                let f = dir.join(format!("{name}.lua"));
                if code.trim().is_empty() {
                    // 空にしたら「そのイベントでは何もしない」= ファイルごと削除
                    let _ = std::fs::remove_file(&f);
                } else {
                    crate::crypto::write_atomic(&f, code)?;
                }
            }
            req.respond(Response::from_string(r#"{"ok":true}"#))?;
        }
        // Windows標準のファイル選択ダイアログを開く。
        // ブラウザは安全のため実ファイルパスを渡せないため、こちら側で開く
        ("POST", "/api/pick") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let p: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let kind = p.get("kind").and_then(|v| v.as_str()).unwrap_or("file");
            let fallback = crate::i18n::t("settings.pick.title");
            let title = p.get("title").and_then(|v| v.as_str()).unwrap_or(&fallback);
            let start = p
                .get("start")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(std::path::PathBuf::from)
                .filter(|p| p.exists())
                .or_else(|| default_pick_dir(kind, config_path));

            // ダイアログは開いた直後に前面へ出す
            // (バックグラウンドのプロセスは自力で前面に立てないため)
            raise_own_dialog();
            let mut dlg = rfd::FileDialog::new().set_title(title);
            if let Some(d) = start {
                dlg = dlg.set_directory(d);
            }
            let picked = if kind == "dir" {
                dlg.pick_folder()
            } else {
                dlg.pick_file()
            };
            let resp = match picked {
                Some(path) => {
                    serde_json::json!({ "ok": true, "path": display_path(&path, config_path) })
                }
                None => serde_json::json!({ "ok": false }),
            };
            req.respond(
                Response::from_string(resp.to_string()).with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            )?;
        }
        // スマホから使う機能の状況 (どのネットワークが使えるかも返す)
        ("GET", "/api/remote") => {
            let info = effective_remote(remote);
            let ts = crate::netaddr::tailscale_ip().map(|i| i.to_string());
            let lan = crate::netaddr::lan_ip().map(|i| i.to_string());
            let resp = serde_json::json!({
                "running": info.running,
                "url": info.url,
                "note": info.note,
                "tailscale": ts,
                "lan": lan,
            });
            req.respond(
                Response::from_string(resp.to_string()).with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            )?;
        }
        // 接続用QRコード (URLとトークンを手入力させない)
        ("GET", "/api/remote/qr") => {
            let url = effective_remote(remote).url;
            let svg = if url.is_empty() {
                String::new()
            } else {
                crate::netaddr::qr_svg(&url, 6)
            };
            req.respond(Response::from_string(svg).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"image/svg+xml; charset=utf-8"[..])
                    .unwrap(),
            ))?;
        }
        // 使えるAI CLIを調べる (画面を出す前に判定し、無ければ機能ごと隠す)
        ("GET", "/api/ai") => {
            let list: Vec<serde_json::Value> = AI_ENGINES
                .iter()
                .filter(|(name, _, _)| crate::tab::resolve_command(name).is_some())
                .map(|(name, _, label)| serde_json::json!({ "id": name, "label": label }))
                .collect();
            let resp = Response::from_string(serde_json::json!({ "engines": list }).to_string())
                .with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                );
            req.respond(resp)?;
        }
        // 自然言語からLuaを生成する (ローカルのAI CLIをワンショット実行)
        ("POST", "/api/generate") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let event = parsed.get("event").and_then(|v| v.as_str()).unwrap_or("on_done");
            let want = parsed.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            // 指定が無ければ設定の ai_engine を使う
            let from_cfg = std::fs::read_to_string(config_path)
                .ok()
                .and_then(|t| serde_json::from_str::<crate::config::Config>(&t).ok())
                .and_then(|c| c.ai_engine)
                .filter(|s| !s.is_empty());
            let engine = parsed
                .get("engine")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .or(from_cfg);
            let layout = describe_tabs(&parsed);
            let resp = match generate_with_local_ai(event, want, &layout, engine.as_deref(), config_path)
            {
                Ok(code) => serde_json::json!({ "ok": true, "code": code }),
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            };
            req.respond(
                Response::from_string(resp.to_string()).with_header(
                    Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .unwrap(),
                ),
            )?;
        }
        ("POST", "/api/config") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            // 壊れたJSONで設定を失わないよう、保存前に必ず検証する
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(_) => {
                    crate::crypto::write_atomic(config_path, &body)?;
                    req.respond(Response::from_string(r#"{"ok":true}"#))?;
                }
                Err(e) => {
                    let msg = serde_json::json!({ "ok": false, "error": e.to_string() });
                    req.respond(
                        Response::from_string(msg.to_string()).with_status_code(400),
                    )?;
                }
            }
        }
        _ => {
            req.respond(Response::new(
                404.into(),
                vec![],
                Cursor::new(b"not found".to_vec()),
                None,
                None,
            ))?;
        }
    }
    Ok(())
}

// 設定画面。本体のサイバー調とは別に、読みやすさを優先した静かなUIにしている
// (サイドバー + 詳細ペイン。一覧は「何があるか」だけを見せ、編集は1つに集中させる)
const PAGE: &str = r##"<!doctype html>
<html lang="{{__lang__}}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{settings.title}}</title>
<style>
 :root {
   --bg:#0f1115; --panel:#161a20; --panel2:#1b2027; --line:#262d37;
   --text:#e6e9ef; --muted:#8b95a5; --accent:#00aaff; --danger:#ff6b6b;
   color-scheme: dark;
 }
 * { box-sizing:border-box; }
 body { margin:0; background:var(--bg); color:var(--text); font-size:14px; line-height:1.6;
   font-family:system-ui,"Segoe UI","Yu Gothic UI","Hiragino Sans",sans-serif; }
 code, .mono, input.mono { font-family:ui-monospace,Consolas,"Courier New",monospace; }

 header { position:sticky; top:0; z-index:5; display:flex; align-items:center; gap:12px;
   padding:12px 20px; background:rgba(15,17,21,.9); backdrop-filter:blur(8px);
   border-bottom:1px solid var(--line); }
 header h1 { font-size:15px; font-weight:600; margin:0; letter-spacing:.02em; }
 header .spacer { flex:1; }
 #msg { color:var(--muted); font-size:13px; border-radius:6px; padding:4px 10px; }
 #msg.warn { color:var(--danger); }
 /* 同じ文言が続いても押したことが分かるよう、毎回アニメーションを流し直す */
 #msg.flash { animation:msgflash 1.1s ease-out; }
 @keyframes msgflash {
   0%   { background:var(--accent); color:#04121c; }
   60%  { background:var(--accent); color:#04121c; }
   100% { background:transparent; color:var(--muted); }
 }
 button.primary:disabled { opacity:.55; cursor:default; }
 /* 保存の結果はヘッダの小さな文字だと視線が行かないので、画面下に出す */
 #toast { position:fixed; left:50%; bottom:28px; transform:translateX(-50%) translateY(16px);
   padding:11px 20px; border-radius:9px; background:var(--accent); color:#04121c;
   font-weight:600; font-size:13.5px; box-shadow:0 10px 30px rgba(0,0,0,.5);
   opacity:0; pointer-events:none; z-index:50;
   transition:opacity .18s ease, transform .18s ease; }
 #toast.show { opacity:1; transform:translateX(-50%) translateY(0); }
 #toast.warn { background:var(--danger); color:#fff; }
 /* 未保存の変更があるあいだは保存ボタンに印を出す。
    押す前から「保存が要る状態か」が分かれば、押したあと不安にならない */
 #savebtn.dirty::before { content:"● "; }

 .layout { display:flex; align-items:flex-start; }
 nav { width:260px; flex:none; border-right:1px solid var(--line); min-height:calc(100vh - 53px);
   padding:12px 10px; position:sticky; top:53px; max-height:calc(100vh - 53px); overflow:auto; }
 main { flex:1; padding:24px 28px; max-width:820px; }

 .navitem { display:block; width:100%; text-align:left; border:0; background:none; color:var(--text);
   padding:7px 10px; border-radius:7px; cursor:pointer; font-size:13.5px; font-family:inherit; }
 .navitem:hover { background:var(--panel); }
 .navitem.sel { background:var(--panel2); }
 .navitem .sub { display:block; color:var(--muted); font-size:11.5px; margin-top:1px;
   white-space:nowrap; overflow:hidden; text-overflow:ellipsis; }
 .navgroup { color:var(--muted); font-size:11px; letter-spacing:.08em; text-transform:uppercase;
   margin:14px 10px 4px; }
 .navtab { padding-left:18px; }
 .navtab.child { padding-left:34px; }
 .navadd { color:var(--muted); }

 .card { background:var(--panel); border:1px solid var(--line); border-radius:10px;
   padding:6px 18px 14px; margin-bottom:18px; }
 .card h2 { font-size:12px; color:var(--muted); font-weight:600; letter-spacing:.06em;
   margin:14px 0 10px; text-transform:uppercase; }
 .row { display:flex; align-items:center; gap:12px; padding:7px 0; flex-wrap:wrap; }
 .row > label:first-child { width:150px; flex:none; color:var(--muted); font-size:13px; }
 .hint { color:var(--muted); font-size:12px; }
 .grow { flex:1; min-width:180px; }

 input[type=text], input[type=number], select, textarea {
   background:var(--panel2); color:var(--text); border:1px solid var(--line); border-radius:7px;
   padding:7px 10px; font-size:13.5px; font-family:inherit; outline:none; }
 input:focus, select:focus, textarea:focus { border-color:var(--accent); }
 input[type=text]::placeholder, textarea::placeholder { color:#5d6773; }
 input[type=checkbox] { width:16px; height:16px; accent-color:var(--accent); margin:0; }
 label.check { display:flex; align-items:center; gap:8px; width:auto; color:var(--text);
   font-size:13.5px; cursor:pointer; }
 textarea { width:100%; min-height:220px; line-height:1.55; resize:vertical; }

 button { font-family:inherit; font-size:13px; border-radius:7px; cursor:pointer; padding:7px 14px;
   border:1px solid var(--line); background:var(--panel2); color:var(--text); }
 button:hover { border-color:#39424f; }
 button.primary { background:var(--accent); border-color:var(--accent); color:#04121c; font-weight:600; }
 button.quiet { background:none; border-color:transparent; color:var(--muted); padding:6px 8px; }
 button.quiet:hover { color:var(--text); background:var(--panel2); }
 button.danger { color:var(--danger); background:none; border-color:transparent; }
 button.danger:hover { background:rgba(229,83,75,.1); }

 details { border-top:1px solid var(--line); margin-top:6px; }
 details > summary { cursor:pointer; color:var(--muted); font-size:13px; padding:12px 0 4px;
   list-style:none; }
 details > summary::before { content:"▸ "; }
 details[open] > summary::before { content:"▾ "; }

 .events { display:flex; flex-direction:column; }
 .event { display:flex; align-items:center; gap:12px; padding:9px 0; border-bottom:1px solid var(--line); }
 .event:last-child { border-bottom:0; }
 .event .name { flex:1; }
 .event .state { color:var(--muted); font-size:12px; }
 .event .state.on { color:var(--accent); }

 .empty { color:var(--muted); text-align:center; padding:40px 20px; }
 .empty .big { font-size:15px; color:var(--text); margin-bottom:6px; }

 .modal { position:fixed; inset:0; background:rgba(0,0,0,.6); display:flex; align-items:center;
   justify-content:center; z-index:20; }
 .modal-inner { background:var(--panel); border:1px solid var(--line); border-radius:12px;
   width:min(880px,92vw); max-height:88vh; overflow:auto; padding:20px 24px; }
 .modal-inner h2 { text-transform:none; font-size:15px; color:var(--text); margin:0 0 4px; }
 pre { background:var(--panel2); border:1px solid var(--line); border-radius:8px; padding:12px;
   overflow:auto; max-height:240px; font-size:12.5px; }
 a { color:var(--accent); }
 /* AIの生成は数十秒かかることがある。止まっていないと一目で分かるように、
    回るもの・伸びるもの・進む数字を揃える */
 #aibusy { flex-direction:column; gap:9px; margin-top:10px; padding:12px 14px;
   background:var(--panel2); border:1px solid var(--accent); border-radius:9px; }
 #aibusy .head { display:flex; align-items:center; gap:10px; }
 #aibusytext { color:var(--accent); font-weight:600; }
 .spin { width:16px; height:16px; flex:none; border-radius:50%;
   border:2px solid var(--line); border-top-color:var(--accent);
   animation:spin .8s linear infinite; }
 @keyframes spin { to { transform:rotate(360deg); } }
 .bar { height:4px; border-radius:2px; background:var(--line); overflow:hidden; }
 .bar > i { display:block; height:100%; width:35%; border-radius:2px;
   background:var(--accent); animation:slide 1.3s ease-in-out infinite; }
 @keyframes slide { from { margin-left:-35%; } to { margin-left:100%; } }
</style></head><body>

<header>
  <h1>{{settings.title}}</h1>
  <div class="spacer"></div>
  <span id="msg"></span>
  <button class="quiet" onclick="load()">{{common.reload}}</button>
  <button class="quiet" id="backbtn" onclick="closeSettings()">{{settings.close}}</button>
  <button class="primary" id="savebtn" onclick="save()">{{common.save}}</button>
</header>

<div class="layout">
  <nav id="nav"></nav>
  <main id="detail"></main>
</div>

<div id="toast" role="status" aria-live="polite"></div>

<datalist id="cmdlist"></datalist>

<div id="autobox" class="modal" style="display:none">
  <div class="modal-inner">
    <h2 id="autotitle">{{settings.tab.automation}}</h2>
    <div class="hint" id="autopath"></div>
    <div class="row" style="margin-top:12px">
      <label>{{automation.editor.when}}</label>
      <select id="autoevent" onchange="switchEvent()"></select>
      <span class="hint" id="autohint"></span>
    </div>
    <textarea id="autocode" spellcheck="false"
      placeholder="{{automation.editor.code.ph}}"></textarea>
    <div class="row" id="airow">
      <label>{{automation.editor.ask}}</label>
      <input type="text" id="autoask" class="grow"
             placeholder="{{automation.editor.ask.ph}}">
      <button onclick="askAi()" id="aibtn">{{automation.editor.generate}}</button>
    </div>
    <div id="aibusy" style="display:none">
      <div class="head"><span class="spin"></span><span id="aibusytext"></span></div>
      <div class="bar"><i></i></div>
    </div>
    <div class="row" id="ainone" style="display:none">
      <span class="hint">{{automation.editor.no_ai}}</span>
    </div>
    <div id="aipreview" style="display:none">
      <div class="hint">{{automation.editor.generated}}</div>
      <pre id="aicode"></pre>
      <button class="primary" onclick="applyAi()">{{automation.editor.apply}}</button>
      <button class="quiet" onclick="document.getElementById('aipreview').style.display='none'">{{automation.editor.discard}}</button>
    </div>
    <div class="row" style="border-top:1px solid var(--line); margin-top:12px; padding-top:14px">
      <button class="primary" onclick="saveAuto()">{{automation.editor.save}}</button>
      <button class="quiet" onclick="closeAuto()">{{common.cancel}}</button>
      <span class="spacer" style="flex:1"></span>
      <a href="/help?token=__TOKEN__" target="_blank">{{automation.editor.help}}</a>
      <span id="automsg" class="hint"></span>
    </div>
  </div>
</div>

<script>
const TOKEN = "__TOKEN__";
const T = __DICT__;
// {name} 差し込み (Rust側の tp と同じ規則)
const fill = (s, args) => Object.entries(args)
  .reduce((acc, [k, v]) => acc.replaceAll("{" + k + "}", v), s || "");
const api = (m, b) => fetch("/api/config", {
   method: m, headers: {"X-Token": TOKEN, "Content-Type":"application/json"}, body: b });
const wsApi = (m, file, b) => fetch("/api/workspace?file=" + encodeURIComponent(file), {
   method: m, headers: {"X-Token": TOKEN, "Content-Type":"application/json"}, body: b });

let current = {};        // config.json の中身 (基本設定の保持用)
let wss = [];            // ワークスペースとタブ
let sel = {ws:0, tab:null, global:true};
let aiEngines = [];

const el = (tag, attrs = {}, ...kids) => {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") n.className = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else if (v !== null && v !== undefined) n.setAttribute(k, v);
  }
  for (const c of kids) if (c !== null && c !== undefined) n.append(c);
  return n;
};
const msg = (t, warn) => {
  const m = document.getElementById("msg");
  m.textContent = t;
  m.classList.toggle("warn", !!warn);
  // クラスを付け直すだけでは再生されないので、一度外して強制的に再計算させる
  m.classList.remove("flash");
  void m.offsetWidth;
  if (!warn) m.classList.add("flash");
};

let toastTimer = null;
function toast(text, warn) {
  const t = document.getElementById("toast");
  t.textContent = (warn ? "⚠ " : "✓ ") + text;
  t.classList.toggle("warn", !!warn);
  t.classList.add("show");
  clearTimeout(toastTimer);
  // 失敗は読む時間が要るので長く出す
  toastTimer = setTimeout(() => t.classList.remove("show"), warn ? 6000 : 2200);
}

// 操作の結果はヘッダにも残しつつ、トーストで必ず気づけるようにする
const result = (text, warn) => { msg(text, warn); toast(text, warn); };

// 未保存かどうかの判定。保存/読込した時点の内容を覚えておいて見比べる
let savedSnapshot = "";
// 生の入力状態ではなく「保存したら実際に何が書かれるか」を比べる。
// 入力欄は数値を文字列にしてしまうので、10 と "10" を別物と見てしまう
const snapshot = () => JSON.stringify(payload());
function markClean() { savedSnapshot = snapshot(); refreshSave(); }
function refreshSave() {
  const b = document.getElementById("savebtn");
  // 読み込みが終わるまでは比較対象が無い。ここで抜けないと開いた瞬間に未保存の印が出る
  if (!b || b.disabled || savedSnapshot === "") return;
  const dirty = snapshot() !== savedSnapshot;
  b.classList.toggle("dirty", dirty);
  b.title = dirty ? T["settings.unsaved"] : "";
}
// タブの追加・削除・並べ替えは input イベントを出さないので、
// イベントを拾うだけでは取りこぼす。内容そのものを見比べるほうが確実
setInterval(refreshSave, 600);

// ── 部品 ─────────────────────────────────────────────
// 中身はミリ秒で持つが、人に見せるのは秒。
// 「10000」と書かせるより「10」のほうが、設定として素直に読める
function secondsField(obj, key, placeholderSec) {
  const i = el("input", {type:"number", step:"1", min:"0",
                         placeholder:String(placeholderSec), class:"grow"});
  i.style.maxWidth = "110px";
  const ms = obj[key];
  i.value = (ms === "" || ms === null || ms === undefined) ? "" : String(Number(ms) / 1000);
  i.oninput = () => {
    const v = i.value.trim();
    if (v === "") delete obj[key];
    else obj[key] = Math.round(Number(v) * 1000);
  };
  return i;
}
function field(obj, key, ph, opts = {}) {
  const i = el("input", {type: opts.type || "text", placeholder: ph,
                         class: (opts.mono ? "mono " : "") + (opts.grow === false ? "" : "grow")});
  if (opts.width) i.style.width = opts.width + "px";
  i.value = obj[key] ?? "";
  i.addEventListener("input", () => { obj[key] = i.value; if (opts.onInput) opts.onInput(i.value); });
  return i;
}
function check(obj, key, label) {
  const i = el("input", {type:"checkbox"});
  i.checked = !!obj[key];
  i.addEventListener("change", () => { obj[key] = i.checked; });
  const l = el("label", {class:"check"}); l.append(i, document.createTextNode(label));
  return l;
}
// 既定がオンの項目。未設定とオンを見分けず、外したときだけ false を持つ
function checkDefaultOn(obj, key, label) {
  const i = el("input", {type:"checkbox"});
  i.checked = obj[key] !== false;
  i.addEventListener("change", () => {
    if (i.checked) delete obj[key];
    else obj[key] = false;
  });
  const l = el("label", {class:"check"}); l.append(i, document.createTextNode(label));
  return l;
}
function choose(obj, key, opts, onChange) {
  const s = el("select");
  for (const [v, label] of opts) s.append(el("option", {value:v}, label));
  s.value = obj[key] || "";
  s.addEventListener("change", () => { obj[key] = s.value; if (onChange) onChange(s.value); });
  return s;
}
function row(label, ...kids) { return el("div", {class:"row"}, el("label", {}, label), ...kids); }
function card(title, ...kids) { return el("div", {class:"card"}, el("h2", {}, title), ...kids); }

async function pickPath(kind, title, start) {
  try {
    const r = await fetch("/api/pick", {method:"POST",
        headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
        body: JSON.stringify({kind, title, start: start || ""})});
    const j = await r.json();
    return j.ok ? j.path : null;
  } catch (e) { return null; }
}
function pathField(obj, key, ph, kind, title) {
  const i = field(obj, key, ph, {mono:true});
  const b = el("button", {class:"quiet", onclick: async () => {
    const p = await pickPath(kind, title, obj[key]);
    if (p !== null) { obj[key] = p; i.value = p; }
  }}, T["common.browse"]);
  return [i, b];
}

// ── コマンドの組み立て (SSH / Docker / WSL) ───────────
function parseSsh(cmd) {
  const t = (cmd || "").trim().split(/\s+/);
  if (t[0] !== "ssh") return null;
  const o = {host:"", port:"", user:"", key:"", agent:false, x11:false,
             forwards:[], jump:"", keepalive:"", extra:[]};
  for (let i = 1; i < t.length; i++) {
    const a = t[i];
    if (a === "-p") o.port = t[++i] || "";
    else if (a === "-i") o.key = t[++i] || "";
    else if (a === "-J") o.jump = t[++i] || "";
    else if (a === "-A") o.agent = true;
    else if (a === "-X") o.x11 = true;
    else if (a === "-L" || a === "-R" || a === "-D") o.forwards.push(a + " " + (t[++i] || ""));
    else if (a === "-o") {
      const v = t[++i] || "";
      const m = v.match(/^ServerAliveInterval=(\d+)$/);
      if (m) o.keepalive = m[1]; else o.extra.push("-o " + v);
    }
    else if (a.startsWith("-")) o.extra.push(a);
    else if (!o.host) {
      const at = a.split("@");
      if (at.length === 2) { o.user = at[0]; o.host = at[1]; } else o.host = a;
    }
    else o.extra.push(a);
  }
  return o;
}
function buildSsh(o) {
  const p = ["ssh"];
  if (o.port) p.push("-p", o.port);
  if (o.key) p.push("-i", o.key);
  if (o.jump) p.push("-J", o.jump);
  if (o.agent) p.push("-A");
  if (o.x11) p.push("-X");
  if (o.keepalive) p.push("-o", "ServerAliveInterval=" + o.keepalive);
  for (const f of o.forwards) if (f.trim()) p.push(f.trim());
  p.push(...o.extra);
  if (o.host) p.push((o.user ? o.user + "@" : "") + o.host);
  return p.join(" ");
}
function parseDocker(cmd) {
  const t = (cmd || "").trim().split(/\s+/);
  if (t[0] !== "docker" || t[1] !== "exec") return null;
  const o = {container:"", dir:"", shell:""}; const rest = [];
  for (let i = 2; i < t.length; i++) {
    const a = t[i];
    if (a === "-w") o.dir = t[++i] || "";
    else if (a === "-it" || a === "-i" || a === "-t") continue;
    else if (a.startsWith("-")) rest.push(a);
    else if (!o.container) o.container = a;
    else rest.push(a);
  }
  o.shell = rest.join(" ");
  return o;
}
const buildDocker = o => ["docker exec -it", o.dir ? "-w " + o.dir : "", o.container,
  o.shell || "bash"].filter(Boolean).join(" ");
function parseWsl(cmd) {
  const t = (cmd || "").trim().split(/\s+/);
  if (t[0] !== "wsl") return null;
  const o = {distro:"", dir:"", shell:""}; const rest = [];
  for (let i = 1; i < t.length; i++) {
    const a = t[i];
    if (a === "-d" || a === "--distribution") o.distro = t[++i] || "";
    else if (a === "--cd") o.dir = t[++i] || "";
    else if (a === "--") { rest.push(...t.slice(i + 1)); i = t.length; }
    else rest.push(a);
  }
  o.shell = rest.join(" ");
  return o;
}
const buildWsl = o => ["wsl", o.distro ? "-d " + o.distro : "", o.dir ? "--cd " + o.dir : "",
  o.shell ? "-- " + o.shell : ""].filter(Boolean).join(" ");

// ブラウザのコマンドを読む。持ち物はURLひとつ。
// 頭の語 (browser / web) は書いた人のものなので、そのまま残す
function parseBrowser(c) {
  const m = /^\s*(browser|web)\s+(\S.*)$/i.exec(cmdToText(c));
  return m ? {head: m[1], url: m[2].trim()} : null;
}
const buildBrowser = o => (o.head || "browser") + " " + (o.url || "");
/// 窓の中に置けるURLか。file: や data: は開けない
const openableUrl = u => /^https?:\/\/\S/i.test((u || "").trim());
const kindOf = c => parseBrowser(c) ? "browser"
  : parseSsh(c) ? "ssh" : parseDocker(c) ? "docker" : parseWsl(c) ? "wsl" : "cmd";
const KIND_LABEL = {cmd:T["settings.tab.command"], ssh:"SSH", docker:"Docker", wsl:"WSL",
  browser:T["settings.tab.kind.browser"]};
const KIND_START = {cmd:"", ssh:"ssh ", docker:"docker exec -it ", wsl:"wsl ",
  browser:"browser https://"};
const COMMON_COMMANDS = [
  {label:"Claude Code", cmd:"claude",         check:"claude"},
  {label:"Codex CLI",   cmd:"codex",          check:"codex"},
  {label:"Gemini CLI",  cmd:"gemini",         check:"gemini"},
  {label:"PowerShell",  cmd:"powershell.exe", check:null},
  {label:T["settings.tab.kind.cmdprompt"], cmd:"cmd.exe", check:null},
];
const cmdToText = c => Array.isArray(c) ? c.join(" ") : (c || "");

// ── サイドバー ───────────────────────────────────────
function renderNav() {
  const nav = document.getElementById("nav");
  nav.textContent = "";
  nav.append(el("button", {class:"navitem" + (sel.global ? " sel" : ""),
    onclick:() => { sel = {ws:sel.ws, tab:null, global:true}; render(); }}, T["settings.global"]));

  wss.forEach((ws, wi) => {
    nav.append(el("div", {class:"navgroup"}, ws.name || T["settings.tab.unnamed"]));
    nav.append(el("button", {class:"navitem" + (!sel.global && sel.ws === wi && sel.tab === null ? " sel" : ""),
      onclick:() => { sel = {ws:wi, tab:null, global:false}; render(); }},
      el("span", {}, T["settings.workspace.settings"])));
    (ws.tabs || []).forEach((t, ti) => {
      const b = el("button", {class:"navitem navtab" + (t.depth ? " child" : "") +
        (!sel.global && sel.ws === wi && sel.tab === ti ? " sel" : ""),
        onclick:() => { sel = {ws:wi, tab:ti, global:false}; render(); }});
      b.append(el("span", {}, (t.depth ? "└ " : "") + (t.name || T["settings.tab.unnamed"])));
      b.append(el("span", {class:"sub"}, cmdToText(t.command) || T["automation.unset"]));
      nav.append(b);
    });
    nav.append(el("button", {class:"navitem navtab navadd",
      onclick:() => { sel = {ws:wi, tab:addTabTo(ws), global:false}; render(); }},
      T["settings.tab.add"]));
  });
  nav.append(el("div", {class:"navgroup"}, ""));
  nav.append(el("button", {class:"navitem navadd", onclick:addWs}, T["settings.workspace.add"]));
  nav.append(el("button", {class:"navitem navadd", onclick:importWs}, T["settings.ws.import.nav"]));
}

const newTab = (o = {}) => Object.assign(
  {name:"", id:"", command:"", profile:"", automation:"", locked:false, auto_restart:false,
   cwd:"", encoding:"", scrollback:"", log:false, depth:0}, o);

// 名前もコマンドも空のタブ (作りかけ) の番号。無ければ -1。
// 「タブを追加」を連打しても、まだ何も書いていないタブがあれば
// そこへ移るだけにして、空のタブが積み上がらないようにする
function firstEmptyTab(ws) {
  return (ws.tabs || []).findIndex(t =>
    !(t.name || "").trim() && !(t.command || "").trim() && !(t.id || "").trim());
}

// タブを1つ増やす。ただし作りかけの空タブがあるなら、それを選ぶだけ。
// 増やした(または見つけた)タブの番号を返す
function addTabTo(ws) {
  ws.tabs = ws.tabs || [];
  let i = firstEmptyTab(ws);
  if (i < 0) { ws.tabs.push(newTab()); i = ws.tabs.length - 1; }
  return i;
}

function addWs() {
  wss.push({name:T["settings.workspace"], automation:"", tabs:[]});
  sel = {ws:wss.length - 1, tab:null, global:false};
  render();
}

// ── 詳細ペイン ───────────────────────────────────────
function render() { renderNav(); renderDetail(); }

function renderDetail() {
  const d = document.getElementById("detail");
  d.textContent = "";
  if (sel.global) return d.append(globalPane());
  const ws = wss[sel.ws];
  if (!ws) return;
  if (sel.tab === null) return d.append(wsPane(ws));
  const t = ws.tabs[sel.tab];
  if (!t) { sel.tab = null; return renderDetail(); }
  d.append(tabPane(ws, t));
}

function globalPane() {
  const box = el("div");
  box.append(card(T["settings.tab.basic"],
    row(T["settings.tabbar_width"], field(current, "tab_bar_width", T["settings.tab.automation_dir.ph"], {type:"number", width:110, grow:false}),
        el("span", {class:"hint"}, T["settings.tabbar_width.hint"])),
    row(T["settings.max_chain"], field(current, "max_chain", "10", {type:"number", width:110, grow:false}),
        el("span", {class:"hint"}, T["settings.max_chain.hint"])),
    row(T["settings.done_confirm"], secondsField(current, "done_confirm_ms", 10),
        el("span", {class:"hint"}, T["settings.done_confirm.hint"])),
    row(T["settings.follow"], checkDefaultOn(current, "follow_ball", T["settings.follow.label"]),
        el("span", {class:"hint"}, T["settings.follow.hint"])),
    row(T["settings.restore_ws"], checkDefaultOn(current, "restore_workspace", T["settings.restore_ws.label"]),
        el("span", {class:"hint"}, T["settings.restore_ws.hint"])),
    row(T["settings.ai_engine"], aiSelect(),
        el("span", {class:"hint", id:"aihint"}, "")),
    row(T["settings.browser_data"],
        choose(current, "browser_data", [
          ["", T["settings.browser_data.local"] || "このPCだけ (推奨)"],
          ["portable", T["settings.browser_data.portable"] || "全PCで共有 (Drive同期)"],
        ]),
        el("span", {class:"hint"}, T["settings.browser_data.hint"] || ""))));
  box.append(remoteCard());
  box.append(card(T["settings.section.files"],
    row(T["settings.automation_global"], ...pathField(current, "automation", "scripts/common", "dir",
        T["settings.tab.automation_dir.pick"]),
        el("span", {class:"hint"}, T["settings.automation_global.hint"])),
    row("secrets", ...pathField(current, "secrets", "secrets.json", "file",
        T["settings.secrets"]),
        el("span", {class:"hint"}, T["settings.secrets.hint"]))));
  box.append(secretsCard());
  return box;
}

// 秘密 (GitHub Secrets 相当)。キーで参照し、値は保存したら二度と表示されない。
// マスターパスワードがあれば暗号化、無ければ平文 (自己責任) — どちらも同じUIで扱う
function secretsCard() {
  const status = el("div", {class:"hint", id:"secretsmode"});
  const listBox = el("div", {id:"secretslist"}, el("div", {class:"hint"}, T["common.reload"] ? "…" : "…"));
  const keyIn = el("input", {class:"mono", placeholder:"キー 例: diary_saas", style:"width:200px"});
  const descIn = el("input", {placeholder:"説明 例: 日記SaaSのログイン", style:"flex:1;min-width:160px"});
  const valIn = el("input", {type:"password", placeholder:"値（保存後は二度と表示されません）", style:"flex:1;min-width:160px"});
  const addBtn = el("button", {class:"primary", onclick: async () => {
    const key = keyIn.value.trim();
    if (!key) { toast("キーを入れてください", true); return; }
    if (!valIn.value) { toast("値を入れてください", true); return; }
    const r = await fetch("/api/secrets/set", {method:"POST",
      headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
      body: JSON.stringify({key, description: descIn.value, value: valIn.value})}).then(r=>r.json());
    if (r.ok) { toast("保存しました: " + key); keyIn.value=""; descIn.value=""; valIn.value=""; loadSecrets(); }
    else toast(r.error || "保存に失敗", true);
  }}, "＋ 登録 / 更新");
  const form = el("div", {class:"row", style:"flex-wrap:wrap;gap:8px;margin-top:12px;align-items:center"},
    keyIn, descIn, valIn, addBtn);
  const c = card("秘密 (Secrets)", status, listBox, form);
  // カードがDOMに入ってから読み込む (getElementById が効くように)
  setTimeout(loadSecrets, 0);
  return c;
}

async function loadSecrets() {
  const listBox = document.getElementById("secretslist");
  const status = document.getElementById("secretsmode");
  if (!listBox) return;
  let j;
  try { j = await fetch("/api/secrets", {headers:{"X-Token":TOKEN}}).then(r=>r.json()); }
  catch (e) { listBox.textContent=""; listBox.append(el("div",{class:"hint warn"},"読み込み失敗")); return; }
  const modes = {
    plaintext: "🔓 平文で保存中（マスターパスワード未設定）。より安全にするには INDEX の [k] で設定できます",
    encrypted: "🔒 暗号化して保存中（マスターパスワード設定済み）",
    locked: "🔒 暗号化されています。編集にはアプリ側でマスターパスワードの入力が必要です",
    empty: "🔓 まだ何も登録されていません（登録すると secrets.json が作られます）",
  };
  status.textContent = modes[j.mode] || "";
  status.classList.toggle("warn", j.mode === "locked");
  listBox.textContent = "";
  if (!j.secrets || !j.secrets.length) {
    if (j.mode !== "empty" && j.mode !== "locked")
      listBox.append(el("div", {class:"hint"}, "登録された秘密はありません"));
    return;
  }
  for (const s of j.secrets) {
    const del = el("button", {class:"quiet", onclick: async () => {
      if (!confirm(s.key + " を削除しますか？")) return;
      const r = await fetch("/api/secrets/delete", {method:"POST",
        headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
        body: JSON.stringify({key: s.key})}).then(r=>r.json());
      if (r.ok) { toast("削除しました: " + s.key); loadSecrets(); }
      else toast(r.error || "削除に失敗", true);
    }}, "削除");
    listBox.append(el("div", {class:"row",
      style:"align-items:center;gap:10px;padding:7px 0;border-bottom:1px solid var(--line)"},
      el("span", {class:"mono", style:"min-width:180px;color:var(--text)"}, s.key),
      el("span", {class:"hint", style:"flex:1"}, s.description || "(説明なし)"),
      el("span", {class:"hint mono", title:"値は表示されません"}, "••••"),
      del));
  }
}
// スマホから使う設定。危険性は隠さず説明したうえで、1クリックで有効にできるようにする
function remoteCard() {
  current.remote = current.remote || {};
  const r = current.remote;
  const box = el("div", {class:"card"}, el("h2", {}, T["settings.section.phone"]));
  const status = el("div", {class:"hint"}, T["settings.phone.checking"]);
  const qrbox = el("div", {style:"margin:10px 0"});

  const onoff = el("input", {type:"checkbox"});
  onoff.checked = !!r.enabled;
  onoff.addEventListener("change", async () => {
    r.enabled = onoff.checked;
    if (r.enabled) { if (!r.bind) r.bind = "auto"; if (!r.port) r.port = 8787; }
    await save();          // 保存すると本体がすぐ待ち受けを開始/停止する
    setTimeout(refreshRemote, 1200);
  });
  const l = el("label", {class:"check"});
  l.append(onoff, document.createTextNode(T["settings.phone.enable.label"]));

  box.append(el("div", {class:"row"}, el("label", {}, T["settings.phone.enable"]), l));
  box.append(el("div", {class:"row"}, el("label", {}, T["settings.phone.port"]),
    (() => {
      const i = el("input", {type:"number", style:"width:110px"});
      i.value = r.port || 8787;
      i.addEventListener("input", () => { r.port = Number(i.value) || 8787; });
      return i;
    })(),
    el("span", {class:"hint"}, T["settings.phone.port.hint"])));
  box.append(el("div", {class:"row"}, status));
  box.append(qrbox);
  box.append(el("div", {class:"hint", style:"margin-top:6px"},
    T["settings.phone.note"]));

  refreshRemote();
  async function refreshRemote() {
    let j = {};
    try { j = await (await fetch("/api/remote", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    const net = j.tailscale ? fill(T["settings.phone.tailscale"], {ip: j.tailscale})
              : j.lan ? fill(T["settings.phone.lan"], {ip: j.lan})
              : T["settings.phone.none"];
    const head = j.running ? T["settings.phone.listening"] : T["settings.phone.stopped"];
    status.textContent = head + " — " + net + (j.note ? " / " + j.note : "");
    status.style.color = j.running ? "var(--accent)" : "var(--muted)";
    qrbox.textContent = "";
    if (j.running && j.url) {
      // 画像は fetch ではなく直接読み込まれるので、認証はURLのtokenで渡す
      const img = el("img", {src:"/api/remote/qr?token=" + encodeURIComponent(TOKEN),
        style:"width:200px;height:200px;border-radius:8px;background:#fff;padding:6px"});
      qrbox.append(el("div", {class:"hint"}, T["settings.phone.scan"]), img,
        el("div", {class:"hint mono", style:"word-break:break-all"}, j.url));
    }
  }
  return box;
}

function aiSelect() {
  const s = el("select", {id:"aiengine"});
  const hint = () => document.getElementById("aihint");
  if (!aiEngines.length) {
    s.append(el("option", {value:""}, T["settings.ai_engine.none"])); s.disabled = true;
  } else {
    s.append(el("option", {value:""}, T["settings.ai_engine.auto"]));
    for (const e of aiEngines) s.append(el("option", {value:e.id}, e.label));
  }
  s.value = current.ai_engine || "";
  s.addEventListener("change", () => { current.ai_engine = s.value; });
  setTimeout(() => { const h = hint(); if (h) h.textContent = aiEngines.length
    ? "" : T["settings.ai_engine.missing"]; }, 0);
  return s;
}

function wsPane(ws) {
  const box = el("div");
  box.append(card(T["settings.workspace"],
    row(T["settings.workspace.name"], field(ws, "name", T["settings.workspace.name"], {grow:false, width:280,
        onInput:() => renderNav()})),
    ws.file ? row(T["settings.workspace.file"], el("span", {class:"hint mono"}, ws.file)) : null,
    row(T["settings.tab.automation"], ...pathField(ws, "automation", T["settings.workspace.automation.hint"], "dir",
        T["settings.tab.automation_dir.pick"]),
        el("span", {class:"hint"}, T["settings.workspace.automation.hint"]))));

  if (!(ws.tabs || []).length) {
    const e = el("div", {class:"empty"},
      el("div", {class:"big"}, T["settings.template.empty"]),
      el("div", {}, T["settings.template.hint"]));
    const bar = el("div", {class:"row", style:"justify-content:center"});
    for (const [k, label] of [["single",T["settings.template.single"]],["review",T["settings.template.review"]],
                              ["ssh",T["settings.template.ssh"]],["docker",T["settings.template.docker"]],["wsl",T["settings.template.wsl"]]])
      bar.append(el("button", {onclick:() => addTemplate(k)}, label));
    e.append(bar);
    box.append(e);
  }
  box.append(card(T["settings.ws.share"],
    el("div", {class:"row"},
      el("button", {onclick:() => exportWs(sel.ws)}, T["settings.ws.export"]),
      el("button", {onclick:importWs}, T["settings.ws.import"])),
    el("div", {class:"hint"}, T["settings.ws.share.hint"])));

  box.append(el("div", {class:"row"},
    el("button", {class:"danger", onclick:() => {
      if (confirm(fill(T["settings.workspace.delete_confirm"], {name: ws.name}))) {
        wss.splice(sel.ws, 1); sel = {ws:0, tab:null, global:true}; render();
      }
    }}, T["settings.workspace.delete"])));
  return box;
}

// 書き出しも取り込みも、ディスクにある設定が相手。
// 編集中の姿を書き出すと、渡した相手だけが持っている設定ができてしまう
function savedAlready() {
  if (snapshot() === savedSnapshot) return true;
  result(T["settings.ws.save_first"], true);
  return false;
}

const wsShare = (path, body) => fetch(path, {method:"POST",
  headers:{"Content-Type":"application/json", "X-Token":TOKEN}, body})
  .then(r => r.json()).catch(e => ({ok:false, error:e.message || e}));

async function exportWs(i) {
  if (!savedAlready()) return;
  const j = await wsShare("/api/workspace/export", JSON.stringify({index:i}));
  if (j.cancelled) return;
  if (!j.ok) return result(fill(T["settings.ws.export_failed"], {error:j.error || ""}), true);
  result(fill(T["settings.ws.exported"], {path:j.path}));
}

async function importWs() {
  if (!savedAlready()) return;
  const j = await wsShare("/api/workspace/import", null);
  if (j.cancelled) return;
  if (!j.ok) return result(fill(T["settings.ws.import_failed"], {error:j.error || ""}), true);
  // 設定はサーバ側で書き換わっている。画面はそれを読み直す
  await load();
  sel = {ws:wss.length - 1, tab:null, global:false};
  render();
  const moved = (j.moved || []).map(m => m[0] + " → " + m[1]).join(" / ");
  result(fill(T["settings.ws.imported"], {name:j.name, files:j.files})
    + (moved ? "  " + fill(T["settings.ws.imported.moved"], {moved}) : ""));
}

const TEMPLATES = {
  single: [ {name:"Claude", command:"claude"} ],
  review: [ {name:T["settings.template.tab.build"], command:"claude"},
            {name:T["settings.template.tab.review"], id:"reviewer", command:"codex", depth:1, locked:true} ],
  ssh:    [ {name:T["settings.template.tab.server"], command:"ssh user@example.com", profile:"claude", auto_restart:true} ],
  docker: [ {name:T["settings.template.tab.container"], command:"docker exec -it -w /app myapp bash", profile:"claude"} ],
  wsl:    [ {name:"Ubuntu", command:"wsl -d Ubuntu --cd /home/me/proj -- bash", profile:"claude"} ],
};
function addTemplate(kind) {
  const ws = wss[sel.ws];
  ws.tabs = (ws.tabs || []).concat(TEMPLATES[kind].map(x => newTab(x)));
  sel.tab = ws.tabs.length - TEMPLATES[kind].length;
  render();
  msg(T["settings.template.added"]);
}

function tabPane(ws, t) {
  const box = el("div");
  const kind = kindOf(t.command);

  // 基本: 名前とIDは identity なので隣に置く
  box.append(card(T["settings.tab.basic"],
    row(T["settings.tab.name"], field(t, "name", T["settings.tab.name.ph"], {grow:false, width:280,
        onInput:() => renderNav()})),
    row(T["settings.tab.id"], field(t, "id", T["settings.tab.id.ph"], {grow:false, width:280, mono:true}),
        el("span", {class:"hint"}, T["settings.tab.id.hint"]))));

  // 起動するもの
  const cmdRow = el("div", {class:"row"});
  const cmdInput = field(t, "command", T["settings.tab.command.ph"], {mono:true, onInput:() => renderNav()});
  cmdInput.setAttribute("list", "cmdlist");
  const detailBox = el("div");
  const rebuild = () => { detailBox.textContent = ""; detailBox.append(kindPanel(t, cmdInput, rebuild)); };
  cmdRow.append(el("label", {}, T["settings.tab.kind"]),
    choose({k:kind}, "k", Object.entries(KIND_LABEL), v => {
      t.command = KIND_START[v] || ""; cmdInput.value = t.command; rebuild(); renderNav();
    }));
  rebuild();
  box.append(card(T["settings.tab.launch"], cmdRow, detailBox,
    row(T["settings.tab.command"], cmdInput),
    row(T["settings.tab.cwd"], ...pathField(t, "cwd", T["settings.tab.cwd.ph"], "dir",
        T["settings.tab.cwd.pick"]),
        el("span", {class:"hint"}, T["settings.tab.cwd.hint"]))));

  // 自動化: 何が設定済みか一覧で分かるようにする
  const ev = el("div", {class:"events"});
  for (const [id, label, hint] of eventsFor(t).filter(e => e[0] !== "_shared")) {
    ev.append(el("div", {class:"event"},
      el("div", {class:"name"}, label, el("div", {class:"hint"}, hint)),
      el("span", {class:"state", id:"st-" + id}, "—"),
      el("button", {class:"quiet", onclick:() => openAuto(ws, t, id)}, T["common.edit"])));
  }
  box.append(card(T["settings.tab.automation"], ev));
  loadAutoStates(ws, t);

  // 詳細: めったに触らないものは畳む
  const det = el("details");
  det.append(el("summary", {}, T["settings.tab.details"]));
  det.append(
    row(T["settings.tab.profile"], field(t, "profile", T["settings.tab.profile.ph"], {grow:false, width:220}),
        el("span", {class:"hint"}, T["settings.tab.profile.hint"])),
    row(T["settings.tab.automation_dir"], ...pathField(t, "automation", T["settings.tab.automation_dir.ph"], "dir",
        T["settings.tab.automation_dir.pick"])),
    row(T["settings.tab.encoding"], choose(t, "encoding",
        [["",T["settings.tab.encoding.utf8"]],["shift_jis","Shift_JIS"],["euc-jp","EUC-JP"]])),
    row(T["settings.tab.scrollback"], field(t, "scrollback", "5000", {type:"number", width:120, grow:false})),
    el("div", {class:"row"}, el("label", {}, T["settings.tab.behavior"]),
       check(t, "locked", T["settings.tab.locked"]),
       check(t, "auto_restart", T["settings.tab.auto_restart"]),
       check(t, "log", T["settings.tab.log"])),
    el("div", {class:"row"}, el("label", {}, T["settings.tab.order"]),
       el("button", {class:"quiet", onclick:() => moveTab(ws, -1)}, T["settings.tab.move_up"]),
       el("button", {class:"quiet", onclick:() => moveTab(ws, 1)}, T["settings.tab.move_down"]),
       el("button", {class:"quiet", onclick:() => { t.depth = Math.min((t.depth||0)+1, sel.tab); render(); }}, T["settings.tab.indent"]),
       el("button", {class:"quiet", onclick:() => { t.depth = Math.max((t.depth||0)-1, 0); render(); }}, T["settings.tab.outdent"])));
  box.append(el("div", {class:"card"}, det));

  box.append(el("div", {class:"row"},
    el("button", {class:"danger", onclick:() => {
      if (confirm(fill(T["settings.tab.delete_confirm"], {name: t.name || T["settings.tab.unnamed"]}))) {
        ws.tabs.splice(sel.tab, 1); sel.tab = null; render();
      }
    }}, T["settings.tab.delete"])));
  return box;
}

function moveTab(ws, d) {
  const i = sel.tab, j = i + d;
  if (j < 0 || j >= ws.tabs.length) return;
  [ws.tabs[i], ws.tabs[j]] = [ws.tabs[j], ws.tabs[i]];
  sel.tab = j; render();
}

/// 種類ごとの入力補助 (SSH / Docker / WSL)
function kindPanel(t, cmdInput, rebuild) {
  const box = el("div");
  const ssh = parseSsh(t.command), dk = parseDocker(t.command), wsl = parseWsl(t.command);
  const web = parseBrowser(t.command);
  const sync = (build, o) => () => {
    t.command = build(o); cmdInput.value = t.command; renderNav();
  };
  const f = (obj, key, label, ph, upd, w) => {
    const i = el("input", {type:"text", placeholder:ph, class:"mono"});
    if (w) i.style.width = w + "px";
    i.value = obj[key] || "";
    i.addEventListener("input", () => { obj[key] = i.value.trim(); upd(); });
    return [el("label", {}, label), i];
  };
  if (ssh) {
    const upd = sync(buildSsh, ssh);
    box.append(el("div", {class:"row"}, ...f(ssh, "host", T["settings.ssh.host"], "example.com", upd, 240),
      el("label", {style:"width:auto"}, T["settings.phone.port"]),
      (() => { const i = el("input", {type:"text", class:"mono", style:"width:70px"});
               i.value = ssh.port || ""; i.placeholder = "22";
               i.addEventListener("input", () => { ssh.port = i.value.trim(); upd(); }); return i; })(),
      el("label", {style:"width:auto"}, T["settings.ssh.user"]),
      (() => { const i = el("input", {type:"text", class:"mono", style:"width:130px"});
               i.value = ssh.user || ""; i.placeholder = "root";
               i.addEventListener("input", () => { ssh.user = i.value.trim(); upd(); }); return i; })()));
    const keyIn = el("input", {type:"text", class:"mono grow", placeholder:T["settings.ssh.key.ph"]});
    keyIn.value = ssh.key || "";
    keyIn.addEventListener("input", () => { ssh.key = keyIn.value.trim(); upd(); });
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.ssh.key"]), keyIn,
      el("button", {class:"quiet", onclick: async () => {
        const p = await pickPath("key", T["settings.ssh.key.pick"], ssh.key);
        if (p !== null) { ssh.key = p; keyIn.value = p; upd(); }
      }}, T["common.browse"])));
    const adv = el("details"); adv.append(el("summary", {}, T["settings.ssh.details"]));
    const fwd = el("input", {type:"text", class:"mono grow",
      placeholder:T["settings.ssh.forward.ph"]});
    fwd.value = (ssh.forwards || []).join(", ");
    fwd.addEventListener("input", () => {
      ssh.forwards = fwd.value.split(",").map(s => s.trim()).filter(Boolean); upd(); });
    adv.append(el("div", {class:"row"}, el("label", {}, T["settings.ssh.forward"]), fwd),
      el("div", {class:"row"}, ...f(ssh, "jump", T["settings.ssh.jump"], "gw.example.com", upd, 200),
        ...f(ssh, "keepalive", T["settings.ssh.keepalive"], "60", upd, 80)),
      el("div", {class:"row"}, el("label", {}, T["settings.ssh.allow"]),
        (() => { const c = el("input", {type:"checkbox"}); c.checked = ssh.agent;
          c.addEventListener("change", () => { ssh.agent = c.checked; upd(); });
          const l = el("label", {class:"check"}); l.append(c, document.createTextNode(T["settings.ssh.agent"]));
          return l; })(),
        (() => { const c = el("input", {type:"checkbox"}); c.checked = ssh.x11;
          c.addEventListener("change", () => { ssh.x11 = c.checked; upd(); });
          const l = el("label", {class:"check"}); l.append(c, document.createTextNode(T["settings.ssh.x11"]));
          return l; })()));
    box.append(adv);
  } else if (web) {
    const upd = sync(buildBrowser, web);
    const u = el("input", {type:"text", class:"mono grow",
      placeholder:"https://example.com/"});
    u.value = web.url || "";
    // 開けないURLは、開いてから気づくより、書いている最中に言う方がいい
    const note = el("span", {class:"hint"});
    const check = () => {
      const bad = web.url && !openableUrl(web.url);
      note.textContent = bad ? T["settings.browser.url.bad"] : T["settings.browser.url.hint"];
      note.style.color = bad ? "var(--danger)" : "";
    };
    u.addEventListener("input", () => { web.url = u.value.trim(); check(); upd(); });
    check();
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.browser.url"]), u));
    box.append(el("div", {class:"row"}, el("label", {}, ""), note));

    // ページの上に出す操作。出したものだけが効く
    t.nav = t.nav || {};
    const part = (key, label) => {
      const c = el("input", {type:"checkbox"});
      c.checked = !!t.nav[key];
      c.addEventListener("change", () => { t.nav[key] = c.checked; });
      const l = el("label", {class:"check"});
      l.append(c, document.createTextNode(label));
      return l;
    };
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.browser.nav"]),
      part("back", T["tui.nav.back"]),
      part("forward", T["tui.nav.forward"]),
      part("reload", T["tui.nav.reload"]),
      part("url", T["tui.nav.url"])));
    box.append(el("div", {class:"row"}, el("label", {}, ""),
      el("span", {class:"hint"}, T["settings.browser.nav.hint"])));

    // 下に出す帯。チェック1つでは足りない (文言とボタンの字が要る) ので、
    // 「出す」を入れたときだけ中身の欄を出す
    const askOn = el("input", {type:"checkbox"});
    askOn.checked = !!t.ask;
    const askLabel = el("label", {class:"check"});
    askLabel.append(askOn, document.createTextNode(T["settings.browser.ask.on"]));
    const askBody = el("div");
    const drawAsk = () => {
      askBody.textContent = "";
      if (!t.ask) return;
      askBody.append(
        el("div", {class:"row"}, el("label", {}, T["settings.browser.ask.text"]),
          field(t.ask, "text", T["settings.browser.ask.text.ph"])),
        el("div", {class:"row"}, el("label", {}, T["settings.browser.ask.label"]),
          field(t.ask, "label", T["tui.ask.label"], {grow:false, width:200})));
    };
    askOn.addEventListener("change", () => {
      t.ask = askOn.checked ? (t.ask || {text:"", label:""}) : null;
      drawAsk();
    });
    drawAsk();
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.browser.ask"]), askLabel));
    box.append(askBody);
    box.append(el("div", {class:"row"}, el("label", {}, ""),
      el("span", {class:"hint"}, T["settings.browser.ask.hint"])));
  } else if (dk || wsl) {
    const o = dk || wsl, upd = sync(dk ? buildDocker : buildWsl, o);
    box.append(el("div", {class:"row"},
      ...(dk ? f(o, "container", T["settings.container.name"], "myapp", upd, 200)
             : f(o, "distro", T["settings.container.distro"], "Ubuntu", upd, 200)),
      ...f(o, "dir", T["settings.container.dir"], "/home/me/proj", upd, 220)));
    box.append(el("div", {class:"row"},
      ...f(o, "shell", T["settings.container.shell"], "bash / claude", upd, 220),
      el("span", {class:"hint"}, T["settings.container.hint"])));
  } else {
    const s = el("select");
    s.append(el("option", {value:""}, T["settings.tab.common.pick"]));
    for (const c of COMMON_COMMANDS) {
      const ok = c.check ? aiEngines.some(e => e.id === c.check) : true;
      s.append(el("option", {value:c.cmd}, c.label + (c.check && !ok ? T["settings.tab.common.missing"] : "")));
    }
    s.addEventListener("change", () => {
      if (!s.value) return;
      t.command = s.value; cmdInput.value = s.value; s.value = ""; renderNav();
    });
    box.append(el("div", {class:"row"}, el("label", {}, T["settings.tab.common"]), s));
  }
  return box;
}

// ── 自動化エディタ ───────────────────────────────────
// セッションのフック。ブラウザには一つも飛ばない
const TAB_EVENTS = [
  ["on_start",    T["automation.on_start"],           T["automation.on_start.hint"]],
  ["on_done",     T["automation.on_done"],     T["automation.on_done.hint"]],
  ["on_question", T["automation.on_question"],     T["automation.on_question.hint"]],
  ["on_exit",     T["automation.on_exit"],           T["automation.on_exit.hint"]],
  ["on_busy",     T["automation.on_busy"],     T["automation.on_busy.hint"]],
  ["_shared",     T["automation._shared"],       ""],
];
// ブラウザのフック。ページには状態が無いので、言葉が違う
const PAGE_EVENTS = [
  ["on_load",     T["automation.on_load"],     T["automation.on_load.hint"]],
  ["on_press",    T["automation.on_press"],    T["automation.on_press.hint"]],
  ["_shared",     T["automation._shared"],       ""],
];
// そのタブで本当に飛ぶものだけを並べる。
// 書ける場所があるのに動かないのは、無いより悪い
const eventsFor = t => kindOf(t.command) === "browser" ? PAGE_EVENTS : TAB_EVENTS;
let autoTarget = null, autoData = {}, autoEvent = "on_done";

function autoDirOf(ws, t) {
  if (t.automation) return t.automation;
  const slug = s => (s || "").replace(/[^A-Za-z0-9_-]/g, "").toLowerCase();
  const wi = wss.indexOf(ws) + 1, ti = (ws.tabs || []).indexOf(t) + 1;
  return "scripts/" + (slug(ws.name) || ("ws" + wi)) + "/" + (slug(t.id) || slug(t.name) || ("tab" + ti));
}

async function fetchAuto(dir) {
  try {
    return await (await fetch("/api/automation?dir=" + encodeURIComponent(dir),
        {headers:{"X-Token":TOKEN}})).json();
  } catch (e) { return {}; }
}
async function loadAutoStates(ws, t) {
  const data = await fetchAuto(autoDirOf(ws, t));
  for (const [id] of eventsFor(t)) {
    const s = document.getElementById("st-" + id);
    if (!s) continue;
    const on = (data[id] || "").trim().length > 0;
    s.textContent = on ? T["automation.set"] : T["automation.unset"];
    s.className = "state" + (on ? " on" : "");
  }
}

async function openAuto(ws, t, event) {
  autoTarget = { ws, t, dir: autoDirOf(ws, t) };
  document.getElementById("autotitle").textContent =
      fill(T["automation.editor.title"], {name: t.name || T["settings.tab.unnamed"]});
  document.getElementById("autopath").textContent = autoTarget.dir;
  const s = document.getElementById("autoevent");
  s.textContent = "";
  const events = eventsFor(t);
  for (const [id, label] of events) s.append(el("option", {value:id}, label));
  autoData = await fetchAuto(autoTarget.dir);
  // showEvent を使う (switchEvent だと、まだ前のタブの内容が入っている
  // テキストエリアを新しいタブのデータとして取り込んでしまう)
  // 既定は、そのタブで最初に並ぶもの (ブラウザなら on_load)
  showEvent(event || events[0][0]);
  document.getElementById("airow").style.display = aiEngines.length ? "flex" : "none";
  document.getElementById("ainone").style.display = aiEngines.length ? "none" : "flex";
  document.getElementById("aipreview").style.display = "none";
  // AIへの依頼文と生成結果も前のタブのものを残さない
  document.getElementById("autoask").value = "";
  document.getElementById("aicode").textContent = "";
  aiBusy(false);
  automsg("");
  document.getElementById("autobox").style.display = "flex";
}
// イベントを切り替える。今表示している内容は失わないよう先に退避する
function switchEvent() {
  autoData[autoEvent] = document.getElementById("autocode").value;
  showEvent(document.getElementById("autoevent").value);
}

// 退避せずに表示だけ差し替える。
// 開いた直後は前のタブの内容が残っているので、退避してはいけない
function showEvent(id) {
  autoEvent = id;
  document.getElementById("autoevent").value = id;
  document.getElementById("autocode").value = autoData[autoEvent] || "";
  const list = autoTarget ? eventsFor(autoTarget.t) : TAB_EVENTS;
  const e = list.find(x => x[0] === autoEvent);
  document.getElementById("autohint").textContent = e ? e[2] : "";
}
function closeAuto() { document.getElementById("autobox").style.display = "none"; }

async function saveAuto() {
  autoData[autoEvent] = document.getElementById("autocode").value;
  const r = await fetch("/api/automation?dir=" + encodeURIComponent(autoTarget.dir),
      {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
       body: JSON.stringify(autoData)});
  if (!r.ok) return automsg(T["automation.editor.save_failed"], true);
  const created = autoTarget.t.automation !== autoTarget.dir;
  autoTarget.t.automation = autoTarget.dir;
  closeAuto();
  loadAutoStates(autoTarget.ws, autoTarget.t);
  msg(created ? T["automation.editor.saved_new"] : T["automation.editor.saved"]);
}

// 生成中の見た目。経過秒を出すのは、止まっているのか考えているのかの差が
// 待つ側にとっていちばん重要だから
let aiTimer = null;
function aiBusy(on) {
  clearInterval(aiTimer);
  const btn = document.getElementById("aibtn");
  btn.disabled = on;
  btn.textContent = T[on ? "automation.editor.generating" : "automation.editor.generate"];
  document.getElementById("autoask").disabled = on;
  document.getElementById("aibusy").style.display = on ? "flex" : "none";
  if (!on) return;
  const started = Date.now();
  const tick = () => {
    document.getElementById("aibusytext").textContent =
      fill(T["automation.editor.thinking"], {sec: Math.round((Date.now() - started) / 1000)});
  };
  tick();
  aiTimer = setInterval(tick, 1000);
}

async function askAi() {
  const want = document.getElementById("autoask").value.trim();
  if (!want) return automsg(T["automation.editor.want"], true);
  const ws = autoTarget.ws;
  automsg("");
  aiBusy(true);
  try {
    const r = await fetch("/api/generate", {method:"POST",
        headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
        body: JSON.stringify({event: autoEvent, prompt: want,
          engine: current.ai_engine || null,
          tabs: (ws.tabs || []).map((x, i) => ({index:i+1, name:x.name || fill(T["settings.tab.default_name"], {n: i+1}), id:x.id || ""})),
          self: (ws.tabs || []).indexOf(autoTarget.t) + 1})});
    const j = await r.json();
    if (!j.ok) return automsg(fill(T["automation.editor.failed"], {error: j.error}), true);
    document.getElementById("aicode").textContent = j.code;
    document.getElementById("aipreview").style.display = "block";
    automsg(T["automation.editor.check"]);
  } catch (e) {
    // 通信ごと失敗した場合、今までは黙って終わっていた
    automsg(fill(T["automation.editor.failed"], {error: e.message || e}), true);
  } finally {
    aiBusy(false);
  }
}
function applyAi() {
  document.getElementById("autocode").value = document.getElementById("aicode").textContent;
  document.getElementById("aipreview").style.display = "none";
  automsg(T["automation.editor.applied"]);
}
function automsg(t, warn) { const m = document.getElementById("automsg");
  m.textContent = t; m.style.color = warn ? "var(--danger)" : "var(--muted)"; }

// ── 読み込み / 保存 ──────────────────────────────────
function flatten(tabs, depth, out) {
  for (const t of tabs || []) {
    out.push({ name: t.name || "", id: t.id || "", command: cmdToText(t.command),
               profile: t.profile || "", automation: t.automation || t.lua || "",
               locked: !!t.locked, auto_restart: !!t.auto_restart, cwd: t.cwd || "",
               encoding: t.encoding || "", scrollback: t.scrollback ?? "", log: !!t.log,
               nav: t.nav || null, ask: t.ask || null, depth });
    flatten(t.children, depth + 1, out);
  }
  return out;
}
function nest(flat) {
  const roots = [], stack = [];
  for (const f of flat) {
    const node = { name: f.name, command: f.command };
    if (f.id) node.id = f.id;
    if (f.profile) node.profile = f.profile;
    if (f.automation) node.automation = f.automation;
    if (f.locked) node.locked = true;
    if (f.auto_restart) node.auto_restart = true;
    if (f.cwd) node.cwd = f.cwd;
    if (f.encoding) node.encoding = f.encoding;
    if (f.scrollback) node.scrollback = Number(f.scrollback);
    if (f.log) node.log = true;
    // 1つも出さないなら書かない。false ばかりの塊を残しても読みにくいだけ
    if (f.nav && Object.values(f.nav).some(Boolean)) {
      node.nav = {};
      for (const k of ["back", "forward", "reload", "url"])
        if (f.nav[k]) node.nav[k] = true;
    }
    // 帯は書いてあること自体が「出す」の意味。空欄は書かない
    if (f.ask) {
      node.ask = {};
      if (f.ask.text) node.ask.text = f.ask.text;
      if (f.ask.label) node.ask.label = f.ask.label;
    }
    const d = Math.min(f.depth, stack.length);
    if (d === 0) roots.push(node);
    else (stack[d - 1].children = stack[d - 1].children || []).push(node);
    stack[d] = node; stack.length = d + 1;
  }
  return roots;
}

async function loadAi() {
  try { aiEngines = (await (await fetch("/api/ai", {headers:{"X-Token":TOKEN}})).json()).engines || []; }
  catch (e) { aiEngines = []; }
  const dl = document.getElementById("cmdlist");
  dl.textContent = "";
  for (const c of COMMON_COMMANDS) dl.append(el("option", {value:c.cmd}, c.label));
}

async function load() {
  await loadAi();
  current = await (await api("GET")).json();
  const list = (Array.isArray(current.workspaces) && current.workspaces.length)
      ? current.workspaces
      : [{ name:"DEFAULT", tabs: current.tabs || [] }];
  wss = [];
  for (const w of list) {
    const ws = { name:w.name || "", file:w.file || null,
                 automation:w.automation || w.lua || "", tabs:[],
                 // 画面では触らないが、保存で消さないために持っておく
                 browsers:w.browsers || null };
    if (ws.file) {
      const f = await (await wsApi("GET", ws.file)).json().catch(() => ({}));
      ws.tabs = flatten(f.tabs, 0, []);
      if (!ws.automation) ws.automation = f.automation || f.lua || "";
    } else ws.tabs = flatten(w.tabs, 0, []);
    wss.push(ws);
  }
  if (sel.ws >= wss.length) sel = {ws:0, tab:null, global:true};
  render();
  markClean();
  msg(T["common.loaded"]);
}

async function save() {
  const btn = document.getElementById("savebtn");
  btn.disabled = true;
  btn.classList.remove("dirty");
  btn.textContent = T["common.saving"];
  try {
    await doSave();
  } catch (e) {
    // 通信自体が失敗した場合、今までは何も出ないまま終わっていた
    result(fill(T["settings.save_failed"], {error: e.message || e}), true);
  } finally {
    btn.disabled = false;
    btn.textContent = T["common.save"];
    refreshSave();
  }
}

// 書き込む内容を組み立てる。保存と未保存判定の両方がこれを使う
function payload() {
  const out = Object.assign({}, current);
  ["tab_bar_width","max_chain"].forEach(k => {
    const v = out[k]; if (v === "" || v === null || v === undefined) delete out[k]; else out[k] = Number(v);
  });
  ["automation","secrets","ai_engine","browser_data"].forEach(k => { if (!out[k]) delete out[k]; });
  if (out.remote && !out.remote.enabled && !out.remote.allow_public) delete out.remote;
  delete out.lua; delete out.tabs;

  // 別ファイルに切り出されたワークスペースは、そのファイルへ書く
  const files = [];
  for (const w of wss) {
    if (!w.file) continue;
    const body = { name:w.name, tabs:nest(w.tabs) };
    if (w.automation) body.automation = w.automation;
    files.push({ file:w.file, body });
  }
  out.workspaces = wss.map(w => {
    const o = { name:w.name };
    if (w.file) o.file = w.file;
    else { if (w.automation) o.automation = w.automation; o.tabs = nest(w.tabs); }
    // 画面に無い設定を、画面から保存しただけで失わない
    if (w.browsers) o.browsers = w.browsers;
    return o;
  });
  return { out, files };
}

async function doSave() {
  const { out, files } = payload();
  for (const f of files) {
    const rf = await wsApi("POST", f.file, JSON.stringify(f.body, null, 2));
    const jf = await rf.json().catch(() => ({ok:false}));
    if (!jf.ok) { result(fill(T["settings.file_save_failed"], {file: f.file}), true); return; }
  }
  const r = await api("POST", JSON.stringify(out, null, 2));
  const j = await r.json();
  if (!j.ok) { result(fill(T["settings.save_failed"], {error: j.error}), true); return; }
  markClean();
  result(T["common.saved"]);
  // 保存したら用は済んでいる。開いたままだと盤面へ戻る道が
  // 「別のタブを押す」しかなく、設定画面が居座っているように見える
  goIndex();
}

// この画面は窓の中に置かれたページなので、本体へ直接ものが言える。
// 外のブラウザで開いているときは window.ipc が無いだけ (何も起きない)
function goIndex() {
  try { window.ipc.postMessage(JSON.stringify({kind:"select", tab:0})); } catch (e) {}
}

// 設定を閉じる。稼働盤(INDEX)へ戻り、設定タブごと畳んで左の一覧からも消す。
// 未保存なら、消えることを先に伝える
function closeSettings() {
  if (snapshot() !== savedSnapshot && !confirm(T["settings.back.confirm"])) return;
  try { window.ipc.postMessage(JSON.stringify({kind:"closesettings"})); } catch (e) { goIndex(); }
}

// URLに addtab=<ワークスペース番号> が付いていたら、読み込み後に
// そのワークスペースへタブを1つ足した状態で始める (タブバーの + から来る)
load().then(() => {
  const wi = Number(new URLSearchParams(location.search).get("addtab"));
  if (!Number.isInteger(wi) || !wss[wi]) return;
  sel = {ws:wi, tab:addTabTo(wss[wi]), global:false};
  render();
  const s = document.querySelector(".navitem.sel");
  if (s) s.scrollIntoView({block:"center"});
});
</script></body></html>
"##;

/// マニュアル表示ページ (Markdownの必要な部分だけを描画する簡易レンダラ)
const HELP_PAGE: &str = r##"<!doctype html>
<html lang="{{__lang__}}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1"><title>{{help.page.title}}</title>
<style>
 :root { color-scheme: dark; }
 body { background:#05080c; color:#d3e6f5; font-family:"Consolas","Meiryo",monospace;
        margin:0; padding:24px 32px; line-height:1.7; }
 h1,h2,h3 { color:#4ec9ff; border-bottom:1px solid #1d3a4d; padding-bottom:6px; }
 h1 { font-size:20px; } h2 { font-size:17px; margin-top:32px; } h3 { font-size:15px; }
 code { background:#0a1014; color:#ffc857; padding:1px 5px; border-radius:3px; }
 pre { background:#0a1014; border:1px solid #1d3a4d; padding:12px; overflow:auto; }
 pre code { color:#4ec9ff; background:none; padding:0; }
 table { border-collapse:collapse; margin:12px 0; }
 th,td { border:1px solid #1d3a4d; padding:5px 10px; text-align:left; }
 th { color:#00aaff; }
 hr { border:0; border-top:1px solid #1d3a4d; margin:28px 0; }
 a { color:#00aaff; }
</style></head><body><div id="doc"></div>
<script>
const MD = __MD__;
const esc = s => s.replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;");
const inline = s => esc(s)
   .replace(/`([^`]+)`/g, "<code>$1</code>")
   .replace(/\*\*([^*]+)\*\*/g, "<b>$1</b>");
function render(md) {
  const out = []; const lines = md.split(/\r?\n/);
  let i = 0;
  while (i < lines.length) {
    const l = lines[i];
    if (l.startsWith("```")) {                      // コードブロック
      const buf = []; i++;
      while (i < lines.length && !lines[i].startsWith("```")) buf.push(lines[i++]);
      i++; out.push("<pre><code>" + esc(buf.join("\n")) + "</code></pre>"); continue;
    }
    if (/^\|/.test(l)) {                            // 表
      const rows = [];
      while (i < lines.length && /^\|/.test(lines[i])) rows.push(lines[i++]);
      const cells = r => r.split("|").slice(1,-1).map(c => c.trim());
      let html = "<table>";
      rows.forEach((r, n) => {
        if (/^\|[\s:|-]+\|$/.test(r)) return;        // 区切り行
        const tag = n === 0 ? "th" : "td";
        html += "<tr>" + cells(r).map(c => `<${tag}>${inline(c)}</${tag}>`).join("") + "</tr>";
      });
      out.push(html + "</table>"); continue;
    }
    const h = l.match(/^(#{1,3})\s+(.*)$/);
    if (h) { const n = h[1].length; out.push(`<h${n}>${inline(h[2])}</h${n}>`); i++; continue; }
    if (/^---+$/.test(l)) { out.push("<hr>"); i++; continue; }
    if (/^[-*]\s+/.test(l)) {                        // 箇条書き
      const buf = [];
      while (i < lines.length && /^[-*]\s+/.test(lines[i]))
        buf.push("<li>" + inline(lines[i++].replace(/^[-*]\s+/, "")) + "</li>");
      out.push("<ul>" + buf.join("") + "</ul>"); continue;
    }
    if (l.trim() === "") { i++; continue; }
    const buf = [];
    while (i < lines.length && lines[i].trim() !== "" && !/^(#{1,3}\s|```|\||[-*]\s|---)/.test(lines[i]))
      buf.push(lines[i++]);
    out.push("<p>" + inline(buf.join(" ")) + "</p>");
  }
  return out.join("\n");
}
document.getElementById("doc").innerHTML = render(MD);
</script></body></html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_is_embedded_and_usable() {
        // どこから起動してもAIに渡す仕様書が手に入ること (言語を問わず)
        for (code, text) in EMBEDDED_MANUALS {
            assert!(text.contains("shikisha.send_to_tab"), "{code} の仕様書が空");
        }
        let m = load_manual(std::path::Path::new("/nonexistent/config.json"));
        assert!(m.contains("shikisha."), "埋め込みにフォールバックする");
    }

    /// 仕様書が、画面で選べるイベントを全部説明していること。
    ///
    /// この仕様書はAIへそのまま渡している。載っていないイベントを頼むと、
    /// AIは嘘をつかずに「仕様に無いので何もしません」と正しく答えてしまう。
    /// 機能を足したのに書き足し忘れると、その機能はAIから見えないまま残る
    #[test]
    fn the_manual_covers_every_event_the_screen_offers() {
        for (code, text) in EMBEDDED_MANUALS {
            for event in EVENT_FILES {
                // _shared はイベントではなく、共通の置き場
                if event == "_shared" {
                    continue;
                }
                assert!(
                    text.contains(event),
                    "{code} の仕様書に {event} の説明が無い (AIはこのイベントを知らないまま書く)"
                );
            }
        }
    }


    /// トップレベルの const/let が重複していないこと。
    /// 重複すると SyntaxError でスクリプト全体が動かず、静的なHTMLだけが残る。
    /// 画面は出ているのに何も動かないという、原因の分かりにくい壊れ方をする
    fn top_level_bindings(page: &str) -> Vec<String> {
        page.lines()
            .filter_map(|l| l.strip_prefix("const ").or_else(|| l.strip_prefix("let ")))
            .filter_map(|rest| rest.split(|c: char| !(c.is_alphanumeric() || c == '_')).next())
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect()
    }
    
    fn assert_no_duplicate_bindings(name: &str, page: &str) {
        let names = top_level_bindings(page);
        for (i, n) in names.iter().enumerate() {
            assert!(
                !names[..i].contains(n),
                "{name}: `{n}` がトップレベルで二重宣言されている (JS全体が動かなくなる)"
            );
        }
    }

    /// 画面に `{{key}}` や `__DICT__` がそのまま出ていないこと。
    /// 差し込み忘れは実行して初めて気づくので、ここで止める
    #[test]
    fn pages_are_fully_rendered() {
        for (name, page) in [("PAGE", PAGE), ("HELP_PAGE", HELP_PAGE)] {
            assert_no_duplicate_bindings(name, page);
            let html = crate::i18n::render(page)
                .replace("__TOKEN__", "t")
                .replace("__DICT__", "{}")
                .replace("__MD__", "\"\"");
            assert!(!html.contains("{{"), "{name} に未置換の {{{{key}}}} が残っている");
            assert!(!html.contains("__"), "{name} に未置換のプレースホルダが残っている");
            assert!(html.contains("<html lang=\"en\">"), "{name} の lang 属性");
        }
    }

    /// JSから引くキーがすべて辞書にあること (T["..."] が undefined にならない)
    #[test]
    fn page_script_only_uses_known_keys() {
        let en: serde_json::Value = serde_json::from_str(include_str!("../lang/en.json")).unwrap();
        let mut rest = PAGE;
        while let Some(i) = rest.find("T[\"") {
            rest = &rest[i + 3..];
            let key = &rest[..rest.find('"').unwrap()];
            assert!(en.get(key).is_some(), "lang/en.json に無いキー: {key}");
        }
    }

    #[test]
    fn token_compare_rejects_mismatch() {
        assert!(token_eq("abc123", "abc123"));
        assert!(!token_eq("abc123", "abc124"));
        assert!(!token_eq("abc", "abc123"));
    }

    #[test]
    fn token_is_parsed_from_query() {
        assert_eq!(query_token("/?token=deadbeef"), "deadbeef");
        assert_eq!(query_token("/api/config?x=1&token=zz"), "zz");
        assert_eq!(query_token("/"), "");
    }

    #[test]
    fn tab_layout_is_described_for_the_ai() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"self":1,"tabs":[{"index":1,"name":"実装"},{"index":2,"name":"検査"}]}"#,
        )
        .unwrap();
        let s = describe_tabs(&v);
        // 名前で指示できるよう、番号と名前の対応をAIに渡す
        assert!(s.contains("1. 実装"), "{s}");
        assert!(s.contains("2. 検査"), "{s}");
        assert!(s.contains("this script runs in"), "{s}");
        // タブ情報が無いときは何も足さない
        assert_eq!(describe_tabs(&serde_json::json!({})), "");
    }

    #[test]
    fn extracts_lua_from_ai_output() {
        // マーカー付き (期待する形)
        let s = "了解しました\n<<<LUA\nshikisha.log(\"hi\")\n>>>\n以上です";
        assert_eq!(extract_lua(s).unwrap(), "shikisha.log(\"hi\")");
        // コードフェンスのみでもコードらしければ受け入れる
        let s2 = "```lua\nshikisha.send_to_tab(1, tab.output)\n```";
        assert_eq!(
            extract_lua(s2).unwrap(),
            "shikisha.send_to_tab(1, tab.output)"
        );
        // 会話文だけならエラーにして、保存させない
        assert!(extract_lua("どのような自動化を作りますか？").is_err());
    }

    #[test]
    fn picked_paths_stay_portable_when_inside_the_config_folder() {
        let cfg = std::path::Path::new("D:/app/config.json");
        // 設定フォルダ配下は相対パスにする (フォルダごと持ち運べる)
        assert_eq!(
            display_path(std::path::Path::new("D:/app/scripts/reviewer"), cfg),
            "scripts/reviewer"
        );
        // 外にあるものは絶対パスのまま、区切りだけ揃える
        assert_eq!(
            display_path(std::path::Path::new("C:\\Users\\me\\.ssh\\id_ed25519"), cfg),
            "C:/Users/me/.ssh/id_ed25519"
        );
    }

    #[test]
    fn workspace_path_rejects_traversal() {
        let cfg = std::path::Path::new("C:/app/config.json");
        // 正常系
        assert!(safe_workspace_path("/api/workspace?file=workspaces/x.json", cfg).is_some());
        // パストラバーサル・絶対パス・非JSONは拒否
        assert!(safe_workspace_path("/api/workspace?file=../secrets.json", cfg).is_none());
        assert!(safe_workspace_path("/api/workspace?file=workspaces/../../x.json", cfg).is_none());
        assert!(safe_workspace_path("/api/workspace?file=C:/windows/x.json", cfg).is_none());
        assert!(safe_workspace_path("/api/workspace?file=workspaces/x.lua", cfg).is_none());
        // URLエンコードされた .. も拒否
        assert!(safe_workspace_path("/api/workspace?file=%2E%2E%2Fsecrets.json", cfg).is_none());
    }

    #[test]
    fn tokens_are_unique_and_long() {
        let a = random_token().unwrap();
        let b = random_token().unwrap();
        assert_eq!(a.len(), 48);
        assert_ne!(a, b);
    }

    /// 実際にサーバーを起動し、認証と保存の動作を確認する
    #[test]
    fn server_requires_token_and_saves_valid_json() {
        let dir = std::env::temp_dir().join("shikisha-webui-test");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join("config.json");
        std::fs::write(&cfg, r#"{"max_chain":10}"#).unwrap();

        let ui = WebUi::start_with(
            cfg.clone(),
            Arc::new(std::sync::Mutex::new(RemoteInfo::default())),
            Arc::new(std::sync::Mutex::new(None)),
        )
        .unwrap();
        let token = ui.url.split("token=").nth(1).unwrap().to_string();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let agent = ureq::Agent::new_with_defaults();

        // トークン無し → 403
        let status = agent
            .get(&format!("{base}/api/config"))
            .call()
            .map(|r| r.status().as_u16())
            .unwrap_or_else(|e| match e {
                ureq::Error::StatusCode(c) => c,
                other => panic!("unexpected: {other}"),
            });
        assert_eq!(status, 403, "トークン無しは拒否される");

        // 正しいトークン → 現在の設定が読める
        let body = agent
            .get(&format!("{base}/api/config?token={token}"))
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        assert!(body.contains("max_chain"));

        // 壊れたJSONは保存されない
        let bad = agent
            .post(&format!("{base}/api/config?token={token}"))
            .send("{ broken")
            .map(|r| r.status().as_u16())
            .unwrap_or_else(|e| match e {
                ureq::Error::StatusCode(c) => c,
                other => panic!("unexpected: {other}"),
            });
        assert_eq!(bad, 400);
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            r#"{"max_chain":10}"#,
            "検証に失敗したら元の設定は保たれる"
        );

        // 正しいJSONは保存される
        agent
            .post(&format!("{base}/api/config?token={token}"))
            .send(r#"{"max_chain":5}"#)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&cfg).unwrap(),
            r#"{"max_chain":5}"#
        );

        ui.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

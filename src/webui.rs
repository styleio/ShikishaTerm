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
    /// config_path は編集対象 (通常 config.json)
    pub fn start_with(
        config_path: std::path::PathBuf,
        remote: Arc<std::sync::Mutex<RemoteInfo>>,
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
                    if let Err(e) = handle(req, &token, &config_path, &remote) {
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
/// 配布物にドキュメントを同梱し忘れても壊れない
const EMBEDDED_MANUAL: &str = include_str!("../docs/AUTOMATION.md");

/// 隣に置かれたファイルがあればそちらを優先する (利用者が加筆できるように)
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
    for d in dirs {
        for rel in ["docs/AUTOMATION.md", "AUTOMATION.md"] {
            if let Ok(s) = std::fs::read_to_string(d.join(rel)) {
                if !s.trim().is_empty() {
                    return s;
                }
            }
        }
    }
    EMBEDDED_MANUAL.to_string()
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
            note: "本体を起動している間だけ繋がります".into(),
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

const EVENT_FILES: [&str; 6] = [
    "on_start",
    "on_done",
    "on_question",
    "on_exit",
    "on_busy",
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
    let mut s = String::from(
        "## タブ構成\n\
         送信先はタブ名で指定すること (例: shikisha.send_to_tab(\"検査\", ...))。\n\
         番号は並べ替えで変わるため、名前の方が安全。\n",
    );
    for t in tabs {
        let i = t.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
        let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            s.push_str(&format!("{i}. {name}"));
        } else {
            // IDがある場合はそちらで指させる (タブ名を変えても壊れない)
            s.push_str(&format!("{i}. {name} → 指定は \"{id}\""));
        }
        if i == me {
            s.push_str("  ← このスクリプトが動くタブ (tab)");
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
        anyhow::bail!("やりたいことを入力してください");
    }
    // マニュアルを仕様書としてAIに渡す (独自APIは学習データに無いため)
    let manual = load_manual(config_path);

    // 会話文を返させないため、出力形式をマーカーで固定する
    let prompt = format!(
        "あなたはShikishaTerm-AIの自動化スクリプトを書く変換器です。\n\
         次の「やりたいこと」を満たす `{event}.lua` の中身を書いてください。\n\n\
         ## 出力の決まり (厳守)\n\
         - 必ず <<<LUA で始めて >>> で終わる。その間にLuaコードだけを書く\n\
         - 挨拶・説明・確認・質問・コードフェンスは一切書かない\n\
         - function ... end で包まない (処理の本体だけを書く)\n\
         - 仕様書に載っていない関数やライブラリは使わない\n\
         - ファイルの探索や確認は不要。この指示だけで完結させる\n\n\
         ## やりたいこと\n{want}\n\n\
         {layout}\n\
         ## 仕様書\n{manual}\n\n\
         では <<<LUA から始めてください。\n"
    );

    let (cmd, args) = pick_local_ai(engine)?;
    let mut child = std::process::Command::new(&cmd)
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .with_context(|| format!("{cmd} を実行できません"))?;
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().context("stdinを開けません")?;
        stdin.write_all(prompt.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        anyhow::bail!(
            "{cmd} がエラーを返しました: {}",
            String::from_utf8_lossy(&out.stderr).trim()
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
        "AIがコードを返しませんでした。表現を変えて試してください（返答: {}）",
        text.trim().chars().take(120).collect::<String>()
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

fn handle(
    req: tiny_http::Request,
    token: &str,
    config_path: &std::path::Path,
    remote: &Arc<std::sync::Mutex<RemoteInfo>>,
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
            let html = PAGE.replace("__TOKEN__", token);
            let resp = Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            );
            req.respond(resp)?;
        }
        // 書き方の説明 (GUIから開ける。ファイルを探させない)
        ("GET", "/help") => {
            let md = load_manual(config_path);
            let html = HELP_PAGE.replace("__MD__", &serde_json::to_string(&md)?);
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
            let title = p.get("title").and_then(|v| v.as_str()).unwrap_or("選択");
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
<html lang="ja"><head><meta charset="utf-8">
<title>ShikishaTerm-AI 設定</title>
<style>
 :root {
   --bg:#0f1115; --panel:#161a20; --panel2:#1b2027; --line:#262d37;
   --text:#e6e9ef; --muted:#8b95a5; --accent:#35c46a; --danger:#e5534b;
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
 #msg { color:var(--muted); font-size:13px; }

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
 button.primary { background:var(--accent); border-color:var(--accent); color:#08130c; font-weight:600; }
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
</style></head><body>

<header>
  <h1>ShikishaTerm-AI 設定</h1>
  <div class="spacer"></div>
  <span id="msg"></span>
  <button class="quiet" onclick="load()">再読込</button>
  <button class="primary" onclick="save()">保存</button>
</header>

<div class="layout">
  <nav id="nav"></nav>
  <main id="detail"></main>
</div>

<datalist id="cmdlist"></datalist>

<div id="autobox" class="modal" style="display:none">
  <div class="modal-inner">
    <h2 id="autotitle">自動化</h2>
    <div class="hint" id="autopath"></div>
    <div class="row" style="margin-top:12px">
      <label>タイミング</label>
      <select id="autoevent" onchange="switchEvent()"></select>
      <span class="hint" id="autohint"></span>
    </div>
    <textarea id="autocode" spellcheck="false"
      placeholder="ここに処理を書きます。空にすると「何もしない」になります"></textarea>
    <div class="row" id="airow">
      <label>AIに書いてもらう</label>
      <input type="text" id="autoask" class="grow"
             placeholder="例: 完了したら検査タブに送ってレビューさせて">
      <button onclick="askAi()">生成</button>
    </div>
    <div class="row" id="ainone" style="display:none">
      <span class="hint">Claude Code / Codex CLI / Gemini CLI のいずれかを入れると、
        日本語で指示してコードを書いてもらえます。</span>
    </div>
    <div id="aipreview" style="display:none">
      <div class="hint">生成されたコード（確認してから反映してください）</div>
      <pre id="aicode"></pre>
      <button class="primary" onclick="applyAi()">反映</button>
      <button class="quiet" onclick="document.getElementById('aipreview').style.display='none'">破棄</button>
    </div>
    <div class="row" style="border-top:1px solid var(--line); margin-top:12px; padding-top:14px">
      <button class="primary" onclick="saveAuto()">保存して閉じる</button>
      <button class="quiet" onclick="closeAuto()">キャンセル</button>
      <span class="spacer" style="flex:1"></span>
      <a href="/help?token=__TOKEN__" target="_blank">書き方を見る</a>
      <span id="automsg" class="hint"></span>
    </div>
  </div>
</div>

<script>
const TOKEN = "__TOKEN__";
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
const msg = (t, warn) => { const m = document.getElementById("msg");
  m.textContent = t; m.style.color = warn ? "var(--danger)" : "var(--muted)"; };

// ── 部品 ─────────────────────────────────────────────
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
  }}, "参照…");
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

const kindOf = c => parseSsh(c) ? "ssh" : parseDocker(c) ? "docker" : parseWsl(c) ? "wsl" : "cmd";
const KIND_LABEL = {cmd:"コマンド", ssh:"SSH", docker:"Docker", wsl:"WSL"};
const KIND_START = {cmd:"", ssh:"ssh ", docker:"docker exec -it ", wsl:"wsl "};
const COMMON_COMMANDS = [
  {label:"Claude Code", cmd:"claude",         check:"claude"},
  {label:"Codex CLI",   cmd:"codex",          check:"codex"},
  {label:"Gemini CLI",  cmd:"gemini",         check:"gemini"},
  {label:"PowerShell",  cmd:"powershell.exe", check:null},
  {label:"コマンドプロンプト", cmd:"cmd.exe",  check:null},
];
const cmdToText = c => Array.isArray(c) ? c.join(" ") : (c || "");

// ── サイドバー ───────────────────────────────────────
function renderNav() {
  const nav = document.getElementById("nav");
  nav.textContent = "";
  nav.append(el("button", {class:"navitem" + (sel.global ? " sel" : ""),
    onclick:() => { sel = {ws:sel.ws, tab:null, global:true}; render(); }}, "全体設定"));

  wss.forEach((ws, wi) => {
    nav.append(el("div", {class:"navgroup"}, ws.name || "(名称未設定)"));
    nav.append(el("button", {class:"navitem" + (!sel.global && sel.ws === wi && sel.tab === null ? " sel" : ""),
      onclick:() => { sel = {ws:wi, tab:null, global:false}; render(); }},
      el("span", {}, "ワークスペース設定")));
    (ws.tabs || []).forEach((t, ti) => {
      const b = el("button", {class:"navitem navtab" + (t.depth ? " child" : "") +
        (!sel.global && sel.ws === wi && sel.tab === ti ? " sel" : ""),
        onclick:() => { sel = {ws:wi, tab:ti, global:false}; render(); }});
      b.append(el("span", {}, (t.depth ? "└ " : "") + (t.name || "(無名)")));
      b.append(el("span", {class:"sub"}, cmdToText(t.command) || "未設定"));
      nav.append(b);
    });
    nav.append(el("button", {class:"navitem navtab navadd",
      onclick:() => { (ws.tabs = ws.tabs || []).push(newTab());
                      sel = {ws:wi, tab:ws.tabs.length - 1, global:false}; render(); }},
      "＋ タブを追加"));
  });
  nav.append(el("div", {class:"navgroup"}, ""));
  nav.append(el("button", {class:"navitem navadd", onclick:addWs}, "＋ ワークスペースを追加"));
}

const newTab = (o = {}) => Object.assign(
  {name:"", id:"", command:"", profile:"", automation:"", locked:false, auto_restart:false,
   cwd:"", encoding:"", scrollback:"", log:false, depth:0}, o);

function addWs() {
  wss.push({name:"新しいワークスペース", automation:"", tabs:[]});
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
  box.append(card("基本",
    row("タブバーの幅", field(current, "tab_bar_width", "自動", {type:"number", width:110, grow:false}),
        el("span", {class:"hint"}, "空欄ならタブ名に合わせます")),
    row("自動チェーン上限", field(current, "max_chain", "10", {type:"number", width:110, grow:false}),
        el("span", {class:"hint"}, "AI同士の自動転送が続く回数の上限")),
    row("コードを書くAI", aiSelect(),
        el("span", {class:"hint", id:"aihint"}, ""))));
  box.append(remoteCard());
  box.append(card("ファイル",
    row("自動化(全体共通)", ...pathField(current, "automation", "scripts/common", "dir",
        "自動化フォルダを選んでください"),
        el("span", {class:"hint"}, "各タブに設定が無いときに使われます")),
    row("secrets", ...pathField(current, "secrets", "secrets.json", "file",
        "secretsファイルを選んでください"),
        el("span", {class:"hint"}, "通知先やトークン"))));
  return box;
}
// スマホから使う設定。危険性は隠さず説明したうえで、1クリックで有効にできるようにする
function remoteCard() {
  current.remote = current.remote || {};
  const r = current.remote;
  const box = el("div", {class:"card"}, el("h2", {}, "スマホから使う"));
  const status = el("div", {class:"hint"}, "確認中…");
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
  l.append(onoff, document.createTextNode("外出先やスマホから、状況の確認と指示ができるようにする"));

  box.append(el("div", {class:"row"}, el("label", {}, "有効にする"), l));
  box.append(el("div", {class:"row"}, el("label", {}, "ポート"),
    (() => {
      const i = el("input", {type:"number", style:"width:110px"});
      i.value = r.port || 8787;
      i.addEventListener("input", () => { r.port = Number(i.value) || 8787; });
      return i;
    })(),
    el("span", {class:"hint"}, "ふつうは変更不要です")));
  box.append(el("div", {class:"row"}, status));
  box.append(qrbox);
  box.append(el("div", {class:"hint", style:"margin-top:6px"},
    "接続できるのは同じネットワークにいる人だけです。" +
    "Tailscale（無料）を入れておくと、外出先からでも自分の端末だけが繋がる状態になり、" +
    "いちばん安全に使えます。Tailscableが無い場合は家庭内LANだけで繋がります" +
    "（同じWi-Fiにいる人がURLとトークンを知れば操作できます）。" +
    "インターネットに直接公開する設定は、設定ファイルで明示しない限り行いません。"));

  refreshRemote();
  async function refreshRemote() {
    let j = {};
    try { j = await (await fetch("/api/remote", {headers:{"X-Token":TOKEN}})).json(); }
    catch (e) { return; }
    const net = j.tailscale ? "Tailscale (" + j.tailscale + ") が使えます"
              : j.lan ? "Tailscaleは未導入。家庭内LAN (" + j.lan + ") で使えます"
              : "接続できるネットワークが見つかりません";
    status.textContent = (j.running ? "待ち受け中 — " : "停止中 — ") + net + (j.note ? " / " + j.note : "");
    status.style.color = j.running ? "var(--accent)" : "var(--muted)";
    qrbox.textContent = "";
    if (j.running && j.url) {
      // 画像は fetch ではなく直接読み込まれるので、認証はURLのtokenで渡す
      const img = el("img", {src:"/api/remote/qr?token=" + encodeURIComponent(TOKEN),
        style:"width:200px;height:200px;border-radius:8px;background:#fff;padding:6px"});
      qrbox.append(el("div", {class:"hint"}, "スマホのカメラで読み取ってください"), img,
        el("div", {class:"hint mono", style:"word-break:break-all"}, j.url));
    }
  }
  return box;
}

function aiSelect() {
  const s = el("select", {id:"aiengine"});
  const hint = () => document.getElementById("aihint");
  if (!aiEngines.length) {
    s.append(el("option", {value:""}, "見つかりません")); s.disabled = true;
  } else {
    s.append(el("option", {value:""}, "自動（見つかったものを使う）"));
    for (const e of aiEngines) s.append(el("option", {value:e.id}, e.label));
  }
  s.value = current.ai_engine || "";
  s.addEventListener("change", () => { current.ai_engine = s.value; });
  setTimeout(() => { const h = hint(); if (h) h.textContent = aiEngines.length
    ? "" : "Claude Code / Codex CLI / Gemini CLI のいずれかを入れると使えます"; }, 0);
  return s;
}

function wsPane(ws) {
  const box = el("div");
  box.append(card("ワークスペース",
    row("名前", field(ws, "name", "名前", {grow:false, width:280,
        onInput:() => renderNav()})),
    ws.file ? row("定義ファイル", el("span", {class:"hint mono"}, ws.file)) : null,
    row("自動化", ...pathField(ws, "automation", "各タブの設定を使う", "dir",
        "自動化フォルダを選んでください"),
        el("span", {class:"hint"}, "このワークスペース共通"))));

  if (!(ws.tabs || []).length) {
    const e = el("div", {class:"empty"},
      el("div", {class:"big"}, "タブがありません"),
      el("div", {}, "テンプレートから作ると簡単です"));
    const bar = el("div", {class:"row", style:"justify-content:center"});
    for (const [k, label] of [["single","Claude 1つ"],["review","実装＋レビュー往復"],
                              ["ssh","SSH先のAI"],["docker","Dockerの中"],["wsl","WSLの中"]])
      bar.append(el("button", {onclick:() => addTemplate(k)}, label));
    e.append(bar);
    box.append(e);
  }
  box.append(el("div", {class:"row"},
    el("button", {class:"danger", onclick:() => {
      if (confirm(`ワークスペース「${ws.name}」を削除しますか？`)) {
        wss.splice(sel.ws, 1); sel = {ws:0, tab:null, global:true}; render();
      }
    }}, "このワークスペースを削除")));
  return box;
}

const TEMPLATES = {
  single: [ {name:"Claude", command:"claude"} ],
  review: [ {name:"実装", command:"claude"},
            {name:"検査", id:"reviewer", command:"codex", depth:1, locked:true} ],
  ssh:    [ {name:"サーバー", command:"ssh user@example.com", profile:"claude", auto_restart:true} ],
  docker: [ {name:"コンテナ", command:"docker exec -it -w /app myapp bash", profile:"claude"} ],
  wsl:    [ {name:"Ubuntu", command:"wsl -d Ubuntu --cd /home/me/proj -- bash", profile:"claude"} ],
};
function addTemplate(kind) {
  const ws = wss[sel.ws];
  ws.tabs = (ws.tabs || []).concat(TEMPLATES[kind].map(x => newTab(x)));
  sel.tab = ws.tabs.length - TEMPLATES[kind].length;
  render();
  msg("テンプレートを追加しました");
}

function tabPane(ws, t) {
  const box = el("div");
  const kind = kindOf(t.command);

  // 基本: 名前とIDは identity なので隣に置く
  box.append(card("基本",
    row("表示名", field(t, "name", "例: 実装", {grow:false, width:280,
        onInput:() => renderNav()})),
    row("自動化での呼び名", field(t, "id", "表示名をそのまま使う", {grow:false, width:280, mono:true}),
        el("span", {class:"hint"}, "付けると表示名を変えても自動化が壊れません"))));

  // 起動するもの
  const cmdRow = el("div", {class:"row"});
  const cmdInput = field(t, "command", "例: claude", {mono:true, onInput:() => renderNav()});
  cmdInput.setAttribute("list", "cmdlist");
  const detailBox = el("div");
  const rebuild = () => { detailBox.textContent = ""; detailBox.append(kindPanel(t, cmdInput, rebuild)); };
  cmdRow.append(el("label", {}, "種類"),
    choose({k:kind}, "k", Object.entries(KIND_LABEL), v => {
      t.command = KIND_START[v] || ""; cmdInput.value = t.command; rebuild(); renderNav();
    }));
  rebuild();
  box.append(card("起動するもの", cmdRow, detailBox,
    row("コマンド", cmdInput),
    row("作業フォルダ", ...pathField(t, "cwd", "アプリと同じ場所", "dir",
        "作業フォルダを選んでください"),
        el("span", {class:"hint"}, "AIはここのプロジェクトを見ます"))));

  // 自動化: 何が設定済みか一覧で分かるようにする
  const ev = el("div", {class:"events"});
  for (const [id, label, hint] of EVENTS.filter(e => e[0] !== "_shared")) {
    ev.append(el("div", {class:"event"},
      el("div", {class:"name"}, label, el("div", {class:"hint"}, hint)),
      el("span", {class:"state", id:"st-" + id}, "—"),
      el("button", {class:"quiet", onclick:() => openAuto(ws, t, id)}, "編集")));
  }
  box.append(card("自動化", ev));
  loadAutoStates(ws, t);

  // 詳細: めったに触らないものは畳む
  const det = el("details");
  det.append(el("summary", {}, "詳細設定"));
  det.append(
    row("プロファイル", field(t, "profile", "自動判別", {grow:false, width:220}),
        el("span", {class:"hint"}, "状態の検出ルール。SSH先のAIを指定するときに使う")),
    row("自動化フォルダ", ...pathField(t, "automation", "自動", "dir",
        "自動化フォルダを選んでください")),
    row("文字コード", choose(t, "encoding",
        [["","UTF-8（標準）"],["shift_jis","Shift_JIS"],["euc-jp","EUC-JP"]])),
    row("スクロール行数", field(t, "scrollback", "5000", {type:"number", width:120, grow:false})),
    el("div", {class:"row"}, el("label", {}, "動作"),
       check(t, "locked", "入力をロックする"),
       check(t, "auto_restart", "終了したら自動で再起動"),
       check(t, "log", "セッションログを保存")),
    el("div", {class:"row"}, el("label", {}, "並び順"),
       el("button", {class:"quiet", onclick:() => moveTab(ws, -1)}, "↑ 上へ"),
       el("button", {class:"quiet", onclick:() => moveTab(ws, 1)}, "↓ 下へ"),
       el("button", {class:"quiet", onclick:() => { t.depth = Math.min((t.depth||0)+1, sel.tab); render(); }}, "→ 子にする"),
       el("button", {class:"quiet", onclick:() => { t.depth = Math.max((t.depth||0)-1, 0); render(); }}, "← 親に戻す")));
  box.append(el("div", {class:"card"}, det));

  box.append(el("div", {class:"row"},
    el("button", {class:"danger", onclick:() => {
      if (confirm(`タブ「${t.name || "(無名)"}」を削除しますか？`)) {
        ws.tabs.splice(sel.tab, 1); sel.tab = null; render();
      }
    }}, "このタブを削除")));
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
    box.append(el("div", {class:"row"}, ...f(ssh, "host", "接続先", "example.com", upd, 240),
      el("label", {style:"width:auto"}, "ポート"),
      (() => { const i = el("input", {type:"text", class:"mono", style:"width:70px"});
               i.value = ssh.port || ""; i.placeholder = "22";
               i.addEventListener("input", () => { ssh.port = i.value.trim(); upd(); }); return i; })(),
      el("label", {style:"width:auto"}, "ユーザー"),
      (() => { const i = el("input", {type:"text", class:"mono", style:"width:130px"});
               i.value = ssh.user || ""; i.placeholder = "root";
               i.addEventListener("input", () => { ssh.user = i.value.trim(); upd(); }); return i; })()));
    const keyIn = el("input", {type:"text", class:"mono grow", placeholder:"省略時はパスワード入力"});
    keyIn.value = ssh.key || "";
    keyIn.addEventListener("input", () => { ssh.key = keyIn.value.trim(); upd(); });
    box.append(el("div", {class:"row"}, el("label", {}, "鍵ファイル"), keyIn,
      el("button", {class:"quiet", onclick: async () => {
        const p = await pickPath("key", "SSH鍵ファイルを選んでください", ssh.key);
        if (p !== null) { ssh.key = p; keyIn.value = p; upd(); }
      }}, "参照…")));
    const adv = el("details"); adv.append(el("summary", {}, "接続の詳細"));
    const fwd = el("input", {type:"text", class:"mono grow",
      placeholder:"例: -L 8080:localhost:80（複数はカンマ区切り）"});
    fwd.value = (ssh.forwards || []).join(", ");
    fwd.addEventListener("input", () => {
      ssh.forwards = fwd.value.split(",").map(s => s.trim()).filter(Boolean); upd(); });
    adv.append(el("div", {class:"row"}, el("label", {}, "ポート転送"), fwd),
      el("div", {class:"row"}, ...f(ssh, "jump", "踏み台", "gw.example.com", upd, 200),
        ...f(ssh, "keepalive", "接続維持(秒)", "60", upd, 80)),
      el("div", {class:"row"}, el("label", {}, "許可"),
        (() => { const c = el("input", {type:"checkbox"}); c.checked = ssh.agent;
          c.addEventListener("change", () => { ssh.agent = c.checked; upd(); });
          const l = el("label", {class:"check"}); l.append(c, document.createTextNode("鍵の転送 (-A)"));
          return l; })(),
        (() => { const c = el("input", {type:"checkbox"}); c.checked = ssh.x11;
          c.addEventListener("change", () => { ssh.x11 = c.checked; upd(); });
          const l = el("label", {class:"check"}); l.append(c, document.createTextNode("画面転送 (-X)"));
          return l; })()));
    box.append(adv);
  } else if (dk || wsl) {
    const o = dk || wsl, upd = sync(dk ? buildDocker : buildWsl, o);
    box.append(el("div", {class:"row"},
      ...(dk ? f(o, "container", "コンテナ名", "myapp", upd, 200)
             : f(o, "distro", "ディストリ", "Ubuntu", upd, 200)),
      ...f(o, "dir", "中のフォルダ", "/home/me/proj", upd, 220)));
    box.append(el("div", {class:"row"},
      ...f(o, "shell", "実行するもの", "bash / claude", upd, 220),
      el("span", {class:"hint"}, "コンテナ/WSLの中のフォルダはここで指定します")));
  } else {
    const s = el("select");
    s.append(el("option", {value:""}, "よく使うものから選ぶ…"));
    for (const c of COMMON_COMMANDS) {
      const ok = c.check ? aiEngines.some(e => e.id === c.check) : true;
      s.append(el("option", {value:c.cmd}, c.label + (c.check && !ok ? "（未インストール）" : "")));
    }
    s.addEventListener("change", () => {
      if (!s.value) return;
      t.command = s.value; cmdInput.value = s.value; s.value = ""; renderNav();
    });
    box.append(el("div", {class:"row"}, el("label", {}, "よく使うもの"), s));
  }
  return box;
}

// ── 自動化エディタ ───────────────────────────────────
const EVENTS = [
  ["on_start",    "起動したとき",           "作業フォルダへ移動して前回の続きを再開する等"],
  ["on_done",     "応答が完了したとき",     "結果を他のタブへ渡す / 通知する"],
  ["on_question", "確認を聞かれたとき",     "文字列を返すと自動送信、返さなければ人が判断"],
  ["on_exit",     "終了したとき",           "切断されたら再接続する等"],
  ["on_busy",     "応答が始まったとき",     "処理中に定期的に様子を見る（上級）"],
  ["_shared",     "共通の下請け関数",       ""],
];
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
  for (const [id] of EVENTS) {
    const s = document.getElementById("st-" + id);
    if (!s) continue;
    const on = (data[id] || "").trim().length > 0;
    s.textContent = on ? "設定あり" : "未設定";
    s.className = "state" + (on ? " on" : "");
  }
}

async function openAuto(ws, t, event) {
  autoTarget = { ws, t, dir: autoDirOf(ws, t) };
  document.getElementById("autotitle").textContent = "自動化 — " + (t.name || "無名タブ");
  document.getElementById("autopath").textContent = autoTarget.dir;
  const s = document.getElementById("autoevent");
  s.textContent = "";
  for (const [id, label] of EVENTS) s.append(el("option", {value:id}, label));
  autoData = await fetchAuto(autoTarget.dir);
  autoEvent = event || "on_done"; s.value = autoEvent;
  switchEvent();
  document.getElementById("airow").style.display = aiEngines.length ? "flex" : "none";
  document.getElementById("ainone").style.display = aiEngines.length ? "none" : "flex";
  document.getElementById("aipreview").style.display = "none";
  automsg("");
  document.getElementById("autobox").style.display = "flex";
}
function switchEvent() {
  autoData[autoEvent] = document.getElementById("autocode").value;
  autoEvent = document.getElementById("autoevent").value;
  document.getElementById("autocode").value = autoData[autoEvent] || "";
  const e = EVENTS.find(x => x[0] === autoEvent);
  document.getElementById("autohint").textContent = e ? e[2] : "";
}
function closeAuto() { document.getElementById("autobox").style.display = "none"; }

async function saveAuto() {
  autoData[autoEvent] = document.getElementById("autocode").value;
  const r = await fetch("/api/automation?dir=" + encodeURIComponent(autoTarget.dir),
      {method:"POST", headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
       body: JSON.stringify(autoData)});
  if (!r.ok) return automsg("保存に失敗しました", true);
  const created = autoTarget.t.automation !== autoTarget.dir;
  autoTarget.t.automation = autoTarget.dir;
  closeAuto();
  loadAutoStates(autoTarget.ws, autoTarget.t);
  msg(created ? "自動化を保存しました。「保存」も押してください" : "自動化を保存しました");
}

async function askAi() {
  const want = document.getElementById("autoask").value.trim();
  if (!want) return automsg("やりたいことを書いてください", true);
  automsg("AIに問い合わせています…");
  const ws = autoTarget.ws;
  const r = await fetch("/api/generate", {method:"POST",
      headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
      body: JSON.stringify({event: autoEvent, prompt: want,
        engine: current.ai_engine || null,
        tabs: (ws.tabs || []).map((x, i) => ({index:i+1, name:x.name || ("タブ"+(i+1)), id:x.id || ""})),
        self: (ws.tabs || []).indexOf(autoTarget.t) + 1})});
  const j = await r.json();
  if (!j.ok) return automsg("生成できませんでした: " + j.error, true);
  document.getElementById("aicode").textContent = j.code;
  document.getElementById("aipreview").style.display = "block";
  automsg("内容を確認して「反映」を押してください");
}
function applyAi() {
  document.getElementById("autocode").value = document.getElementById("aicode").textContent;
  document.getElementById("aipreview").style.display = "none";
  automsg("反映しました（まだ保存されていません）");
}
function automsg(t, warn) { const m = document.getElementById("automsg");
  m.textContent = t; m.style.color = warn ? "var(--danger)" : "var(--muted)"; }

// ── 読み込み / 保存 ──────────────────────────────────
function flatten(tabs, depth, out) {
  for (const t of tabs || []) {
    out.push({ name: t.name || "", id: t.id || "", command: cmdToText(t.command),
               profile: t.profile || "", automation: t.automation || t.lua || "",
               locked: !!t.locked, auto_restart: !!t.auto_restart, cwd: t.cwd || "",
               encoding: t.encoding || "", scrollback: t.scrollback ?? "", log: !!t.log, depth });
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
                 automation:w.automation || w.lua || "", tabs:[] };
    if (ws.file) {
      const f = await (await wsApi("GET", ws.file)).json().catch(() => ({}));
      ws.tabs = flatten(f.tabs, 0, []);
      if (!ws.automation) ws.automation = f.automation || f.lua || "";
    } else ws.tabs = flatten(w.tabs, 0, []);
    wss.push(ws);
  }
  if (sel.ws >= wss.length) sel = {ws:0, tab:null, global:true};
  render();
  msg("読み込みました");
}

async function save() {
  const out = Object.assign({}, current);
  ["tab_bar_width","max_chain"].forEach(k => {
    const v = out[k]; if (v === "" || v === null || v === undefined) delete out[k]; else out[k] = Number(v);
  });
  ["automation","secrets","ai_engine"].forEach(k => { if (!out[k]) delete out[k]; });
  if (out.remote && !out.remote.enabled && !out.remote.allow_public) delete out.remote;
  delete out.lua; delete out.tabs;

  for (const w of wss) {
    if (!w.file) continue;
    const body = { name:w.name, tabs:nest(w.tabs) };
    if (w.automation) body.automation = w.automation;
    const rf = await wsApi("POST", w.file, JSON.stringify(body, null, 2));
    const jf = await rf.json().catch(() => ({ok:false}));
    if (!jf.ok) return msg(w.file + " の保存に失敗しました", true);
  }
  out.workspaces = wss.map(w => {
    const o = { name:w.name };
    if (w.file) o.file = w.file;
    else { if (w.automation) o.automation = w.automation; o.tabs = nest(w.tabs); }
    return o;
  });
  const r = await api("POST", JSON.stringify(out, null, 2));
  const j = await r.json();
  msg(j.ok ? "保存しました" : "保存失敗: " + j.error, !j.ok);
}

load();
</script></body></html>
"##;

/// マニュアル表示ページ (Markdownの必要な部分だけを描画する簡易レンダラ)
const HELP_PAGE: &str = r##"<!doctype html>
<html lang="ja"><head><meta charset="utf-8"><title>自動化の書き方</title>
<style>
 :root { color-scheme: dark; }
 body { background:#05080a; color:#c8f7c0; font-family:"Consolas","Meiryo",monospace;
        margin:0; padding:24px 32px; line-height:1.7; }
 h1,h2,h3 { color:#39ff14; border-bottom:1px solid #1f4d2a; padding-bottom:6px; }
 h1 { font-size:20px; } h2 { font-size:17px; margin-top:32px; } h3 { font-size:15px; }
 code { background:#0a1014; color:#ffea00; padding:1px 5px; border-radius:3px; }
 pre { background:#0a1014; border:1px solid #1f4d2a; padding:12px; overflow:auto; }
 pre code { color:#39ff14; background:none; padding:0; }
 table { border-collapse:collapse; margin:12px 0; }
 th,td { border:1px solid #1f4d2a; padding:5px 10px; text-align:left; }
 th { color:#00aaff; }
 hr { border:0; border-top:1px solid #1f4d2a; margin:28px 0; }
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
        // どこから起動してもAIに渡す仕様書が手に入ること
        assert!(EMBEDDED_MANUAL.contains("shikisha.send_to_tab"));
        let m = load_manual(std::path::Path::new("/nonexistent/config.json"));
        assert!(m.contains("shikisha."), "埋め込みにフォールバックする");
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
        assert!(s.contains("← このスクリプトが動くタブ"), "{s}");
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

        let ui =
            WebUi::start_with(cfg.clone(), Arc::new(std::sync::Mutex::new(RemoteInfo::default())))
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

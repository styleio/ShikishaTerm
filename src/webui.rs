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

pub struct WebUi {
    pub url: String,
    stop: Arc<AtomicBool>,
}

impl WebUi {
    /// 設定ファイルを編集するローカルサーバーを起動する。
    /// config_path は編集対象 (通常 config.json)
    pub fn start(config_path: std::path::PathBuf) -> Result<Self> {
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
                    if let Err(e) = handle(req, &token, &config_path) {
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

fn handle(req: tiny_http::Request, token: &str, config_path: &std::path::Path) -> Result<()> {
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

// 色指定の "#..." が終端と衝突しないよう r##"..."## で囲む
const PAGE: &str = r##"<!doctype html>
<html lang="ja"><head><meta charset="utf-8">
<title>ShikishaTerm-AI 設定</title>
<style>
 :root { color-scheme: dark; }
 body { background:#05080a; color:#39ff14; font-family: "Consolas","Meiryo",monospace;
        margin:0; padding:24px; }
 h1 { font-size:18px; letter-spacing:2px; border-bottom:1px solid #39ff14; padding-bottom:8px; }
 .row { margin:14px 0; display:flex; gap:8px; align-items:center; flex-wrap:wrap; }
 label { color:#00aaff; min-width:130px; }
 input, textarea { background:#0a1014; color:#39ff14; border:1px solid #1f4d2a;
        padding:6px 8px; font-family:inherit; font-size:13px; border-radius:3px; }
 input[type=text], input[type=number] { width:220px; }
 textarea { width:100%; min-height:340px; }
 button { background:#39ff14; color:#05080a; border:0; padding:8px 18px; font-weight:bold;
        cursor:pointer; border-radius:3px; font-family:inherit; }
 button.ghost { background:transparent; color:#39ff14; border:1px solid #39ff14; }
 #msg { margin-left:12px; }
 .warn { color:#ffea00; font-size:12px; margin-top:6px; }
 fieldset { border:1px solid #1f4d2a; margin:18px 0; padding:12px 16px; }
 legend { color:#ffea00; padding:0 6px; }
 table { border-collapse:collapse; width:100%; margin:10px 0; }
 td { padding:4px 6px; border-bottom:1px solid #12261a; vertical-align:middle; }
 td input[type=text] { width:100%; min-width:90px; }
 td button { padding:2px 8px; font-size:12px; }
 input[type=checkbox] { width:16px; height:16px; accent-color:#39ff14; }
 b { color:#00aaff; font-weight:normal; font-size:12px; }
 .tree { color:#ffea00; margin-left:4px; cursor:help; }
 .tab { border:1px solid #12261a; border-radius:4px; padding:8px 10px; margin:8px 0;
        background:#070d10; }
 .tabhead { display:flex; gap:10px; align-items:center; flex-wrap:wrap; }
 .tabhead input[type=text] { width:190px; }
 .detail { margin-top:10px; padding-top:8px; border-top:1px dashed #1f4d2a; }
 .modal { position:fixed; inset:0; background:rgba(0,0,0,.75); display:flex;
          align-items:center; justify-content:center; z-index:10; }
 .modal-inner { background:#05080a; border:1px solid #39ff14; border-radius:4px;
        padding:20px 24px; width:min(900px, 92vw); max-height:88vh; overflow:auto; }
 h2 { font-size:15px; color:#ffea00; margin:0 0 12px; }
 select { background:#0a1014; color:#39ff14; border:1px solid #1f4d2a; padding:6px;
        font-family:inherit; }
 pre { background:#0a1014; border:1px solid #1f4d2a; padding:10px; overflow:auto;
        max-height:240px; color:#39ff14; }
 #autocode { min-height:200px; }
</style></head><body>
<h1>SHIKISHA-TERM-AI :: CONFIG</h1>

<fieldset><legend>基本設定</legend>
 <div class="row"><label>タブバー幅</label>
   <input type="number" id="tabw" min="10" max="40" placeholder="自動">
   <span class="warn">空にするとタブ名に合わせて自動調整</span></div>
 <div class="row"><label>自動チェーン上限</label>
   <input type="number" id="chain" min="1" max="100">
   <span class="warn">AI同士の自動転送が何回続いたら止めるか</span></div>
 <div class="row"><label>自動化(全体共通)</label>
   <input type="text" id="lua" placeholder="scripts/common">
   <button class="ghost" onclick="pickInto('lua','dir','自動化フォルダを選んでください')">参照…</button>
   <span class="warn">各タブに設定が無いときに使われます</span></div>
 <div class="row"><label>secretsファイル</label>
   <input type="text" id="secrets" placeholder="secrets.json">
   <button class="ghost" onclick="pickInto('secrets','file','secretsファイルを選んでください')">参照…</button></div>
 <div class="row"><label>コードを書くAI</label>
   <select id="aiengine"></select>
   <span class="warn" id="aihint"></span></div>
</fieldset>

<datalist id="cmdlist"></datalist>
<fieldset><legend>ワークスペースとタブ</legend>
 <div id="wslist"></div>
 <button class="ghost" onclick="addWs()">＋ ワークスペースを追加</button>
 <span class="warn">　テンプレートから作る:</span>
 <button class="ghost" onclick="addTemplate('single')">Claude 1つ</button>
 <button class="ghost" onclick="addTemplate('review')">実装＋レビュー往復</button>
 <button class="ghost" onclick="addTemplate('ssh')">SSH先のAI</button>
 <button class="ghost" onclick="addTemplate('docker')">Dockerの中</button>
 <button class="ghost" onclick="addTemplate('wsl')">WSLの中</button>
</fieldset>

<!-- 自動化エディタ (タブの [⚙自動化] で開く) -->
<div id="autobox" class="modal" style="display:none">
 <div class="modal-inner">
   <h2 id="autotitle">自動化</h2>
   <div class="row">
     <label>いつ動かすか</label>
     <select id="autoevent" onchange="switchEvent()"></select>
     <span class="warn" id="autohint"></span>
   </div>
   <textarea id="autocode" spellcheck="false" placeholder="ここに処理を書きます。空にすると「何もしない」になります"></textarea>
   <div class="row" id="airow">
     <label>AIに書いてもらう</label>
     <input type="text" id="autoask" placeholder="例: 完了したらタブ2にレビューさせて。5往復したら止めて"
            style="flex:1; min-width:320px">
     <button class="ghost" onclick="askAi()">生成</button>
   </div>
   <div class="row" id="ainone" style="display:none">
     <span class="warn">日本語で指示してコードを書いてもらう機能は、
       Claude Code / Codex CLI / Gemini CLI のいずれかを入れると使えます。</span>
   </div>
   <div id="aipreview" style="display:none">
     <div class="warn">生成されたコード（内容を確認してから反映してください）</div>
     <pre id="aicode"></pre>
     <button onclick="applyAi()">この内容を反映</button>
     <button class="ghost" onclick="document.getElementById('aipreview').style.display='none'">破棄</button>
   </div>
   <div class="row">
     <button onclick="saveAuto()">保存して閉じる</button>
     <button class="ghost" onclick="closeAuto()">キャンセル</button>
     <span id="automsg"></span>
   </div>
   <p class="warn">
     <a href="/help?token=__TOKEN__" target="_blank" style="color:#00aaff">📖 書き方を見る（変数・命令の一覧と例）</a>
     　自動化からファイル操作やインターネット接続はできません（サンドボックス）。</p>
 </div>
</div>

<div class="row">
  <button onclick="save()">保存</button>
  <button class="ghost" onclick="load()">再読込</button>
  <span id="msg"></span>
</div>
<p class="warn">接続先・文字コード・スクロール行数を変えたタブには「⟳」が付きます。
実行中の作業を切らないよう、本体で Ctrl+B r を押したときに切り替わります。</p>

<script>
const TOKEN = "__TOKEN__";
const api = (m, b) => fetch("/api/config", {
   method: m, headers: {"X-Token": TOKEN, "Content-Type":"application/json"}, body: b });

// 画面の状態。JSONは触らせず、この配列を編集して保存時に組み立てる
let current = {};
let wss = [];   // [{name, file, tabs:[{name,command,profile,lua,locked,auto_restart,depth}]}]

const el = (tag, attrs = {}, ...kids) => {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") n.className = v;
    else if (k.startsWith("on")) n.addEventListener(k.slice(2), v);
    else if (v !== null && v !== undefined) n.setAttribute(k, v);
  }
  for (const c of kids) n.append(c);
  return n;
};

/// config上の入れ子(children)を、画面用のフラットな配列に変換する
function flatten(tabs, depth, out) {
  for (const t of tabs || []) {
    out.push({ name: t.name || "", id: t.id || "", command: cmdToText(t.command), profile: t.profile || "",
               automation: t.automation || t.lua || "",
               locked: !!t.locked, auto_restart: !!t.auto_restart,
               cwd: t.cwd || "",
               encoding: t.encoding || "", scrollback: t.scrollback ?? "", log: !!t.log,
               depth });
    flatten(t.children, depth + 1, out);
  }
  return out;
}
/// フラットな配列を depth に従って children へ組み直す
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
    else {
      const parent = stack[d - 1];
      (parent.children = parent.children || []).push(node);
    }
    stack[d] = node; stack.length = d + 1;
  }
  return roots;
}
const cmdToText = c => Array.isArray(c) ? c.join(" ") : (c || "");

// 普段使う3つ (タブ名 / 動かすもの / 自動化) だけを見せ、残りは [詳細] に畳む
function render() {
  const box = document.getElementById("wslist");
  box.textContent = "";
  wss.forEach((ws, wi) => {
    const head = el("div", {class:"row"},
      el("label", {}, "ワークスペース名"),
      input(ws, "name", "名前", "text"),
      el("button", {class:"ghost", onclick:() => { moveWs(wi,-1); }}, "↑"),
      el("button", {class:"ghost", onclick:() => { moveWs(wi, 1); }}, "↓"),
      el("button", {class:"ghost", onclick:() => {
        if (confirm(`ワークスペース「${ws.name}」を削除しますか？`)) { wss.splice(wi,1); render(); }
      }}, "削除"));

    const list = el("div");
    (ws.tabs || []).forEach((t, ti) => {
      const card = el("div", {class:"tab"});
      const cmdInput = input(t, "command", "動かすもの (例: claude / ssh user@host)", "text");
      // 入力欄からも候補を選べるようにする (詳細を開かなくてよい)
      cmdInput.setAttribute("list", "cmdlist");
      const detail = el("div", {class:"detail", style:"display:none"},
        sshPanel(t, cmdInput),
        el("div", {class:"row"},
          el("label", {}, "作業フォルダ"),
          ...pathField(t, "cwd", "アプリと同じ場所", "dir", "作業フォルダを選んでください", 300),
          el("span", {class:"warn"},
            "AIはここのプロジェクトを見ます（Docker/WSLの中は上の欄で指定します）")),
        el("div", {class:"row"},
          el("label", {}, "文字コード"), select(t, "encoding",
            [["","UTF-8（標準）"],["shift_jis","Shift_JIS"],["euc-jp","EUC-JP"]]),
          el("label", {style:"min-width:auto"}, "　スクロール行数"),
          input(t, "scrollback", "5000", "number"),
          label2(check(t, "log"), "セッションログを保存する（logs/）")),
        el("div", {class:"row"},
          el("label", {}, "自動化での呼び名"), input(t, "id", "タブ名をそのまま使う", "text"),
          el("span", {class:"warn"},
            "設定すると、タブ名を変えても自動化が壊れません（同名タブがある場合も必要）")),
        el("div", {class:"row"},
          el("label", {}, "プロファイル"), input(t, "profile", "自動判別", "text"),
          el("span", {class:"warn"}, "検出ルール。SSH先のAIを指定するときに使う")),
        el("div", {class:"row"},
          el("label", {}, "自動化フォルダ"),
          ...pathField(t, "automation", "自動", "dir", "自動化フォルダを選んでください", 260),
          el("span", {class:"warn"}, "空欄なら自動で決まります。他のタブと同じ場所を指定すると共有できます")),
        el("div", {class:"row"},
          label2(check(t, "locked"), "入力をロックする（人間の誤操作を防ぐ）"),
          label2(check(t, "auto_restart"), "終了したら自動で再起動する")),
        el("div", {class:"row"},
          el("label", {}, "表示の階層"),
          el("button", {class:"ghost", title:"1段下げる (上のタブの子にする)",
            onclick:() => { t.depth = Math.min((t.depth||0)+1, ti); render(); }}, "→ 下げる"),
          el("button", {class:"ghost", title:"1段上げる",
            onclick:() => { t.depth = Math.max((t.depth||0)-1, 0); render(); }}, "← 上げる")));

      card.append(el("div", {class:"tabhead"},
        // 番号は自動化で shikisha.send_to_tab(番号, ...) と書くときに使う
        el("span", {class:"tree", title:`自動化では shikisha.send_to_tab(${ti+1}, ...) と書きます`},
           "　".repeat(t.depth||0) + (t.depth ? "└" : "") + `(${ti+1})`),
        input(t, "name", "タブ名 (例: A:実装)", "text"),
        cmdInput,
        el("button", {class:"ghost", onclick:() => openAuto(ws, t)}, "⚙ 自動化"),
        el("button", {class:"ghost", onclick:(e) => {
          const d = detail.style.display === "none";
          detail.style.display = d ? "block" : "none";
          e.target.textContent = d ? "詳細 ▴" : "詳細 ▾";
        }}, "詳細 ▾"),
        el("button", {class:"ghost", onclick:() => { moveTab(ws, ti,-1); }}, "↑"),
        el("button", {class:"ghost", onclick:() => { moveTab(ws, ti, 1); }}, "↓"),
        el("button", {class:"ghost", onclick:() => {
          if (confirm(`タブ「${t.name || "(無名)"}」を削除しますか？`)) { ws.tabs.splice(ti,1); render(); }
        }}, "削除")), detail);
      list.append(card);
    });

    box.append(el("fieldset", {}, el("legend", {}, ws.name || "(名称未設定)"), head, list,
      el("button", {class:"ghost", onclick:() => {
        (ws.tabs = ws.tabs || []).push(newTab());
        render();
      }}, "＋ タブを追加")));
  });
}
// ── SSH接続の入力補助 (PuTTYの設定画面に相当。コマンドを知らなくても繋げる) ──
// 実体はコマンド文字列なので、玄人はそのまま上の欄に書いてもよい
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

// cmdInput は上段の「動かすもの」欄。ここを書き換えても再描画せず値だけ同期する
// (入力のたびに再描画すると詳細が閉じたりフォーカスが飛んだりするため)
// Docker: docker exec -it -w /app <コンテナ> bash
function parseDocker(cmd) {
  const t = (cmd || "").trim().split(/\s+/);
  if (t[0] !== "docker" || t[1] !== "exec") return null;
  const o = {container:"", dir:"", shell:""};
  const rest = [];
  for (let i = 2; i < t.length; i++) {
    const a = t[i];
    if (a === "-w") o.dir = t[++i] || "";
    else if (a === "-it" || a === "-i" || a === "-t") continue;
    else if (a.startsWith("-")) { rest.push(a); }
    else if (!o.container) o.container = a;
    else rest.push(a);
  }
  o.shell = rest.join(" ");
  return o;
}
const buildDocker = o => ["docker exec -it",
  o.dir ? "-w " + o.dir : "", o.container, o.shell || "bash"]
  .filter(Boolean).join(" ");

// WSL: wsl -d Ubuntu --cd /home/me/proj -- claude
function parseWsl(cmd) {
  const t = (cmd || "").trim().split(/\s+/);
  if (t[0] !== "wsl") return null;
  const o = {distro:"", dir:"", shell:""};
  const rest = [];
  for (let i = 1; i < t.length; i++) {
    const a = t[i];
    if (a === "-d" || a === "--distribution") o.distro = t[++i] || "";
    else if (a === "--cd") o.dir = t[++i] || "";
    else if (a === "--") rest.push(...t.slice(i + 1)), i = t.length;
    else rest.push(a);
  }
  o.shell = rest.join(" ");
  return o;
}
const buildWsl = o => ["wsl", o.distro ? "-d " + o.distro : "",
  o.dir ? "--cd " + o.dir : "", o.shell ? "-- " + o.shell : ""]
  .filter(Boolean).join(" ");

const KIND_START = {cmd:"", ssh:"ssh ", docker:"docker exec -it ", wsl:"wsl "};

// よく使うコマンド。check はインストール判定に使う名前
const COMMON_COMMANDS = [
  {label:"Claude Code",       cmd:"claude",         check:"claude"},
  {label:"Codex CLI",         cmd:"codex",          check:"codex"},
  {label:"Gemini CLI",        cmd:"gemini",         check:"gemini"},
  {label:"PowerShell",        cmd:"powershell.exe", check:null},
  {label:"コマンドプロンプト", cmd:"cmd.exe",        check:null},
];

function sshPanel(t, cmdInput) {
  const box = el("div");
  const build = () => {
    box.textContent = "";
    const ssh = parseSsh(t.command);
    const dk = parseDocker(t.command);
    const wsl = parseWsl(t.command);
    const kind = ssh ? "ssh" : dk ? "docker" : wsl ? "wsl" : "cmd";
    const head = el("div", {class:"row"},
      el("label", {}, "接続の種類"),
      (() => {
        const s = el("select");
        s.append(el("option", {value:"cmd"}, "コマンドを実行"),
                 el("option", {value:"ssh"}, "SSH接続（サーバーに繋ぐ）"),
                 el("option", {value:"docker"}, "Dockerコンテナの中"),
                 el("option", {value:"wsl"}, "WSL（Windows上のLinux）の中"));
        s.value = kind;
        s.addEventListener("change", () => {
          t.command = KIND_START[s.value] || "";
          if (cmdInput) cmdInput.value = t.command;
          build();
        });
        return s;
      })());
    // 「コマンドを実行」のときは、よく使うものを選ぶだけで入力できるようにする
    // (AI用ターミナルなので、AIの名前が並んでいるのが自然)
    if (kind === "cmd") {
      const s = el("select", {style:"width:230px"});
      s.append(el("option", {value:""}, "選んで入力…"));
      for (const c of COMMON_COMMANDS) {
        const installed = c.check ? aiEngines.some(e => e.id === c.check) : true;
        s.append(el("option", {value:c.cmd},
          c.label + (c.check ? (installed ? "（利用できます）" : "（未インストール）") : "")));
      }
      s.addEventListener("change", () => {
        if (!s.value) return;
        t.command = s.value;
        if (cmdInput) cmdInput.value = t.command;
        s.value = "";
      });
      head.append(el("label", {style:"min-width:auto"}, "よく使うもの"), s);
    }
    box.append(head);

    // Docker / WSL は「中のフォルダ」をコマンドで指定する (作業フォルダ欄はWindows側)
    if (dk || wsl) {
      const o = dk || wsl;
      const preview = el("span", {class:"warn"}, "");
      const upd = () => {
        t.command = dk ? buildDocker(o) : buildWsl(o);
        if (cmdInput) cmdInput.value = t.command;
        preview.textContent = "実行されるコマンド: " + t.command;
      };
      const f = (label, key, ph, w) => {
        const i = el("input", {type:"text", placeholder:ph, style:`width:${w||180}px`});
        i.value = o[key] || "";
        i.addEventListener("change", () => { o[key] = i.value.trim(); upd(); });
        return [el("label", {style:"min-width:auto"}, label), i];
      };
      box.append(el("div", {class:"row"},
        ...(dk ? f("コンテナ名", "container", "myapp", 200)
               : f("ディストリ", "distro", "Ubuntu（既定なら空欄）", 200)),
        ...f("中のフォルダ", "dir", "/home/me/proj", 220),
        ...f("実行するもの", "shell", dk ? "bash / claude" : "bash / claude", 180)));
      upd();
      box.append(el("div", {class:"row"}, preview));
      return;
    }
    if (!ssh) return;

    const preview = el("span", {class:"warn"}, "");
    const upd = () => {
      t.command = buildSsh(ssh);
      if (cmdInput) cmdInput.value = t.command;
      preview.textContent = "実行されるコマンド: " + t.command;
    };
    const f = (label, key, ph, w) => {
      const i = el("input", {type:"text", placeholder:ph, style:`width:${w||150}px`});
      i.value = ssh[key] || "";
      i.addEventListener("change", () => { ssh[key] = i.value.trim(); upd(); });
      return [el("label", {style:"min-width:auto"}, label), i];
    };
    const cb = (label, key) => {
      const i = el("input", {type:"checkbox"});
      i.checked = !!ssh[key];
      i.addEventListener("change", () => { ssh[key] = i.checked; upd(); });
      return label2(i, label);
    };
    box.append(el("div", {class:"row"},
      ...f("接続先", "host", "example.com", 200),
      ...f("ポート", "port", "22", 70),
      ...f("ユーザー名", "user", "root", 120)));
    const keyRow = f("鍵ファイル", "key", "省略時はパスワード入力", 300);
    box.append(el("div", {class:"row"},
      ...keyRow,
      el("button", {class:"ghost", onclick: async () => {
        const p = await pickPath("key", "SSH鍵ファイルを選んでください", ssh.key);
        if (p !== null) { ssh.key = p; keyRow[1].value = p; upd(); }
      }}, "参照…"),
      cb("鍵の転送を許可 (-A)", "agent"),
      cb("画面転送 (-X)", "x11")));
    const fwd = el("input", {type:"text", style:"width:340px",
        placeholder:"例: -L 8080:localhost:80 （複数はカンマ区切り）"});
    fwd.value = (ssh.forwards || []).join(", ");
    fwd.addEventListener("change", () => {
      ssh.forwards = fwd.value.split(",").map(s => s.trim()).filter(Boolean); upd();
    });
    box.append(el("div", {class:"row"},
      el("label", {style:"min-width:auto"}, "ポート転送"), fwd,
      ...f("踏み台", "jump", "gw.example.com", 180),
      ...f("接続維持(秒)", "keepalive", "60", 70)));
    preview.textContent = "実行されるコマンド: " + t.command;
    box.append(el("div", {class:"row"}, preview));
  };
  build();
  return box;
}

// Windows標準のファイル選択ダイアログを開いてもらう (手打ち不要)
async function pickPath(kind, title, start) {
  try {
    const r = await fetch("/api/pick", {method:"POST",
        headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
        body: JSON.stringify({kind, title, start: start || ""})});
    const j = await r.json();
    return j.ok ? j.path : null;
  } catch (e) { return null; }
}

// 基本設定の入力欄へ、選んだパスを入れる
async function pickInto(id, kind, title) {
  const i = document.getElementById(id);
  const p = await pickPath(kind, title, i.value);
  if (p !== null) i.value = p;
}

/// 入力欄と「参照…」ボタンの組
function pathField(obj, key, ph, kind, title, width) {
  const i = el("input", {type:"text", placeholder:ph, style:`width:${width||220}px`});
  i.value = obj[key] ?? "";
  i.addEventListener("input", () => { obj[key] = i.value; });
  const b = el("button", {class:"ghost", onclick: async () => {
    const p = await pickPath(kind, title, obj[key]);
    if (p !== null) { obj[key] = p; i.value = p; }
  }}, "参照…");
  return [i, b];
}

function select(obj, key, opts) {
  const s = el("select");
  for (const [v, label] of opts) s.append(el("option", {value:v}, label));
  s.value = obj[key] || "";
  s.addEventListener("change", () => { obj[key] = s.value; });
  return s;
}

const newTab = (o = {}) => Object.assign(
  {name:"", id:"", command:"", profile:"", automation:"", locked:false, auto_restart:false,
   cwd:"", encoding:"", scrollback:"", log:false, depth:0}, o);
const label2 = (ctrl, text) => {
  const l = el("label", {style:"min-width:auto; color:#39ff14; cursor:pointer"});
  l.append(ctrl, document.createTextNode(" " + text));
  return l;
};
function input(obj, key, ph, type) {
  const i = el("input", {type, placeholder: ph});
  i.value = obj[key] ?? "";
  i.addEventListener("input", () => { obj[key] = i.value; });
  return i;
}
function check(obj, key) {
  const i = el("input", {type:"checkbox"});
  i.checked = !!obj[key];
  i.addEventListener("change", () => { obj[key] = i.checked; });
  return i;
}
function moveWs(i, d) { const j=i+d; if(j<0||j>=wss.length) return;
  [wss[i],wss[j]]=[wss[j],wss[i]]; render(); }
function moveTab(ws, i, d) { const j=i+d; if(j<0||j>=ws.tabs.length) return;
  [ws.tabs[i],ws.tabs[j]]=[ws.tabs[j],ws.tabs[i]]; render(); }
function addWs() { wss.push({name:"新しいワークスペース", automation:"", tabs:[]}); render(); }

// 白紙から始めさせないためのテンプレート
const TEMPLATES = {
  single: { name:"マイAI", tabs:[ newTab({name:"Claude", command:"claude"}) ] },
  review: { name:"実装＋レビュー", tabs:[
      newTab({name:"A:実装", command:"claude"}),
      newTab({name:"B:レビュー", command:"codex", depth:1, locked:true}) ] },
  ssh:    { name:"リモート作業", tabs:[
      newTab({name:"サーバー", command:"ssh user@example.com", profile:"claude",
              auto_restart:true}) ] },
  docker: { name:"コンテナ作業", tabs:[
      newTab({name:"コンテナ", command:"docker exec -it -w /app myapp bash",
              profile:"claude"}) ] },
  wsl:    { name:"WSL作業", tabs:[
      newTab({name:"Ubuntu", command:"wsl -d Ubuntu --cd /home/me/proj -- bash",
              profile:"claude"}) ] },
};
function addTemplate(kind) {
  const t = TEMPLATES[kind];
  wss.push({ name: t.name, automation:"", tabs: t.tabs.map(x => newTab(x)) });
  render();
  msg("テンプレートを追加しました。名前とコマンドを調整して保存してください", "#39ff14");
}

// ── 自動化エディタ ─────────────────────────────────────────────
const EVENTS = [
  ["on_start",    "起動したとき",           "例: 作業フォルダへ移動して前回の続きを再開する"],
  ["on_done",     "応答が完了したとき",     "例: 結果を他のタブへ渡す / Slackに通知する"],
  ["on_question", "確認を聞かれたとき",     "文字列を返すと自動で送信、返さなければ人間の判断待ち"],
  ["on_exit",     "セッションが終了したとき", "例: 切断されたら再接続する"],
  ["on_busy",     "応答が始まったとき（上級）",
   "例: 処理中に定期的に様子を見る (while shikisha.state(tab)==\"BUSY\" do ... end)"],
  ["_shared",     "共通の下請け関数",       "他のイベントから呼べる関数を定義しておく場所"],
];
let autoTarget = null, autoData = {}, autoEvent = "on_done";

function autoDirOf(ws, t) {
  if (t.automation) return t.automation;
  // 規約で自動命名し、以後は設定に保存される（リネームしても壊れない）
  const slug = s => (s || "").replace(/[^A-Za-z0-9_-]/g, "").toLowerCase();
  const wi = wss.indexOf(ws) + 1, ti = (ws.tabs || []).indexOf(t) + 1;
  return "scripts/" + (slug(ws.name) || ("ws" + wi)) + "/" + (slug(t.name) || ("tab" + ti));
}

async function openAuto(ws, t) {
  autoTarget = { ws, t, dir: autoDirOf(ws, t) };
  const idx = (ws.tabs || []).indexOf(t) + 1;
  document.getElementById("autotitle").textContent =
      `自動化 — (${idx}) ` + (t.name || "無名タブ") + "　[" + autoTarget.dir + "]";
  const sel = document.getElementById("autoevent");
  sel.textContent = "";
  for (const [id, label] of EVENTS) sel.append(el("option", {value:id}, label));
  try {
    autoData = await (await fetch("/api/automation?dir=" + encodeURIComponent(autoTarget.dir),
        {headers:{"X-Token":TOKEN}})).json();
  } catch (e) { autoData = {}; }
  autoEvent = "on_done"; sel.value = autoEvent;
  switchEvent();
  // AIが1つも無ければ、その機能だけ隠して案内を出す
  document.getElementById("airow").style.display = aiEngines.length ? "flex" : "none";
  document.getElementById("ainone").style.display = aiEngines.length ? "none" : "flex";
  document.getElementById("aipreview").style.display = "none";
  document.getElementById("autobox").style.display = "flex";
}
function switchEvent() {
  // 表示を切り替える前に、今の内容を控えておく
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
  if (!r.ok) return automsg("保存に失敗しました", "#ff4646");
  // 使ったフォルダを設定にも記録する（以後リネームしても壊れない）。
  // 新しく割り当てた場合だけ、設定側の保存が別途必要になる
  const created = autoTarget.t.automation !== autoTarget.dir;
  autoTarget.t.automation = autoTarget.dir;
  closeAuto(); render();
  msg(created ? "自動化を保存しました。下の「保存」も押してください" : "自動化を保存しました",
      "#39ff14");
}

async function askAi() {
  const want = document.getElementById("autoask").value.trim();
  if (!want) return automsg("やりたいことを書いてください", "#ff4646");
  automsg("AIに問い合わせています（数十秒かかることがあります）…", "#ffea00");
  const r = await fetch("/api/generate", {method:"POST",
      headers:{"X-Token":TOKEN,"Content-Type":"application/json"},
      // タブ構成も渡す。これが無いと「検査タブへ送って」と書いても
      // AIは送信先の番号が分からない
      body: JSON.stringify({event: autoEvent, prompt: want,
                            engine: document.getElementById("aiengine").value || null,
                            tabs: (autoTarget.ws.tabs || []).map((x, i) =>
                                    ({index: i + 1, name: x.name || ("タブ" + (i + 1)),
                                      id: x.id || ""})),
                            self: (autoTarget.ws.tabs || []).indexOf(autoTarget.t) + 1})});
  const j = await r.json();
  if (!j.ok) return automsg("生成できませんでした: " + j.error, "#ff4646");
  document.getElementById("aicode").textContent = j.code;
  document.getElementById("aipreview").style.display = "block";
  automsg("内容を確認して「反映」を押してください", "#39ff14");
}
function applyAi() {
  document.getElementById("autocode").value = document.getElementById("aicode").textContent;
  document.getElementById("aipreview").style.display = "none";
  automsg("反映しました（まだ保存されていません）", "#39ff14");
}
function automsg(t, c) { const m = document.getElementById("automsg");
  m.textContent = t; m.style.color = c; }

// 外部ファイル参照のワークスペース定義を読み書きする
const wsApi = (m, file, b) => fetch("/api/workspace?file=" + encodeURIComponent(file), {
   method: m, headers: {"X-Token": TOKEN, "Content-Type":"application/json"}, body: b });

// 使えるAIコマンドを調べて、基本設定の選択肢を作る
let aiEngines = [];
async function loadAi() {
  try { aiEngines = (await (await fetch("/api/ai", {headers:{"X-Token":TOKEN}})).json()).engines || []; }
  catch (e) { aiEngines = []; }
  const sel = document.getElementById("aiengine");
  sel.textContent = "";
  const hint = document.getElementById("aihint");
  if (!aiEngines.length) {
    sel.append(el("option", {value:""}, "（見つかりません）"));
    sel.disabled = true;
    hint.textContent = "Claude Code / Codex CLI / Gemini CLI のいずれかを入れると、日本語で指示してコードを書いてもらえます";
    return;
  }
  sel.disabled = false;
  sel.append(el("option", {value:""}, "自動（見つかったものを使う）"));
  for (const e of aiEngines) sel.append(el("option", {value:e.id}, e.label));
  hint.textContent = aiEngines.length > 1
      ? "複数見つかりました。使いたいものを選べます"
      : "検出: " + aiEngines[0].label;
  // コマンド入力欄の候補も用意する
  const dl = document.getElementById("cmdlist");
  dl.textContent = "";
  for (const c of COMMON_COMMANDS) dl.append(el("option", {value:c.cmd}, c.label));
}

async function load() {
  await loadAi();
  current = await (await api("GET")).json();
  document.getElementById("aiengine").value = current.ai_engine ?? "";
  document.getElementById("tabw").value    = current.tab_bar_width ?? "";
  document.getElementById("chain").value   = current.max_chain ?? "";
  document.getElementById("lua").value     = current.automation ?? current.lua ?? "";
  document.getElementById("secrets").value = current.secrets ?? "";
  const list = (Array.isArray(current.workspaces) && current.workspaces.length)
      ? current.workspaces
      : [{ name: "DEFAULT", tabs: current.tabs || [] }];
  wss = [];
  for (const w of list) {
    const ws = { name: w.name || "", file: w.file || null,
                 automation: w.automation || w.lua || "", tabs: [] };
    if (ws.file) {
      // 定義ファイルの中身も読み込み、GUIから編集できるようにする
      try {
        const f = await (await wsApi("GET", ws.file)).json();
        ws.tabs = flatten(f.tabs, 0, []);
        if (!ws.automation) ws.automation = f.automation || f.lua || "";
      } catch (e) { ws.loadError = String(e); }
    } else {
      ws.tabs = flatten(w.tabs, 0, []);
    }
    wss.push(ws);
  }
  render();
  msg("読み込みました", "#39ff14");
}

async function save() {
  const out = Object.assign({}, current);
  const tabw  = document.getElementById("tabw").value;
  const chain = document.getElementById("chain").value;
  const lua   = document.getElementById("lua").value.trim();
  const sec   = document.getElementById("secrets").value.trim();
  tabw  ? out.tab_bar_width = Number(tabw) : delete out.tab_bar_width;
  chain ? out.max_chain     = Number(chain) : delete out.max_chain;
  lua   ? out.automation    = lua : delete out.automation;
  sec   ? out.secrets       = sec : delete out.secrets;
  const eng = document.getElementById("aiengine").value;
  eng   ? out.ai_engine     = eng : delete out.ai_engine;
  delete out.lua;

  delete out.tabs;
  // 外部ファイル参照のワークスペースは、その定義ファイル側に中身を書き戻す
  for (const w of wss) {
    if (!w.file) continue;
    const body = { name: w.name, tabs: nest(w.tabs) };
    if (w.automation) body.automation = w.automation;
    const rf = await wsApi("POST", w.file, JSON.stringify(body, null, 2));
    const jf = await rf.json().catch(() => ({ok:false, error:"保存に失敗"}));
    if (!jf.ok) return msg(w.file + " の保存に失敗: " + (jf.error || ""), "#ff4646");
  }
  out.workspaces = wss.map(w => {
    const o = { name: w.name };
    // 定義ファイル側にluaを書いたので、config側は参照だけにする
    if (w.file) o.file = w.file;
    else {
      if (w.automation) o.automation = w.automation;
      o.tabs = nest(w.tabs);
    }
    return o;
  });
  const r = await api("POST", JSON.stringify(out, null, 2));
  const j = await r.json();
  msg(j.ok ? "保存しました" : "保存失敗: " + j.error,
      j.ok ? "#39ff14" : "#ff4646");
}

function msg(t, c) { const m = document.getElementById("msg"); m.textContent = t; m.style.color = c; }
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

        let ui = WebUi::start(cfg.clone()).unwrap();
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

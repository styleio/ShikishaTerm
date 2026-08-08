//! スマホ等から見る監視・指示用のリモートUI。DESIGN.md 10.4章。
//!
//! 端末画面をそのまま再現するのではなく「状況を見て、一言指示する」ことに絞る。
//! 実装は既存の材料 (状態検出・応答キャプチャ・画面テキスト) をJSONで返すだけで、
//! WebSocketも端末エミュレータも要らない。
//!
//! 安全性:
//!   - 既定で無効。設定で明示的に有効化したときだけ待ち受ける
//!   - 待ち受け先はプライベート網に限定 (netaddr.rs)
//!   - 長さ32バイトのトークン必須。定数時間比較
//!   - 遠隔からの入力は「人間の操作」として扱う (自動チェーンをリセットする)

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use serde::Serialize;
use tiny_http::{Header, Response, Server};

/// 画面に見せるタブの状態 (本体から毎ティック更新される)
#[derive(Clone, Serialize, Default)]
pub struct RemoteTab {
    pub index: usize,
    pub name: String,
    pub state: String,
    pub locked: bool,
    /// 直近の応答 (無ければ画面の末尾)
    pub output: String,
    /// 確認待ちのときの画面 (選択肢を読むため)
    pub screen: String,
}

#[derive(Clone, Serialize, Default)]
pub struct Snapshot {
    /// 窓と同じ状態。スマホも同じページで描く
    #[serde(default)]
    pub ui: Option<crate::uistate::UiState>,
    /// 見ているタブの画面 (色付きのHTML)
    #[serde(default)]
    pub screen_html: String,
    pub workspace: String,
    pub tabs: Vec<RemoteTab>,
    pub auto_enabled: bool,
    /// 端末の桁数。画面はこの幅で描かれているので、
    /// スマホ側はこれを使って「折り返さずに収める」文字サイズを決める
    pub cols: u16,
}

/// リモートから届く操作。本体のループで実行される
#[derive(Debug)]
pub enum RemoteCmd {
    /// タブへ指示を送る (人間の入力として扱う)
    Send { tab: usize, text: String },
    /// 確認への返答など、生のキーを送る
    Keys { tab: usize, keys: String },
    /// 自動化の緊急停止 / 再開
    SetAuto(bool),
    /// 画面からの操作 (タブの切り替え・メニュー・打鍵)。
    /// 窓から来たものと同じ扱いで、同じ列に入る
    Ui(crate::browser::Ev),
}

/// スマホから受け付ける操作かどうか。
///
/// 同じページを配っている以上、送れる意図は窓と同じだけある。
/// だが窓の前にいないと意味が無いもの、窓を止めてしまうものがある。
/// 通すものを数え上げる側で書く。増やすのは、理由を書いてからでいい
fn allowed_from_afar(ev: &crate::browser::Ev) -> bool {
    use crate::browser::Ev;
    match ev {
        // 見たいタブを選ぶ・打つ・止める。遠隔操作の本体
        Ev::Select { .. } | Ev::Key { .. } | Ev::Stop => true,
        // 選んだ文字を控える。窓と同じ作法 (PuTTY と同じ) を保つ
        Ev::Copy { .. } => true,
        Ev::Menu { key } => !matches!(
            key.as_str(),
            // 設定とブラウザは窓の中に出る。手元では何も起きない
            "e" | "o"
            // マスターパスワードは窓に尋ねる。
            // 遠くから呼ぶと、窓の前の人が答えるまで本体が止まる
            | "k"
        ),
        // 大きさは窓が決める。手元の画面に合わせて
        // 相手のターミナルを畳んでしまう理由が無い。
        // 貼り付けも同じで、長押しひとつでAIの入力欄に流れ込む
        _ => false,
    }
}

/// 中継フレームの配信先 (接続中のWSクライアントごとに1本)。
/// 送れなくなった線は次のフレームで掃除する
type FrameClients = Arc<Mutex<Vec<Sender<Vec<u8>>>>>;

pub struct RemoteUi {
    pub url: String,
    pub note: Option<String>,
    pub snapshot: Arc<Mutex<Snapshot>>,
    pub rx: Receiver<RemoteCmd>,
    stop: Arc<AtomicBool>,
    /// 中継フレームの配信先。ブラウザから届いたJPEGをここへ流す
    frame_clients: FrameClients,
}

impl RemoteUi {
    pub fn start(bind: std::net::Ipv4Addr, port: u16, token: String) -> Result<Self> {
        let server = Server::http((bind, port)).map_err(|e| {
            let addr = format!("{bind}:{port}");
            // 「使用中」はOSの原文が長いわりに何をすればいいか言わない。
            // 相手は大抵、前に起動したままの自分自身
            let in_use = e
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::AddrInUse);
            if in_use {
                anyhow::anyhow!(crate::i18n::tp("remote.err.in_use", &[("addr", &addr)]))
            } else {
                anyhow::anyhow!(crate::i18n::tp(
                    "remote.err.start",
                    &[("addr", &addr), ("error", &e.to_string())]
                ))
            }
        })?;
        let real_port = server
            .server_addr()
            .to_ip()
            .context("ポート取得に失敗")?
            .port();
        let url = format!("http://{bind}:{real_port}/?t={token}");
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let (tx, rx) = channel::<RemoteCmd>();
        let stop = Arc::new(AtomicBool::new(false));
        let frame_clients: FrameClients = Arc::new(Mutex::new(Vec::new()));

        {
            let snapshot = Arc::clone(&snapshot);
            let stop = Arc::clone(&stop);
            let clients = Arc::clone(&frame_clients);
            std::thread::spawn(move || {
                for req in server.incoming_requests() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Err(e) = handle(req, &token, &snapshot, &tx, &clients) {
                        crate::append_hook_log(&format!("リモートUI: {e}"));
                    }
                }
            });
        }
        Ok(Self {
            url,
            note: None,
            snapshot,
            rx,
            stop,
            frame_clients,
        })
    }

    /// 中継フレーム (JPEGのバイト列) を、接続中のWSクライアント全員へ配る。
    /// 受け取れない線は捨てる (相手が閉じた・詰まった)
    pub fn push_frame(&self, jpeg: Vec<u8>) {
        let mut clients = self.frame_clients.lock().unwrap();
        clients.retain(|tx| tx.send(jpeg.clone()).is_ok());
    }

    /// 中継を見ているクライアントが1人でもいるか (誰も見ていなければ中継を止められる)
    pub fn has_frame_clients(&self) -> bool {
        !self.frame_clients.lock().unwrap().is_empty()
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn query_value(url: &str, key: &str) -> String {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .unwrap_or("")
        .to_string()
}

fn json_response(v: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(v.to_string()).with_header(
        Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap(),
    )
}

fn handle(
    req: tiny_http::Request,
    token: &str,
    snapshot: &Arc<Mutex<Snapshot>>,
    tx: &Sender<RemoteCmd>,
    frame_clients: &FrameClients,
) -> Result<()> {
    let supplied = {
        let h = req
            .headers()
            .iter()
            .find(|h| h.field.equiv("X-Token"))
            .map(|h| h.value.as_str().to_string())
            .unwrap_or_default();
        if h.is_empty() {
            query_value(req.url(), "t")
        } else {
            h
        }
    };
    if !token_eq(&supplied, token) {
        return req
            .respond(Response::from_string("forbidden").with_status_code(403))
            .map_err(Into::into);
    }

    let method = req.method().as_str().to_string();
    let path = req.url().split('?').next().unwrap_or("/").to_string();
    match (method.as_str(), path.as_str()) {
        // 窓と同じ外皮。見た目を2回書かないための入口
        ("GET", "/") | ("GET", "/shell") => {
            req.respond(
                Response::from_string(crate::shell::page(token))
                    .with_header(
                        Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    )
                    // 更新したのに古い画面が出る、を起こさない
                    .with_header(
                        Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap(),
                    ),
            )?;
        }
        ("GET", "/api/state") => {
            let snap = snapshot.lock().unwrap().clone();
            req.respond(json_response(serde_json::to_value(snap)?))?;
        }
        // 画面中継の受け口。握手してWebSocketへ格上げし、以後はこの線へ
        // JPEGフレームを流す (下り専用。ソケットは書き込みスレッドが持つ)
        ("GET", "/ws") => {
            let key = req
                .headers()
                .iter()
                .find(|h| h.field.equiv("Sec-WebSocket-Key"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            if key.is_empty() {
                return req
                    .respond(Response::from_string("expected websocket").with_status_code(400))
                    .map_err(Into::into);
            }
            let accept = crate::ws::accept_key(&key);
            let resp = Response::empty(101).with_header(
                Header::from_bytes(&b"Sec-WebSocket-Accept"[..], accept.as_bytes()).unwrap(),
            );
            let stream = req.upgrade("websocket", resp);
            let (ftx, frx) = channel::<Vec<u8>>();
            frame_clients.lock().unwrap().push(ftx);
            std::thread::spawn(move || {
                let mut w = crate::ws::WsWriter::new(stream);
                while let Ok(jpeg) = frx.recv() {
                    if w.send_binary(&jpeg).is_err() {
                        break;
                    }
                }
                let _ = w.send_close();
            });
        }
        ("POST", "/api/send") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let tab = v.get("tab").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
            if let Some(text) = v.get("text").and_then(|x| x.as_str()) {
                let _ = tx.send(RemoteCmd::Send {
                    tab,
                    text: text.to_string(),
                });
            } else if let Some(keys) = v.get("keys").and_then(|x| x.as_str()) {
                let _ = tx.send(RemoteCmd::Keys {
                    tab,
                    keys: keys.to_string(),
                });
            }
            req.respond(json_response(serde_json::json!({"ok": true})))?;
        }
        // 画面からの操作。窓と同じ意図を、同じ言葉で受ける
        ("POST", "/api/intent") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let mut took = false;
            if let Some(ev) = crate::browser::parse_intent(&v) {
                if allowed_from_afar(&ev) {
                    let _ = tx.send(RemoteCmd::Ui(ev));
                    took = true;
                }
            }
            req.respond(json_response(serde_json::json!({"ok": took})))?;
        }
        ("POST", "/api/auto") => {
            let mut req = req;
            let mut body = String::new();
            req.as_reader().read_to_string(&mut body)?;
            let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
            let on = v.get("on").and_then(|x| x.as_bool()).unwrap_or(false);
            let _ = tx.send(RemoteCmd::SetAuto(on));
            req.respond(json_response(serde_json::json!({"ok": true})))?;
        }
        _ => {
            req.respond(Response::from_string("not found").with_status_code(404))?;
        }
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;


    /// 遠くから送れる操作を、通す側で数えていること。
    ///
    /// 同じページを配る以上、送れる意図は窓と同じだけある。
    /// だが大きさは窓が決めるものだし、マスターパスワードを遠くから呼ぶと
    /// 窓の前の人が答えるまで本体が止まる
    #[test]
    fn the_phone_cannot_reach_what_only_the_window_can_answer() {
        use crate::browser::Ev;
        let menu = |k: &str| super::allowed_from_afar(&Ev::Menu { key: k.into() });
        assert!(super::allowed_from_afar(&Ev::Select { tab: 2 }));
        assert!(super::allowed_from_afar(&Ev::Stop));
        assert!(menu("a") && menu("?") && menu("w"), "普通の操作が通らない");

        assert!(!menu("k"), "マスターパスワードを遠くから呼べてしまう");
        assert!(!menu("e") && !menu("o"), "窓の中にしか出ないものを呼べる");
        assert!(
            !super::allowed_from_afar(&Ev::Resize {
                rows: 10,
                cols: 20,
                area: (0, 0, 0, 0)
            }),
            "手元の画面に合わせて相手のターミナルを畳めてしまう"
        );
        assert!(
            !super::allowed_from_afar(&Ev::Paste),
            "長押しひとつでAIの入力欄に流れ込む"
        );
    }

    #[test]
    fn token_is_required_and_compared_safely() {
        assert!(token_eq("abc", "abc"));
        assert!(!token_eq("abc", "abd"));
        assert!(!token_eq("abc", "abcd"));
        assert_eq!(query_value("/?t=xyz", "t"), "xyz");
        assert_eq!(query_value("/api/state", "t"), "");
    }

    /// 実際にサーバーを起動し、認証と操作の受け渡しを確認する
    #[test]
    fn serves_state_and_forwards_commands() {
        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into()).unwrap();
        let base = ui.url.split("/?").next().unwrap().to_string();
        let agent = ureq::Agent::new_with_defaults();
        let status = |r: Result<ureq::http::Response<ureq::Body>, ureq::Error>| match r {
            Ok(x) => x.status().as_u16(),
            Err(ureq::Error::StatusCode(c)) => c,
            Err(e) => panic!("unexpected: {e}"),
        };

        // トークン無しは拒否
        assert_eq!(status(agent.get(&format!("{base}/api/state")).call()), 403);

        // 状態を返す
        ui.snapshot.lock().unwrap().tabs = vec![RemoteTab {
            index: 1,
            name: "実装".into(),
            state: "QUESTION".into(),
            ..Default::default()
        }];
        let body = agent
            .get(&format!("{base}/api/state?t=tok123456789012"))
            .call()
            .unwrap()
            .body_mut()
            .read_to_string()
            .unwrap();
        assert!(body.contains("実装") && body.contains("QUESTION"), "{body}");

        // 指示が本体へ届く
        agent
            .post(&format!("{base}/api/send?t=tok123456789012"))
            .send(r#"{"tab":1,"text":"続けて"}"#)
            .unwrap();
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Send { tab, text } => {
                assert_eq!((tab, text.as_str()), (1, "続けて"));
            }
            other => panic!("想定外: {other:?}"),
        }

        // 入口が窓と同じ外皮を配っていること。
        // 以前はスマホ用の古いページを別に持っていて、直しても
        // スマホ側には一度も出ないままだった
        for entry in ["/", "/shell"] {
            let page = agent
                .get(&format!("{base}{entry}?t=tok123456789012"))
                .call()
                .unwrap()
                .body_mut()
                .read_to_string()
                .unwrap();
            assert!(
                page.contains("api/intent") && page.contains("window.__state"),
                "{entry} が窓と同じ外皮を配っていない"
            );
        }

        // 画面からの操作が本体へ届くこと
        agent
            .post(&format!("{base}/api/intent?t=tok123456789012"))
            .send(r#"{"kind":"select","tab":2}"#)
            .unwrap();
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Ui(crate::browser::Ev::Select { tab }) => assert_eq!(tab, 2),
            other => panic!("想定外: {other:?}"),
        }

        // 窓にしか答えられないものは、受け取った時点で止める。
        // 通らなかったことは、次の受信で分かる (select が先に出てくる)
        agent
            .post(&format!("{base}/api/intent?t=tok123456789012"))
            .send(r#"{"kind":"menu","key":"k"}"#)
            .unwrap();
        agent
            .post(&format!("{base}/api/intent?t=tok123456789012"))
            .send(r#"{"kind":"select","tab":3}"#)
            .unwrap();
        match ui.rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap() {
            RemoteCmd::Ui(crate::browser::Ev::Select { tab }) => {
                assert_eq!(tab, 3, "止めたはずの操作が先に届いた")
            }
            other => panic!("窓にしか答えられないものが通った: {other:?}"),
        }
        ui.shutdown();
    }

    /// /ws が握手し、push_frame で流したJPEGがWSのバイナリフレームで届くこと。
    /// 生のTCPと自作の ws モジュールだけで端から端まで確かめる (電話も外部ツールも不要)
    #[test]
    fn ws_upgrades_and_delivers_a_frame() {
        use std::io::{Read, Write};
        use std::net::TcpStream;

        let ui = RemoteUi::start("127.0.0.1".parse().unwrap(), 0, "tok123456789012".into()).unwrap();
        let hostport = ui
            .url
            .trim_start_matches("http://")
            .split("/?")
            .next()
            .unwrap()
            .to_string();

        let mut sock = TcpStream::connect(&hostport).unwrap();
        // RFC 6455 の例と同じキー (accept は s3pP... になる)
        let req = "GET /ws?t=tok123456789012 HTTP/1.1\r\n\
             Host: localhost\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
             Sec-WebSocket-Version: 13\r\n\r\n";
        sock.write_all(req.as_bytes()).unwrap();

        // 応答ヘッダを \r\n\r\n まで読む
        let mut buf = Vec::new();
        let mut one = [0u8; 1];
        loop {
            sock.read_exact(&mut one).unwrap();
            buf.push(one[0]);
            if buf.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        let head = String::from_utf8_lossy(&buf);
        assert!(head.contains("101"), "101 で格上げされていない: {head}");
        assert!(
            head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
            "Sec-WebSocket-Accept が違う: {head}"
        );

        // 登録が済むまでの隙をおいてからフレームを流す
        std::thread::sleep(std::time::Duration::from_millis(200));
        ui.push_frame(vec![0xDE, 0xAD, 0xBE, 0xEF]);

        // サーバー→クライアントのフレームはマスク無し。ここで直接ほどく
        let mut hdr = [0u8; 2];
        sock.read_exact(&mut hdr).unwrap();
        assert_eq!(hdr[0] & 0x0F, 0x2, "バイナリフレームでない");
        let len = (hdr[1] & 0x7F) as usize;
        assert_eq!(hdr[1] & 0x80, 0, "サーバーフレームにマスクが付いている");
        let mut payload = vec![0u8; len];
        sock.read_exact(&mut payload).unwrap();
        assert_eq!(payload, vec![0xDE, 0xAD, 0xBE, 0xEF]);
        ui.shutdown();
    }
}

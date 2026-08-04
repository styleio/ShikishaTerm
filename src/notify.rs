//! 通知先 (Slack / Telegram)。DESIGN.md 8.4章。
//!
//! Luaサンドボックスは任意URLへ通信できない。通知はここに登録済みの宛先に対してのみ、
//! Rust側が送信する (ケーパビリティ注入)。悪意あるスクリプトを拾っても
//! 資格情報の外部送信には使えない。

use std::collections::HashMap;
use std::sync::mpsc;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Destination {
    /// Slack Incoming Webhook
    Slack { webhook: String },
    /// Telegram Bot API
    Telegram { token: String, chat_id: String },
}

pub struct Notifier {
    dests: HashMap<String, Destination>,
    /// 送信は別スレッドで行い、UIをブロックしない
    tx: mpsc::Sender<(Destination, String)>,
}

impl Notifier {
    pub fn new(dests: HashMap<String, Destination>) -> Self {
        let (tx, rx) = mpsc::channel::<(Destination, String)>();
        std::thread::spawn(move || {
            while let Ok((dest, text)) = rx.recv() {
                if let Err(e) = send_blocking(&dest, &text) {
                    crate::append_hook_log(&format!("通知失敗: {e}"));
                }
            }
        });
        Self { dests, tx }
    }

    pub fn is_empty(&self) -> bool {
        self.dests.is_empty()
    }

    /// 登録済みの全宛先へ送る (疎通確認用)
    pub fn send_all(&self, text: &str) -> String {
        let mut names: Vec<&str> = Vec::new();
        for (name, dest) in &self.dests {
            let _ = self.tx.send((dest.clone(), text.to_string()));
            names.push(name);
        }
        names.sort_unstable();
        format!(">> テスト通知を送信: {}", names.join(", "))
    }

    /// 宛先名で送信をキューイングする。戻り値は画面表示用のメッセージ
    pub fn send(&self, name: &str, text: &str) -> String {
        match self.dests.get(name) {
            Some(dest) => {
                let _ = self.tx.send((dest.clone(), text.to_string()));
                format!(">> NOTIFY[{name}] {text}")
            }
            None => format!(">> 通知先 '{name}' は未登録です (config.jsonのnotifyを確認)"),
        }
    }
}

fn send_blocking(dest: &Destination, text: &str) -> Result<(), String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();
    let result = match dest {
        Destination::Slack { webhook } => agent
            .post(webhook)
            .send_json(serde_json::json!({ "text": text })),
        Destination::Telegram { token, chat_id } => agent
            .post(&format!("https://api.telegram.org/bot{token}/sendMessage"))
            .send_json(serde_json::json!({ "chat_id": chat_id, "text": text })),
    };
    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_destination_is_reported() {
        let n = Notifier::new(HashMap::new());
        assert!(n.send("slack", "hi").contains("未登録"));
    }

    #[test]
    fn destination_parses_from_config_shape() {
        let d: HashMap<String, Destination> = serde_json::from_str(
            r#"{"slack":{"type":"slack","webhook":"https://example.com/x"},
                "tg":{"type":"telegram","token":"t","chat_id":"1"}}"#,
        )
        .unwrap();
        assert!(matches!(d["slack"], Destination::Slack { .. }));
        assert!(matches!(d["tg"], Destination::Telegram { .. }));
    }
}

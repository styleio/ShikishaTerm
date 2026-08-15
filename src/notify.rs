//! Notification destinations (Slack / Telegram). DESIGN.md section 8.4.
//!
//! The Lua sandbox can't talk to arbitrary URLs. Notifications are sent by
//! the Rust side, and only to destinations already registered here
//! (capability injection). Even a malicious script that gets picked up
//! can't use this to exfiltrate credentials.

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
    /// Sending happens on a separate thread, so it doesn't block the UI.
    tx: mpsc::Sender<(Destination, String)>,
}

impl Notifier {
    pub fn new(dests: HashMap<String, Destination>) -> Self {
        let (tx, rx) = mpsc::channel::<(Destination, String)>();
        std::thread::spawn(move || {
            while let Ok((dest, text)) = rx.recv() {
                if let Err(e) = send_blocking(&dest, &text) {
                    crate::append_hook_log(&crate::i18n::tp(
                        "err.notify.send_failed",
                        &[("e", e.as_str())],
                    ));
                }
            }
        });
        Self { dests, tx }
    }

    pub fn is_empty(&self) -> bool {
        self.dests.is_empty()
    }

    /// Send to every registered destination (for connectivity testing).
    pub fn send_all(&self, text: &str) -> String {
        let mut names: Vec<&str> = Vec::new();
        for (name, dest) in &self.dests {
            let _ = self.tx.send((dest.clone(), text.to_string()));
            names.push(name);
        }
        names.sort_unstable();
        crate::i18n::tp("err.notify.test_sent", &[("names", &names.join(", "))])
    }

    /// Queue a send by destination name. The return value is a message for
    /// on-screen display.
    pub fn send(&self, name: &str, text: &str) -> String {
        match self.dests.get(name) {
            Some(dest) => {
                let _ = self.tx.send((dest.clone(), text.to_string()));
                format!(">> NOTIFY[{name}] {text}")
            }
            None => crate::i18n::tp("err.notify.unknown_target", &[("name", name)]),
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
        assert!(n.send("slack", "hi").contains("not registered"));
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

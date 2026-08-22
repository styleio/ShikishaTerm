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
    /// The destination an unnamed `notify(text)` reaches (config's
    /// primary_notify). With exactly one destination configured, that one
    /// stands in when no primary was chosen
    primary: Option<String>,
    /// Sending happens on a separate thread, so it doesn't block the UI.
    tx: mpsc::Sender<(Destination, String)>,
}

impl Notifier {
    pub fn new(dests: HashMap<String, Destination>, primary: Option<String>) -> Self {
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
        Self { dests, primary, tx }
    }

    /// Send to a named destination, or — with `None` — to the primary.
    /// The return value is a message for on-screen display
    pub fn send_opt(&self, name: Option<&str>, text: &str) -> String {
        let resolved = name
            .map(str::to_string)
            .or_else(|| self.primary.clone())
            .or_else(|| {
                // A single configured destination is unambiguous
                (self.dests.len() == 1).then(|| self.dests.keys().next().unwrap().clone())
            });
        match resolved {
            Some(n) => self.send(&n, text),
            None => crate::i18n::t("err.notify.no_primary"),
        }
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

pub fn send_blocking(dest: &Destination, text: &str) -> Result<(), String> {
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
        let n = Notifier::new(HashMap::new(), None);
        assert!(n.send("slack", "hi").contains("not registered"));
    }

    #[test]
    fn unnamed_send_resolves_the_primary() {
        let two: HashMap<String, Destination> = serde_json::from_str(
            r#"{"a":{"type":"slack","webhook":"https://example.com/a"},
                "b":{"type":"slack","webhook":"https://example.com/b"}}"#,
        )
        .unwrap();
        // An explicit primary wins
        let n = Notifier::new(two.clone(), Some("b".into()));
        assert!(n.send_opt(None, "hi").contains("NOTIFY[b]"), "明示プライマリへ");
        // A named destination overrides the primary
        assert!(n.send_opt(Some("a"), "hi").contains("NOTIFY[a]"), "名指しが勝つ");
        // No primary + two destinations = ambiguous, refused with guidance
        let n = Notifier::new(two, None);
        assert!(!n.send_opt(None, "hi").contains("NOTIFY["), "曖昧なら送らない");
        // No primary + exactly one destination = unambiguous
        let one: HashMap<String, Destination> = serde_json::from_str(
            r#"{"solo":{"type":"slack","webhook":"https://example.com/x"}}"#,
        )
        .unwrap();
        let n = Notifier::new(one, None);
        assert!(n.send_opt(None, "hi").contains("NOTIFY[solo]"), "1件ならそれがプライマリ");
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

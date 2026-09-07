//! Notification destinations (Slack / Discord / Telegram). DESIGN.md section 8.4.
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
    /// Discord webhook. Same shape as Slack's -- a URL that takes a JSON body
    /// -- with a different field name and a much shorter limit
    Discord { webhook: String },
    /// This PC's own notification area. No address, no account, no network --
    /// the one destination that is configured by choosing it. It reaches the
    /// person sitting here with the window behind a browser, which is the one
    /// person a chat app is the wrong way to reach.
    Windows {},
}

impl Destination {
    /// How much text this service will take.
    ///
    /// Kept under each documented ceiling rather than at it. A message that
    /// runs over is not shortened by the service, it is refused, and a
    /// notification that fails is worse than one that is cut: nobody is
    /// waiting for a log line. Counted in characters, because the message
    /// this program sends most often is Japanese.
    fn limit(&self) -> usize {
        match self {
            // 2,000 is the documented ceiling
            Destination::Discord { .. } => 1_900,
            // 4,096 for sendMessage
            Destination::Telegram { .. } => 4_000,
            // Slack takes far more, but a long wall in a channel helps nobody
            Destination::Slack { .. } => 3_000,
            // A banner is two lines on screen and one line in the tray.
            // Windows itself stops drawing long before this; the cut is here
            // so that what it does draw ends in a word rather than mid-way.
            Destination::Windows {} => 200,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Destination::Slack { .. } => "slack",
            Destination::Telegram { .. } => "telegram",
            Destination::Discord { .. } => "discord",
            Destination::Windows {} => "windows",
        }
    }
}

/// Cut to `max` characters, marking the cut.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
}

pub struct Notifier {
    dests: HashMap<String, Destination>,
    /// The destination an unnamed `notify(text)` reaches (config's
    /// primary_notify). With exactly one destination configured, that one
    /// stands in when no primary was chosen
    primary: Option<String>,
    /// Sending happens on a separate thread, so it doesn't block the UI.
    /// The name travels with it: a failure that cannot say which destination
    /// failed is a failure nobody can act on.
    tx: mpsc::Sender<(String, Destination, String, Option<usize>)>,
}

impl Notifier {
    pub fn new(dests: HashMap<String, Destination>, primary: Option<String>) -> Self {
        let (tx, rx) = mpsc::channel::<(String, Destination, String, Option<usize>)>();
        std::thread::spawn(move || {
            while let Ok((name, dest, text, tab)) = rx.recv() {
                if let Err(e) = send_blocking_about(&dest, &text, tab) {
                    crate::append_hook_log(&crate::i18n::tp(
                        "err.notify.send_failed",
                        &[("e", &format!("{name} ({}): {e}", dest.name()))],
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
            let _ = self.tx.send((name.clone(), dest.clone(), text.to_string(), None));
            names.push(name);
        }
        names.sort_unstable();
        crate::i18n::tp("err.notify.test_sent", &[("names", &names.join(", "))])
    }

    /// Queue a send by destination name. The return value is a message for
    /// on-screen display.
    pub fn send(&self, name: &str, text: &str) -> String {
        self.send_about(name, text, None)
    }

    /// The same, saying which tab the message is about.
    ///
    /// Only this PC's own notification area has anywhere to put that: a banner
    /// can be clicked, and a click that lands on the tab the message came from
    /// saves the search that the notification was supposed to spare. A chat
    /// app gets a link or nothing, and neither of those is a tab number.
    pub fn send_about(&self, name: &str, text: &str, tab: Option<usize>) -> String {
        match self.dests.get(name) {
            Some(dest) => {
                let _ = self.tx.send((name.to_string(), dest.clone(), text.to_string(), tab));
                format!(">> NOTIFY[{name}] {text}")
            }
            None => crate::i18n::tp("err.notify.unknown_target", &[("name", name)]),
        }
    }
}

pub fn send_blocking(dest: &Destination, text: &str) -> Result<(), String> {
    send_blocking_about(dest, text, None)
}

pub fn send_blocking_about(
    dest: &Destination,
    text: &str,
    tab: Option<usize>,
) -> Result<(), String> {
    // Nothing to post, nothing to time out: this one is a call into the shell.
    if let Destination::Windows {} = dest {
        // A banner has room for a heading and a line under it. The message is
        // built with the most important thing first (which tab), the next most
        // important on the line after (what it said), and everything below
        // that -- a link to answer from a phone -- for the services that show
        // a wall of text. So the banner takes the first two lines and lets the
        // rest go, rather than showing a URL nobody at this PC needs.
        let mut lines = text.lines();
        let title = lines.next().unwrap_or_default();
        let body = lines.next().unwrap_or_default();
        return crate::wintoast::show(title, &clip(body, dest.limit()), tab);
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();
    let text = &clip(text, dest.limit());
    let result = match dest {
        Destination::Slack { webhook } => agent
            .post(webhook)
            .send_json(serde_json::json!({ "text": text })),
        Destination::Telegram { token, chat_id } => agent
            .post(&format!("https://api.telegram.org/bot{token}/sendMessage"))
            .send_json(serde_json::json!({ "chat_id": chat_id, "text": text })),
        // Discord answers 204 with no body when it takes the message
        Destination::Discord { webhook } => agent
            .post(webhook)
            .send_json(serde_json::json!({ "content": text })),
        // Handled above, before an HTTP agent was ever built.
        Destination::Windows {} => unreachable!(),
    };
    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

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
                "dc":{"type":"discord","webhook":"https://discord.com/api/webhooks/1/x"},
                "tg":{"type":"telegram","token":"t","chat_id":"1"}}"#,
        )
        .unwrap();
        assert!(matches!(d["slack"], Destination::Slack { .. }));
        assert!(matches!(d["dc"], Destination::Discord { .. }));
        assert!(matches!(d["tg"], Destination::Telegram { .. }));
    }

    /// A message over the service's ceiling is refused, not shortened, and a
    /// notification that never arrives is the one failure this feature cannot
    /// afford -- nobody is watching for it.
    #[test]
    fn a_message_too_long_is_cut_rather_than_lost() {
        let discord = Destination::Discord { webhook: String::new() };
        let telegram = Destination::Telegram { token: String::new(), chat_id: String::new() };
        assert!(discord.limit() < 2000, "Discord の 2,000 文字を超えない");
        assert!(telegram.limit() < 4096, "Telegram の 4,096 文字を超えない");

        let long = "あ".repeat(5000);
        for d in [&discord, &telegram] {
            let cut = clip(&long, d.limit());
            assert_eq!(cut.chars().count(), d.limit(), "上限ちょうどに収まる");
            assert!(cut.ends_with('…'), "切ったことが読み手に分かる");
        }
        // Counted in characters, not bytes: three bytes each, and a limit
        // measured in bytes would cut a Japanese message to a third
        assert!(clip(&long, 100).len() > 100, "バイト数で切っていない");
        // Short enough is left exactly as it was
        assert_eq!(clip("そのまま", 10), "そのまま");
        assert_eq!(clip("", 10), "");
    }

    /// What actually goes on the wire.
    ///
    /// Each service wants the text under a different name, and getting that
    /// wrong is not visible from here -- the request succeeds or it does not,
    /// and a webhook that answers 204 to a body it ignored looks exactly like
    /// one that delivered. So the body is read back from a server of our own.
    #[test]
    fn each_service_gets_the_body_it_expects() {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("listen");
        let port = server.server_addr().to_ip().expect("ip").port();
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let t = std::thread::spawn(move || {
            for _ in 0..2 {
                let Ok(mut req) = server.recv() else { return };
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                kept.lock().unwrap().push((req.url().to_string(), body));
                let _ = req.respond(tiny_http::Response::empty(204));
            }
        });

        let hook = format!("http://127.0.0.1:{port}/hook");
        send_blocking(&Destination::Discord { webhook: hook.clone() }, "終わりました").unwrap();
        send_blocking(&Destination::Slack { webhook: hook }, "終わりました").unwrap();
        t.join().unwrap();

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2, "2件届いていない");
        // Discord reads "content"; Slack reads "text". Neither accepts the other's
        let discord: serde_json::Value = serde_json::from_str(&seen[0].1).unwrap();
        assert_eq!(discord["content"], "終わりました");
        assert!(discord.get("text").is_none());
        let slack: serde_json::Value = serde_json::from_str(&seen[1].1).unwrap();
        assert_eq!(slack["text"], "終わりました");
        assert!(slack.get("content").is_none());
        // ...and the webhook URL is used as given, path and all
        assert_eq!(seen[0].0, "/hook");
    }

    /// The name travels with the message so a failed send can say which
    /// destination failed. Before, the log said only that something did.
    #[test]
    fn a_failure_can_name_the_destination() {
        assert_eq!(Destination::Discord { webhook: String::new() }.name(), "discord");
        assert_eq!(Destination::Slack { webhook: String::new() }.name(), "slack");
        assert_eq!(
            Destination::Telegram { token: String::new(), chat_id: String::new() }.name(),
            "telegram"
        );
    }
}

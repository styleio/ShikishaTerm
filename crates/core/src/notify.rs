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
    /// The phones that have asked to be notified (src/push.rs). Also no
    /// address and no account -- a phone subscribes itself from the settings
    /// screen, and the message is encrypted for that phone before it is handed
    /// to anybody to carry.
    Phone {},
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
            // A push service guarantees to carry 4KB, and what it carries here
            // is encrypted, which costs a little more than what went in. A
            // phone's lock screen shows two or three lines regardless.
            Destination::Phone {} => 1_000,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Destination::Slack { .. } => "slack",
            Destination::Telegram { .. } => "telegram",
            Destination::Discord { .. } => "discord",
            Destination::Windows {} => "windows",
            Destination::Phone {} => "phone",
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

/// Where the workspace on screen is allowed to send, and where it sends when
/// nobody named a destination.
///
/// One answer, settled at launch by [`crate::config::Config::resolve_workspaces`]
/// and swapped when the workspace changes. Nothing here knows whether the
/// workspace said it or the app did
#[derive(Default)]
struct Reach {
    /// The names that can be reached, or `None` for every registered one
    only: Option<Vec<String>>,
    /// The destination an unnamed `notify(text)` reaches. With exactly one
    /// destination reachable, that one stands in when none was chosen
    primary: Option<String>,
}

pub struct Notifier {
    dests: HashMap<String, Destination>,
    /// Swapped on a workspace switch, so it sits behind a cell: everything
    /// holds the notifier by reference, and a send and a switch never happen
    /// at the same moment
    reach: std::cell::RefCell<Reach>,
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
                    record_failure(&name, &dest, &e);
                }
            }
        });
        Self {
            dests,
            reach: std::cell::RefCell::new(Reach { only: None, primary }),
            tx,
        }
    }

    /// Point it at the workspace now on screen.
    ///
    /// Called on every switch, with that workspace's settled answer. Until this
    /// existed there was one destination list for the whole app, so the AI in
    /// the work workspace and the AI in the personal one finished their tasks
    /// into the same chat -- and which chat it was depended on nothing a person
    /// could see from where they were working
    pub fn scope_to(&self, only: Option<Vec<String>>, primary: Option<String>) {
        *self.reach.borrow_mut() = Reach { only, primary };
    }

    /// Whether this workspace can reach a destination at all. A name nobody
    /// registered is not reachable either, and is reported as unknown
    fn reachable(&self, name: &str) -> bool {
        self.dests.contains_key(name)
            && self
                .reach
                .borrow()
                .only
                .as_ref()
                .is_none_or(|l| l.iter().any(|n| n == name))
    }

    /// The destinations this workspace can reach, in name order
    fn reaching(&self) -> Vec<String> {
        let mut names: Vec<String> = self.dests.keys().filter(|n| self.reachable(n)).cloned().collect();
        names.sort();
        names
    }

    /// Send to a named destination, or — with `None` — to the primary.
    /// The return value is a message for on-screen display
    pub fn send_opt(&self, name: Option<&str>, text: &str) -> String {
        let resolved = name
            .map(str::to_string)
            .or_else(|| self.reach.borrow().primary.clone())
            .or_else(|| {
                // A single reachable destination is unambiguous
                let mut reaching = self.reaching();
                (reaching.len() == 1).then(|| reaching.remove(0))
            });
        match resolved {
            Some(n) => self.send(&n, text),
            None => crate::i18n::t("err.notify.no_primary"),
        }
    }

    /// Whether this workspace has anywhere to send at all
    pub fn is_empty(&self) -> bool {
        self.reaching().is_empty()
    }

    /// Send to every destination this workspace can reach (for connectivity
    /// testing). The test button is answering "does a message from here
    /// arrive", so it sends exactly where work from here would
    pub fn send_all(&self, text: &str) -> String {
        let names = self.reaching();
        for name in &names {
            if let Some(dest) = self.dests.get(name) {
                let _ = self.tx.send((name.clone(), dest.clone(), text.to_string(), None));
            }
        }
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
        // Registered, but not from here. Said as its own sentence rather than
        // as "no such destination": the name is spelled right and the
        // destination does exist, and a person told otherwise would go looking
        // for a typo that is not there
        if self.dests.contains_key(name) && !self.reachable(name) {
            return crate::i18n::tp("err.notify.not_reachable", &[("name", name)]);
        }
        match self.dests.get(name) {
            Some(dest) => {
                let _ = self.tx.send((name.to_string(), dest.clone(), text.to_string(), tab));
                format!(">> NOTIFY[{name}] {text}")
            }
            None => crate::i18n::tp("err.notify.unknown_target", &[("name", name)]),
        }
    }
}

/// The sends that failed, waiting to be said on screen.
///
/// The sending thread has no screen of its own. Until this queue existed a
/// failure went only to hooks.log, and a person whose phone stayed quiet had
/// nothing in front of them to say why -- the board reads it off here
/// (`take_failed`) and shows it as the same toast everything else uses.
static FAILED: std::sync::Mutex<std::collections::VecDeque<String>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// A send that did not happen: logged, and queued for the screen. The
/// destination is named, so the failure can say which one it was.
fn record_failure(name: &str, dest: &Destination, e: &str) {
    let said = crate::i18n::tp(
        "err.notify.send_failed",
        &[("e", &format!("{name} ({}): {e}", dest.name()))],
    );
    crate::append_hook_log(&said);
    let mut q = FAILED.lock().unwrap_or_else(|p| p.into_inner());
    // A destination that keeps failing is not a backlog to work through: the
    // queue stays short, and the oldest goes first.
    if q.len() >= 8 {
        q.pop_front();
    }
    q.push_back(said);
}

/// The oldest failure not yet shown, if any.
pub fn take_failed() -> Option<String> {
    FAILED.lock().unwrap_or_else(|p| p.into_inner()).pop_front()
}

pub fn send_blocking(dest: &Destination, text: &str) -> Result<(), String> {
    send_blocking_about(dest, text, None)
}

pub fn send_blocking_about(
    dest: &Destination,
    text: &str,
    tab: Option<usize>,
) -> Result<(), String> {
    // A phone that subscribed for itself. The message is split the same way a
    // banner splits it -- a heading and a line -- and the link, when there is
    // one, becomes where a tap goes rather than text nobody can tap.
    if let Destination::Phone {} = dest {
        let mut lines = text.lines();
        let title = lines.next().unwrap_or_default();
        let rest: Vec<&str> = lines.collect();
        // The reply link is written on its own line, last (see
        // on_done_message). On a phone it is worth more as the destination of
        // a tap than as a line of text, so it is lifted out of the body.
        let link = rest
            .last()
            .copied()
            .filter(|l| l.starts_with("http://") || l.starts_with("https://"));
        let body = match link {
            Some(_) => &rest[..rest.len() - 1],
            None => &rest[..],
        };
        let body = clip(body.join(" ").trim(), dest.limit());
        return crate::push::send(title, &body, link).map(|_| ());
    }
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
        return match local_banners() {
            Some(t) => t.show(title, &clip(body, dest.limit()), tab),
            None => Err("no shell is running to show a banner".into()),
        };
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
        // Both handled above, before an HTTP agent was ever built.
        Destination::Windows {} | Destination::Phone {} => unreachable!(),
    };
    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// The shell that can show a banner here, if one is running.
///
/// Set once, by whatever owns the desktop, before notifications start flowing.
/// The runtime never constructs one: it only asks whether there is one.
static LOCAL_BANNERS: std::sync::OnceLock<Box<dyn shikisha_shared::Toasts>> =
    std::sync::OnceLock::new();

pub fn use_local_banners(t: Box<dyn shikisha_shared::Toasts>) {
    let _ = LOCAL_BANNERS.set(t);
}

fn local_banners() -> Option<&'static dyn shikisha_shared::Toasts> {
    LOCAL_BANNERS.get().map(|b| b.as_ref())
}

/// The tab a person pressed a banner for, if a shell is showing banners at all.
pub fn banner_clicked_tab() -> Option<usize> {
    local_banners().and_then(|t| t.clicked_tab())
}

/// Bring whatever is showing this to the front. Nothing to bring, nothing done.
pub fn banner_raise() {
    if let Some(t) = local_banners() {
        t.raise();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A send that failed reaches the screen, naming the destination, once.
    #[test]
    fn a_failure_waits_to_be_shown_on_screen() {
        record_failure("test", &Destination::Phone {}, "nobody there");
        // Other tests may queue failures of their own beside this one; the
        // one recorded here is found by its name.
        let mut seen = Vec::new();
        while let Some(said) = take_failed() {
            seen.push(said);
        }
        let mine: Vec<&String> = seen.iter().filter(|s| s.contains("nobody there")).collect();
        assert_eq!(mine.len(), 1, "一度の失敗は一度だけ画面に出る: {seen:?}");
        assert!(mine[0].contains("test (phone)"), "宛先の名前が無い: {}", mine[0]);
    }
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

    /// A workspace can only send where that workspace is allowed to send.
    ///
    /// The accident this prevents is not a mistake in the script: the
    /// destinations are registered once for the whole app, so automation
    /// written for work, running in the work workspace, could name the personal
    /// chat and be obeyed. With a line drawn, it is refused -- and told that it
    /// was refused from here, rather than that the name does not exist
    #[test]
    fn a_workspace_only_reaches_its_own_destinations() {
        let two: HashMap<String, Destination> = serde_json::from_str(
            r#"{"work":{"type":"slack","webhook":"https://example.com/a"},
                "mine":{"type":"slack","webhook":"https://example.com/b"}}"#,
        )
        .unwrap();
        let n = Notifier::new(two, Some("mine".into()));
        // Before anybody draws a line, the app's answer is the answer
        assert!(n.send_opt(None, "hi").contains("NOTIFY[mine]"));
        assert!(n.send("work", "hi").contains("NOTIFY[work]"));

        // The work workspace: its own default, and the personal chat out of reach
        n.scope_to(Some(vec!["work".into()]), Some("work".into()));
        assert!(n.send_opt(None, "hi").contains("NOTIFY[work]"), "この環境の既定に行かない");
        let said = n.send("mine", "hi");
        assert!(!said.contains("NOTIFY["), "他の環境の宛先に送れてしまう");
        assert!(said.contains("mine"), "どの宛先のことか言っていない: {said}");
        assert!(!said.contains("not registered"), "存在しないと言ってはいけない: {said}");
        // What the test button sends, and whether there is anywhere to send
        assert!(!n.is_empty());
        let sent = n.send_all("test");
        assert!(sent.contains("work") && !sent.contains("mine"), "テスト送信が外へ漏れる: {sent}");

        // A workspace with nothing it can reach says so, rather than falling
        // back to the app's destination
        n.scope_to(Some(Vec::new()), None);
        assert!(n.is_empty(), "送れないのに送れると言っている");
        assert!(!n.send_opt(None, "hi").contains("NOTIFY["));

        // One reachable destination and no default named is unambiguous
        n.scope_to(Some(vec!["work".into()]), None);
        assert!(n.send_opt(None, "hi").contains("NOTIFY[work]"));
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


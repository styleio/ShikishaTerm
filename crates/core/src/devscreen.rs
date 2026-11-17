//! What the two doors a DevTools screen connects through agree on.
//!
//! The browser's own DevTools screen (`devtools://devtools/...`) talks the
//! DevTools protocol over a WebSocket it is given the address of. This
//! program gives it a door of its own on the loopback, never the browser's
//! port: one in the window (`src/devtools.rs`, for a page the window draws)
//! and one beside the server's browser (`chrome.rs`). Both let in only that
//! screen, and both hold to the same limits, which are written once here

/// The one origin a connection is answered from
pub const ORIGIN: &str = "devtools://devtools";

/// The most the screen may say in one message. What it sends are commands --
/// a node to inspect, an expression to run -- and a command larger than
/// this is a script pasted into the console, which still fits
pub const ONE_COMMAND: usize = 8 * 1024 * 1024;

/// How far behind a screen may fall before it is let go. A page that says a
/// great deal to a screen that has stopped reading -- a tab asleep, a line
/// gone slow -- would otherwise be kept in memory without end
pub const BEHIND: usize = 64 * 1024 * 1024;

/// How long a screen may take nothing of what is written to it before it is
/// taken for gone
pub const STALLED: std::time::Duration = std::time::Duration::from_secs(30);

/// What becomes of something a screen asks
#[derive(Debug, PartialEq, Eq)]
pub enum Asked {
    /// Passed on to the page
    Pass,
    /// Answered here with nothing, as if done: the screen asks it as a
    /// matter of course and goes on without what it would have brought
    Nothing,
    /// Refused: it reaches past the page the screen was opened for
    Refused,
}

/// Whether a screen may ask `method`, with these parameters, of the page
/// it was opened for. `own` says whether a session it names is one of the
/// screen's own (a frame's, a worker's), and `target` is the page's own id
/// when the door knows it.
///
/// A screen is for one page. The protocol it speaks is not: from a page's
/// session it can list every page in the browser, attach to any of them,
/// open new ones, close them, and stop the browser -- so a screen opened on
/// one tab could read another tab's cookies. What does that is refused
/// here, by name. The `Target` domain is taken the other way round, since
/// reaching other pages is what it is for: only what a screen needs to
/// follow its own page's frames and workers is let through
pub fn asked(method: &str, params: &serde_json::Value, target: Option<&str>, own: &dyn Fn(&str) -> bool) -> Asked {
    let named = |k: &str| params.get(k).and_then(|v| v.as_str());
    match method {
        // The page's own frames and workers, attached as they appear
        "Target.setAutoAttach" | "Target.autoAttachRelated" => Asked::Pass,
        // About itself; about another page, never
        "Target.getTargetInfo" => match named("targetId") {
            None => Asked::Pass,
            Some(t) if target == Some(t) => Asked::Pass,
            Some(_) => Asked::Refused,
        },
        "Target.detachFromTarget" => match named("sessionId") {
            Some(s) if own(s) => Asked::Pass,
            _ => Asked::Refused,
        },
        // Being told of every page there is: asked by every screen as it
        // opens, and answered as done with nothing told
        "Target.setDiscoverTargets" => Asked::Nothing,
        m if m.starts_with("Target.") => Asked::Refused,
        // The browser as a whole is not the page's
        "Browser.close" | "Browser.crash" | "Browser.crashGpuProcess" | "Browser.setDownloadBehavior" | "Browser.cancelDownload" => Asked::Refused,
        _ => Asked::Pass,
    }
}

/// What a door that passes a screen's words on as they are does with one
/// message
#[derive(Debug, PartialEq, Eq)]
pub enum Said {
    /// Passed on
    On,
    /// Not passed on: answered with this instead
    Instead(String),
    /// Not a message at all
    Dropped,
}

/// One message from a screen, judged by [`asked`], for a door that passes
/// messages on whole. The answer given instead carries the session the
/// message named, so the screen files it where it asked from
pub fn said(text: &str, target: Option<&str>, own: &dyn Fn(&str) -> bool) -> Said {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Said::Dropped };
    let Some(method) = v.get("method").and_then(|m| m.as_str()) else { return Said::Dropped };
    let id = v.get("id").cloned().unwrap_or_default();
    let mut answer = match asked(method, v.get("params").unwrap_or(&serde_json::Value::Null), target, own) {
        Asked::Pass => return Said::On,
        Asked::Nothing => serde_json::json!({ "id": id, "result": {} }),
        Asked::Refused => refusal(&id),
    };
    if let Some(through) = v.get("sessionId") {
        answer["sessionId"] = through.clone();
    }
    Said::Instead(answer.to_string())
}

/// How many connections a door serves at once. A person has a screen or two
/// open; whoever opens more than this is not opening screens
pub const AT_ONCE: usize = 32;

/// The connections a door is serving, counted: anybody on the machine can
/// knock, and past [`AT_ONCE`] the rest are not answered
#[derive(Clone, Default)]
pub struct Serving(std::sync::Arc<std::sync::atomic::AtomicUsize>);

/// One connection being served; counted out again when let go of
pub struct Served(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Serving {
    /// Count one more in, or `None` when there is no room for it
    pub fn enter(&self) -> Option<Served> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.0.fetch_add(1, SeqCst) >= AT_ONCE {
            self.0.fetch_sub(1, SeqCst);
            return None;
        }
        Some(Served(self.0.clone()))
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The answer a refused ask is given, under the number it was asked by
pub fn refusal(id: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "id": id, "error": { "code": -32000, "message": "Not allowed from a DevTools screen opened for one page." } })
}

/// How much a screen may say before its page has a session for it. It says
/// a handful of commands as it opens; more than this is not a screen opening
pub const EARLY: usize = 16 * 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    /// A screen follows its own page and its frames, and reaches no other
    #[test]
    fn a_screen_is_refused_what_reaches_past_its_page() {
        let own = |s: &str| s == "CHILD";
        let ask = |method: &str, params: serde_json::Value| asked(method, &params, Some("PAGE"), &own);
        let none = serde_json::json!({});
        // The everyday talk of a screen
        for m in ["Runtime.evaluate", "DOM.getDocument", "Page.enable", "Network.enable", "Browser.getVersion", "Emulation.setDeviceMetricsOverride"] {
            assert_eq!(ask(m, none.clone()), Asked::Pass, "{m}");
        }
        assert_eq!(ask("Target.setAutoAttach", serde_json::json!({"autoAttach": true, "flatten": true})), Asked::Pass);
        assert_eq!(ask("Target.getTargetInfo", none.clone()), Asked::Pass);
        assert_eq!(ask("Target.getTargetInfo", serde_json::json!({"targetId": "PAGE"})), Asked::Pass);
        assert_eq!(ask("Target.detachFromTarget", serde_json::json!({"sessionId": "CHILD"})), Asked::Pass);
        assert_eq!(ask("Target.setDiscoverTargets", serde_json::json!({"discover": true})), Asked::Nothing);
        // Another page, by any road
        assert_eq!(ask("Target.attachToTarget", serde_json::json!({"targetId": "OTHER", "flatten": true})), Asked::Refused);
        assert_eq!(ask("Target.attachToTarget", serde_json::json!({"targetId": "PAGE"})), Asked::Refused, "a second session on its own page is asked for by nothing a screen does");
        assert_eq!(ask("Target.getTargetInfo", serde_json::json!({"targetId": "OTHER"})), Asked::Refused);
        assert_eq!(ask("Target.detachFromTarget", serde_json::json!({"sessionId": "SOMEBODY"})), Asked::Refused);
        for m in ["Target.getTargets", "Target.createTarget", "Target.closeTarget", "Target.attachToBrowserTarget", "Target.exposeDevToolsProtocol", "Target.sendMessageToTarget", "Target.createBrowserContext", "Target.somethingNew"] {
            assert_eq!(ask(m, none.clone()), Asked::Refused, "{m}");
        }
        // With the page's id not known, no page may be named at all
        assert_eq!(asked("Target.getTargetInfo", &serde_json::json!({"targetId": "PAGE"}), None, &own), Asked::Refused);
        // The browser itself
        for m in ["Browser.close", "Browser.crash", "Browser.setDownloadBehavior"] {
            assert_eq!(ask(m, none.clone()), Asked::Refused, "{m}");
        }
    }

    /// A door that passes messages on whole answers the ones it does not,
    /// under their own number and session
    #[test]
    fn a_message_not_passed_on_is_answered_where_it_was_asked_from() {
        let own = |_: &str| true;
        assert_eq!(said(r#"{"id":1,"method":"Runtime.evaluate","params":{"expression":"1"}}"#, None, &own), Said::On);
        let Said::Instead(answer) = said(r#"{"id":7,"sessionId":"W1","method":"Target.attachToTarget","params":{"targetId":"X"}}"#, None, &own) else {
            panic!("reaching another page was passed on");
        };
        let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["sessionId"], "W1");
        assert!(answer.get("error").is_some(), "{answer}");
        let Said::Instead(answer) = said(r#"{"id":8,"method":"Target.setDiscoverTargets","params":{"discover":true}}"#, None, &own) else {
            panic!("not answered");
        };
        assert_eq!(serde_json::from_str::<serde_json::Value>(&answer).unwrap(), serde_json::json!({"id": 8, "result": {}}));
        assert_eq!(said("not json", None, &own), Said::Dropped);
        assert_eq!(said(r#"{"id":9}"#, None, &own), Said::Dropped);
    }

    /// A door serves so many at once, and has room again when one goes
    #[test]
    fn a_door_serves_only_so_many_at_once() {
        let door = Serving::default();
        let held: Vec<Served> = (0..AT_ONCE).map(|_| door.enter().expect("room")).collect();
        assert!(door.enter().is_none(), "one more than the most was let in");
        drop(held);
        assert!(door.enter().is_some(), "no room after they left");
    }
}

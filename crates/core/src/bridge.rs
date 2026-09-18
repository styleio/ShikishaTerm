//! model bridge: hits an OpenAI-compatible `/chat/completions` endpoint to
//! get response text.
//!
//! DeepSeek (cloud), Ollama (local Qwen/DeepSeek), OpenRouter, etc. all go
//! through the same path — **just swap base_url and model**. SHIKISHA's own
//! identity is "a conductor that directs existing AIs," so this module is
//! deliberately kept to a thin pipe that just does "prompt -> hit the API ->
//! return the response," not an "AI" itself.
//!
//! Usage from discussions **does not spawn a subprocess**. Because the main
//! binary is a GUI subsystem and its ConPTY children have no console I/O,
//! when `Command::SendPrompt` arrives at a model pane, the main binary calls
//! `complete()` directly on a thread and injects the response into the tab
//! screen plus writes it to say.txt (in-process).
//!
//! The `--bridge` child process is kept around for direct terminal execution
//! via pipes. It reads the connection info from env, reads stdin once, and
//! writes the response to stdout (for testing / one-off use).

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;

/// A resolved model connection (held by a tab, passed to complete() each turn)
#[derive(Debug, Clone)]
pub struct ModelConn {
    /// The provider name as registered in settings (e.g. "deepseek"). Shown on
    /// the tab's title box; not used for the request itself.
    pub provider: String,
    pub url: String,
    pub model: String,
    pub headers: HashMap<String, String>,
    /// How long to wait for a whole reply. `None` waits as long as it takes
    /// (the setting's 0), which is what a local thinking model needs.
    pub timeout: Option<std::time::Duration>,
    /// The stance/persona for a discussion. The bridge is stateless, so
    /// unless this is attached as the system message every turn, the model
    /// forgets its stance and drifts off topic (only set when this is a
    /// discussion participant).
    pub persona: Option<String>,
    /// When set, this model is a browser-operation *brain*: it drives the
    /// browser tab with this id. Unlike a CLI agent (which writes `in.lua`
    /// itself), a model brain emits a ```lua block in its reply; the tab
    /// extracts it and hands it to the same rally orchestrator. Toggles the
    /// rally system prompt and per-turn `on_done` firing in the tab's chat.
    pub drives: Option<String>,
}

/// The `--bridge` child process (for direct terminal execution via pipes).
/// Reads stdin once and returns the response to stdout.
pub fn run() -> Result<()> {
    let url = std::env::var("SHIKISHA_BRIDGE_URL")
        .with_context(|| crate::i18n::t("err.bridge.url_unset"))?;
    let model = std::env::var("SHIKISHA_BRIDGE_MODEL")
        .with_context(|| crate::i18n::t("err.bridge.model_unset"))?;
    let headers = std::env::var("SHIKISHA_BRIDGE_HEADERS")
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
        .unwrap_or_default();
    let system = std::env::var("SHIKISHA_BRIDGE_SYSTEM").ok();
    // Seconds, with 0 meaning "wait as long as it takes" -- the same words the
    // setting uses, so the child and the app cannot come to mean different things
    let timeout = match std::env::var("SHIKISHA_BRIDGE_TIMEOUT")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(crate::config::PROVIDER_TIMEOUT_DEFAULT_SEC)
    {
        0 => None,
        secs => Some(std::time::Duration::from_secs(secs)),
    };
    let mut prompt = String::new();
    std::io::stdin().read_to_string(&mut prompt)?;
    let out = complete(&url, &model, &headers, timeout, system.as_deref(), prompt.trim())?;
    print!("{out}");
    Ok(())
}

/// Why the request never came back, in words a person can act on.
///
/// ureq's own account of running out of time is "timeout: global", which names
/// neither what was waited for nor for how long. The number is the point: it is
/// a setting, and the person reading this is the one who can change it — and
/// with a model on their own machine, the honest answer is often "it was still
/// thinking".
fn why_it_failed(e: &ureq::Error, endpoint: &str, timeout: Option<std::time::Duration>) -> String {
    match (e, timeout) {
        (ureq::Error::Timeout(_), Some(t)) => crate::i18n::tp(
            "err.bridge.timed_out",
            &[("endpoint", endpoint), ("secs", &t.as_secs().to_string())],
        ),
        _ => crate::i18n::tp(
            "err.bridge.connect_failed",
            &[("endpoint", endpoint), ("e", &e.to_string())],
        ),
    }
}

/// Hit an OpenAI-compatible `/chat/completions` once and return the response
/// body.
pub fn complete(
    base_url: &str,
    model: &str,
    headers: &HashMap<String, String>,
    timeout: Option<std::time::Duration>,
    system: Option<&str>,
    user: &str,
) -> Result<String> {
    let mut messages = Vec::new();
    if let Some(s) = system.filter(|s| !s.trim().is_empty()) {
        messages.push(serde_json::json!({"role": "system", "content": s}));
    }
    messages.push(serde_json::json!({"role": "user", "content": user}));
    complete_messages(base_url, model, headers, timeout, &messages)
}

/// Like `complete`, but takes a full pre-built message list. Used for
/// multi-turn chat: the bridge is stateless, so the whole conversation is
/// replayed each call. `messages` is an OpenAI-style array of
/// `{"role":..., "content":...}` objects (system first, then the turns).
pub fn complete_messages(
    base_url: &str,
    model: &str,
    headers: &HashMap<String, String>,
    timeout: Option<std::time::Duration>,
    messages: &[serde_json::Value],
) -> Result<String> {
    let endpoint = chat_endpoint(base_url);
    let body = serde_json::json!({ "model": model, "messages": messages, "stream": false });

    let agent = ureq::Agent::config_builder()
        .timeout_global(timeout)
        .build()
        .new_agent();
    let mut req = agent.post(&endpoint);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let mut resp = req
        .send_json(&body)
        .map_err(|e| anyhow!(why_it_failed(&e, &endpoint, timeout)))?;
    let v: serde_json::Value = resp
        .body_mut()
        .read_json()
        .with_context(|| crate::i18n::t("err.bridge.bad_response_json"))?;
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .ok_or_else(|| {
            anyhow!(crate::i18n::tp(
                "err.bridge.no_content",
                &[("v", &v.to_string())]
            ))
        })?;
    Ok(strip_think(content).trim().to_string())
}

/// List the available models from an OpenAI-compatible `{base_url}/models`
/// endpoint. Returns the model ids. Works for DeepSeek, Ollama (/v1),
/// OpenRouter, etc. — the same providers `complete()` talks to.
///
/// Keeps its own short wait rather than the connection's. Asking what models
/// exist is a filing-cabinet question — no model loads, nothing thinks — so the
/// minutes a reply may be worth waiting for buy nothing here, and a dropdown
/// that hangs is a settings screen that looks broken.
pub fn list_models(base_url: &str, headers: &HashMap<String, String>) -> Result<Vec<String>> {
    let endpoint = models_endpoint(base_url);
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .build()
        .new_agent();
    let mut req = agent.get(&endpoint);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let mut resp = req.call().map_err(|e| {
        anyhow!(crate::i18n::tp(
            "err.bridge.connect_failed",
            &[("endpoint", &endpoint), ("e", &e.to_string())]
        ))
    })?;
    let v: serde_json::Value = resp
        .body_mut()
        .read_json()
        .with_context(|| crate::i18n::t("err.bridge.bad_response_json"))?;
    // OpenAI-compatible response: { "data": [ { "id": "..." }, ... ] }
    let mut out = Vec::new();
    if let Some(arr) = v.pointer("/data").and_then(|d| d.as_array()) {
        for m in arr {
            if let Some(id) = m.get("id").and_then(|i| i.as_str()) {
                out.push(id.to_string());
            }
        }
    }
    Ok(out)
}

/// Append `/models` to base_url (or use as-is if it already points at models).
fn models_endpoint(base: &str) -> String {
    let b = base.trim();
    if b.ends_with("/models") {
        return b.to_string();
    }
    // Tolerate a base that includes the chat path (swap it for /models).
    if let Some(prefix) = b.strip_suffix("/chat/completions") {
        return format!("{}/models", prefix.trim_end_matches('/'));
    }
    format!("{}/models", b.trim_end_matches('/'))
}

/// Append `/chat/completions` to base_url if needed.
/// If it's already a full path (e.g. Azure) or has a query string, use it
/// as-is.
fn chat_endpoint(base: &str) -> String {
    let b = base.trim();
    if b.contains("/chat/completions") || b.contains('?') {
        return b.to_string();
    }
    format!("{}/chat/completions", b.trim_end_matches('/'))
}

/// Strip the `<think>...</think>` block that reasoning models mix in (takes
/// everything after the last closing tag).
fn strip_think(s: &str) -> String {
    match s.rfind("</think>") {
        Some(i) => s[i + "</think>".len()..].to_string(),
        None => s.to_string(),
    }
}

/// Extract `SHIKISHA_SAY=<path>` from the tail of a discussion prompt (last
/// match; the path may contain spaces).
pub fn extract_say(s: &str) -> Option<String> {
    s.lines().rev().find_map(|l| {
        l.trim()
            .strip_prefix("SHIKISHA_SAY=")
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
    })
}

/// The model connections of the desk on screen, resolved (name -> address,
/// headers, wait).
///
/// Swapped whole on a desk switch and when the settings are read again. Each
/// desk registers its own, so a name here means this desk's connection by
/// that name and nothing else: the same `claude` in another desk may be
/// another account entirely
static PROVIDERS: Mutex<Option<HashMap<String, crate::config::ProviderConn>>> =
    Mutex::new(None);

/// Point it at a desk's connections (see [`crate::config::desk_providers`]).
pub fn use_desk(conns: HashMap<String, crate::config::ProviderConn>) {
    if let Ok(mut g) = PROVIDERS.lock() {
        *g = Some(conns);
    }
}

/// The connections of the desk on screen, in name order
pub fn reaching() -> Vec<String> {
    let Ok(g) = PROVIDERS.lock() else {
        return Vec::new();
    };
    let mut names: Vec<String> = g.as_ref().map(|m| m.keys().cloned().collect()).unwrap_or_default();
    names.sort();
    names
}

/// Whether this command line is a conversation with a model rather than a
/// program to run: `model <provider>/<model>`.
///
/// A tab of these runs nothing on this PC -- the turn is an exchange with a
/// server -- which is why it needs no working folder and starts no process
pub fn is_model_line(argv: &[String]) -> bool {
    argv.first().map(String::as_str) == Some("model")
}

/// Why a `model <provider>/<model>` line has no connection, in the words the
/// person reads. `None` when it is not such a line, or when it has one.
pub fn why_not(argv: &[String]) -> Option<String> {
    if !is_model_line(argv) {
        return None;
    }
    let line = argv.get(1).map(|s| s.trim()).unwrap_or_default();
    let Some((provider, _)) = line.split_once('/') else {
        return Some(crate::i18n::tp("err.model.bad_line", &[("line", line)]));
    };
    let known = PROVIDERS
        .lock()
        .ok()
        .is_some_and(|g| g.as_ref().is_some_and(|m| m.contains_key(provider)));
    (!known).then(|| crate::i18n::tp("err.model.unknown_provider", &[("name", provider)]))
}

/// If this is `model <provider>/<model>`, the connection it names on the desk
/// on screen (None if that desk has none by that name). The model name may
/// itself contain "/" (Ollama tags), so split on the first "/" only.
pub fn launch_for(argv: &[String]) -> Option<ModelConn> {
    let g = PROVIDERS.lock().ok()?;
    conn_in(g.as_ref()?, argv)
}

/// The same, asked of a given desk's connections -- for tabs parked in a desk
/// that is not on screen, which keep that desk's and nobody else's
pub fn conn_in(
    conns: &HashMap<String, crate::config::ProviderConn>,
    argv: &[String],
) -> Option<ModelConn> {
    if !is_model_line(argv) {
        return None;
    }
    let (provider, model) = argv.get(1)?.trim().split_once('/')?;
    let conn = conns.get(provider)?.clone();
    Some(ModelConn {
        provider: provider.to_string(),
        url: conn.url,
        model: model.to_string(),
        headers: conn.headers,
        timeout: conn.timeout,
        persona: None,
        drives: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A model line reaches only the connection its own desk registered by that
    /// name. The account behind a connection is billed for the work and handed
    /// the code, so another desk's is never within reach
    #[test]
    fn a_desk_uses_only_the_connections_it_registered() {
        let line = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let conn = |url: &str| crate::config::ProviderConn {
            url: url.to_string(),
            headers: HashMap::new(),
            timeout: None,
        };
        let work: HashMap<_, _> = [("claude".to_string(), conn("https://work.example/v1"))].into();
        let mine: HashMap<_, _> = [("mine".to_string(), conn("http://localhost:11434/v1"))].into();
        let argv = line(&["model", "claude/sonnet"]);
        assert_eq!(conn_in(&work, &argv).map(|c| c.url), Some("https://work.example/v1".to_string()));
        assert!(conn_in(&mine, &argv).is_none(), "another desk's destination can be used");
        assert!(conn_in(&HashMap::new(), &argv).is_none(), "a desk with nothing registered can connect");
    }

    /// A model line with no connection is refused in its own words, never by
    /// going on to look for a program called "model".
    #[test]
    fn a_model_line_without_a_connection_says_why() {
        let line = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(why_not(&line(&["claude"])), None, "it meddles with lines other than model");
        assert!(why_not(&line(&["model", "no-slash"])).is_some(), "a line of the wrong shape passes silently");
        assert!(why_not(&line(&["model"])).is_some(), "an empty line passes silently");
    }

    #[test]
    fn endpoint_appends_or_keeps() {
        assert_eq!(
            chat_endpoint("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            chat_endpoint("http://localhost:11434/v1/"),
            "http://localhost:11434/v1/chat/completions"
        );
        let full = "https://x.openai.azure.com/openai/deployments/g/chat/completions?api-version=2024";
        assert_eq!(chat_endpoint(full), full);
    }

    #[test]
    fn models_endpoint_builds() {
        assert_eq!(
            models_endpoint("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            models_endpoint("http://localhost:11434/v1/"),
            "http://localhost:11434/v1/models"
        );
        assert_eq!(models_endpoint("https://x/v1/models"), "https://x/v1/models");
        assert_eq!(
            models_endpoint("https://x/v1/chat/completions"),
            "https://x/v1/models"
        );
    }

    #[test]
    fn strip_think_keeps_answer() {
        assert_eq!(strip_think("<think>reasoning</think>Answer"), "Answer");
        assert_eq!(strip_think("no think here"), "no think here");
    }

    #[test]
    fn extract_say_finds_marker() {
        assert_eq!(
            extract_say("hello\nSHIKISHA_SAY=C:/a b/say.txt\n").as_deref(),
            Some("C:/a b/say.txt")
        );
        assert_eq!(extract_say("no marker"), None);
    }
}

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
    /// Which protocol the far end answers: `"chat"` or `"choice"`
    /// (see [`crate::config::ProviderSpec::speaks`])
    pub speaks: String,
}

/// Who answers a question asked from automation (`ai_choose`, `ai_text`)
#[derive(Debug, Clone)]
pub enum Answerer {
    /// A model reached over the network, on a connection the desk registered
    Api(ModelConn),
    /// An AI program installed on this PC (`claude`, `codex`, `gemini`),
    /// answering on the person's own subscription. `model` left out is the
    /// model that program uses by itself
    Installed { ai: String, model: Option<String> },
}

/// What marks an installed AI in a `<connection>/<model>` name: `@claude`,
/// `@claude/haiku`. A connection's own name cannot begin with it -- settings
/// hold those to letters, digits and `_ . -` -- so the two never collide
pub const INSTALLED_MARK: char = '@';

/// The installed AI a name means, and the model it names, when it names one:
/// `@claude` -> ("claude", None), `@claude/haiku` -> ("claude", Some("haiku")).
/// `None` for a connection's name, and for an AI this program does not know
pub fn installed_named(name: &str) -> Option<(String, Option<String>)> {
    let rest = name.trim().strip_prefix(INSTALLED_MARK)?;
    let (ai, model) = match rest.split_once('/') {
        Some((ai, model)) => (ai.trim(), Some(model.trim()).filter(|m| !m.is_empty())),
        None => (rest.trim(), None),
    };
    crate::webui::assistant_label(ai)?;
    Some((ai.to_string(), model.map(str::to_string)))
}

/// Who a name held in a setting reaches: an installed AI (`@claude/haiku`),
/// or a connection of the desk on screen (`deepseek/deepseek-chat`)
pub fn answerer_named(name: &str) -> Option<Answerer> {
    if name.trim().starts_with(INSTALLED_MARK) {
        let (ai, model) = installed_named(name)?;
        return Some(Answerer::Installed { ai, model });
    }
    conn_named(name).map(Answerer::Api)
}

/// How long an answer is waited for, as the asking itself waits: a
/// connection's own setting (`None` is as long as it takes), and an installed
/// AI's fixed limit
pub fn patience(who: &Answerer) -> Option<std::time::Duration> {
    match who {
        Answerer::Api(conn) => conn.timeout,
        Answerer::Installed { .. } => Some(crate::webui::INSTALLED_TIMEOUT),
    }
}

/// The assistant AI as a model name ("@claude"): what drives a page when
/// neither the page nor its desk chose a model. The one chosen under Basic ›
/// Assistant AI, or the first installed -- the one every other question the
/// app asks goes to. `None` when none is installed.
///
/// Held for a few seconds: the board asks on every frame it draws, and
/// finding an installed program means reading the settings and walking PATH
pub fn assistant_model() -> Option<String> {
    use std::time::{Duration, Instant};
    static HELD: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);
    const FOR: Duration = Duration::from_secs(5);
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, name)) = held.as_ref()
        && at.elapsed() < FOR
    {
        return name.clone();
    }
    let want = crate::config::load().and_then(|c| c.ai_engine).filter(|w| !w.trim().is_empty());
    let name = crate::webui::assistant_ai(want.as_deref()).map(|(ai, _)| format!("{INSTALLED_MARK}{ai}"));
    *held = Some((Instant::now(), name.clone()));
    name
}

/// A name as a person reads it: an installed AI by the name it is known by
/// (`Claude Code`, `Claude Code / haiku`), a connection as it is written
pub fn shown_name(name: &str) -> String {
    match installed_named(name) {
        Some((ai, model)) => {
            let label = crate::webui::assistant_label(&ai).unwrap_or_default();
            match model {
                Some(m) => format!("{label} / {m}"),
                None => label.to_string(),
            }
        }
        None => name.trim().to_string(),
    }
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

/// The agents, kept alive between calls, one per wait-length.
///
/// A new agent per request is a new TCP connection and a new TLS handshake
/// per request. Measured from here that is roughly 280ms paid before the far
/// end has read a single byte -- nothing at all when a turn is a person
/// typing, and more than the whole answer when the loop is driving a page.
/// Agents pool their connections, so keeping them is keeping the handshake
static AGENTS: Mutex<Option<HashMap<u64, ureq::Agent>>> = Mutex::new(None);

/// The pooled agent for this wait-length (0 = wait as long as it takes)
fn agent_for(timeout: Option<std::time::Duration>) -> ureq::Agent {
    let key = timeout.map(|t| t.as_secs().max(1)).unwrap_or(0);
    let mut g = AGENTS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    map.entry(key)
        .or_insert_with(|| {
            ureq::Agent::config_builder()
                .timeout_global(timeout)
                .build()
                .new_agent()
        })
        .clone()
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
    complete_shaped(base_url, model, headers, timeout, messages, None)
}

/// As `complete_messages`, with the shape of the answer demanded up front.
///
/// `shape` is a JSON Schema. Asking for a shape beats asking in words and
/// hoping: told to answer with an object, a model that feels chatty writes
/// "Sure! Here it is:" first and the parse fails on text nobody needed
pub fn complete_shaped(
    base_url: &str,
    model: &str,
    headers: &HashMap<String, String>,
    timeout: Option<std::time::Duration>,
    messages: &[serde_json::Value],
    shape: Option<&serde_json::Value>,
) -> Result<String> {
    let endpoint = chat_endpoint(base_url);
    let mut body = serde_json::json!({ "model": model, "messages": messages, "stream": false });
    if let Some(schema) = shape
        && let Some(map) = body.as_object_mut() {
            map.insert(
                "response_format".into(),
                serde_json::json!({
                    "type": "json_schema",
                    "json_schema": { "name": "answer", "strict": true, "schema": schema },
                }),
            );
        }

    let agent = agent_for(timeout);
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
/// Ask for a decision: here is the state, here are the answers allowed, which
/// one -- and how sure are you.
///
/// Two very different services answer this. One is built for it: it is handed
/// the allowed answers and returns one of them with a probability for each,
/// in a fraction of the time a sentence would take to write. The other is an
/// ordinary conversational model, told the same thing in words and held to
/// the same shape. **The answer comes back identical either way**, because
/// everything above this line is written once and must not care which is
/// installed -- that is what lets the whole feature work for somebody who has
/// only an ordinary AI, slower and otherwise the same.
///
/// `ask` is `{"state": …, "questions": {name: {type, criteria, instructions}}}`.
/// The answer is `{name: {"choice": …, "confidence": …, "probabilities": …}}`,
/// where a `score` question answers with `score` and a `noul` with `noul`
///
/// An AI installed on this PC answers the way an ordinary model does, asked
/// through its own program rather than over the network
pub fn choose(who: &Answerer, ask: &serde_json::Value) -> Result<serde_json::Value> {
    let questions = ask
        .get("questions")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow!(crate::i18n::t("err.choose.no_questions")))?;
    if questions.is_empty() {
        return Err(anyhow!(crate::i18n::t("err.choose.no_questions")));
    }
    let answers = match who {
        Answerer::Api(conn) if conn.speaks == crate::config::SPEAKS_CHOICE => choose_direct(conn, ask)?,
        _ => choose_by_words(who, ask, questions)?,
    };
    validate_answers(&answers, questions)?;
    Ok(answers)
}

/// Ask for words: `prompt` under `system`, held to `shape` (a JSON Schema)
/// when there is one. Whoever answers, what comes back is the answer's text
pub fn text(
    who: &Answerer,
    prompt: &str,
    system: Option<&str>,
    shape: Option<&serde_json::Value>,
) -> Result<String> {
    let system = system.filter(|s| !s.trim().is_empty());
    match who {
        Answerer::Api(conn) => {
            let mut messages = Vec::new();
            if let Some(s) = system {
                messages.push(serde_json::json!({ "role": "system", "content": s }));
            }
            messages.push(serde_json::json!({ "role": "user", "content": prompt }));
            complete_shaped(&conn.url, &conn.model, &conn.headers, conn.timeout, &messages, shape)
        }
        Answerer::Installed { ai, model } => {
            let schema = shape.map(serde_json::Value::to_string);
            let said = crate::webui::ask_installed(ai, model.as_deref(), prompt, system, schema.as_deref())?;
            Ok(strip_think(&said).trim().to_string())
        }
    }
}

/// The service built for choosing: state and questions in, typed answers out
fn choose_direct(conn: &ModelConn, ask: &serde_json::Value) -> Result<serde_json::Value> {
    // The address is used exactly as it was typed. What lives at which path
    // is the far end's business, and writing a guess here would make this
    // work for one company and silently fail for the next
    let endpoint = conn.url.trim().to_string();
    let mut body = ask.clone();
    if let Some(map) = body.as_object_mut() {
        map.insert("model".into(), serde_json::Value::String(conn.model.clone()));
    }
    let agent = agent_for(conn.timeout);
    let mut req = agent.post(&endpoint);
    for (k, v) in &conn.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let mut resp = req
        .send_json(&body)
        .map_err(|e| anyhow!(why_it_failed(&e, &endpoint, conn.timeout)))?;
    let v: serde_json::Value = resp
        .body_mut()
        .read_json()
        .with_context(|| crate::i18n::t("err.bridge.bad_response_json"))?;
    v.get("answers")
        .cloned()
        .ok_or_else(|| anyhow!(crate::i18n::tp("err.choose.no_answers", &[("v", &v.to_string())])))
}

/// An ordinary model, told the same thing in words and held to the same shape
fn choose_by_words(
    who: &Answerer,
    ask: &serde_json::Value,
    questions: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let mut props = serde_json::Map::new();
    for (name, q) in questions {
        let kind = q.get("type").and_then(serde_json::Value::as_str).unwrap_or("choice");
        let answer = match kind {
            "noul" => serde_json::json!({
                "type": "object",
                "properties": { "noul": { "type": "number", "minimum": 0, "maximum": 1 } },
                "required": ["noul"], "additionalProperties": false,
            }),
            // Both of the others pick one of a fixed set, so both are spelled
            // as an enum: a model that cannot name an answer outside the list
            // cannot invent one, which is the whole point of asking this way
            _ => {
                let field = if kind == "score" { "score" } else { "choice" };
                let mut fields = serde_json::Map::new();
                fields.insert(field.into(), serde_json::json!({ "enum": offered(q) }));
                fields.insert(
                    "confidence".into(),
                    serde_json::json!({ "type": "number", "minimum": 0, "maximum": 1 }),
                );
                serde_json::json!({
                    "type": "object",
                    "properties": fields,
                    "required": [field, "confidence"],
                    "additionalProperties": false,
                })
            }
        };
        props.insert(name.clone(), answer);
    }
    let required: Vec<String> = questions.keys().cloned().collect();
    let shape = serde_json::json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    });
    let reply = text(who, &ask.to_string(), Some(crate::asking::CHOOSING), Some(&shape))?;
    // A model that ignored the shape still tends to put the object inside
    // something. Take the outermost object rather than refusing outright
    let body = reply
        .find('{')
        .and_then(|i| reply.rfind('}').map(|j| &reply[i..=j]))
        .unwrap_or(reply.as_str());
    serde_json::from_str(body)
        .with_context(|| crate::i18n::tp("err.choose.unreadable", &[("reply", &reply)]))
}

/// The answers a question allows, as plain strings
fn offered(q: &serde_json::Value) -> Vec<String> {
    match q.get("criteria") {
        // choice: a map of option -> what it means
        Some(serde_json::Value::Object(m)) => m.keys().cloned().collect(),
        // score: the levels, in order, answered by their position
        Some(serde_json::Value::Array(a)) => (1..=a.len()).map(|i| i.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// Refuse an answer that is not one of the ones offered.
///
/// Whoever answered, the caller is about to act on this. An option the
/// question never listed is not a decision, it is a typo with consequences
fn validate_answers(
    answers: &serde_json::Value,
    questions: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    for (name, q) in questions {
        let kind = q.get("type").and_then(serde_json::Value::as_str).unwrap_or("choice");
        let Some(a) = answers.get(name) else {
            return Err(anyhow!(crate::i18n::tp("err.choose.missing", &[("name", name)])));
        };
        if kind == "noul" {
            let ok = a
                .get("noul")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|n| (0.0..=1.0).contains(&n));
            if !ok {
                return Err(anyhow!(crate::i18n::tp("err.choose.missing", &[("name", name)])));
            }
            continue;
        }
        let field = if kind == "score" { "score" } else { "choice" };
        let picked = a.get(field).and_then(serde_json::Value::as_str).unwrap_or("");
        let allowed = offered(q);
        if allowed.is_empty() || !allowed.iter().any(|o| o == picked) {
            return Err(anyhow!(crate::i18n::tp(
                "err.choose.not_offered",
                &[("name", name), ("picked", picked), ("allowed", &allowed.join(", "))]
            )));
        }
    }
    Ok(())
}

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

/// The connection a `<connection>/<model>` name reaches on the desk on
/// screen, for the places that hold such a name in a setting rather than on
/// a tab's command line
pub fn conn_named(name: &str) -> Option<ModelConn> {
    let argv = vec!["model".to_string(), name.trim().to_string()];
    launch_for(&argv)
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
        speaks: conn.speaks,
        persona: None,
        drives: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn questions() -> serde_json::Map<String, serde_json::Value> {
        serde_json::json!({
            "operation": {
                "type": "choice",
                "criteria": { "CLICK": "press something", "DONE": "it is finished" },
            },
            "how_well": { "type": "score", "criteria": ["bad", "fine", "good"] },
            "is_signed_in": { "type": "noul" },
        })
        .as_object()
        .cloned()
        .unwrap()
    }

    /// Each AI installed on this PC answers a decision, and words, the way a
    /// conversational model does. Asks the real programs, on the account
    /// signed in here, so it is run by hand:
    ///     cargo test -p shikisha-core installed_ais_answer -- --ignored --nocapture
    #[test]
    #[ignore]
    fn installed_ais_answer() {
        let ask = serde_json::json!({
            "state": {"page": "A shop. The basket holds one book. There is a Checkout button."},
            "questions": {"operation": {"type": "choice", "criteria": {
                "CLICK_CHECKOUT": "press the Checkout button", "DONE": "the goal is met"},
                "instructions": "Goal: buy what is in the basket"}},
        });
        for name in ["@claude/haiku", "@codex", "@gemini"] {
            if crate::webui::assistant_ai(installed_named(name).map(|(a, _)| a).as_deref()).is_none() {
                continue;
            }
            let who = answerer_named(name).expect("an installed AI");
            let t = std::time::Instant::now();
            let answers = choose(&who, &ask).unwrap_or_else(|e| panic!("{name}: {e:#}"));
            println!("{name} chose {answers} in {:?}", t.elapsed());
            let t = std::time::Instant::now();
            let said = text(&who, "Write the word 'apple' and nothing else.", None, None)
                .unwrap_or_else(|e| panic!("{name}: {e:#}"));
            println!("{name} wrote {said:?} in {:?}", t.elapsed());
            assert!(said.to_lowercase().contains("apple"), "{name}: {said}");
        }
    }

    /// An AI installed on this PC is named with the mark in front, and a
    /// connection never is: the two spellings cannot be taken for each other
    #[test]
    fn an_installed_ai_is_told_from_a_connection_by_its_mark() {
        assert_eq!(installed_named("@claude"), Some(("claude".into(), None)));
        assert_eq!(installed_named(" @codex/gpt-5-codex "), Some(("codex".into(), Some("gpt-5-codex".into()))));
        assert_eq!(installed_named("@gemini/"), Some(("gemini".into(), None)), "a slash with nothing after it names no model");
        assert_eq!(installed_named("claude/haiku"), None, "a connection called claude is a connection");
        assert_eq!(installed_named("@aider"), None, "an AI this program cannot ask is not an installed AI");
        assert!(answerer_named("@nobody").is_none(), "an unknown one is refused, not taken for a connection");
        assert!(matches!(
            answerer_named("@claude/haiku"),
            Some(Answerer::Installed { ref ai, model: Some(ref m) }) if ai == "claude" && m == "haiku"
        ));
        assert_eq!(shown_name("@claude/haiku"), "Claude Code / haiku");
        assert_eq!(shown_name("@codex"), "Codex CLI");
        assert_eq!(shown_name("deepseek/deepseek-chat"), "deepseek/deepseek-chat");
    }

    /// An answer is about to be acted on. One that names something the
    /// question never offered is not a decision -- it is a typo with
    /// consequences, and it is refused whoever produced it
    #[test]
    fn an_answer_outside_what_was_offered_is_refused() {
        let q = questions();
        let good = serde_json::json!({
            "operation": {"choice": "CLICK", "confidence": 0.9},
            "how_well": {"score": "2", "confidence": 0.5},
            "is_signed_in": {"noul": 0.8},
        });
        assert!(validate_answers(&good, &q).is_ok());

        let invented = serde_json::json!({
            "operation": {"choice": "LOG_IN", "confidence": 1.0},
            "how_well": {"score": "2", "confidence": 0.5},
            "is_signed_in": {"noul": 0.8},
        });
        let why = validate_answers(&invented, &q).unwrap_err().to_string();
        assert!(why.contains("LOG_IN"), "the refusal names what was answered: {why}");

        // A score is answered by which level, so a level that does not exist
        // is the same mistake wearing a number
        let past_the_end = serde_json::json!({
            "operation": {"choice": "DONE", "confidence": 1.0},
            "how_well": {"score": "4", "confidence": 0.5},
            "is_signed_in": {"noul": 0.8},
        });
        assert!(validate_answers(&past_the_end, &q).is_err());

        // Nothing at all for a question is not an answer either
        let short = serde_json::json!({ "operation": {"choice": "DONE", "confidence": 1.0} });
        assert!(validate_answers(&short, &q).is_err());

        // A yes/no outside 0..1 is not a probability
        let impossible = serde_json::json!({
            "operation": {"choice": "DONE", "confidence": 1.0},
            "how_well": {"score": "1", "confidence": 0.5},
            "is_signed_in": {"noul": 4.0},
        });
        assert!(validate_answers(&impossible, &q).is_err());
    }

    /// The decision, against a service that really answers it.
    ///
    /// Not run by default: it needs an endpoint, a key and a model, which are
    /// the person's own. What it proves is the part no offline test can --
    /// that what goes out is what the far end expects, and that what comes
    /// back fits through `validate_answers` without being coaxed.
    ///
    ///   SHIKISHA_PROBE_URL=... SHIKISHA_PROBE_KEY=... SHIKISHA_PROBE_MODEL=... \
    ///   SHIKISHA_PROBE_SPEAKS=choice \
    ///   cargo test -p shikisha-core choosing_for_real -- --ignored --nocapture
    #[test]
    #[ignore]
    fn choosing_for_real() {
        let var = |k: &str| std::env::var(k).unwrap_or_default();
        let url = var("SHIKISHA_PROBE_URL");
        assert!(!url.is_empty(), "SHIKISHA_PROBE_URL is what this needs");
        let mut headers = HashMap::new();
        let key = var("SHIKISHA_PROBE_KEY");
        if !key.is_empty() {
            headers.insert("Authorization".to_string(), format!("Bearer {key}"));
        }
        let conn = ModelConn {
            provider: "probe".into(),
            url,
            model: var("SHIKISHA_PROBE_MODEL"),
            headers,
            timeout: Some(std::time::Duration::from_secs(30)),
            persona: None,
            drives: None,
            speaks: match var("SHIKISHA_PROBE_SPEAKS").as_str() {
                "choice" => crate::config::SPEAKS_CHOICE.to_string(),
                _ => crate::config::SPEAKS_CHAT.to_string(),
            },
        };
        // A page with one empty box and one button, and a goal that can only
        // be met by typing first. Small enough to read, and the right answer
        // is not a matter of taste
        let ask = serde_json::json!({
            "state": {
                "page": { "text": "Search\nQuery\nSearch" },
                "elements": [
                    { "ref": 1, "role": "textbox", "name": "Query", "value": "", "can": ["fill", "click"] },
                    { "ref": 2, "role": "button", "name": "Search", "can": ["click"] },
                ],
                "done": [],
            },
            "questions": {
                "operation": {
                    "type": "choice",
                    "criteria": {
                        "CLICK": "Click an element.",
                        "TYPE": "Put text into a field.",
                        "DONE": "The goal is visibly satisfied.",
                    },
                    "instructions": { "goal": "Search for cats", "rules": "Advance the goal from the page as it is." },
                },
                "type_target": {
                    "type": "choice",
                    "criteria": { "1": { "element": "[1] textbox Query", "holds": "" } },
                    "instructions": { "goal": "Search for cats", "operation": "TYPE", "rules": "Pick the element." },
                },
            },
        });
        let began = std::time::Instant::now();
        // `choose` refuses an answer that was not one of the ones offered, so
        // getting here at all is the thing being proved: what went out was
        // what the far end expects, and what came back fits
        let answers = choose(&Answerer::Api(conn), &ask).expect("the decision came back unusable");
        let picked = answers["operation"]["choice"].as_str().unwrap_or_default();
        println!("{}ms -> {answers}", began.elapsed().as_millis());
        assert_eq!(answers["type_target"]["choice"].as_str(), Some("1"));
        // Whether it picked *well* is the model's business, not this code's --
        // a small model gets this wrong some of the time and is still being
        // driven correctly. Said out loud rather than asserted, so the probe
        // reports the quality of the model without failing over it
        match picked {
            "TYPE" => println!("picked the sensible move (the box is empty)"),
            other => println!("picked {other} — this model is not reliable for deciding"),
        }
    }

    /// What each kind of question allows, which is what an ordinary model is
    /// held to and what every answer is checked against. Both readings come
    /// from the same function, so the two can never drift apart
    #[test]
    fn the_answers_a_question_allows_are_read_the_same_way_everywhere() {
        let q = questions();
        assert_eq!(
            {
                let mut v = offered(&q["operation"]);
                v.sort();
                v
            },
            vec!["CLICK".to_string(), "DONE".to_string()]
        );
        // A score is answered by position, so the levels become 1..n
        assert_eq!(offered(&q["how_well"]), vec!["1", "2", "3"]);
        // A yes/no offers nothing to pick from; its answer is a number
        assert!(offered(&q["is_signed_in"]).is_empty());
    }

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
            speaks: crate::config::SPEAKS_CHAT.to_string(),
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

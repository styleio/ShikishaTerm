//! config/config.json: defines the workspace / tab layout. See DESIGN.md chapter 7.4.
//! Looked up in the config folder beside the exe, then the config folder in the
//! current directory.
//!
//! Terminology: a "workspace" is the unit you switch between (like a virtual
//! desktop). Its externalized contents form a "workspace definition file".
//!
//! Settings are split into 3 kinds by role (everything the user owns lives
//! under the config folder):
//!   config/config.json  ... global settings + workspace list (rarely changed)
//!   workspaces/*.json   ... workspace definition files (copyable/shareable units)
//!   config/secrets.json ... credentials (can be encrypted, never share)

use anyhow::{Context as _, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    /// List of workspaces (projects). Switched between like virtual desktops
    #[serde(default)]
    pub workspaces: Vec<WorkspaceSpec>,
    /// Backward compat: tabs written directly when workspaces are not used
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
    /// Global automation shared by everything (e.g. "scripts/common" or "scripts/hooks.lua")
    #[serde(default)]
    pub automation: Option<String>,
    #[serde(default)]
    pub lua: Option<String>,
    /// Max depth of the auto-forward chain (default 10).
    /// Incremented by 1 each time it auto-forwards between tabs; reset to 0 by manual human input
    #[serde(default)]
    pub max_chain: Option<u32>,
    /// Whether automation may switch which tab is on screen (default: yes).
    ///
    /// Only `shikisha.show()` ever moves the view; handing work to a tab does not.
    /// This is the person's answer to that request — see main::ViewMove
    pub auto_switch: Option<bool>,
    /// Whether to start from the last-opened workspace (default: yes)
    pub restore_workspace: Option<bool>,
    /// Whether to overlay the browser on the terminal (default: overlay).
    /// Turning it off makes it a standalone window you can move yourself, but it no longer feels like a tab
    pub browser_overlay: Option<bool>,
    /// Wait time (ms) before a response is considered finished.
    /// If the profile specifies its own value, that takes priority
    pub done_confirm_ms: Option<u64>,
    /// Width (columns) of the left tab bar. Auto-sized to fit tab names when omitted.
    /// Can also be changed at runtime by dragging the divider
    #[serde(default)]
    pub tab_bar_width: Option<u16>,
    /// Registered notification destinations (Lua can only send to destinations registered here).
    /// Recommended to keep tokens separated out in secrets.json (gitignored)
    #[serde(default)]
    pub notify: std::collections::HashMap<String, crate::notify::Destination>,
    /// The destination `shikisha.notify(text)` reaches when none is named —
    /// like the default assistant AI, but for notifications. Unset with
    /// exactly one destination configured, that one serves as the primary
    #[serde(default)]
    pub primary_notify: Option<String>,
    /// Load secrets such as notification destinations from a separate file (e.g. "secrets.json")
    #[serde(default)]
    pub secrets: Option<String>,
    /// The AI that writes automation code ("claude" / "codex" / "gemini").
    /// Uses whichever is found if empty
    #[serde(default)]
    pub ai_engine: Option<String>,
    /// Capabilities granted to automation (file/HTTP). Default is empty = nothing allowed.
    /// Not editable from the GUI since this is an advanced feature
    #[serde(default)]
    pub capabilities: crate::caps::CapabilitySpec,
    /// Remote UI viewable from a phone etc. Disabled by default
    #[serde(default)]
    pub remote: RemoteSpec,
    /// Bounds for files pasted/attached into the sub-input bar (saved beside the tab)
    #[serde(default)]
    pub attach: AttachSpec,
    /// Quick actions shown in the sub-input bar. Each inserts text (beginner) or,
    /// when `lua` is set, runs Lua (advanced).
    #[serde(default)]
    pub actions: Vec<ActionSpec>,
    /// Bounds and stall behavior for ad-hoc "operate a tab" (🎯) sessions.
    #[serde(default)]
    pub operate: OperateSpec,
    /// Display language ("ja" etc). Follows the OS setting if omitted
    #[serde(default)]
    pub language: Option<String>,
    /// Where the browser (WebView2) stores its data. Holds cache and login state.
    ///   "local" (default) ... each PC's %LOCALAPPDATA% (not Drive-synced, lightweight)
    ///   "portable"         ... data\webview2 beside the app (shared across PCs via Drive, logins shared too)
    ///   anything else      ... used as an absolute path
    #[serde(default)]
    pub browser_data: Option<String>,
    /// Auxiliary key row for the relay screen (phone remote control). Listed left to right.
    /// Usable names: esc tab space enter backspace delete
    ///   left up down right home end pageup pagedown
    ///   f1-f12 ctrl alt (ctrl/alt are fixed toggles).
    /// Uses cast_keys_default() when omitted
    #[serde(default)]
    pub cast_keys: Option<Vec<String>>,
    /// Connection info for the model bridge (OpenAI-compatible API). name -> {base_url, api_key, headers}.
    /// The bridge that lets discussions and browser operation use cheap/local models (DeepSeek/Qwen/Ollama etc).
    /// A `model <name>/<model>` tab looks this up when it launches
    #[serde(default)]
    pub providers: std::collections::HashMap<String, ProviderSpec>,
}

/// Connection info for an OpenAI-compatible API (DeepSeek cloud / Ollama local / OpenRouter / Azure etc).
/// Two orthogonal axes -- "connection (base_url + auth)" x "model name" -- let cloud/local and model kind vary independently
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProviderSpec {
    /// OpenAI-compatible base URL (e.g. https://api.deepseek.com/v1, http://localhost:11434/v1).
    /// Full paths/queries such as Azure's are used as-is
    #[serde(default)]
    pub base_url: String,
    /// Auth key. "@name" refers to a token in secrets.json's tokens (a literal value also works).
    /// Becomes an Authorization: Bearer <resolved value> header when headers is not specified.
    /// Can be omitted where not needed, e.g. local (Ollama)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Explicit outgoing headers (for Azure's `api-key` or a custom gateway). Values also support
    /// "@name" secrets references. When given, the default api_key Bearer header is not sent -- these are sent instead
    #[serde(default)]
    pub headers: std::collections::HashMap<String, String>,
}

/// Default order of the auxiliary key row. Frequently used Enter/Space/Backspace and the
/// arrow keys come first; F1-F12 and Ctrl/Alt come later (reachable by scrolling sideways).
/// Users can freely override this via cast_keys in config
pub fn cast_keys_default() -> Vec<String> {
    [
        "esc", "tab", "left", "up", "down", "right", "space", "enter", "backspace", "ctrl", "alt",
        "home", "end", "pageup", "pagedown", "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9",
        "f10", "f11", "f12",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Get the auxiliary key row from config (default if unset). Passed to the relay screen client
pub fn cast_keys() -> Vec<String> {
    load()
        .and_then(|c| c.cast_keys)
        .filter(|v| !v.is_empty())
        .unwrap_or_else(cast_keys_default)
}

/// Decide where WebView2 stores its data, based on config. To avoid Drive cache churn
/// and EBWebView sync notifications, the default is the non-synced local folder (%LOCALAPPDATA%)
pub fn browser_data_dir() -> std::path::PathBuf {
    let mode = load()
        .and_then(|c| c.browser_data)
        .unwrap_or_default();
    match mode.trim() {
        "portable" => root_dir().join("data").join("webview2"),
        "" | "local" => local_appdata().join("ShikishaTerm").join("webview2"),
        other => std::path::PathBuf::from(other),
    }
}

fn local_appdata() -> std::path::PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root_dir().join("data"))
}

/// Remote UI settings. Off by default since this lets AI be operated from afar
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteSpec {
    #[serde(default)]
    pub enabled: bool,
    /// "auto" (tries Tailscale, then LAN) / "127.0.0.1" / an explicit IP
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_remote_port")]
    pub port: u16,
    /// Explicitly allow exposing this outside the private network
    #[serde(default)]
    pub allow_public: bool,
    /// Optional second factor on top of the URL token. Empty = off (the
    /// token alone opens the board — the user's own risk to accept). Set,
    /// the phone must enter it once per app run; notification URLs then
    /// carry only the token, never this
    #[serde(default)]
    pub password: String,
    /// Keep the pairing on the phone: the token stays in the URL (bookmarkable)
    /// and in persistent storage, so a discarded tab or a closed browser does
    /// not cost a QR scan. The "disconnect" control then only drops live
    /// connections and password sessions — a new token is an explicit
    /// re-issue from settings. Off (default) = the token lives only in the
    /// tab's session storage and every disconnect rotates it
    #[serde(default)]
    pub sticky_token: bool,
    /// The token itself when sticky: written by the person (or generated
    /// into the settings field for them). Plain text in config.json — the
    /// trade they accepted. Used only when `sticky_token` is on and it is at
    /// least 16 characters; otherwise the usual persisted random token
    #[serde(default)]
    pub fixed_token: String,
}

impl Default for RemoteSpec {
    fn default() -> Self {
        Self {
            enabled: false,
            bind: default_bind(),
            port: default_remote_port(),
            allow_public: false,
            password: String::new(),
            sticky_token: false,
            fixed_token: String::new(),
        }
    }
}

/// Bounds for a pasted/attached file. Nothing here executes the file (it is saved
/// beside the tab and only its path is handed to the AI), so `extensions` is a UX
/// guard and a nudge to be deliberate — power users can widen it at their own risk.
#[derive(Debug, Clone, Deserialize)]
pub struct AttachSpec {
    /// Max size of one attachment, in megabytes.
    #[serde(default = "default_attach_mb")]
    pub max_mb: u32,
    /// Extensions the user opted into (lowercase, no dot).
    #[serde(default = "default_attach_ext")]
    pub extensions: Vec<String>,
}

impl Default for AttachSpec {
    fn default() -> Self {
        Self {
            max_mb: default_attach_mb(),
            extensions: default_attach_ext(),
        }
    }
}

fn default_attach_mb() -> u32 {
    25
}

/// A one-click action in the sub-input bar. `body` is text to insert into the
/// composer (the beginner default) or, when `lua` is true, Lua run in the scoped
/// sandbox (advanced). Advanced is per-action, so a list can mix both freely.
#[derive(Debug, Clone, Deserialize)]
pub struct ActionSpec {
    /// Button label shown in the bar.
    pub label: String,
    /// Text to insert, or Lua source when `lua` is set.
    #[serde(default)]
    pub body: String,
    /// Advanced: `body` is Lua run in the sandbox, not text to insert.
    #[serde(default)]
    pub lua: bool,
}

/// The quick actions for the sub-input bar (empty if none configured).
pub fn actions() -> Vec<ActionSpec> {
    load().map(|c| c.actions).unwrap_or_default()
}

/// Bounds and stall behavior for an "operate a tab" (🎯) session. The limits are
/// a runaway safety net; `on_limit` decides what happens when one is reached.
/// Every limit accepts 0 to mean "no limit". These also feed a configured
/// browser Agent tab and browser rallies (same built-in orchestrator).
#[derive(Debug, Clone, Deserialize)]
pub struct OperateSpec {
    /// Operator turns before the safety net trips. 0 = unlimited.
    #[serde(default = "default_operate_rounds")]
    pub max_rounds: u32,
    /// Wall-clock seconds before the safety net trips. 0 = unlimited.
    #[serde(default = "default_operate_seconds")]
    pub max_seconds: u32,
    /// Operator output characters (a rough token proxy) before the net trips. 0 = unlimited.
    #[serde(default = "default_operate_tokens")]
    pub max_tokens: u32,
    /// What to do when a limit is reached:
    ///   "stop"     ... halt and tell the operator (default; the safe textbook choice)
    ///   "continue" ... reset the budget and keep going, trusting the operator to
    ///                  judge DONE itself (never stop on the user mid-task)
    #[serde(default = "default_operate_on_limit")]
    pub on_limit: String,
    /// After each browser action, how long to wait for the page to settle (its
    /// text to stop changing) before reading it back, in milliseconds. Guards
    /// against reading a half-rendered page. 0 disables the wait.
    #[serde(default = "default_operate_settle_ms")]
    pub settle_ms: u32,
    /// A brake before the operator acts, so a person can hold a risky step:
    ///   "off"   ... run every action immediately (default)
    ///   "sends" ... pause for approval only before a submit/click/auth step
    ///   "all"   ... pause for approval before every action
    /// Approval is a button shown on the target page; declining holds the run.
    #[serde(default = "default_operate_confirm")]
    pub confirm: String,
}

impl Default for OperateSpec {
    fn default() -> Self {
        Self {
            max_rounds: default_operate_rounds(),
            max_seconds: default_operate_seconds(),
            max_tokens: default_operate_tokens(),
            on_limit: default_operate_on_limit(),
            settle_ms: default_operate_settle_ms(),
            confirm: default_operate_confirm(),
        }
    }
}

fn default_operate_rounds() -> u32 {
    40
}
fn default_operate_seconds() -> u32 {
    900
}
fn default_operate_tokens() -> u32 {
    400_000
}
fn default_operate_on_limit() -> String {
    "stop".into()
}
fn default_operate_settle_ms() -> u32 {
    1800
}
fn default_operate_confirm() -> String {
    "off".into()
}

/// The operate limits/policy (defaults if none configured).
pub fn operate() -> OperateSpec {
    load().map(|c| c.operate).unwrap_or_default()
}

fn default_attach_ext() -> Vec<String> {
    ["jpg", "jpeg", "png", "gif", "webp", "pdf"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

fn default_bind() -> String {
    "auto".into()
}
fn default_remote_port() -> u16 {
    8787
}

/// secrets.json: a file holding only credentials, kept separate (never share)
#[derive(Debug, Deserialize, Default)]
pub struct Secrets {
    #[serde(default)]
    pub notify: std::collections::HashMap<String, crate::notify::Destination>,
    /// Auth info used by the HTTP gateway (not readable from scripts)
    #[serde(default)]
    pub tokens: std::collections::HashMap<String, String>,
    /// Description of each token (shown in the GUI list; the value itself never is)
    #[serde(default)]
    pub descriptions: std::collections::HashMap<String, String>,
    /// Remote UI token. Setting this pins the URL and avoids needing to re-pair
    #[serde(default)]
    pub remote_token: Option<String>,
}

impl Config {
    /// Merge the secrets file's contents into config.json's notify
    /// (secrets wins on name collision). secrets may be encrypted
    pub fn resolve_notify(
        &self,
        password: Option<&str>,
    ) -> (
        std::collections::HashMap<String, crate::notify::Destination>,
        Option<String>,
    ) {
        let mut map = self.notify.clone();
        let mut tokens: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut err = None;
        if let Some(path) = self.secrets_path().filter(|p| p.exists()) {
            match crate::crypto::read_maybe_encrypted(&path, password).and_then(|t| {
                serde_json::from_str::<Secrets>(&t)
                    .with_context(|| crate::i18n::t("err.config.secrets_json_invalid"))
            }) {
                Ok(s) => {
                    map.extend(s.notify);
                    tokens = s.tokens;
                }
                Err(e) => err = Some(format!("secrets: {e:#}")),
            }
        }
        // A "@name" webhook/token is expanded from the tokens store, so the
        // sensitive value stays encrypted in secrets.json rather than sitting in
        // config.json (same convention as a provider's api_key).
        let deref = |v: &str| -> String {
            match v.strip_prefix('@') {
                Some(k) => tokens.get(k).cloned().unwrap_or_default(),
                None => v.to_string(),
            }
        };
        for d in map.values_mut() {
            match d {
                crate::notify::Destination::Slack { webhook } => *webhook = deref(webhook),
                crate::notify::Destination::Telegram { token, chat_id } => {
                    *token = deref(token);
                    *chat_id = deref(chat_id);
                }
            }
        }
        (map, err)
    }

    /// Path to the secrets file. A relative path is resolved next to config.json.
    ///
    /// Without an explicit setting, this defaults to config/secrets.json. Otherwise
    /// the app would be unable to read secrets the settings GUI created in the
    /// default location (registered, but unusable). The reader treats a missing
    /// file as empty
    pub fn secrets_path(&self) -> Option<std::path::PathBuf> {
        let p = self.secrets.as_deref().unwrap_or("secrets.json");
        if std::path::Path::new(p).is_absolute() {
            return Some(std::path::PathBuf::from(p));
        }
        let mut c = config_file_path();
        c.set_file_name(p);
        Some(c)
    }

    /// Remote UI token (used if present in secrets)
    pub fn remote_token(&self, password: Option<&str>) -> Option<String> {
        let path = self.secrets_path()?;
        crate::crypto::read_maybe_encrypted(&path, password)
            .ok()
            .and_then(|t| serde_json::from_str::<Secrets>(&t).ok())
            .and_then(|s| s.remote_token)
            .filter(|t| t.len() >= 16)
    }

    /// Retrieve the auth info used by the HTTP gateway (never passed to scripts)
    pub fn resolve_tokens(
        &self,
        password: Option<&str>,
    ) -> std::collections::HashMap<String, String> {
        let Some(path) = self.secrets_path() else {
            return Default::default();
        };
        crate::crypto::read_maybe_encrypted(&path, password)
            .ok()
            .and_then(|t| serde_json::from_str::<Secrets>(&t).ok())
            .map(|s| s.tokens)
            .unwrap_or_default()
    }

    /// Resolve connection info from a provider name. Returns (base_url, outgoing headers).
    /// An "@name" inside a value is expanded from secrets.json's tokens.
    /// If headers is unset and api_key is present, builds an Authorization: Bearer header.
    /// This is passed into the model bridge child process's env (key decryption happens only here, in the parent)
    pub fn resolve_provider(
        &self,
        name: &str,
        password: Option<&str>,
    ) -> Option<(String, std::collections::HashMap<String, String>)> {
        let p = self.providers.get(name)?;
        if p.base_url.trim().is_empty() {
            return None;
        }
        let tokens = self.resolve_tokens(password);
        let deref = |v: &str| -> String {
            match v.strip_prefix('@') {
                Some(k) => tokens.get(k).cloned().unwrap_or_default(),
                None => v.to_string(),
            }
        };
        let mut headers = std::collections::HashMap::new();
        if !p.headers.is_empty() {
            for (k, v) in &p.headers {
                headers.insert(k.clone(), deref(v));
            }
        } else if let Some(key) = p.api_key.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            headers.insert("Authorization".into(), format!("Bearer {}", deref(key)));
        }
        Some((p.base_url.trim().to_string(), headers))
    }
}

// -- Secrets store (equivalent to GitHub Secrets) ---------------------------
// Referenced by key name; the value itself is never returned. Writing encrypts
// it if a password is set. Reads/writes the whole JSON so other entries such
// as notify and remote_token aren't clobbered

/// Read the secrets file as JSON (empty if missing)
fn read_secrets_value(
    path: &std::path::Path,
    password: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let text = crate::crypto::read_maybe_encrypted(path, password)?;
    Ok(serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({})))
}

/// Write the secrets file back (encrypted if a password is set)
fn write_secrets_value(
    path: &std::path::Path,
    password: Option<&str>,
    root: &serde_json::Value,
) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(root)?;
    match password {
        Some(pw) if !pw.is_empty() => {
            let env = crate::crypto::encrypt(&json, pw)?;
            crate::crypto::write_atomic(path, &serde_json::to_string_pretty(&env)?)
        }
        _ => crate::crypto::write_atomic(path, &json),
    }
}

/// Whether this is a valid key name (alphanumeric and _ - . only). Rejects substitution tricks and odd characters
pub fn valid_secret_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// List of secrets (keys and descriptions only). **Values are never returned**
pub fn list_secrets(
    path: &std::path::Path,
    password: Option<&str>,
) -> anyhow::Result<Vec<(String, String)>> {
    let root = read_secrets_value(path, password)?;
    let descs = root.get("descriptions").and_then(|v| v.as_object());
    let mut out: Vec<(String, String)> = root
        .get("tokens")
        .and_then(|v| v.as_object())
        .map(|t| {
            t.keys()
                .map(|k| {
                    let d = descs
                        .and_then(|d| d.get(k))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    (k.clone(), d)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Add/update a secret (write-only; once saved, the value can't be read back)
pub fn upsert_secret(
    path: &std::path::Path,
    password: Option<&str>,
    key: &str,
    desc: &str,
    value: &str,
) -> anyhow::Result<()> {
    if !valid_secret_key(key) {
        anyhow::bail!(crate::i18n::t("err.config.invalid_key_chars"));
    }
    let mut root = read_secrets_value(path, password)?;
    if !root.get("tokens").map(|v| v.is_object()).unwrap_or(false) {
        root["tokens"] = serde_json::json!({});
    }
    root["tokens"][key] = serde_json::json!(value);
    if !root
        .get("descriptions")
        .map(|v| v.is_object())
        .unwrap_or(false)
    {
        root["descriptions"] = serde_json::json!({});
    }
    root["descriptions"][key] = serde_json::json!(desc);
    write_secrets_value(path, password, &root)
}

/// Delete a secret
pub fn delete_secret(
    path: &std::path::Path,
    password: Option<&str>,
    key: &str,
) -> anyhow::Result<()> {
    let mut root = read_secrets_value(path, password)?;
    if let Some(t) = root.get_mut("tokens").and_then(|v| v.as_object_mut()) {
        t.remove(key);
    }
    if let Some(d) = root.get_mut("descriptions").and_then(|v| v.as_object_mut()) {
        d.remove(key);
    }
    write_secrets_value(path, password, &root)
}

/// A workspace entry inside config.json. Either inline tabs or a reference to a definition file
#[derive(Debug, Deserialize)]
pub struct WorkspaceSpec {
    pub name: String,
    /// Reference to a workspace definition file (e.g. "workspaces/projectx.json")
    #[serde(default)]
    pub file: Option<String>,
    /// Inline definition
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
    /// Automation shared across this workspace (used when a tab doesn't specify its own)
    #[serde(default)]
    pub automation: Option<String>,
    /// Browsers opened alongside this workspace. Referred to by id from automation
    #[serde(default)]
    pub browsers: Vec<BrowserConfig>,
    #[serde(default)]
    pub lua: Option<String>,
    /// Secret keys this workspace's rally is allowed to use (default is empty = deny all)
    #[serde(default)]
    pub secrets_allow: Vec<String>,
    /// Allow all secrets, knowingly accepting the risk
    #[serde(default)]
    pub secrets_allow_all: bool,
    /// Stop conditions (the referee). Read by built-in controllers such as browser-operation mode
    #[serde(default)]
    pub stops: Vec<StopCond>,
    /// AI-vs-AI discussion settings (when present, the built-in discussion orchestrator is put into each AI tab)
    #[serde(default)]
    pub discuss: Option<DiscussSpec>,
}

/// Contents of a workspace definition file (workspaces/*.json)
#[derive(Debug, Deserialize)]
pub struct WorkspaceFile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub tabs: Vec<TabConfig>,
    /// Automation shared across this workspace
    #[serde(default)]
    pub automation: Option<String>,
    #[serde(default)]
    pub lua: Option<String>,
    #[serde(default)]
    pub secrets_allow: Vec<String>,
    #[serde(default)]
    pub secrets_allow_all: bool,
    #[serde(default)]
    pub stops: Vec<StopCond>,
    #[serde(default)]
    pub discuss: Option<DiscussSpec>,
}

/// AI-vs-AI (N-party) discussion settings. Per workspace. Read by the built-in discussion orchestrator.
/// Participants (agents) are listed in turn order. Cycled round-robin; once max_rounds is reached, the judge (if any) rules
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DiscussSpec {
    /// ids of the participating AI tabs (in turn order)
    #[serde(default)]
    pub agents: Vec<String>,
    /// How turns are cycled. Currently only "round-robin"
    #[serde(default = "default_order")]
    pub order: String,
    /// Max number of rounds each participant speaks (once exceeded, goes to the judge/ends)
    #[serde(default = "default_rounds")]
    pub max_rounds: u32,
    /// Tab id of the judge (referee). If omitted, hitting the round limit just folds up as "discussion ended"
    #[serde(default)]
    pub judge: Option<String>,
    /// How the judge renders its verdict: "winner" / "synthesis". Default is winner
    #[serde(default = "default_verdict")]
    pub verdict: String,
    /// Tab id of the moderator. When order="moderated", nominates the next speaker
    #[serde(default)]
    pub moderator: Option<String>,
    /// Each tab's stance/persona (tab id -> persona text).
    /// e.g. {"safety":"You are a safety-first faction...", ...}.
    /// Told to that AI at the start. Empty means a plain (neutral) AI
    #[serde(default)]
    pub personas: std::collections::HashMap<String, String>,
}

fn default_order() -> String {
    "round-robin".into()
}
fn default_rounds() -> u32 {
    6
}
fn default_verdict() -> String {
    "winner".into()
}

/// Stop conditions (the referee). Held per workspace. Evaluated top to bottom; the first match wins.
/// Defines "when this collaboration ends (success/failure)". Can span multiple participants (tabs)
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct StopCond {
    /// Kind of monitoring: screen|css|xpath|console|rounds|time|tokens
    pub when: String,
    /// Tab id being watched (for screen/css/xpath/console; defaults to the target being operated)
    #[serde(default)]
    pub tab: Option<String>,
    /// String pattern (screen = browser body text, console = tab output)
    #[serde(default)]
    pub pattern: Option<String>,
    /// Selector (for css/xpath; either "#id" or an xpath string)
    #[serde(default)]
    pub sel: Option<String>,
    /// Threshold (rounds = count, tokens = estimate)
    #[serde(default)]
    pub max: Option<i64>,
    /// Seconds (for time)
    #[serde(default)]
    pub sec: Option<i64>,
    /// Verdict: "success" | "fail"
    #[serde(default)]
    pub outcome: String,
    /// Exit code
    #[serde(default)]
    pub code: i32,
    /// Reason (human-readable, kept in the record)
    #[serde(default)]
    pub reason: Option<String>,
}

/// Turn the list of stop conditions into a Lua table literal to pass to the built-in controller.
/// Strings are quoted safely as if by %q (done on the Rust side, not via Lua's string.format)
pub fn stops_to_lua(stops: &[StopCond]) -> String {
    fn q(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                _ => out.push(c),
            }
        }
        out.push('"');
        out
    }
    let mut b = String::from("{\n");
    for s in stops {
        if s.when.trim().is_empty() {
            continue;
        }
        b.push_str("  { when=");
        b.push_str(&q(&s.when));
        if let Some(t) = &s.tab {
            b.push_str(", tab=");
            b.push_str(&q(t));
        }
        if let Some(p) = &s.pattern {
            b.push_str(", pattern=");
            b.push_str(&q(p));
        }
        if let Some(sel) = &s.sel {
            b.push_str(", sel=");
            b.push_str(&q(sel));
        }
        if let Some(m) = s.max {
            b.push_str(&format!(", max={m}"));
        }
        if let Some(sec) = s.sec {
            b.push_str(&format!(", sec={sec}"));
        }
        let outcome = if s.outcome.is_empty() { "success" } else { &s.outcome };
        b.push_str(", outcome=");
        b.push_str(&q(outcome));
        b.push_str(&format!(", code={}", s.code));
        b.push_str(", reason=");
        b.push_str(&q(s.reason.as_deref().unwrap_or("")));
        b.push_str(" },\n");
    }
    b.push('}');
    b
}

/// Controls shown above the browser. Just holds which ones to show.
///
/// All false by default = nothing shown. Doesn't crowd a project's screen with
/// controls it doesn't need. The user picks each one individually
#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
pub struct NavSpec {
    #[serde(default)]
    pub back: bool,
    #[serde(default)]
    pub forward: bool,
    #[serde(default)]
    pub reload: bool,
    /// URL bar. Lets a person navigate to any page
    #[serde(default)]
    pub url: bool,
}

impl NavSpec {
    /// If none are shown, the bar itself isn't needed
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Show all of them. Used when the spec is omitted, as in `browser_nav(id)`
    pub fn all() -> Self {
        Self { back: true, forward: true, reload: true, url: true }
    }
}

/// The banner shown below a page.
///
/// Its mere presence means "show it". Unlike the nav bar, it needs actual text,
/// so a plain show/hide boolean isn't enough
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct AskSpec {
    /// Text shown on the left of the banner
    #[serde(default)]
    pub text: String,
    /// Button label. Uses the default wording if empty
    #[serde(default)]
    pub label: String,
}

/// A single browser opened alongside the workspace
#[derive(Debug, Clone, Deserialize)]
pub struct BrowserConfig {
    /// Name referred to from automation (e.g. "br")
    pub id: String,
    /// URL opened initially. http/https only
    pub url: String,
    /// Browser profile name (defaults to "default"). Ignored if private is true
    #[serde(default)]
    pub browser_profile: Option<String>,
    /// Private (disposable) browser
    #[serde(default)]
    pub private: bool,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct TabConfig {
    /// Tab name (generated from the command name if omitted)
    pub name: Option<String>,
    /// Name referred to from automation (optional). Setting this means renaming the
    /// tab won't break scripts. If omitted, the tab can be referred to by its name
    #[serde(default)]
    pub id: Option<String>,
    /// Launch command: "ssh user@host" or ["ssh", "user@host"]
    pub command: CommandSpec,
    /// Explicit detection profile (auto-selected from the command name if omitted)
    pub profile: Option<String>,
    /// Input lock (soft lock). Prevents accidental input into a mid-pipeline tab.
    /// Can be released at runtime with Ctrl+B l or by clicking the lock icon
    #[serde(default)]
    pub locked: bool,
    /// Automatically restart when the child process exits.
    /// Used to recover from an SSH disconnect or after a CLI tool self-updates
    #[serde(default)]
    pub auto_restart: bool,
    /// Browser-operation mode. Setting this to the id of the browser tab being
    /// operated makes this tab a "browser-operating agent". The built-in controller
    /// runs, and the goal is typed into the input field (not written in config). Takes priority over automation
    #[serde(default)]
    pub drives: Option<String>,
    /// Working folder at launch. A relative path is resolved against the config file's location.
    /// A folder inside Docker/WSL cannot be specified this way (use the command's own -w / --cd)
    #[serde(default)]
    pub cwd: Option<String>,
    /// Scrollback line count (defaults to 5000)
    #[serde(default)]
    pub scrollback: Option<usize>,
    /// Character encoding ("shift_jis" etc). Defaults to UTF-8
    #[serde(default)]
    pub encoding: Option<String>,
    /// Save the session log under logs/
    #[serde(default)]
    pub log: bool,
    /// Notification destination to ping each time this tab's AI finishes a
    /// response (a beginner-friendly shortcut for an on_done that calls notify).
    #[serde(default)]
    pub notify_on_done: Option<String>,
    /// Automation dedicated to this tab (matched with the highest priority).
    /// A directory means per-event files; a .lua file means function definitions
    #[serde(default)]
    pub automation: Option<String>,
    /// Old name. Used when automation is not set
    #[serde(default)]
    pub lua: Option<String>,
    /// Controls shown on a browser tab (back/forward/reload/URL bar).
    /// Meaningless for a terminal tab, so it's not read there
    #[serde(default)]
    pub nav: Option<NavSpec>,
    /// Banner shown below a browser tab (text and button label)
    #[serde(default)]
    pub ask: Option<AskSpec>,
    /// Browser profile name (the box holding cookies/login). Defaults to "default".
    /// Tabs sharing a profile name share login state, the same idea as Chrome's "person".
    /// Ignored when private is true
    #[serde(default)]
    pub browser_profile: Option<String>,
    /// Private (disposable) browser. When true, opens in a temporary area that
    /// keeps no cookies/history and is wiped on close. browser_profile is unused in that case
    #[serde(default)]
    pub private: bool,
    /// Child tabs for display purposes (forwarding relationships are decided by Lua; this is display hierarchy only)
    #[serde(default)]
    pub children: Vec<TabConfig>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum CommandSpec {
    Line(String),
    Argv(Vec<String>),
}

impl Default for CommandSpec {
    /// The nothing-written state. argv ends up empty
    fn default() -> Self {
        CommandSpec::Argv(Vec::new())
    }
}

impl CommandSpec {
    /// Normalize a whitespace-separated string or an array into argv.
    /// Use the array form if a path contains whitespace
    pub fn argv(&self) -> Vec<String> {
        match self {
            CommandSpec::Line(s) => s.split_whitespace().map(str::to_string).collect(),
            CommandSpec::Argv(v) => v.clone(),
        }
    }
}

/// A workspace resolved at launch time (tabs are flattened; depth preserves the hierarchy)
pub struct Workspace {
    pub name: String,
    pub tabs: Vec<FlatTab>,
    /// Automation at the workspace level
    pub automation: Option<String>,
    /// Browsers opened alongside it
    pub browsers: Vec<BrowserConfig>,
    /// Secret keys this workspace's rally is allowed to use (default is empty = deny all)
    pub secrets_allow: Vec<String>,
    /// Allow all secrets, knowingly accepting the risk
    pub secrets_allow_all: bool,
    /// Stop conditions (the referee)
    pub stops: Vec<StopCond>,
    /// AI-vs-AI discussion settings
    pub discuss: Option<DiscussSpec>,
}

/// Whether this tab is a browser. Returns the URL if so.
///
/// Told apart the same way as ssh/docker/wsl: by the head of the command string.
/// The settings screen's "type" field follows this same rule
pub fn browser_url_of(argv: &[String]) -> Option<String> {
    let (head, rest) = argv.split_first()?;
    if !head.eq_ignore_ascii_case("browser") && !head.eq_ignore_ascii_case("web") {
        return None;
    }
    let url = rest.first()?.trim().to_string();
    (!url.is_empty()).then_some(url)
}

impl TabConfig {
    /// Prefer automation, falling back to the old name lua
    pub fn automation_path(&self) -> Option<String> {
        self.automation.clone().or_else(|| self.lua.clone())
    }
}

impl Config {
    pub fn automation_path(&self) -> Option<String> {
        self.automation.clone().or_else(|| self.lua.clone())
    }
}

pub struct FlatTab {
    pub cfg: TabConfig,
    /// Display indent depth (0 = parent)
    pub depth: u16,
}

/// Verify each tab can be addressed uniquely from automation.
/// If the same name is used more than once the destination is ambiguous, so this warns at startup
pub fn duplicate_keys(ws: &Workspace) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = Default::default();
    for t in &ws.tabs {
        let key = t
            .cfg
            .id
            .clone()
            .or_else(|| t.cfg.name.clone())
            .unwrap_or_default();
        if key.is_empty() {
            continue;
        }
        *seen.entry(key).or_insert(0) += 1;
    }
    let mut dups: Vec<String> = seen
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(k, _)| k)
        .collect();
    dups.sort();
    dups
}

/// Flatten children depth-first (keeps display order matching tab numbers)
fn flatten(tabs: &[TabConfig], depth: u16, out: &mut Vec<FlatTab>) {
    for t in tabs {
        out.push(FlatTab {
            cfg: t.clone(),
            depth,
        });
        flatten(&t.children, depth + 1, out);
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T> {
    let text = std::fs::read_to_string(path).with_context(|| {
        crate::i18n::tp(
            "err.config.read_failed",
            &[("path", &path.display().to_string())],
        )
    })?;
    serde_json::from_str(&text).with_context(|| {
        crate::i18n::tp(
            "err.config.json_invalid",
            &[("path", &path.display().to_string())],
        )
    })
}

/// Resolve a data file path, preferring beside the exe (portable layout).
/// Configs pointing at the old projects/ name also fall back to workspaces/ (compat)
pub fn resolve_data_path(p: &str) -> std::path::PathBuf {
    let mut candidates = vec![p.to_string()];
    // Fall back between projects/ and workspaces/ in either direction
    if let Some(rest) = p.strip_prefix("projects/") {
        candidates.push(format!("workspaces/{rest}"));
    } else if let Some(rest) = p.strip_prefix("workspaces/") {
        candidates.push(format!("projects/{rest}"));
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf));
    for cand in &candidates {
        if let Some(dir) = &exe_dir {
            let full = dir.join(cand);
            if full.exists() {
                return full;
            }
        }
        let local = std::path::PathBuf::from(cand);
        if local.exists() {
            return local;
        }
    }
    std::path::PathBuf::from(p)
}

impl Config {
    /// Resolve the workspace definitions.
    /// If workspaces isn't defined, inline tabs are treated as a single unnamed workspace
    pub fn resolve_workspaces(&self) -> (Vec<Workspace>, Vec<String>) {
        let mut out = Vec::new();
        let mut errors = Vec::new();
        if self.workspaces.is_empty() {
            if !self.tabs.is_empty() {
                let mut tabs = Vec::new();
                flatten(&self.tabs, 0, &mut tabs);
                out.push(Workspace {
                    name: "DEFAULT".into(),
                    tabs,
                    automation: None,
                    browsers: Vec::new(),
                    secrets_allow: Vec::new(),
                    secrets_allow_all: false,
                    stops: Vec::new(),
                    discuss: None,
                });
            }
            return (out, errors);
        }
        for ws in &self.workspaces {
            #[allow(clippy::type_complexity)]
            #[allow(clippy::type_complexity)]
            let (tab_defs, file_name, file_lua, file_secrets, file_stops, file_discuss): (
                Vec<TabConfig>,
                Option<String>,
                Option<String>,
                (Vec<String>, bool),
                Vec<StopCond>,
                Option<DiscussSpec>,
            ) = match &ws.file {
                Some(f) => match read_json::<WorkspaceFile>(&resolve_data_path(f)) {
                    Ok(p) => (
                        p.tabs,
                        p.name,
                        p.automation.or(p.lua),
                        (p.secrets_allow, p.secrets_allow_all),
                        p.stops,
                        p.discuss,
                    ),
                    Err(e) => {
                        errors.push(format!("{}: {e:#}", ws.name));
                        continue;
                    }
                },
                None => (ws.tabs.clone(), None, None, (Vec::new(), false), Vec::new(), None),
            };
            let mut tabs = Vec::new();
            flatten(&tab_defs, 0, &mut tabs);
            out.push(Workspace {
                // Prefer the display name from config; fall back to the definition file's name if empty
                name: if ws.name.is_empty() {
                    file_name.unwrap_or_else(|| "UNNAMED".into())
                } else {
                    ws.name.clone()
                },
                tabs,
                // Prefer config's setting; fall back to the definition file's if absent
                automation: ws.automation.clone().or_else(|| ws.lua.clone()).or(file_lua),
                browsers: ws.browsers.clone(),
                // Prefer config's setting; fall back to the definition file's if absent
                secrets_allow: if ws.secrets_allow.is_empty() {
                    file_secrets.0
                } else {
                    ws.secrets_allow.clone()
                },
                secrets_allow_all: ws.secrets_allow_all || file_secrets.1,
                // Prefer config's setting; fall back to the definition file's if absent
                stops: if ws.stops.is_empty() { file_stops } else { ws.stops.clone() },
                discuss: ws.discuss.clone().or(file_discuss),
            });
        }
        (out, errors)
    }
}

/// Root of the portable layout (where the exe and its folders sit side by side).
/// data / logs / config all live under here
pub fn root_dir() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// Search order for the config file. Prefers the new layout (config folder),
/// but also reads the old layout (config.json directly under root). Current-dir-relative paths come last
fn config_candidates() -> Vec<std::path::PathBuf> {
    let root = root_dir();
    vec![
        root.join("config").join("config.json"), // new: the config folder beside the exe
        root.join("config.json"),                // old: directly beside the exe (migration target)
        std::path::PathBuf::from("config/config.json"), // new: relative to current dir
        std::path::PathBuf::from("config.json"),        // old: relative to current dir
    ]
}

/// Move the old layout (config.json / secrets.json directly under root) into the new config folder.
/// Done only once. Not fatal if it fails, since loading still falls back to the old layout
pub fn migrate_legacy_config() {
    let root = root_dir();
    let new_cfg = root.join("config").join("config.json");
    let old_cfg = root.join("config.json");
    if new_cfg.exists() || !old_cfg.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(new_cfg.parent().unwrap());
    // rename if on the same volume; otherwise copy then delete
    if std::fs::rename(&old_cfg, &new_cfg).is_err()
        && std::fs::copy(&old_cfg, &new_cfg).is_ok()
    {
        let _ = std::fs::remove_file(&old_cfg);
    }
    // Move secrets.json alongside it too, if present
    let (old_s, new_s) = (root.join("secrets.json"), root.join("config").join("secrets.json"));
    if old_s.exists() && !new_s.exists() {
        if std::fs::rename(&old_s, &new_s).is_err() && std::fs::copy(&old_s, &new_s).is_ok() {
            let _ = std::fs::remove_file(&old_s);
        }
    }
}

/// Path to the config file the web GUI edits.
/// Returns the existing file's path if present, otherwise the path where a new one would be created beside the exe.
/// Home for state files (ones a human doesn't edit), gathered under the root's data folder.
/// The exe is the only file directly at the root (folders are config / data / logs / lang / workspaces / scripts)
pub fn state_path(name: &str) -> std::path::PathBuf {
    let p = root_dir().join("data");
    let _ = std::fs::create_dir_all(&p);
    p.join(name)
}

/// Home for logs. Pinned to the root's logs folder rather than the current directory
/// (if the log destination changed depending on how the app was launched, crash records would get lost)
pub fn logs_dir() -> std::path::PathBuf {
    let p = root_dir().join("logs");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// Home for the name of the last-open workspace.
///
/// Not written back into config.json -- that would interrupt a user mid-edit,
/// and the change-watcher would react to its own write and trigger a reload
fn last_workspace_path() -> std::path::PathBuf {
    state_path("last-workspace")
}

/// Name of the last-open workspace
pub fn load_last_workspace() -> Option<String> {
    let s = std::fs::read_to_string(last_workspace_path()).ok()?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Remember the name of the currently open workspace. Fails silently if it can't
/// (being unable to remember it is no reason for things to stop working)
pub fn save_last_workspace(name: &str) {
    let _ = crate::crypto::write_atomic(&last_workspace_path(), name);
}

pub fn config_file_path() -> std::path::PathBuf {
    let candidates = config_candidates();
    candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| candidates[0].clone())
}

pub fn load() -> Option<Config> {
    for path in config_candidates() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        match serde_json::from_str::<Config>(&text) {
            Ok(c) if !c.tabs.is_empty() || !c.workspaces.is_empty() => return Some(c),
            _ => continue,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_store_roundtrips_and_never_reveals_values() {
        let dir = std::env::temp_dir().join("shikisha-secrets-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("secrets.json");

        // Register in plaintext -> the list shows only key and description (no value)
        upsert_secret(&path, None, "diary_saas", "日記SaaSのログイン", "hunter2秘密").unwrap();
        let list = list_secrets(&path, None).unwrap();
        assert_eq!(list, vec![("diary_saas".into(), "日記SaaSのログイン".into())]);
        // The value really is stored (retrievable via resolve_tokens)
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("hunter2秘密"), "値が保存されていない");
        // But the value never appears in the listing API
        assert!(!format!("{list:?}").contains("hunter2"), "一覧に値が漏れている");

        // Update / add
        upsert_secret(&path, None, "diary_saas", "説明更新", "newpass").unwrap();
        upsert_secret(&path, None, "github", "PAT", "ghp_xxx").unwrap();
        let keys: Vec<String> = list_secrets(&path, None).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["diary_saas".to_string(), "github".to_string()], "整列済み");

        // Delete
        delete_secret(&path, None, "github").unwrap();
        let keys: Vec<String> = list_secrets(&path, None).unwrap().into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["diary_saas".to_string()]);

        // With a master password set, it's saved encrypted
        upsert_secret(&path, Some("master"), "enc_key", "暗号化テスト", "topsecret").unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(crate::crypto::is_encrypted(&raw), "パスワードありなら暗号化される");
        assert!(!raw.contains("topsecret"), "暗号化後は生値が見えない");
        // With the correct password the list can be read; the value still doesn't appear
        let list = list_secrets(&path, Some("master")).unwrap();
        assert!(list.iter().any(|(k, _)| k == "enc_key"));
        // An invalid key is rejected
        assert!(upsert_secret(&path, None, "../evil", "x", "y").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn children_flatten_with_depth() {
        let cfg: Config = serde_json::from_str(
            r#"{"tabs":[
                {"name":"A","command":"a","children":[
                    {"name":"B","command":"b","locked":true},
                    {"name":"C","command":"c"}
                ]}
            ]}"#,
        )
        .unwrap();
        let (ws, errs) = cfg.resolve_workspaces();
        assert!(errs.is_empty());
        let tabs = &ws[0].tabs;
        assert_eq!(tabs.len(), 3, "親子が平坦化される");
        assert_eq!(tabs[0].depth, 0);
        assert_eq!(tabs[1].depth, 1);
        assert!(tabs[1].cfg.locked, "lockedが読める");
        assert_eq!(tabs[2].cfg.name.as_deref(), Some("C"));
    }

    #[test]
    fn legacy_projects_path_falls_back_to_workspaces() {
        // An existing config pointing at the old name projects/ should still be able to read workspaces/
        let dir = std::env::temp_dir().join("shikisha-ws-compat");
        let ws_dir = dir.join("workspaces");
        std::fs::create_dir_all(&ws_dir).unwrap();
        std::fs::write(ws_dir.join("x.json"), r#"{"tabs":[]}"#).unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();

        let resolved = resolve_data_path("projects/x.json");
        let ok = resolved.exists();

        std::env::set_current_dir(prev).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(ok, "projects/ 指定が workspaces/ にフォールバックする");
    }

    #[test]
    fn inline_workspaces_are_resolved() {
        let cfg: Config = serde_json::from_str(
            r#"{"workspaces":[
                {"name":"X","tabs":[{"name":"a","command":"a"}]},
                {"name":"Y","tabs":[{"name":"b","command":"b"}]}
            ]}"#,
        )
        .unwrap();
        let (ws, _) = cfg.resolve_workspaces();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[1].name, "Y");
    }

    #[test]
    fn missing_workspace_file_is_reported_not_fatal() {
        let cfg: Config = serde_json::from_str(
            r#"{"workspaces":[
                {"name":"Bad","file":"workspaces/does-not-exist.json"},
                {"name":"Good","tabs":[{"name":"a","command":"a"}]}
            ]}"#,
        )
        .unwrap();
        let (ws, errs) = cfg.resolve_workspaces();
        assert_eq!(ws.len(), 1, "壊れた定義は飛ばして続行");
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn operate_defaults_match_the_baked_in_safety_net() {
        // A config with no `operate` block falls back to the historical limits and
        // the safe "stop" policy, so behavior is unchanged until the user opts in.
        let cfg: Config = serde_json::from_str(r#"{"workspaces":[]}"#).unwrap();
        assert_eq!(cfg.operate.max_rounds, 40);
        assert_eq!(cfg.operate.max_seconds, 900);
        assert_eq!(cfg.operate.max_tokens, 400_000);
        assert_eq!(cfg.operate.on_limit, "stop");
        assert_eq!(cfg.operate.settle_ms, 1800);
        assert_eq!(cfg.operate.confirm, "off");
    }

    #[test]
    fn operate_accepts_partial_overrides_and_unlimited_zeros() {
        // Only some fields set: the rest keep their defaults. 0 means "no limit".
        let cfg: Config =
            serde_json::from_str(r#"{"workspaces":[],"operate":{"max_rounds":0,"on_limit":"continue"}}"#)
                .unwrap();
        assert_eq!(cfg.operate.max_rounds, 0, "0 = unlimited rounds");
        assert_eq!(cfg.operate.on_limit, "continue");
        assert_eq!(cfg.operate.max_seconds, 900, "untouched field keeps its default");
    }
}

#[cfg(test)]
mod browser_kind_tests {
    use super::browser_url_of;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// The settings screen and the app itself must classify "type = browser" by the same rule.
    ///
    /// If the check were split across two places, a tab could look like a browser
    /// on screen but launch a shell instead -- a mismatch
    #[test]
    fn a_browser_tab_is_told_apart_by_its_command() {
        assert_eq!(
            browser_url_of(&v(&["browser", "https://example.com/"])).as_deref(),
            Some("https://example.com/")
        );
        assert_eq!(
            browser_url_of(&v(&["web", "http://127.0.0.1:8080/"])).as_deref(),
            Some("http://127.0.0.1:8080/"),
            "web という綴りも通す"
        );
        assert_eq!(
            browser_url_of(&v(&["BROWSER", "https://a.example/"])).as_deref(),
            Some("https://a.example/"),
            "大文字小文字は問わない"
        );

        // Things that are not a browser
        assert!(browser_url_of(&v(&["cmd.exe"])).is_none());
        assert!(browser_url_of(&v(&["claude"])).is_none());
        assert!(browser_url_of(&v(&["browser"])).is_none(), "URLが無い");
        assert!(browser_url_of(&v(&["browser", "  "])).is_none(), "空白だけ");
        assert!(browser_url_of(&[]).is_none());
        // Don't sweep in some other command that merely starts with "browser"
        assert!(browser_url_of(&v(&["browserify", "x"])).is_none());
    }

    /// A browser tab's appearance can be decided by config alone, with no Lua.
    ///
    /// Only what's written shows up; anything not written falls back to the default (hidden)
    #[test]
    fn a_browser_tab_can_be_dressed_from_the_settings_alone() {
        let t: super::TabConfig = serde_json::from_str(
            r#"{
                "name": "解析",
                "command": "browser https://example.com/",
                "nav": { "reload": true, "url": true },
                "ask": { "text": "読み終わったら押してください", "label": "解析する" }
            }"#,
        )
        .unwrap();
        let nav = t.nav.expect("上のバーが読めていない");
        assert!(nav.reload && nav.url, "書いたものが出ない");
        assert!(!nav.back && !nav.forward, "書いていないものまで出る");
        let ask = t.ask.expect("帯が読めていない");
        assert_eq!(ask.label, "解析する");

        // If nothing is written, neither is shown
        let bare: super::TabConfig =
            serde_json::from_str(r#"{"command": "browser https://example.com/"}"#).unwrap();
        assert!(bare.nav.is_none() && bare.ask.is_none());

        // The banner's mere presence means "show it" -- shown even with empty contents
        let empty: super::TabConfig =
            serde_json::from_str(r#"{"command": "browser https://x/", "ask": {}}"#).unwrap();
        assert!(empty.ask.is_some(), "空の帯を無かったことにしている");
    }
}

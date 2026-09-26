//! Sandboxes on somebody else's machines.
//!
//! A cloud sandbox is a machine that did not exist a second ago and will not
//! exist tomorrow, which is the whole of the difference from an [`crate::ssh`]
//! host: it has to be asked for before anything can run on it. Once it is
//! there, the thing this module is for is the same one thing -- run a command
//! and hear how it went -- so the rest of the app does not have to know which
//! kind of machine it is talking to.
//!
//! Three jobs live here, the same three a machine is wanted for. [`exec`] runs
//! one command and says how it went -- an ending and an exit code, which is
//! what a program asking whether git worked needs. [`shell`] opens a terminal
//! and never ends, which is what a person needs. [`files`] moves and lists
//! things, which is what the file panel needs.
//!
//! All three answer in [`crate::ssh`]'s shapes rather than their own. That is
//! the point: a terminal here wears [`portable_pty::MasterPty`] and a listing
//! here is an [`crate::ssh::Entry`], so nothing above has to learn a second
//! vocabulary for the second kind of machine.

use anyhow::{Result, anyhow, bail};
use std::time::Duration;

/// Where the service lives. One host for the control plane, one for the
/// sandboxes themselves -- which are told apart by a header rather than by a
/// name of their own, so nothing here has to wait for a name to spread
const API: &str = "https://api.e2b.app";
const SANDBOX: &str = "https://sandbox.e2b.app";
/// The port the agent inside a sandbox listens on. Its own default, named here
/// because it travels in a header on every call
const AGENT_PORT: u16 = 49983;

/// A sandbox that exists right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub id: String,
    /// What the agent inside it wants to see before it will do anything.
    /// Absent on an older sandbox, where the connection itself is the proof
    pub token: Option<String>,
}

/// The key this program is using right now.
///
/// Handed over when the settings are read, the same way ssh's passwords are,
/// so that a key taken out of the settings stops working rather than living on
/// in a thread's memory. The environment is a fallback for a machine that is
/// running this without settings at all -- a check, a server, a build
static KEY: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();

pub fn use_key(key: Option<String>) {
    let cell = KEY.get_or_init(Default::default);
    if let Ok(mut k) = cell.lock() {
        *k = key.map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    }
}

pub fn key() -> Option<String> {
    if let Some(k) = KEY.get().and_then(|c| c.lock().ok()).and_then(|k| k.clone()) {
        return Some(k);
    }
    std::env::var("E2B_API_TOKEN").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        // Making one takes a few seconds; the sandbox is being built
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .new_agent()
}

/// How long one command run in a machine may take before it is given up
/// on: a clone of a large project, or a language installed, takes minutes
const COMMAND_WAIT: Duration = Duration::from_secs(20 * 60);

/// How often, in seconds, the agent in a machine is asked to say it is still
/// there on a stream that has nothing else to say. A command that prints
/// nothing for a minute -- an install told to be quiet -- or a terminal left
/// alone otherwise has its connection dropped on the way, and the end of the
/// command is never heard. The service's own client asks the same
const KEEPALIVE: &str = "50";

/// What a new machine is asked for with.
///
/// Every one of them pauses when its time runs out rather than being thrown
/// away, and starts again where it stopped when a request reaches one of its
/// addresses -- which is what lets a MicroVM hold a piece of work for as long
/// as the work lasts, and still take a webhook while nobody is looking at it
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Asking {
    /// The image it is built from
    pub template: String,
    /// How many minutes it runs untouched before it is paused
    pub minutes: u32,
    /// What marks it as this app's, and whose (see [`marks`]). Never a secret:
    /// this is shown on the service's own pages
    pub marks: Vec<(String, String)>,
    /// A git server's sign-in, put on its requests on the way out (see
    /// [`SignIn`]). Absent is a machine that signs in to nothing
    pub sign_in: Option<SignIn>,
}

/// A git server's sign-in, added to the requests a machine sends it.
///
/// Added by the service on the way out, so the token never enters the
/// machine: nothing running in there -- an AI included -- can read it, and a
/// machine paused or copied carries none of it. What runs in there can still
/// use it, by sending a request to that server; that is what it is for
#[derive(Clone, PartialEq, Eq)]
pub struct SignIn {
    /// The server, `github.com`
    pub host: String,
    /// The name sent with the token. GitHub takes any, and expects this one
    pub login: String,
    pub token: String,
    /// The name and address commits made on the machine carry, when the
    /// account says them (see [`crate::microvm::identity_steps`] for what
    /// the machine is told when it does not)
    pub name: Option<String>,
    pub email: Option<String>,
}

impl std::fmt::Debug for SignIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A token is not something a log or a panic prints
        f.debug_struct("SignIn").field("host", &self.host).field("login", &self.login).finish_non_exhaustive()
    }
}

/// The rules that put a sign-in on a machine's requests: git's own server
/// with the sign-in git sends, and -- for GitHub -- its API with the one the
/// API takes, so `gh` and a plain `curl` in there are signed in as well
pub fn network_of(sign_in: Option<&SignIn>) -> serde_json::Value {
    use base64::Engine as _;
    let Some(s) = sign_in else { return serde_json::json!({}) };
    let host = s.host.trim().to_ascii_lowercase();
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", s.login, s.token))
    );
    let header = |value: String| serde_json::json!([{ "transform": { "headers": { "Authorization": value } } }]);
    let mut rules = serde_json::Map::new();
    rules.insert(host.clone(), header(basic));
    if host == crate::config::GITHUB_HOST {
        rules.insert("api.github.com".into(), header(format!("Bearer {}", s.token)));
    }
    serde_json::json!({ "rules": rules })
}

/// What marks a machine as this app's: that it is, which PC asked for it, and
/// for which project. What lets a list show only this app's machines, and a
/// person on the service's own pages tell them apart
pub fn marks(project: &str) -> Vec<(String, String)> {
    let pc = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    vec![
        (MARK.into(), "1".into()),
        ("pc".into(), pc.trim().to_string()),
        ("project".into(), project.trim().to_string()),
    ]
}

/// The mark every machine this app makes carries
pub const MARK: &str = "shikisha";

/// Where the service puts a machine's ports on the internet
const PUBLIC_DOMAIN: &str = "e2b.app";

/// The ports the service's own agents listen on inside every machine, which
/// are not the machine's to offer
pub const OWN_PORTS: &[u16] = &[AGENT_PORT, 49982];

/// The address a machine's port answers on from anywhere, over HTTPS
pub fn public_url(id: &str, port: u16) -> String {
    format!("https://{port}-{id}.{PUBLIC_DOMAIN}")
}

/// One call to the service, and what it answered
fn answered(resp: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<serde_json::Value> {
    let mut resp = resp.map_err(|e| anyhow!(call_failed(&e)))?;
    let said = resp.body_mut().read_to_string()?;
    if said.trim().is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&said).map_err(|_| anyhow!(crate::i18n::tp("err.e2b.said", &[("said", &said)])))
}

/// What a call the service refused says, in words a person can act on. A key
/// that is wrong or no longer valid is said as that -- "could not be reached"
/// sent people looking at their network for a problem in the settings
fn call_failed(e: &ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(401 | 403) => crate::i18n::t("err.e2b.bad_key"),
        other => crate::i18n::tp("err.e2b.call", &[("e", &format!("{other}"))]),
    }
}

/// A machine, as the service describes one it has just made or started
fn sandbox_of(v: &serde_json::Value) -> Result<Sandbox> {
    let id = v
        .get("sandboxID")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!(crate::i18n::tp("err.e2b.said", &[("said", &v.to_string())])))?;
    Ok(Sandbox {
        id: id.to_string(),
        token: v.get("envdAccessToken").and_then(|x| x.as_str()).map(str::to_string),
    })
}

/// Ask for a machine.
pub fn create(key: &str, asking: &Asking) -> Result<Sandbox> {
    let mut body = serde_json::json!({
        "templateID": asking.template,
        "timeout": asking.minutes.max(1) * 60,
        "autoPause": true,
        "autoResume": { "enabled": true },
        "metadata": asking
            .marks
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect::<serde_json::Map<String, serde_json::Value>>(),
    });
    if asking.sign_in.is_some() {
        body["network"] = network_of(asking.sign_in.as_ref());
    }
    let v = answered(
        agent()
            .post(&format!("{API}/v2/sandboxes"))
            .header("X-API-Key", key)
            .header("Content-Type", "application/json")
            .send(serde_json::to_string(&body)?),
    )?;
    let made = sandbox_of(&v)?;
    remember(&made);
    Ok(made)
}

/// The machine by its id, started again if it was paused, and given its full
/// time from now. Asked before anything is done in a machine this program has
/// not spoken to since it started: the answer carries what the agent inside
/// wants to see
pub fn connect(key: &str, id: &str, minutes: u32) -> Result<Sandbox> {
    let v = answered(
        agent()
            .post(&format!("{API}/v2/sandboxes/{id}/connect"))
            .header("X-API-Key", key)
            .header("Content-Type", "application/json")
            .send(serde_json::json!({ "timeout": minutes.max(1) * 60 }).to_string()),
    )?;
    let found = sandbox_of(&v)?;
    remember(&found);
    Ok(found)
}

/// A copy of a machine as it is this moment -- its files, and what was
/// running in it -- as a machine of its own. The machine copied is stopped
/// for the moment it takes and goes on as it was. A paused one is started
/// first: the service copies only a running machine
pub fn fork(key: &str, id: &str, minutes: u32) -> Result<Sandbox> {
    connect(key, id, minutes)?;
    let v = answered(
        agent()
            .post(&format!("{API}/sandboxes/{id}/fork"))
            .header("X-API-Key", key)
            .header("Content-Type", "application/json")
            .send(serde_json::json!({ "timeout": minutes.max(1) * 60, "count": 1 }).to_string()),
    )?;
    let one = v.as_array().and_then(|a| a.first()).cloned().unwrap_or_default();
    if let Some(e) = one.get("error").filter(|e| !e.is_null()) {
        bail!(crate::i18n::tp("err.e2b.said", &[("said", &e.to_string())]));
    }
    let made = sandbox_of(one.get("sandbox").unwrap_or(&serde_json::Value::Null))?;
    remember(&made);
    Ok(made)
}

/// Puts a sign-in on a running machine's requests, in place of the one it
/// had: a token changed since the machine was made reaches it without the
/// machine being made again
pub fn sign_in_as(key: &str, id: &str, sign_in: Option<&SignIn>) -> Result<()> {
    answered(
        agent()
            .put(&format!("{API}/sandboxes/{id}/network"))
            .header("X-API-Key", key)
            .header("Content-Type", "application/json")
            .send(network_of(sign_in).to_string()),
    )
    .map(|_| ())
}

/// Give a machine its full time again, from now.
///
/// A MicroVM pauses when its minutes run out, counted from when it was last
/// given them -- not from when it was last used. Given them only when this
/// program first spoke to it, a machine paused half an hour later whatever was
/// running in it, and an AI in the middle of a long piece of work stopped with
/// it. Asked while something on the machine is at work (see
/// `runtime::keep_machines_up`), so the minutes the settings name are minutes
/// untouched, as they say. Unlike `connect`, which only ever lengthens them,
/// this sets them from now
pub fn keep_up(host: &crate::config::HostSpec) -> Result<()> {
    let id = host
        .instance
        .as_deref()
        .ok_or_else(|| anyhow!(crate::i18n::tp("err.e2b.no_machine", &[("host", &host.name)])))?;
    let key = key().ok_or_else(|| anyhow!(crate::i18n::t("err.e2b.no_key")))?;
    answered(
        agent()
            .post(&format!("{API}/sandboxes/{id}/timeout"))
            .header("X-API-Key", &key)
            .header("Content-Type", "application/json")
            .send(serde_json::json!({ "timeout": host.minutes_or_default() * 60 }).to_string()),
    )
    .map(|_| ())
}

/// What state a machine is in -- `running` or `paused` -- asked of the
/// service's own records and not of the machine, so that asking does not
/// wake one that is paused (every request to a paused machine does)
pub fn state_of(key: &str, id: &str) -> Result<String> {
    let v = answered(agent().get(&format!("{API}/sandboxes/{id}")).header("X-API-Key", key).call())?;
    Ok(v.get("state").and_then(|s| s.as_str()).unwrap_or_default().to_string())
}

/// Let it go. A sandbox nobody kills still pauses when its time runs out, so
/// this is what ends one for good
pub fn kill(key: &str, id: &str) -> Result<()> {
    // Let go of before it is asked: from this moment nothing here speaks to
    // it, so nothing can wake it while the answer is on its way -- or after
    // one that says it is still there
    let_go(id);
    let resp = agent()
        .delete(&format!("{API}/sandboxes/{id}"))
        .header("X-API-Key", key)
        .call();
    match resp {
        Ok(_) => Ok(()),
        // Gone already is what was asked for: a second try after an answer
        // that was lost on the way back finds nothing to kill
        Err(ureq::Error::StatusCode(404)) => Ok(()),
        Err(e) => bail!(call_failed(&e)),
    }
}

/// Throw away a machine a making made and could not finish with.
///
/// Its own call rather than `let _ = kill(..)`: a machine that would not go is
/// one nothing will ever point at, paid for until somebody finds it, so it is
/// at least written down where it can be found -- and it is in the list of
/// machines in the settings, where it can be deleted
pub fn throw_away(key: &str, id: &str) {
    if let Err(e) = kill(key, id) {
        crate::append_hook_log(&format!("could not throw away machine {id}: {e:#}"));
        // Nothing points at it, so the list in the settings is where it is
        // deleted from -- and that list must be able to
        take_back(id);
    }
}

/// The machines being deleted, or that were and would not go.
///
/// Every request to a paused machine starts it again -- they are made to wake
/// on their own -- so a machine whose deletion failed is one request away from
/// running, and costing, again. What was still holding it (the editor a file
/// was open in) is what would send that request. Kept for as long as this
/// program runs; a machine put back in the list is taken off it
static LET_GO: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();

/// Stop speaking to this machine: it is being deleted.
pub fn let_go(id: &str) {
    forget(id);
    if let Ok(mut g) = LET_GO.get_or_init(Default::default).lock() {
        g.insert(id.to_string());
    }
}

/// Speak to it again: the folder on it was put back in the list.
pub fn take_back(id: &str) {
    if let Ok(mut g) = LET_GO.get_or_init(Default::default).lock() {
        g.remove(id);
    }
}

fn let_go_of(id: &str) -> bool {
    LET_GO.get_or_init(Default::default).lock().is_ok_and(|g| g.contains(id))
}

/// One machine as the list gives it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub id: String,
    /// `running` or `paused`
    pub state: String,
    pub marks: std::collections::BTreeMap<String, String>,
    /// When it was started, as the service writes it (RFC 3339)
    pub started: String,
}

impl Listed {
    /// Whether this app made it. A key can be shared with other programs,
    /// and their machines are theirs to delete
    pub fn ours(&self) -> bool {
        self.marks.get(MARK).is_some_and(|v| v == "1")
    }
}

/// Every machine this key has, running or paused.
///
/// Not only the ones carrying this app's mark: a copy of a machine is not
/// promised to carry the marks of the one it was copied from, and a machine
/// nobody can see is one that is paid for until somebody finds it on the
/// service's own pages. Which are this app's is [`Listed::ours`]
pub fn list(key: &str) -> Result<Vec<Listed>> {
    let v = answered(
        agent()
            .get(&format!("{API}/v2/sandboxes?state=running,paused"))
            .header("X-API-Key", key)
            .call(),
    )?;
    let rows = v
        .get("sandboxes")
        .and_then(|x| x.as_array())
        .or_else(|| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(rows
        .iter()
        .filter_map(|r| {
            Some(Listed {
                id: r.get("sandboxID")?.as_str()?.to_string(),
                state: r.get("state").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
                marks: r
                    .get("metadata")
                    .and_then(|m| m.as_object())
                    .map(|m| {
                        m.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string())).collect()
                    })
                    .unwrap_or_default(),
                started: r.get("startedAt").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
            })
        })
        .collect())
}

/// What the agent inside each machine wants to see, as the service last
/// handed it over. Kept for the life of this program, so a machine is asked
/// for once and not before every keystroke: a paused machine is started
/// again by the request itself, and the token does not change while it sleeps
static KNOWN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Sandbox>>> =
    std::sync::OnceLock::new();

/// A machine this program has spoken to, kept so it is not connected to
/// again before every call. What `create` and `connect` do with what they
/// were given; a probe that made a machine its own way does the same
pub fn remember(s: &Sandbox) {
    if let Ok(mut k) = KNOWN.get_or_init(Default::default).lock() {
        k.insert(s.id.clone(), s.clone());
    }
}

fn forget(id: &str) {
    if let Ok(mut k) = KNOWN.get_or_init(Default::default).lock() {
        k.remove(id);
    }
}

/// The machine an entry in the settings names, through the folder that is
/// on it (see [`crate::config::HostSpec::instance`]).
///
/// Never made here: a folder on a MicroVM was made with its machine, and a
/// machine that is gone is said as gone rather than replaced by an empty one
/// -- an empty one in its place would look like the work was lost
pub fn machine(host: &crate::config::HostSpec) -> Result<Sandbox> {
    let id = host
        .instance
        .as_deref()
        .ok_or_else(|| anyhow!(crate::i18n::tp("err.e2b.no_machine", &[("host", &host.name)])))?;
    if let_go_of(id) {
        bail!(crate::i18n::tp("err.e2b.let_go", &[("host", &host.name), ("id", id)]));
    }
    if let Some(s) = KNOWN.get_or_init(Default::default).lock().ok().and_then(|k| k.get(id).cloned()) {
        return Ok(s);
    }
    let key = key().ok_or_else(|| anyhow!(crate::i18n::t("err.e2b.no_key")))?;
    connect(&key, id, host.minutes_or_default())
        .map_err(|e| anyhow!(crate::i18n::tp("err.e2b.gone", &[("host", &host.name), ("id", id), ("e", &format!("{e:#}"))])))
}

/// Run one command inside a sandbox, and wait for it to finish.
///
/// The same answer [`crate::ssh::exec`] gives, from a service that says it
/// very differently: a stream of frames rather than a channel, and an ending
/// that arrives as the words "exit status 0" rather than as a number. Both are
/// turned into the one shape the rest of the app knows, so nothing above here
/// has to care which kind of machine ran it.
pub fn exec(sandbox: &Sandbox, command: &str, cwd: Option<&str>) -> Result<crate::ssh::Ran> {
    let body = serde_json::json!({
        "process": {
            "cmd": "/bin/sh",
            "args": ["-c", command],
            "envs": {},
            "cwd": cwd,
        },
        "pty": serde_json::Value::Null,
        "stdin": false,
    });
    let mut req = waiting(COMMAND_WAIT.as_millis() as u64)
        .post(&format!("{SANDBOX}/process.Process/Start"))
        .header("Keepalive-Ping-Interval", KEEPALIVE)
        .header("Connect-Protocol-Version", "1")
        .header("Content-Type", "application/connect+json")
        .header("E2b-Sandbox-Id", &sandbox.id)
        .header("E2b-Sandbox-Port", &AGENT_PORT.to_string());
    if let Some(t) = sandbox.token.as_deref() {
        req = req.header("X-Access-Token", t);
    }
    let mut resp = req
        .send(frame(&serde_json::to_vec(&body)?))
        .map_err(|e| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))])))?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut resp.body_mut().as_reader(), &mut bytes)?;
    Ok(collect(&bytes))
}

/// One message, in the envelope the Connect protocol puts a stream in: a flag
/// byte, then four bytes of length, then the message
fn frame(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(msg.len() + 5);
    out.push(0u8);
    out.extend_from_slice(&(msg.len() as u32).to_be_bytes());
    out.extend_from_slice(msg);
    out
}

/// What the stream said, as the one answer the rest of the app understands.
///
/// The ending arrives as a sentence -- "exit status 3" -- and the numeric
/// field beside it is not filled in, so the number is read out of the words.
/// Nothing said about an ending is not the same as a clean one: a stream that
/// stopped has not told us the command worked
fn collect(bytes: &[u8]) -> crate::ssh::Ran {
    let (mut out, mut err) = (String::new(), String::new());
    let mut code: Option<i32> = None;
    let mut at = 0usize;
    while at + 5 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[at + 1], bytes[at + 2], bytes[at + 3], bytes[at + 4]])
            as usize;
        at += 5;
        if at + len > bytes.len() {
            break;
        }
        let msg: serde_json::Value = serde_json::from_slice(&bytes[at..at + len]).unwrap_or_default();
        at += len;
        if let Some(d) = msg.get("event").and_then(|e| e.get("data")) {
            for (half, into) in [("stdout", &mut out), (
                "stderr", &mut err,
            )] {
                if let Some(s) = d.get(half).and_then(|x| x.as_str()) {
                    // The halves travel as base64 when they are not text
                    use base64::Engine as _;
                    match base64::engine::general_purpose::STANDARD.decode(s) {
                        Ok(raw) => into.push_str(&String::from_utf8_lossy(&raw)),
                        // Plain text is sent as itself; only bytes are wrapped
                        Err(_) => into.push_str(s),
                    }
                }
            }
        }
        if let Some(e) = msg.get("event").and_then(|e| e.get("end")) {
            let said = e.get("status").and_then(|x| x.as_str()).unwrap_or_default();
            code = Some(match said.rsplit(' ').next().and_then(|n| n.parse::<i32>().ok()) {
                Some(n) => n,
                // A signal, or words nobody planned for. Not zero, because it
                // did not end cleanly
                None => if said.is_empty() { -1 } else { 1 },
            });
        }
    }
    crate::ssh::Ran { code: code.unwrap_or(-1), out, err }
}

// -- A terminal inside a sandbox --------------------------------------------

/// What a shell in there is told it is talking to. Without this it decides it
/// is talking to a teletype and stops drawing anything worth looking at
const TERM: &str = "xterm-256color";
/// How long to wait for the shell to say it has started
const START_MS: u64 = 30_000;
/// How long a keystroke may take to arrive before we stop waiting for it. The
/// terminal is no use if a slow network can hang the window
const INPUT_MS: u64 = 10_000;

/// What the writing thread is asked to do. Typing and resizing go down the
/// same queue because their order matters: a program redrawing for a width
/// that has not arrived yet paints the wrong screen
enum Note {
    Typed(Vec<u8>),
    Size { rows: u16, cols: u16 },
    Ended,
}

/// The reading half of a terminal in a sandbox.
///
/// A queue, for the same reason ssh's is one: everything above reads with
/// `std::io::Read` on a thread of its own and knows nothing about HTTP
struct PtyReader {
    rx: std::sync::mpsc::Receiver<Vec<u8>>,
    rest: Vec<u8>,
    at: usize,
}

impl std::io::Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at >= self.rest.len() {
            match self.rx.recv() {
                Ok(chunk) => {
                    self.rest = chunk;
                    self.at = 0;
                }
                // The machine has gone. Zero is how every reader above says
                // "that was the end", the same as a local program exiting
                Err(_) => return Ok(0),
            }
        }
        let n = (self.rest.len() - self.at).min(buf.len());
        buf[..n].copy_from_slice(&self.rest[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

impl std::fmt::Debug for PtyReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PtyReader")
    }
}

/// The writing half. Typing is handed to the thread that owns the connection,
/// so a slow network never reaches the window
struct PtyWriter {
    to_far_end: std::sync::mpsc::Sender<Note>,
}

impl std::io::Write for PtyWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = self.to_far_end.send(Note::Typed(buf.to_vec()));
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A terminal on a machine that did not exist this morning, wearing the same
/// face as one on this one.
#[derive(Debug)]
pub struct SandboxPty {
    size: std::sync::Mutex<portable_pty::PtySize>,
    reader: std::sync::Mutex<Option<PtyReader>>,
    to_far_end: std::sync::mpsc::Sender<Note>,
    writer_taken: std::sync::atomic::AtomicBool,
}

impl portable_pty::MasterPty for SandboxPty {
    fn resize(&self, size: portable_pty::PtySize) -> Result<()> {
        if let Ok(mut s) = self.size.lock() {
            *s = size;
        }
        let _ = self.to_far_end.send(Note::Size { rows: size.rows, cols: size.cols });
        Ok(())
    }

    fn get_size(&self) -> Result<portable_pty::PtySize> {
        Ok(*self.size.lock().map_err(|_| anyhow!("size"))?)
    }

    fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>> {
        // Once: there is one queue of bytes from over there, and two readers
        // of it would each get half a screen
        match self.reader.lock().map_err(|_| anyhow!("reader"))?.take() {
            Some(r) => Ok(Box::new(r)),
            None => bail!("the reader for this terminal has already been taken"),
        }
    }

    fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>> {
        use std::sync::atomic::Ordering;
        if self.writer_taken.swap(true, Ordering::SeqCst) {
            bail!("the writer for this terminal has already been taken");
        }
        Ok(Box::new(PtyWriter { to_far_end: self.to_far_end.clone() }))
    }

    /// Three questions a unix caller may ask of a local pty, none of which has
    /// an answer for a terminal that is reached over HTTP. Saying so is the
    /// honest answer; inventing a number would have something signal a process
    /// on this machine that happens to share it
    #[cfg(unix)]
    fn process_group_leader(&self) -> Option<i32> {
        None
    }

    #[cfg(unix)]
    fn as_raw_fd(&self) -> Option<std::os::fd::RawFd> {
        None
    }

    #[cfg(unix)]
    fn tty_name(&self) -> Option<std::path::PathBuf> {
        None
    }
}

/// Ending it. There is no process here to kill, so this asks the far end to
/// kill its own and stops the thread that was listening
#[derive(Debug, Clone)]
pub struct SandboxKiller {
    to_far_end: std::sync::mpsc::Sender<Note>,
}

impl portable_pty::ChildKiller for SandboxKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        ENDING.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.to_far_end.send(Note::Ended).is_err() {
            // Its typing thread has already gone, and with it the shell
            ENDING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

/// Shells there that were told to end and have not been ended yet.
///
/// A shell on a MicroVM does not end when this program does: the far end keeps
/// it, and whatever runs in it, until it is told. Telling it is a request on
/// the typing thread, and a program that quits straight after asking can be
/// gone before the request is -- which left the AI of every tab running on the
/// machine, and the next start put a second one on the same conversation
static ENDING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Wait, for at most `most`, for every shell told to end to have been ended.
/// Asked once, as this program closes
pub fn settle(most: Duration) {
    let until = std::time::Instant::now() + most;
    while ENDING.load(std::sync::atomic::Ordering::SeqCst) > 0 && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A name for this terminal, which a listing of what runs on the machine shows
fn a_tag() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("shikisha-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// One terminal's link to its machine, shared by the threads that listen,
/// type, and wake it.
///
/// A machine untouched for its minutes pauses, and the stream carrying this
/// terminal ends with it -- while the shell in there, and whatever runs in
/// it, sleep with the machine and are there again when it wakes. So the
/// terminal is not ended when its stream is: it says so on screen, and the
/// next thing typed wakes the machine and takes the same process up again,
/// by the id the far end gave it. Only a process that is gone gets a new
/// shell in its place. Nothing wakes a machine by itself: a terminal left
/// open is not somebody working in it
struct Link {
    host: crate::config::HostSpec,
    sandbox: std::sync::Mutex<Sandbox>,
    /// A name for the shell, which a listing of what runs there shows
    tag: String,
    /// The shell's process id there, once the far end has said it. What
    /// every call names it by: asked by tag, the far end answered "no
    /// process with that tag" for a shell it listed under that very tag
    pid: std::sync::Mutex<Option<u32>>,
    cwd: Option<String>,
    /// What to type once a new shell stands in the folder (`claude`)
    then: Option<String>,
    out: std::sync::mpsc::Sender<Vec<u8>>,
    /// Whether the stream is up now
    open: std::sync::atomic::AtomicBool,
    /// Whether this terminal was ended from here: nothing is woken after
    ended: std::sync::atomic::AtomicBool,
    /// When the stream last said anything -- output, or the keepalive the
    /// far end sends every fifty seconds when there is none. A stream silent
    /// for far longer than that has died without saying so: a link through a
    /// proxy can be dropped with no word to this end, and a read on it waits
    /// forever. The watch below takes such a stream up again
    last_frame: std::sync::Mutex<std::time::Instant>,
    /// Which stream is the current one. A stream superseded by a fresh one
    /// may still be blocked in a read; anything it says afterwards is stale
    generation: std::sync::atomic::AtomicU64,
}

/// How long a stream may say nothing before it is taken for dead. The far
/// end speaks every fifty seconds at the least
const SILENT_FOR: Duration = Duration::from_secs(130);
/// How often the watch looks
const WATCH_EVERY: Duration = Duration::from_secs(15);

impl Link {
    fn is_open(&self) -> bool {
        self.open.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn set_open(&self, open: bool) {
        self.open.store(open, std::sync::atomic::Ordering::Relaxed);
    }
    fn is_ended(&self) -> bool {
        self.ended.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn sandbox(&self) -> Sandbox {
        self.sandbox.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn heard(&self) {
        *self.last_frame.lock().unwrap_or_else(|e| e.into_inner()) = std::time::Instant::now();
    }
    fn silent_for(&self) -> Duration {
        self.last_frame.lock().unwrap_or_else(|e| e.into_inner()).elapsed()
    }
    fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }
    /// A fresh stream is about to be the current one
    fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }
    fn pid(&self) -> Option<u32> {
        *self.pid.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Which process, the way a call names it: `{"pid": n}`
    fn at(&self) -> Option<serde_json::Value> {
        self.pid().map(|p| serde_json::json!({ "pid": p }))
    }
    /// A line of this program's own on the screen, told apart from the
    /// program's by being dim and in brackets
    fn say(&self, text: &str) {
        let _ = self.out.send(format!("\r\n\x1b[2m[{text}]\x1b[0m\r\n").into_bytes());
    }
}

/// How a stream is asked for: a new shell, or the one already there
enum Stream {
    Start { rows: u16, cols: u16 },
    Connect,
}

/// Open a terminal in a sandbox.
///
/// Returns the pair a tab needs and nothing else, the same as
/// [`crate::ssh::shell`], so that a tab's own code reads the same whether the
/// shell is here, on a server, or on a machine that was made a second ago.
/// The machine is asked for here, by the entry naming it, so that a machine
/// which pauses under the terminal can be asked for again (see [`Link`])
pub fn shell(
    host: &crate::config::HostSpec,
    rows: u16,
    cols: u16,
    cwd: Option<&str>,
    then: Option<&str>,
) -> Result<(Box<dyn portable_pty::MasterPty + Send>, Box<dyn portable_pty::ChildKiller + Send + Sync>)>
{
    let sandbox = machine(host)?;
    let (out_tx, out_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let (up_tx, up_rx) = std::sync::mpsc::channel::<Result<u32>>();
    let (note_tx, note_rx) = std::sync::mpsc::channel::<Note>();
    let link = std::sync::Arc::new(Link {
        host: host.clone(),
        sandbox: std::sync::Mutex::new(sandbox),
        tag: a_tag(),
        pid: std::sync::Mutex::new(None),
        cwd: cwd.map(str::to_string),
        then: then.map(str::to_string),
        out: out_tx,
        open: std::sync::atomic::AtomicBool::new(false),
        ended: std::sync::atomic::AtomicBool::new(false),
        last_frame: std::sync::Mutex::new(std::time::Instant::now()),
        generation: std::sync::atomic::AtomicU64::new(0),
    });

    // The listening thread. It holds the streaming response open for as long
    // as the shell lives, which is why it cannot be the thread anything else
    // is waiting on
    let l = std::sync::Arc::clone(&link);
    let stream_no = link.next_generation();
    std::thread::Builder::new().name("e2b-pty".into()).spawn(move || {
        pump(&l, Stream::Start { rows, cols }, Some(&up_tx), stream_no);
    })?;

    // The watch. A stream that has said nothing for far longer than the far
    // end's keepalive has died without a word; when the machine is running,
    // the same shell is taken up again on a fresh stream, and the person
    // sees their terminal go on. A paused machine is left paused -- the
    // watch asks the service's records, never the machine -- and the link is
    // marked closed for the next thing typed to wake it
    let l = std::sync::Arc::clone(&link);
    std::thread::Builder::new().name("e2b-pty-watch".into()).spawn(move || {
        loop {
            std::thread::sleep(WATCH_EVERY);
            if l.is_ended() || std::sync::Arc::strong_count(&l) <= 1 {
                return;
            }
            if !l.is_open() || l.silent_for() < SILENT_FOR {
                continue;
            }
            let id = l.sandbox().id;
            let Some(key) = key() else { continue };
            match state_of(&key, &id).as_deref() {
                Ok("running") => {
                    crate::append_hook_log(&format!(
                        "e2b: the stream of {} on {} has been silent for {}s; taking the shell up again",
                        l.tag,
                        id,
                        l.silent_for().as_secs()
                    ));
                    l.set_open(false);
                    let (up_tx, up_rx) = std::sync::mpsc::channel::<Result<u32>>();
                    let again = std::sync::Arc::clone(&l);
                    let stream_no = l.next_generation();
                    let spawned = std::thread::Builder::new().name("e2b-pty".into()).spawn(move || {
                        pump(&again, Stream::Connect, Some(&up_tx), stream_no);
                    });
                    let taken = spawned.is_ok()
                        && up_rx.recv_timeout(Duration::from_millis(START_MS)).is_ok_and(|r| r.is_ok());
                    if !taken {
                        crate::append_hook_log(&format!("e2b: the shell {} on {} could not be taken up again", l.tag, id));
                        l.say(&crate::i18n::t("msg.microvm.slept"));
                    }
                }
                Ok(state) => {
                    crate::append_hook_log(&format!(
                        "e2b: the stream of {} on {} is silent and the machine is {state}; the next thing typed wakes it",
                        l.tag, id
                    ));
                    l.set_open(false);
                    l.say(&crate::i18n::t("msg.microvm.slept"));
                }
                Err(e) => crate::append_hook_log(&format!("e2b: could not ask after {id}: {e:#}")),
            }
        }
    })?;

    // The typing thread. One call per note, in the order they were made --
    // and the one that wakes a paused machine, since typing is what asks
    let l = std::sync::Arc::clone(&link);
    std::thread::Builder::new().name("e2b-pty-in".into()).spawn(move || {
        let mut size = (rows, cols);
        while let Ok(note) = note_rx.recv() {
            match note {
                Note::Typed(bytes) => {
                    if !l.is_open() && !l.is_ended() {
                        crate::append_hook_log(&format!("e2b: waking {} for a terminal typed into", l.sandbox().id));
                        if let Err(e) = wake(&l, size) {
                            crate::append_hook_log(&format!("e2b: could not wake {}: {e:#}", l.sandbox().id));
                            l.say(&crate::i18n::tp("msg.microvm.wake_failed", &[("e", &format!("{e:#}"))]));
                            continue;
                        }
                    }
                    // What was typed and did not arrive is said, not dropped:
                    // a terminal that silently ate its first line looks like
                    // one that was never asked for anything
                    let Some(at) = l.at() else { continue };
                    if let Err(e) = send_input(&l.sandbox(), &at, &bytes) {
                        crate::append_hook_log(&format!("e2b: typing into shell {at} failed: {e:#}"));
                    }
                }
                Note::Size { rows, cols } => {
                    size = (rows, cols);
                    if l.is_open()
                        && let Some(at) = l.at()
                    {
                        let _ = resize(&l.sandbox(), &at, rows, cols);
                    }
                }
                Note::Ended => {
                    l.ended.store(true, std::sync::atomic::Ordering::Relaxed);
                    if l.is_open()
                        && let Some(at) = l.at()
                        && let Err(e) = signal(&l.sandbox(), &at, "SIGNAL_SIGKILL")
                    {
                        crate::append_hook_log(&format!("e2b: ending shell {at} failed: {e:#}"));
                    }
                    ENDING.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    return;
                }
            }
        }
    })?;

    // Nothing is handed back until the shell is actually there. A tab given a
    // terminal that never opened shows an empty screen and no reason for it
    up_rx
        .recv_timeout(Duration::from_millis(START_MS))
        .map_err(|_| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", "the terminal did not open")])))??;
    // The shell was started in the folder, so only the program is typed --
    // through the typing thread, ahead of anything a person types after
    if let Some(line) = crate::ssh::typed_first(None, then) {
        let _ = note_tx.send(Note::Typed(line.into_bytes()));
    }

    let pty = SandboxPty {
        size: std::sync::Mutex::new(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        }),
        reader: std::sync::Mutex::new(Some(PtyReader { rx: out_rx, rest: Vec::new(), at: 0 })),
        to_far_end: note_tx.clone(),
        writer_taken: std::sync::atomic::AtomicBool::new(false),
    };
    Ok((Box::new(pty), Box::new(SandboxKiller { to_far_end: note_tx })))
}

/// Wake the machine under a terminal whose stream has ended, and take the
/// terminal up again: the same shell when it is still there (the machine
/// slept with it), a new one in the same folder when it is not
fn wake(link: &std::sync::Arc<Link>, size: (u16, u16)) -> Result<()> {
    let id = link.sandbox().id.clone();
    if let_go_of(&id) {
        bail!(crate::i18n::tp("err.e2b.let_go", &[("host", &link.host.name), ("id", &id)]));
    }
    let key = key().ok_or_else(|| anyhow!(crate::i18n::t("err.e2b.no_key")))?;
    let woken = connect(&key, &id, link.host.minutes_or_default())?;
    *link.sandbox.lock().unwrap_or_else(|e| e.into_inner()) = woken.clone();
    // Whether the shell is still there: asked by sizing it, which is a
    // question the far end answers at once and refuses for a process gone
    let same = link.at().is_some_and(|at| resize(&woken, &at, size.0, size.1).is_ok());
    crate::append_hook_log(&format!(
        "e2b: {} is awake; the shell {} is {}",
        woken.id,
        link.tag,
        if same { "still there" } else { "gone, a new one opens" }
    ));
    let how = match same {
        true => Stream::Connect,
        false => Stream::Start { rows: size.0, cols: size.1 },
    };
    let (up_tx, up_rx) = std::sync::mpsc::channel::<Result<u32>>();
    let l = std::sync::Arc::clone(link);
    let stream_no = link.next_generation();
    std::thread::Builder::new().name("e2b-pty".into()).spawn(move || {
        pump(&l, how, Some(&up_tx), stream_no);
    })?;
    up_rx
        .recv_timeout(Duration::from_millis(START_MS))
        .map_err(|_| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", "the terminal did not open")])))??;
    match same {
        true => link.say(&crate::i18n::t("msg.microvm.woke")),
        false => {
            link.say(&crate::i18n::t("msg.microvm.new_shell"));
            if let Some(line) = crate::ssh::typed_first(None, link.then.as_deref())
                && let Some(at) = link.at()
            {
                let _ = send_input(&link.sandbox(), &at, line.as_bytes());
            }
        }
    }
    Ok(())
}

/// Hold a stream open and pass on what comes out of it.
///
/// For a new shell, the first thing the far end says is that it has started,
/// with the id everything after this is sent to, and that is what releases
/// whoever asked for it (`up`); a shell taken up again is there the moment
/// the far end answers. Everything after is screen. When the stream ends and
/// this terminal was not ended from here, the machine has paused under it:
/// said on screen, and the link is left for the next thing typed to wake
fn pump(link: &Link, how: Stream, up: Option<&std::sync::mpsc::Sender<Result<u32>>>, stream_no: u64) {
    // Whether this stream is still the current one. One taken for dead and
    // superseded may come back to life; what it says then is stale
    let current = || link.generation() == stream_no;
    let tell_up = |r: Result<u32>| {
        if let Some(u) = up {
            let _ = u.send(r);
        }
    };
    let (method, body) = match &how {
        Stream::Start { rows, cols } => (
            "process.Process/Start",
            serde_json::json!({
                "process": {
                    // A login shell, because a person opening a terminal
                    // expects their own profile to have been read -- the
                    // same thing ssh gives them
                    "cmd": "/bin/bash",
                    "args": ["-i", "-l"],
                    "envs": { "TERM": TERM },
                    "cwd": link.cwd,
                },
                "pty": { "size": { "cols": *cols as u32, "rows": *rows as u32 } },
                "tag": link.tag,
                "stdin": true,
            }),
        ),
        Stream::Connect => {
            let Some(at) = link.at() else {
                tell_up(Err(anyhow!(crate::i18n::tp("err.e2b.call", &[("e", "there is no shell to take up")]))));
                return;
            };
            ("process.Process/Connect", serde_json::json!({ "process": at }))
        }
    };
    let sandbox = link.sandbox();
    // No overall deadline on this one: the whole point of it is to stay open.
    // Everything else in this module still has one
    let agent = ureq::Agent::config_builder()
        .timeout_global(None)
        .build()
        .new_agent();
    let sent = serde_json::to_vec(&body).map(|b| frame(&b));
    let resp = match sent {
        Ok(b) => headed(agent.post(&format!("{SANDBOX}/{method}")), &sandbox)
            .header("Keepalive-Ping-Interval", KEEPALIVE)
            .header("Content-Type", "application/connect+json")
            .send(b),
        Err(e) => {
            tell_up(Err(anyhow!("{e}")));
            return;
        }
    };
    let mut resp = match resp {
        Ok(r) => r,
        Err(e) => {
            tell_up(Err(anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))]))));
            return;
        }
    };
    let mut opened = matches!(how, Stream::Connect);
    if opened {
        link.heard();
        link.set_open(true);
        tell_up(Ok(link.pid().unwrap_or_default()));
    }
    let mut reader = resp.body_mut().as_reader();
    let mut held: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    let why = loop {
        let n = match std::io::Read::read(&mut reader, &mut buf) {
            Ok(0) => break "the far end closed the stream".to_string(),
            Err(e) => break format!("{e}"),
            Ok(n) => n,
        };
        if !current() {
            return;
        }
        held.extend_from_slice(&buf[..n]);
        for msg in whole_frames(&mut held) {
            // Anything at all, the keepalive included, is the stream alive
            link.heard();
            let event = msg.get("event");
            if !opened && let Some(start) = event.and_then(|e| e.get("start")) {
                opened = true;
                // The id everything after this is sent to. A start that does
                // not carry one is a shell nothing could ever be typed into
                match start.get("pid").and_then(|p| p.as_u64()).map(|p| p as u32) {
                    Some(pid) => {
                        *link.pid.lock().unwrap_or_else(|e| e.into_inner()) = Some(pid);
                        link.set_open(true);
                        tell_up(Ok(pid));
                    }
                    None => {
                        tell_up(Err(anyhow!(crate::i18n::tp(
                            "err.e2b.call",
                            &[("e", "the shell started without an id")]
                        ))));
                        return;
                    }
                }
            }
            if let Some(s) = event.and_then(|e| e.get("data")).and_then(|d| d.get("pty"))
                && let Some(bytes) = unwrap_bytes(s)
                // Nobody is listening any more: the tab has gone
                && link.out.send(bytes).is_err()
            {
                link.set_open(false);
                return;
            }
            if event.and_then(|e| e.get("end")).is_some() {
                if !opened {
                    tell_up(Err(anyhow!(crate::i18n::tp(
                        "err.e2b.call",
                        &[("e", "the shell ended before it started")]
                    ))));
                }
                // The shell itself ended: what a person typing `exit` gets,
                // and a tab reads it as the program ending
                crate::append_hook_log(&format!("e2b: the shell {} on {} ended", link.tag, sandbox.id));
                link.set_open(false);
                link.ended.store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
    };
    // A stream already superseded ending is nothing: the current one stands
    if !current() {
        return;
    }
    link.set_open(false);
    crate::append_hook_log(&format!(
        "e2b: the stream of {} on {} ended: {why} (opened: {opened}, ended from here: {})",
        link.tag,
        sandbox.id,
        link.is_ended()
    ));
    if !opened {
        tell_up(Err(anyhow!(crate::i18n::tp(
            "err.e2b.call",
            &[("e", "the machine closed the connection")]
        ))));
        return;
    }
    // The stream went, and not because this terminal ended: the machine
    // has paused under it, or the link dropped. Said on screen; the next
    // thing typed wakes it
    if !link.is_ended() {
        link.say(&crate::i18n::t("msg.microvm.slept"));
    }
}

/// Every whole message at the front of what has been read, taken out of it.
///
/// A read off a socket is a length of bytes, not a message: one read can carry
/// three frames and half of a fourth, and the next carries the rest of it. The
/// half is left in place for the next time round, which is the whole reason
/// this is not just a loop over one buffer
fn whole_frames(held: &mut Vec<u8>) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 5 <= held.len() {
        let len =
            u32::from_be_bytes([held[at + 1], held[at + 2], held[at + 3], held[at + 4]]) as usize;
        if at + 5 + len > held.len() {
            break;
        }
        out.push(serde_json::from_slice(&held[at + 5..at + 5 + len]).unwrap_or_default());
        at += 5 + len;
    }
    held.drain(..at);
    out
}

/// Bytes on this wire travel as base64, except when they are plain text, which
/// is sent as itself
fn unwrap_bytes(v: &serde_json::Value) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let s = v.as_str()?;
    Some(match base64::engine::general_purpose::STANDARD.decode(s) {
        Ok(raw) => raw,
        Err(_) => s.as_bytes().to_vec(),
    })
}

/// The three things every call to a sandbox has to say: which sandbox, which
/// port inside it, and that we are allowed to ask
fn headed(req: ureq::RequestBuilder<ureq::typestate::WithBody>, sandbox: &Sandbox) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
    let req = req
        .header("Connect-Protocol-Version", "1")
        .header("E2b-Sandbox-Id", &sandbox.id)
        .header("E2b-Sandbox-Port", &AGENT_PORT.to_string());
    match sandbox.token.as_deref() {
        Some(t) => req.header("X-Access-Token", t),
        None => req,
    }
}

/// One call that asks for nothing back. Unary on this protocol is plain JSON
/// with no envelope, unlike the streams
fn tell(sandbox: &Sandbox, method: &str, body: &serde_json::Value) -> Result<()> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(INPUT_MS)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut resp = headed(agent.post(&format!("{SANDBOX}/{method}")), sandbox)
        .header("Content-Type", "application/json")
        .send(serde_json::to_vec(body)?)
        .map_err(|e| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))])))?;
    // A refusal says why in its body, and the why is the part worth keeping
    if resp.status().as_u16() >= 400 {
        let why = resp.body_mut().read_to_string().unwrap_or_default();
        let said = format!("{} {}", resp.status(), why.trim());
        bail!(crate::i18n::tp("err.e2b.call", &[("e", &said)]));
    }
    Ok(())
}

/// `at` is which process: `{"pid": n}`
fn send_input(sandbox: &Sandbox, at: &serde_json::Value, bytes: &[u8]) -> Result<()> {
    use base64::Engine as _;
    let typed = base64::engine::general_purpose::STANDARD.encode(bytes);
    tell(
        sandbox,
        "process.Process/SendInput",
        &serde_json::json!({ "process": at, "input": { "pty": typed } }),
    )
}

fn resize(sandbox: &Sandbox, at: &serde_json::Value, rows: u16, cols: u16) -> Result<()> {
    tell(
        sandbox,
        "process.Process/Update",
        &serde_json::json!({
            "process": at,
            "pty": { "size": { "cols": cols as u32, "rows": rows as u32 } },
        }),
    )
}

fn signal(sandbox: &Sandbox, at: &serde_json::Value, which: &str) -> Result<()> {
    tell(
        sandbox,
        "process.Process/SendSignal",
        &serde_json::json!({ "process": at, "signal": which }),
    )
}

// -- Files on a machine in the cloud ----------------------------------------

/// The account a sandbox hands out by default. Everything is done as this
/// person, the same one a terminal in there comes up as
const AS_WHOM: &str = "user";

/// Do something with files on a sandbox.
///
/// The same jobs and the same answers as [`crate::ssh::files`], over an
/// entirely different wire: a Connect service for the questions that are about
/// names, and plain HTTP for the two that are about contents. Blocks, like
/// every other file call in this program; whoever asks decides how long they
/// are willing to wait
pub fn files(
    sandbox: &Sandbox,
    job: crate::ssh::FileJob,
    wait_ms: u64,
) -> Result<crate::ssh::FileAnswer> {
    use crate::ssh::{FileAnswer, FileJob};
    match job {
        FileJob::List { path } => {
            // One level. A listing is a folder being looked at, not a search:
            // asking for everything underneath would pull a whole source tree
            // across to draw one panel
            let said = ask(
                sandbox,
                "filesystem.Filesystem/ListDir",
                &serde_json::json!({ "path": path, "depth": 1 }),
                wait_ms,
            )?;
            let rows = said
                .get("entries")
                .and_then(|e| e.as_array())
                .map(|a| a.iter().filter_map(entry_of).collect())
                .unwrap_or_default();
            Ok(FileAnswer::Listing(rows))
        }
        FileJob::Stat { path } => {
            let said = ask(
                sandbox,
                "filesystem.Filesystem/Stat",
                &serde_json::json!({ "path": path }),
                wait_ms,
            )?;
            said.get("entry")
                .and_then(entry_of)
                .map(FileAnswer::One)
                .ok_or_else(|| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &path)])))
        }
        FileJob::MakeDir { path } => {
            ask(
                sandbox,
                "filesystem.Filesystem/MakeDir",
                &serde_json::json!({ "path": path }),
                wait_ms,
            )?;
            Ok(FileAnswer::Nothing)
        }
        FileJob::Rename { from, to } => {
            ask(
                sandbox,
                "filesystem.Filesystem/Move",
                &serde_json::json!({ "source": from, "destination": to }),
                wait_ms,
            )?;
            Ok(FileAnswer::Nothing)
        }
        FileJob::Remove { path } => {
            ask(
                sandbox,
                "filesystem.Filesystem/Remove",
                &serde_json::json!({ "path": path }),
                wait_ms,
            )?;
            Ok(FileAnswer::Nothing)
        }
        FileJob::Read { path } => Ok(FileAnswer::Bytes(download(sandbox, &path, wait_ms)?)),
        FileJob::Get { from, to, overwrite } => {
            if !overwrite && to.exists() {
                bail!(crate::i18n::tp(
                    "err.ssh.file_exists",
                    &[("path", &to.display().to_string())]
                ));
            }
            let bytes = download(sandbox, &from, wait_ms)?;
            if let Some(d) = to.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&to, bytes)?;
            Ok(FileAnswer::Nothing)
        }
        FileJob::Put { from, to, overwrite } => {
            // The far end overwrites without being asked, so the refusing is
            // done here. Asked first rather than after, because after is too
            // late: the file it would have reported on is already gone
            if !overwrite
                && let Ok(FileAnswer::One(_)) =
                    files(sandbox, FileJob::Stat { path: to.clone() }, wait_ms)
            {
                // The same sentence a server gives for the same refusal. One
                // situation reads one way whichever kind of machine it is, and
                // this one is not about a sandbox at all -- the file is there
                bail!(crate::i18n::tp("err.ssh.file_exists", &[("path", &to)]));
            }
            upload(sandbox, &std::fs::read(&from)?, &to, wait_ms)?;
            Ok(FileAnswer::Nothing)
        }
        FileJob::Write { to, bytes } => {
            upload(sandbox, &bytes, &to, wait_ms)?;
            Ok(FileAnswer::Nothing)
        }
    }
}

/// One call that answers with something. Unary on this protocol is plain JSON
/// with no envelope, unlike the streams
fn ask(
    sandbox: &Sandbox,
    method: &str,
    body: &serde_json::Value,
    wait_ms: u64,
) -> Result<serde_json::Value> {
    let mut resp = headed(
        waiting(wait_ms).post(&format!("{SANDBOX}/{method}")),
        sandbox,
    )
    .header("Content-Type", "application/json")
    .send(serde_json::to_vec(body)?)
    .map_err(|e| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))])))?;
    let said = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))])))?;
    Ok(serde_json::from_str(&said).unwrap_or_default())
}

/// Bring a file here, whole. Contents do not go through the Connect service:
/// there is an HTTP door for them, and it is the one that was measured to
/// carry bytes unchanged
fn download(sandbox: &Sandbox, from: &str, wait_ms: u64) -> Result<Vec<u8>> {
    let mut resp = headed_get(
        waiting(wait_ms).get(&format!("{SANDBOX}/files?path={}&username={AS_WHOM}", escaped(from))),
        sandbox,
    )
    .call()
    .map_err(|e| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))])))?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut resp.body_mut().as_reader(), &mut out)?;
    Ok(out)
}

/// Send a file there, whole. Raw rather than a form: the door takes both, and
/// a form would put a boundary in the middle of somebody's binary
fn upload(sandbox: &Sandbox, what: &[u8], to: &str, wait_ms: u64) -> Result<()> {
    headed(
        waiting(wait_ms)
            .post(&format!("{SANDBOX}/files?path={}&username={AS_WHOM}", escaped(to))),
        sandbox,
    )
    .header("Content-Type", "application/octet-stream")
    .send(what)
    .map_err(|e| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))])))?;
    Ok(())
}

/// An agent that gives up after the caller's own deadline.
fn waiting(wait_ms: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_millis(wait_ms.max(1))))
        .build()
        .new_agent()
}

/// The same three headers as a POST, for a call that carries no body.
fn headed_get(
    req: ureq::RequestBuilder<ureq::typestate::WithoutBody>,
    sandbox: &Sandbox,
) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    let req = req
        .header("Connect-Protocol-Version", "1")
        .header("E2b-Sandbox-Id", &sandbox.id)
        .header("E2b-Sandbox-Port", &AGENT_PORT.to_string());
    match sandbox.token.as_deref() {
        Some(t) => req.header("X-Access-Token", t),
        None => req,
    }
}

/// A path as it may be written into a query.
///
/// Only what has to be: a path full of `%2F` is unreadable in a log and in an
/// error message, and the far end takes a plain slash. What cannot be left
/// alone is what would end the value or start another one
fn escaped(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One thing in a folder over there, in the shape the rest of the app knows.
fn entry_of(v: &serde_json::Value) -> Option<crate::ssh::Entry> {
    let name = v.get("name")?.as_str()?.to_string();
    Some(crate::ssh::Entry {
        name,
        dir: v.get("type").and_then(|t| t.as_str()) == Some("FILE_TYPE_DIRECTORY"),
        // A 64-bit number travels as a string on this protocol, which is what
        // the JSON mapping says to do with one. It arrives as a number often
        // enough that both are taken
        size: v
            .get("size")
            .and_then(|s| s.as_u64().or_else(|| s.as_str().and_then(|t| t.parse().ok())))
            .unwrap_or(0),
        modified: v.get("modifiedTime").and_then(|t| t.as_str()).map(epoch_of).unwrap_or(0),
    })
}

/// A time written the way this protocol writes one, as the number of seconds
/// everything else here counts in.
///
/// `2026-09-12T14:22:01.5Z`, or the same with an offset on the end. Done by
/// hand because the answer is arithmetic, not a calendar: no library is worth
/// carrying to turn one fixed shape into one number
fn epoch_of(said: &str) -> u64 {
    let num = |s: &str| s.parse::<i64>().unwrap_or(0);
    let (date, rest) = match said.split_once(['T', 't', ' ']) {
        Some(pair) => pair,
        None => return 0,
    };
    let d: Vec<&str> = date.split('-').collect();
    if d.len() != 3 {
        return 0;
    }
    // Whatever ends the clock: Z, or an offset to be taken off again
    let (clock, offset) = match rest.find(['Z', 'z', '+']) {
        Some(at) => (&rest[..at], &rest[at..]),
        // A minus can only be an offset here; the clock itself has none
        None => match rest.rfind('-') {
            Some(at) => (&rest[..at], &rest[at..]),
            None => (rest, ""),
        },
    };
    let c: Vec<&str> = clock.split(':').collect();
    if c.len() < 2 {
        return 0;
    }
    let secs = c.get(2).map(|s| num(s.split('.').next().unwrap_or("0"))).unwrap_or(0);
    let day = days_from_civil(num(d[0]), num(d[1]), num(d[2]));
    let mut total = day * 86_400 + num(c[0]) * 3_600 + num(c[1]) * 60 + secs;
    // An offset says what was added to get that clock, so it comes back off
    if let Some(sign) = offset.chars().next()
        && (sign == '+' || sign == '-')
    {
        let o: Vec<&str> = offset[1..].split(':').collect();
        let away = num(o[0]) * 3_600 + o.get(1).map(|m| num(m) * 60).unwrap_or(0);
        total += if sign == '+' { -away } else { away };
    }
    total.max(0) as u64
}

/// Days between a date and 1970-01-01, by Howard Hinnant's method: the leap
/// year rule repeats every 400 years, so counting whole eras and the days
/// inside one needs no table and no loop
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    /// A key the service turns away is said as a key to fix in the settings
    #[test]
    fn a_refused_key_is_said_as_the_key() {
        assert_eq!(call_failed(&ureq::Error::StatusCode(401)), crate::i18n::t("err.e2b.bad_key"));
        assert_eq!(call_failed(&ureq::Error::StatusCode(403)), crate::i18n::t("err.e2b.bad_key"));
        assert_ne!(call_failed(&ureq::Error::StatusCode(500)), crate::i18n::t("err.e2b.bad_key"));
    }

    /// Only a machine carrying this app's mark is called this app's: the
    /// key can be another program's too, and its machines are its own
    #[test]
    fn a_machine_is_ours_by_its_mark() {
        let with = |marks: &[(&str, &str)]| Listed {
            id: "m".into(),
            state: "paused".into(),
            marks: marks.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            started: String::new(),
        };
        assert!(with(&[(MARK, "1"), ("project", "site")]).ours());
        assert!(!with(&[("project", "site")]).ours(), "a machine with no mark is taken for this app's");
        assert!(!with(&[(MARK, "0")]).ours());
    }

    /// A machine being deleted is not spoken to again, so nothing still
    /// holding it -- an editor with a file open -- can wake it. Refused here,
    /// before a key is looked for or a request made
    #[test]
    fn a_machine_let_go_of_is_not_woken() {
        let id = "let-go-test-machine";
        let host = crate::config::HostSpec {
            name: "vm".into(),
            instance: Some(id.into()),
            ..Default::default()
        };
        let_go(id);
        let err = format!("{:#}", machine(&host).unwrap_err());
        assert!(err.contains(id) && !err.contains("could not be reached"), "{err}");
        take_back(id);
        assert!(!let_go_of(id), "a machine put back in the list is still refused");
    }

    use super::*;

    /// A read off a socket is a length of bytes, not a message. Proving it at
    /// every possible split is the only way to be sure: a terminal that drops
    /// whatever straddled a read boundary loses characters at random, which is
    /// the kind of fault nobody can reproduce on purpose
    #[test]
    fn a_message_split_across_reads_still_arrives_whole() {
        let says = |t: &str| {
            frame(
                serde_json::to_vec(&serde_json::json!({ "event": { "data": { "pty": t } } }))
                    .unwrap()
                    .as_slice(),
            )
        };
        let mut stream = says("aGk=");
        stream.extend(says("dGhlcmU="));
        stream.extend(says("YmVsbA=="));

        for cut in 1..stream.len() {
            let mut held: Vec<u8> = Vec::new();
            let mut said: Vec<String> = Vec::new();
            for part in [&stream[..cut], &stream[cut..]] {
                held.extend_from_slice(part);
                for msg in whole_frames(&mut held) {
                    let raw = msg["event"]["data"]["pty"].clone();
                    said.push(String::from_utf8(unwrap_bytes(&raw).unwrap()).unwrap());
                }
            }
            assert_eq!(said, ["hi", "there", "bell"], "it dropped something at cut {cut}");
            assert!(held.is_empty(), "there is debris at cut {cut}");
        }
    }

    /// Half a frame is kept, not guessed at. A length that says more is coming
    /// must hold everything back until it does
    #[test]
    fn half_a_message_is_kept_rather_than_read() {
        let whole = frame(b"{\"event\":{}}");
        let mut held = whole[..whole.len() - 1].to_vec();
        assert!(whole_frames(&mut held).is_empty(), "it read something only partly there");
        assert_eq!(held.len(), whole.len() - 1, "it threw away what it could not read");
        held.push(whole[whole.len() - 1]);
        assert_eq!(whole_frames(&mut held).len(), 1);
    }

    /// The screen travels as base64; anything that is not gets taken as it
    /// stands rather than dropped
    #[test]
    fn the_screen_arrives_as_bytes_either_way() {
        let wrapped = serde_json::Value::String("aGVsbG8=".into());
        assert_eq!(unwrap_bytes(&wrapped).unwrap(), b"hello");
        let plain = serde_json::Value::String("~~~~".into());
        assert_eq!(unwrap_bytes(&plain).unwrap(), b"~~~~");
        assert!(unwrap_bytes(&serde_json::Value::Null).is_none());
    }

    /// A date arrives as words and has to come out as the number every other
    /// file in this program counts in. Wrong by an hour is a file panel that
    /// sorts by time and gets it backwards; wrong by a day happens at every
    /// leap year if the arithmetic is guessed at
    #[test]
    fn a_written_date_becomes_the_number_we_count_in() {
        // Known good pairs. The epoch itself, a leap day, the century that is
        // not a leap year in 2000's rule, and a plain afternoon
        for (said, want) in [
            ("1970-01-01T00:00:00Z", 0u64),
            ("2000-02-29T00:00:00Z", 951_782_400),
            ("2024-02-29T12:00:00Z", 1_709_208_000),
            ("2026-09-12T14:22:01Z", 1_789_222_921),
        ] {
            assert_eq!(epoch_of(said), want, "{said}");
        }

        // Fractional seconds are dropped, not misread as seconds
        assert_eq!(epoch_of("2026-09-12T14:22:01.5Z"), epoch_of("2026-09-12T14:22:01Z"));

        // An offset is what was added to get that clock, so it comes back off:
        // the same moment written three ways is one number
        let z = epoch_of("2026-09-12T14:22:01Z");
        assert_eq!(epoch_of("2026-09-12T23:22:01+09:00"), z, "an offset east");
        assert_eq!(epoch_of("2026-09-12T09:22:01-05:00"), z, "an offset west");

        // Nothing recognisable is zero, not a panic and not a guess
        assert_eq!(epoch_of(""), 0);
        assert_eq!(epoch_of("sometime last week"), 0);
        assert_eq!(epoch_of("2026-09-12"), 0);
    }

    /// A path goes into a query, where some characters mean something. A slash
    /// does not -- and leaving it alone is what keeps a path readable in a log
    /// and in an error message
    #[test]
    fn a_path_in_a_query_keeps_its_slashes() {
        assert_eq!(escaped("/home/user/thing.bin"), "/home/user/thing.bin");
        assert_eq!(escaped("/home/user/a b"), "/home/user/a%20b");
        // The ones that would end this value or begin another
        assert_eq!(escaped("a&b=c#d?e"), "a%26b%3Dc%23d%3Fe");
        // Not ours to guess at: anything outside ASCII goes as its bytes
        assert_eq!(escaped("\u{3042}"), "%E3%81%82");
    }

    /// What comes back is turned into the one shape the file panel knows. A
    /// 64-bit number travels as a string on this protocol, which is what the
    /// JSON mapping says to do with one -- and it arrives as a number often
    /// enough that both have to be taken
    #[test]
    fn a_thing_in_a_folder_arrives_in_our_own_shape() {
        let said = serde_json::json!({
            "name": "thing.bin",
            "type": "FILE_TYPE_FILE",
            "size": "4096",
            "modifiedTime": "2026-09-12T14:22:01Z",
        });
        let e = entry_of(&said).expect("cannot read");
        assert_eq!(e.name, "thing.bin");
        assert!(!e.dir);
        assert_eq!(e.size, 4096);
        assert_eq!(e.modified, 1_789_222_921);

        let as_number = serde_json::json!({ "name": "x", "type": "FILE_TYPE_DIRECTORY", "size": 12 });
        let d = entry_of(&as_number).expect("cannot read");
        assert!(d.dir, "it read a folder as a file");
        assert_eq!(d.size, 12);
        // Nothing said about a time is zero, not today
        assert_eq!(d.modified, 0);

        // A row with no name is not a row
        assert!(entry_of(&serde_json::json!({ "size": 1 })).is_none());
    }

    /// Every terminal in this program gets a name of its own, so a listing of
    /// what runs on a machine says which of them are this program's
    #[test]
    fn two_terminals_are_never_the_same_terminal() {
        let (a, b) = (a_tag(), a_tag());
        assert_ne!(a, b);
        assert!(a.starts_with("shikisha-"), "{a}");
    }

    /// The envelope is what the far end reads first; a wrong length is a
    /// request that hangs rather than one that fails
    #[test]
    fn a_message_goes_out_in_its_envelope() {
        let f = frame(b"{}");
        assert_eq!(f[0], 0, "the flag is set");
        assert_eq!(u32::from_be_bytes([f[1], f[2], f[3], f[4]]), 2);
        assert_eq!(&f[5..], b"{}");
    }

    /// The ending is a sentence, and both halves of the output stay apart.
    #[test]
    fn a_stream_turns_into_one_answer() {
        let mut s = Vec::new();
        s.extend(frame(br#"{"event":{"start":{"pid":1}}}"#));
        s.extend(frame(br#"{"event":{"data":{"stdout":"git version 2.0\n"}}}"#));
        s.extend(frame(br#"{"event":{"data":{"stderr":"Preparing worktree\n"}}}"#));
        s.extend(frame(br#"{"event":{"end":{"status":"exit status 0"}}}"#));
        let r = collect(&s);
        assert!(r.ok(), "{r:?}");
        assert_eq!(r.out.trim(), "git version 2.0");
        assert_eq!(r.err.trim(), "Preparing worktree", "the two streams are mixed");

        let mut bad = Vec::new();
        bad.extend(frame(br#"{"event":{"end":{"status":"exit status 128"}}}"#));
        assert_eq!(collect(&bad).code, 128, "the exit code was not taken from the words");

        // Nothing said about an ending is not a clean ending
        assert_eq!(collect(&[]).code, -1);
        assert!(!collect(&[]).ok());
    }
}

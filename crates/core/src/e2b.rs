//! Sandboxes on somebody else's machines.
//!
//! A cloud sandbox is a machine that did not exist a second ago and will not
//! exist tomorrow, which is the whole of the difference from an [`crate::ssh`]
//! host: it has to be asked for before anything can run on it. Once it is
//! there, the thing this module is for is the same one thing -- run a command
//! and hear how it went -- so the rest of the app does not have to know which
//! kind of machine it is talking to.
//!
//! Nothing here streams a terminal. That is a different job with a different
//! shape, and a program that wants to know whether git worked needs an ending
//! and an exit code, not a screen.

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

/// Ask for a machine.
///
/// `template` is the image it is built from; `minutes` is how long it lives
/// without being touched. A sandbox that is never killed still stops on its
/// own, which is the only reason it is safe to make one from a program
pub fn create(key: &str, template: &str, minutes: u32) -> Result<Sandbox> {
    let body = serde_json::json!({
        "templateID": template,
        "timeout": minutes.max(1) * 60,
    });
    let mut resp = agent()
        .post(&format!("{API}/sandboxes"))
        .header("X-API-Key", key)
        .header("Content-Type", "application/json")
        .send(serde_json::to_string(&body)?)
        .map_err(|e| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))])))?;
    let said = resp.body_mut().read_to_string()?;
    let v: serde_json::Value = serde_json::from_str(&said)
        .map_err(|_| anyhow!(crate::i18n::tp("err.e2b.said", &[("said", &said)])))?;
    let id = v
        .get("sandboxID")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!(crate::i18n::tp("err.e2b.said", &[("said", &said)])))?;
    Ok(Sandbox {
        id: id.to_string(),
        token: v.get("envdAccessToken").and_then(|x| x.as_str()).map(str::to_string),
    })
}

/// Let it go. A sandbox nobody kills still stops when its time runs out, so
/// this is tidiness rather than the only way out
pub fn kill(key: &str, id: &str) -> Result<()> {
    let resp = agent()
        .delete(&format!("{API}/sandboxes/{id}"))
        .header("X-API-Key", key)
        .call();
    match resp {
        Ok(_) => Ok(()),
        Err(e) => bail!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))])),
    }
}

/// Every sandbox this key currently has, as ids.
pub fn list(key: &str) -> Result<Vec<String>> {
    let mut resp = agent()
        .get(&format!("{API}/v2/sandboxes"))
        .header("X-API-Key", key)
        .call()
        .map_err(|e| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))])))?;
    let said = resp.body_mut().read_to_string()?;
    let v: serde_json::Value = serde_json::from_str(&said).unwrap_or_default();
    // The list has been in two shapes; take whichever this one is
    let rows = v
        .get("sandboxes")
        .and_then(|x| x.as_array())
        .or_else(|| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(rows
        .iter()
        .filter_map(|r| r.get("sandboxID").and_then(|x| x.as_str()).map(str::to_string))
        .collect())
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
    let mut req = agent()
        .post(&format!("{SANDBOX}/process.Process/Start"))
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
                None => said.is_empty().then_some(-1).unwrap_or(1),
            });
        }
    }
    crate::ssh::Ran { code: code.unwrap_or(-1), out, err }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The envelope is what the far end reads first; a wrong length is a
    /// request that hangs rather than one that fails
    #[test]
    fn a_message_goes_out_in_its_envelope() {
        let f = frame(b"{}");
        assert_eq!(f[0], 0, "旗が立っている");
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
        assert_eq!(r.err.trim(), "Preparing worktree", "二つの流れが混ざっている");

        let mut bad = Vec::new();
        bad.extend(frame(br#"{"event":{"end":{"status":"exit status 128"}}}"#));
        assert_eq!(collect(&bad).code, 128, "終了コードが言葉から取れていない");

        // Nothing said about an ending is not a clean ending
        assert_eq!(collect(&[]).code, -1);
        assert!(!collect(&[]).ok());
    }
}

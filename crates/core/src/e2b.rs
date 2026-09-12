//! Sandboxes on somebody else's machines.
//!
//! A cloud sandbox is a machine that did not exist a second ago and will not
//! exist tomorrow, which is the whole of the difference from an [`crate::ssh`]
//! host: it has to be asked for before anything can run on it. Once it is
//! there, the thing this module is for is the same one thing -- run a command
//! and hear how it went -- so the rest of the app does not have to know which
//! kind of machine it is talking to.
//!
//! Two jobs live here, because they are the same two a machine is wanted for.
//! [`exec`] runs one command and says how it went -- an ending and an exit
//! code, which is what a program asking whether git worked needs. [`shell`]
//! opens a terminal and never ends, which is what a person needs. The second
//! wears [`portable_pty::MasterPty`] so that a tab cannot tell it from a
//! program running on this machine, exactly as [`crate::ssh`] does.

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

// -- A terminal inside a sandbox --------------------------------------------

/// What a shell in there is told it is talking to. Without this it decides it
/// is talking to a teletype and stops drawing anything worth looking at
const TERM: &str = "xterm-256color";
/// How long to wait for the shell to say it has started
const START_MS: u64 = 30_000;
/// How long a keystroke may take to arrive before we stop waiting for it. The
/// terminal is no use if a slow network can hang the window
const INPUT_MS: u64 = 10_000;

/// The machine for this entry in the settings, made if it is not there yet.
///
/// One per name, for the life of this program: a sandbox costs money by the
/// minute, and "open a second tab" must not mean "rent a second machine".
/// Every path that wants a machine comes through here for that reason
pub fn sandbox_for(host: &crate::config::HostSpec, image: Option<&str>) -> Result<Sandbox> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static LIVE: OnceLock<Mutex<HashMap<String, Sandbox>>> = OnceLock::new();
    let live = LIVE.get_or_init(Default::default);
    if let Some(s) = live.lock().ok().and_then(|m| m.get(&host.name).cloned()) {
        return Ok(s);
    }
    let key = key().ok_or_else(|| anyhow!(crate::i18n::t("err.e2b.no_key")))?;
    let template = image
        .map(str::trim)
        .filter(|i| !i.is_empty())
        .or_else(|| host.template.as_deref().map(str::trim).filter(|t| !t.is_empty()))
        .unwrap_or("base");
    let made = create(&key, template, host.minutes.unwrap_or(30))?;
    if let Ok(mut m) = live.lock() {
        m.insert(host.name.clone(), made.clone());
    }
    Ok(made)
}

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
        let _ = self.to_far_end.send(Note::Ended);
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(self.clone())
    }
}

/// A name for this terminal that the far end will answer to.
///
/// Used instead of the process id so that nothing has to wait for the id to
/// come back before it can type: the name is decided here, before the shell
/// exists, and every later call names it
fn a_tag() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!("shikisha-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Open a terminal in a sandbox.
///
/// Returns the pair a tab needs and nothing else, the same as
/// [`crate::ssh::shell`], so that a tab's own code reads the same whether the
/// shell is here, on a server, or on a machine that was made a second ago
pub fn shell(
    sandbox: &Sandbox,
    rows: u16,
    cols: u16,
    cwd: Option<&str>,
) -> Result<(Box<dyn portable_pty::MasterPty + Send>, Box<dyn portable_pty::ChildKiller + Send + Sync>)>
{
    let tag = a_tag();
    let (out_tx, out_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let (up_tx, up_rx) = std::sync::mpsc::channel::<Result<()>>();
    let (note_tx, note_rx) = std::sync::mpsc::channel::<Note>();

    // The listening thread. It holds the streaming response open for as long
    // as the shell lives, which is why it cannot be the thread anything else
    // is waiting on
    let (box_, tag_) = (sandbox.clone(), tag.clone());
    let started = cwd.map(str::to_string);
    std::thread::Builder::new().name("e2b-pty".into()).spawn(move || {
        listen(&box_, &tag_, rows, cols, started.as_deref(), &up_tx, &out_tx);
    })?;

    // The typing thread. One call per note, in the order they were made
    let (box_, tag_) = (sandbox.clone(), tag.clone());
    std::thread::Builder::new().name("e2b-pty-in".into()).spawn(move || {
        while let Ok(note) = note_rx.recv() {
            match note {
                Note::Typed(bytes) => {
                    let _ = send_input(&box_, &tag_, &bytes);
                }
                Note::Size { rows, cols } => {
                    let _ = resize(&box_, &tag_, rows, cols);
                }
                Note::Ended => {
                    let _ = signal(&box_, &tag_, "SIGNAL_SIGKILL");
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

/// Hold the stream open and pass on what comes out of it.
///
/// The first thing the far end says is that the shell has started; that is
/// what releases whoever asked for it. Everything after is screen
fn listen(
    sandbox: &Sandbox,
    tag: &str,
    rows: u16,
    cols: u16,
    cwd: Option<&str>,
    up: &std::sync::mpsc::Sender<Result<()>>,
    out: &std::sync::mpsc::Sender<Vec<u8>>,
) {
    let body = serde_json::json!({
        "process": {
            // A login shell, because a person opening a terminal expects their
            // own profile to have been read -- the same thing ssh gives them
            "cmd": "/bin/bash",
            "args": ["-i", "-l"],
            "envs": { "TERM": TERM },
            "cwd": cwd,
        },
        "pty": { "size": { "cols": cols as u32, "rows": rows as u32 } },
        "tag": tag,
        "stdin": true,
    });
    // No overall deadline on this one: the whole point of it is to stay open.
    // Everything else in this module still has one
    let agent = ureq::Agent::config_builder()
        .timeout_global(None)
        .build()
        .new_agent();
    let sent = serde_json::to_vec(&body).map(|b| frame(&b));
    let resp = match sent {
        Ok(b) => headed(agent.post(&format!("{SANDBOX}/process.Process/Start")), sandbox)
            .header("Content-Type", "application/connect+json")
            .send(b),
        Err(e) => {
            let _ = up.send(Err(anyhow!("{e}")));
            return;
        }
    };
    let mut resp = match resp {
        Ok(r) => r,
        Err(e) => {
            let _ = up.send(Err(anyhow!(crate::i18n::tp(
                "err.e2b.call",
                &[("e", &format!("{e}"))]
            ))));
            return;
        }
    };
    let mut reader = resp.body_mut().as_reader();
    let mut held: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    let mut opened = false;
    loop {
        let n = match std::io::Read::read(&mut reader, &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        held.extend_from_slice(&buf[..n]);
        for msg in whole_frames(&mut held) {
            let event = msg.get("event");
            if !opened && event.and_then(|e| e.get("start")).is_some() {
                opened = true;
                let _ = up.send(Ok(()));
            }
            if let Some(s) = event.and_then(|e| e.get("data")).and_then(|d| d.get("pty"))
                && let Some(bytes) = unwrap_bytes(s)
                // Nobody is listening any more: the tab has gone
                && out.send(bytes).is_err()
            {
                return;
            }
            if event.and_then(|e| e.get("end")).is_some() {
                if !opened {
                    let _ = up.send(Err(anyhow!(crate::i18n::tp(
                        "err.e2b.call",
                        &[("e", "the shell ended before it started")]
                    ))));
                }
                return;
            }
        }
    }
    if !opened {
        let _ = up.send(Err(anyhow!(crate::i18n::tp(
            "err.e2b.call",
            &[("e", "the machine closed the connection")]
        ))));
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
        .build()
        .new_agent();
    headed(agent.post(&format!("{SANDBOX}/{method}")), sandbox)
        .header("Content-Type", "application/json")
        .send(serde_json::to_vec(body)?)
        .map_err(|e| anyhow!(crate::i18n::tp("err.e2b.call", &[("e", &format!("{e}"))])))?;
    Ok(())
}

fn send_input(sandbox: &Sandbox, tag: &str, bytes: &[u8]) -> Result<()> {
    use base64::Engine as _;
    let typed = base64::engine::general_purpose::STANDARD.encode(bytes);
    tell(
        sandbox,
        "process.Process/SendInput",
        &serde_json::json!({ "process": { "tag": tag }, "input": { "pty": typed } }),
    )
}

fn resize(sandbox: &Sandbox, tag: &str, rows: u16, cols: u16) -> Result<()> {
    tell(
        sandbox,
        "process.Process/Update",
        &serde_json::json!({
            "process": { "tag": tag },
            "pty": { "size": { "cols": cols as u32, "rows": rows as u32 } },
        }),
    )
}

fn signal(sandbox: &Sandbox, tag: &str, which: &str) -> Result<()> {
    tell(
        sandbox,
        "process.Process/SendSignal",
        &serde_json::json!({ "process": { "tag": tag }, "signal": which }),
    )
}

#[cfg(test)]
mod tests {
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
            assert_eq!(said, ["hi", "there", "bell"], "切れ目 {cut} で落とした");
            assert!(held.is_empty(), "切れ目 {cut} で残骸がある");
        }
    }

    /// Half a frame is kept, not guessed at. A length that says more is coming
    /// must hold everything back until it does
    #[test]
    fn half_a_message_is_kept_rather_than_read() {
        let whole = frame(b"{\"event\":{}}");
        let mut held = whole[..whole.len() - 1].to_vec();
        assert!(whole_frames(&mut held).is_empty(), "途中まででも読んでしまった");
        assert_eq!(held.len(), whole.len() - 1, "読めないものを捨てた");
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

    /// Every terminal in this program gets a name of its own, decided before
    /// the far end has said anything -- which is what lets typing start
    /// without waiting for a process id to come back
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

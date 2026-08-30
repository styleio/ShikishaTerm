//! The external control API: a named pipe that speaks the same vocabulary Lua
//! does.
//!
//! One pipe per running app (`\\.\pipe\shikisha-<pid>`), one JSON object per
//! line, one line of answer back:
//!
//! ```text
//! → {"id":"1","method":"send_to_tab","params":["reviewer","status?"]}
//! ← {"id":"1","ok":true,"result":null}
//! ```
//!
//! `method` is the name of a Lua primitive with nothing translated in between —
//! see `hooks::HookEngine::call_primitive`. This module never decides what can
//! be called; it carries the call and carries the answer back.
//!
//! Safety:
//!   - The pipe is created with a DACL naming this account and no one else. A
//!     pipe made with no descriptor of its own gets a friendlier default, and
//!     "only my children" would then be a label on an open door
//!   - The first line of a connection has to present the token. In the default
//!     mode the token exists only in the environment of the processes tabs
//!     started; `access: "user"` also writes it beside the exe, and it is
//!     rotated at every launch
//!   - **What this can and cannot protect.** Another process running as you can
//!     read your processes' environment blocks, and an AI in a tab can copy its
//!     own token into a log. What the token stops is an accident and another
//!     account — not someone who is already you

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

/// Environment variable names handed to a tab's child process.
///
/// A CLI in a tab finds the door and its key here, and learns which tab it is
/// sitting in — the answer to "where am I?" that it otherwise has no way to ask
pub(crate) const ENV_PIPE: &str = "SHIKISHA_PIPE";
const ENV_TOKEN: &str = "SHIKISHA_TOKEN";
const ENV_TAB: &str = "SHIKISHA_TAB";

/// Where `access: "user"` leaves the token. Rewritten at every launch
const TOKEN_FILE: &str = "api-token";

/// Who may drive this app from outside. One setting, three values — not three
/// settings that can contradict each other
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// Only what this app started: a tab's CLI, and whatever that spawns. The
    /// token is passed in the environment and never written down
    #[default]
    Children,
    /// Anything running as this user. The token is also written to
    /// `data\api-token` so a script of your own can read it
    User,
    /// No pipe at all
    Off,
}

/// The external API's settings
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApiSpec {
    #[serde(default)]
    pub access: Access,
}

/// One call from outside, to be carried out on the main loop.
///
/// The Lua state is full of `Rc` and belongs to the thread that owns the
/// engine, so nothing here touches it — this crosses the channel and the loop
/// answers it on its next turn (16ms away)
pub struct ApiCall {
    /// The tab whose token was used, when it was a tab's. `None` means the call
    /// came from outside every tab, which counts as a person: what it sends
    /// starts a fresh chain instead of inheriting one
    pub caller: Option<String>,
    pub method: String,
    pub params: Vec<serde_json::Value>,
    /// Where the answer goes. The connection is holding the line for it
    pub reply: Sender<Result<serde_json::Value, String>>,
}

/// token -> the tab that owns it (`None` for the shared `user` token)
type Tokens = Arc<Mutex<HashMap<String, Option<String>>>>;

/// Every key minted this run.
///
/// Belongs to the process, not to one server: the setting can be turned off and
/// on again from the settings screen, and the tabs already running would
/// otherwise be left holding keys to a door that no longer recognises them.
static TOKENS: OnceLock<Tokens> = OnceLock::new();

/// The pipe currently being listened on, or `None` while the API is off — which
/// is also how `child_env` knows to hand a new tab nothing at all
static PIPE: Mutex<Option<String>> = Mutex::new(None);

fn tokens() -> &'static Tokens {
    TOKENS.get_or_init(Tokens::default)
}

/// The environment a tab's child process is launched with.
///
/// Empty when the API is off, so a tab started then carries no stale key.
/// Each tab gets a token of its own: what arrives on the pipe is then an
/// authenticated "I am this tab", not a claim anyone could make, and a chain of
/// AIs handing work to each other through the API is counted the same way as
/// one doing it through the screen
pub fn child_env(tab: &str) -> Vec<(String, String)> {
    let Some(path) = PIPE.lock().ok().and_then(|p| p.clone()) else {
        return Vec::new();
    };
    // A restart is the same tab with a new process. Its old key belonged to
    // the process that is gone, and nothing should still be able to use it
    forget_in(tokens(), tab);
    vec![
        (ENV_PIPE.to_string(), path),
        (ENV_TOKEN.to_string(), mint_into(tokens(), tab)),
        (ENV_TAB.to_string(), tab.to_string()),
    ]
}

/// Where the pipe is, for the settings screen to show. `None` while off
pub fn listening_on() -> Option<String> {
    PIPE.lock().ok().and_then(|p| p.clone())
}

fn mint_into(tokens: &Tokens, tab: &str) -> String {
    let token = crate::random_hex(24);
    if let Ok(mut t) = tokens.lock() {
        t.insert(token.clone(), Some(tab.to_string()));
    }
    token
}

fn forget_in(tokens: &Tokens, tab: &str) {
    if let Ok(mut t) = tokens.lock() {
        t.retain(|_, owner| owner.as_deref() != Some(tab));
    }
}

pub struct ApiServer {
    pub path: String,
    /// Calls waiting to be carried out. Drained by the main loop each turn
    pub rx: Receiver<ApiCall>,
    stop: Arc<AtomicBool>,
    tokens: Tokens,
    accept: Option<std::thread::JoinHandle<()>>,
}

impl ApiServer {
    /// Open the pipe. `Ok(None)` when the setting says not to.
    ///
    /// Failing to build the descriptor is refused rather than waved through:
    /// an unguarded pipe would be a worse outcome than no API
    pub fn start(access: Access) -> anyhow::Result<Option<Self>> {
        if access == Access::Off {
            let _ = std::fs::remove_file(crate::config::state_path(TOKEN_FILE));
            return Ok(None);
        }
        let server = Self::listen(
            format!(r"\\.\pipe\shikisha-{}", std::process::id()),
            Arc::clone(tokens()),
        )?;

        if access == Access::User {
            // Rotated every launch. The folder beside the exe may well sit
            // inside a synced drive, where yesterday's key would otherwise
            // live on in the version history of every machine that shares it
            let shared = crate::random_hex(24);
            server.tokens.lock().unwrap().insert(shared.clone(), None);
            crate::crypto::write_atomic(&crate::config::state_path(TOKEN_FILE), &shared)?;
        } else {
            // Coming down from `user`: deleting the file is not enough, since
            // whoever read it still holds what was in it
            server.tokens.lock().unwrap().retain(|_, owner| owner.is_some());
            let _ = std::fs::remove_file(crate::config::state_path(TOKEN_FILE));
        }

        // From here on, `child_env` can hand a tab's child the way in
        if let Ok(mut p) = PIPE.lock() {
            *p = Some(server.path.clone());
        }
        crate::append_hook_log(&format!(
            "external API listening on {} ({access:?})",
            server.path
        ));
        Ok(Some(server))
    }

    /// Open the pipe and start accepting. Split out from `start` so a test can
    /// run one under a name and a key set of its own — the production name is
    /// the process id, and two servers can no more share that than two apps could
    fn listen(path: String, tokens: Tokens) -> anyhow::Result<Self> {
        // Refused rather than fallen back on: no API is a better outcome than
        // one with no access list
        let sd = SecurityDescriptor::only_me()
            .ok_or_else(|| anyhow::anyhow!("could not build the pipe's access list"))?;
        // The opening instance is made before this returns, so a caller that
        // connects the moment it hears we are up finds something to connect to
        let first = PipeStream::create(&path, &sd, true)
            .ok_or_else(|| anyhow::anyhow!("could not create {path}"))?;

        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let accept = {
            let (path, tokens, stop) = (path.clone(), Arc::clone(&tokens), Arc::clone(&stop));
            std::thread::spawn(move || accept_loop(&path, sd, first, tokens, tx, stop))
        };
        Ok(ApiServer {
            path,
            rx,
            stop,
            tokens,
            accept: Some(accept),
        })
    }

    /// Mint a token for one tab, and remember whose it is. In the running app
    /// this happens inside `child_env`, as the tab's process is launched
    #[cfg(test)]
    pub fn mint(&self, tab: &str) -> String {
        mint_into(&self.tokens, tab)
    }

    /// Retire one tab's key. The app retires them in bulk (`retain_tabs`),
    /// since tabs leave in more ways than one
    #[cfg(test)]
    pub fn forget(&self, tab: &str) {
        forget_in(&self.tokens, tab);
    }

    /// Keep only the keys of tabs that still exist.
    ///
    /// Called with the live set rather than told about each closure: tabs go
    /// away in several places (closed, rebuilt by a config change, swapped out
    /// with the workspace), and a key that outlived its tab is a working key
    /// nobody is watching
    pub fn retain_tabs(&self, live: &[String]) {
        if let Ok(mut t) = self.tokens.lock() {
            t.retain(|_, owner| match owner {
                Some(tab) => live.iter().any(|l| l == tab),
                // The shared `user` token belongs to no tab
                None => true,
            });
        }
    }

    /// Stop listening. The accept thread is parked inside ConnectNamedPipe, so
    /// it is woken the only way it can be: by connecting to it once ourselves
    pub fn shutdown(&mut self) {
        // A tab started from here on gets no key: there is nothing to unlock
        if let Ok(mut p) = PIPE.lock() {
            *p = None;
        }
        self.stop.store(true, Ordering::SeqCst);
        let _ = std::fs::File::open(&self.path);
        if let Some(t) = self.accept.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(crate::config::state_path(TOKEN_FILE));
        // The log said when the door opened; it should say when it closed
        crate::append_hook_log(&format!("external API stopped listening on {}", self.path));
    }
}

fn accept_loop(
    path: &str,
    sd: SecurityDescriptor,
    first: PipeStream,
    tokens: Tokens,
    tx: Sender<ApiCall>,
    stop: Arc<AtomicBool>,
) {
    use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError};
    use windows_sys::Win32::System::Pipes::ConnectNamedPipe;

    // Announced once: a person should be able to tell from the log that
    // something outside this window is driving it
    let announced = Arc::new(AtomicBool::new(false));
    let mut ready = Some(first);
    while !stop.load(Ordering::SeqCst) {
        let pipe = match ready.take() {
            Some(p) => p,
            None => match PipeStream::create(path, &sd, false) {
                Some(p) => p,
                None => {
                    crate::append_hook_log("external API: could not create the pipe instance");
                    return;
                }
            },
        };
        // ERROR_PIPE_CONNECTED means the client got there first — still a client
        let connected = unsafe { ConnectNamedPipe(pipe.0, std::ptr::null_mut()) } != 0
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        if stop.load(Ordering::SeqCst) || !connected {
            continue;
        }
        let (tokens, tx, announced) = (Arc::clone(&tokens), tx.clone(), Arc::clone(&announced));
        std::thread::spawn(move || serve(pipe, tokens, tx, announced));
    }
}

/// One connection: a handshake line, then a call per line until it hangs up
fn serve(pipe: PipeStream, tokens: Tokens, tx: Sender<ApiCall>, announced: Arc<AtomicBool>) {
    let mut out = match pipe.try_clone() {
        Some(w) => w,
        None => return,
    };
    let mut lines = BufReader::new(pipe).lines();

    let hello = match lines.next() {
        Some(Ok(l)) => l,
        _ => return,
    };
    let supplied = serde_json::from_str::<serde_json::Value>(&hello)
        .ok()
        .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_default();
    let caller = tokens.lock().ok().and_then(|t| {
        t.iter()
            .find(|(known, _)| crate::crypto::token_eq(known, &supplied))
            .map(|(_, owner)| owner.clone())
    });
    let Some(caller) = caller else {
        let _ = writeln!(out, r#"{{"ok":false,"error":"unauthorized"}}"#);
        crate::append_hook_log("external API: refused a connection with no valid token");
        return;
    };
    if !announced.swap(true, Ordering::SeqCst) {
        crate::append_hook_log(&format!(
            "external API: first caller accepted ({})",
            caller.as_deref().unwrap_or("outside the tabs")
        ));
    }
    let _ = writeln!(out, r#"{{"ok":true,"result":"hello"}}"#);

    for line in lines {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let answer = handle_line(&line, caller.as_deref(), &tx);
        if writeln!(out, "{answer}").is_err() {
            break;
        }
    }
}

/// Turn one request line into one answer line
fn handle_line(line: &str, caller: Option<&str>, tx: &Sender<ApiCall>) -> String {
    let req: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return error_line(&serde_json::Value::Null, &format!("bad JSON: {e}")),
    };
    let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let Some(method) = req.get("method").and_then(|m| m.as_str()) else {
        return error_line(&id, "a call needs a method");
    };
    let params = match req.get("params") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(a)) => a.clone(),
        // A lone value is the one-argument case written the short way
        Some(other) => vec![other.clone()],
    };
    let (reply, wait) = channel();
    let call = ApiCall {
        caller: caller.map(str::to_string),
        method: method.to_string(),
        params,
        reply,
    };
    if tx.send(call).is_err() {
        return error_line(&id, "the app is shutting down");
    }
    // Generous: a primitive may be waiting on a page, and a caller that asked
    // for that is not helped by being told "timeout" while it is still working
    match wait.recv_timeout(std::time::Duration::from_secs(300)) {
        Ok(Ok(result)) => serde_json::json!({"id": id, "ok": true, "result": result}).to_string(),
        Ok(Err(e)) => error_line(&id, &e),
        Err(_) => error_line(&id, "the app did not answer"),
    }
}

fn error_line(id: &serde_json::Value, msg: &str) -> String {
    serde_json::json!({"id": id, "ok": false, "error": msg}).to_string()
}

/// A security descriptor that names this account and nobody else.
///
/// Built from SDDL — `D:P` is a DACL that inherits nothing, `GA` is full
/// access, and the only entry is the SID we are running under
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

// The descriptor is a plain allocation handed to CreateNamedPipeW; moving it to
// the accept thread is what it is for
unsafe impl Send for SecurityDescriptor {}

impl SecurityDescriptor {
    fn only_me() -> Option<Self> {
        use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
        let sid = current_user_sid()?;
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;{sid})\0").encode_utf16().collect();
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1, // SDDL_REVISION_1
                &mut sd,
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(Self(sd))
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0 as *mut c_void) };
        }
    }
}

/// The account this process is running as, as a SID string (`S-1-5-21-…`)
fn current_user_sid() -> Option<String> {
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u8; len as usize];
        let got = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
        CloseHandle(token);
        if got == 0 {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text: windows_sys::core::PWSTR = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return None;
        }
        let mut n = 0;
        while *text.add(n) != 0 {
            n += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text as *mut c_void);
        Some(sid)
    }
}

/// One end of a connected pipe, as something `Read`/`Write` can be used on
struct PipeStream(HANDLE);

// A handle is just a number; the thread that serves the connection owns it
unsafe impl Send for PipeStream {}

impl PipeStream {
    fn create(path: &str, sd: &SecurityDescriptor, first: bool) -> Option<Self> {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
        };
        use windows_sys::Win32::System::Pipes::{
            CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
            PIPE_WAIT,
        };
        let wide: Vec<u16> = format!("{path}\0").encode_utf16().collect();
        let sa = sd.attributes();
        // FIRST_PIPE_INSTANCE on the opening instance: if something already
        // holds this name, we want to fail loudly rather than share it
        let mode = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        let h = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                mode,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                &sa,
            )
        };
        (!h.is_null() && h as isize != -1).then_some(Self(h))
    }

    /// A second view of the same connection, so the answer can be written while
    /// the reader is parked on the next line
    fn try_clone(&self) -> Option<PipeWriter> {
        Some(PipeWriter(self.0))
    }
}

impl Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, GetLastError};
        use windows_sys::Win32::Storage::FileSystem::ReadFile;
        let mut got = 0u32;
        let ok = unsafe {
            ReadFile(
                self.0,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut got,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            // The client hanging up is the normal end of a conversation, not a
            // failure to report
            return match unsafe { GetLastError() } {
                ERROR_BROKEN_PIPE => Ok(0),
                e => Err(std::io::Error::from_raw_os_error(e as i32)),
            };
        }
        Ok(got as usize)
    }
}

impl Drop for PipeStream {
    fn drop(&mut self) {
        use windows_sys::Win32::Storage::FileSystem::FlushFileBuffers;
        use windows_sys::Win32::System::Pipes::DisconnectNamedPipe;
        unsafe {
            FlushFileBuffers(self.0);
            DisconnectNamedPipe(self.0);
            CloseHandle(self.0);
        }
    }
}

/// The writing half. Borrows the handle the reader owns and closes nothing
struct PipeWriter(HANDLE);

unsafe impl Send for PipeWriter {}

impl Write for PipeWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Storage::FileSystem::WriteFile;
        let mut put = 0u32;
        let ok = unsafe {
            WriteFile(
                self.0,
                buf.as_ptr(),
                buf.len() as u32,
                &mut put,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(std::io::Error::from_raw_os_error(unsafe { GetLastError() } as i32));
        }
        Ok(put as usize)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A client of a running app's pipe: connect, present the token, then a call
/// per line.
///
/// The app is its own first caller: hook mode (`--hook session`) runs as a
/// child of an AI CLI and reports back through this
pub struct ApiClient {
    file: std::fs::File,
    reader: BufReader<std::fs::File>,
    next: u64,
}

impl ApiClient {
    pub fn connect(path: &str, token: &str) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
        let reader = BufReader::new(file.try_clone()?);
        let mut c = Self {
            file,
            reader,
            next: 0,
        };
        c.line(&serde_json::json!({ "token": token }).to_string())?;
        Ok(c)
    }

    /// Connect the way a tab's child does: the environment says where and with what
    pub fn from_env() -> std::io::Result<Self> {
        let path = std::env::var(ENV_PIPE)
            .map_err(|_| std::io::Error::other("SHIKISHA_PIPE is not set"))?;
        let token = std::env::var(ENV_TOKEN).unwrap_or_default();
        Self::connect(&path, &token)
    }

    pub fn call(&mut self, method: &str, params: Vec<serde_json::Value>) -> std::io::Result<serde_json::Value> {
        self.next += 1;
        let req = serde_json::json!({"id": self.next.to_string(), "method": method, "params": params});
        let answer = self.line(&req.to_string())?;
        serde_json::from_str(&answer).map_err(std::io::Error::other)
    }

    fn line(&mut self, text: &str) -> std::io::Result<String> {
        writeln!(self.file, "{text}")?;
        self.file.flush()?;
        let mut got = String::new();
        self.reader.read_line(&mut got)?;
        Ok(got.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server on a name of its own (tests run side by side in one process,
    /// and the production name is the process id), with the main loop's job
    /// played by a thread that answers every call with `answer`
    fn served(answer: impl Fn(ApiCall) + Send + 'static) -> ApiServer {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let path = format!(r"\\.\pipe\shikisha-test-{}-{n}", std::process::id());
        let mut server = ApiServer::listen(path, Tokens::default()).unwrap();
        let rx = std::mem::replace(&mut server.rx, channel().1);
        std::thread::spawn(move || {
            while let Ok(call) = rx.recv() {
                answer(call);
            }
        });
        server
    }

    #[test]
    fn a_call_arrives_with_its_arguments_and_the_answer_comes_back() {
        let mut server = served(|call| {
            let _ = call.reply.send(Ok(serde_json::json!({
                "method": call.method,
                "params": call.params,
                "caller": call.caller,
            })));
        });
        let token = server.mint("reviewer");
        let mut c = ApiClient::connect(&server.path, &token).unwrap();
        let got = c
            .call("send_to_tab", vec![serde_json::json!("build"), serde_json::json!("go")])
            .unwrap();
        assert_eq!(got["ok"], serde_json::json!(true));
        assert_eq!(got["result"]["method"], "send_to_tab");
        assert_eq!(got["result"]["params"][1], "go");
        // The caller is the tab whose token it used — it never had to say so,
        // and could not have said otherwise
        assert_eq!(got["result"]["caller"], "reviewer");
        server.shutdown();
    }

    #[test]
    fn a_connection_without_the_token_gets_nowhere() {
        let mut server = served(|call| {
            let _ = call.reply.send(Ok(serde_json::json!("should never run")));
        });
        let mut c = ApiClient::connect(&server.path, "not-the-token").unwrap();
        // The refusal is the answer to the handshake itself; the connection is
        // then closed, so the call that follows cannot succeed
        let refused = c.call("send_to_tab", vec![]);
        assert!(
            refused.is_err() || refused.unwrap()["ok"] == serde_json::json!(false),
            "トークン無しでは何も通らない"
        );
        server.shutdown();
    }

    #[test]
    fn a_broken_request_is_answered_rather_than_dropped() {
        let mut server = served(|call| {
            let _ = call.reply.send(Err("no such primitive: nope".into()));
        });
        let token = server.mint("t");
        let mut c = ApiClient::connect(&server.path, &token).unwrap();
        let bad = c.line("{not json").unwrap();
        assert!(bad.contains("bad JSON"), "{bad}");
        let missing = c.line(r#"{"id":"7"}"#).unwrap();
        assert!(missing.contains("needs a method"), "{missing}");
        assert!(missing.contains(r#""id":"7""#), "答えは呼んだ側のidを返す: {missing}");
        // ...and the connection is still usable afterwards
        let after = c.call("nope", vec![]).unwrap();
        assert_eq!(after["ok"], serde_json::json!(false));
        assert!(
            after["error"].as_str().unwrap_or_default().contains("no such primitive"),
            "{after}"
        );
        server.shutdown();
    }

    #[test]
    fn a_forgotten_tab_takes_its_key_with_it() {
        let mut server = served(|call| {
            let _ = call.reply.send(Ok(serde_json::Value::Null));
        });
        let token = server.mint("gone");
        server.forget("gone");
        let mut c = ApiClient::connect(&server.path, &token).unwrap();
        let refused = c.call("show", vec![]);
        assert!(refused.is_err() || refused.unwrap()["ok"] == serde_json::json!(false));
        server.shutdown();
    }

    #[test]
    fn a_key_outlives_the_server_that_issued_it() {
        // The setting can be switched off and on again from the settings
        // screen. The pipe goes away and comes back; the keys must not, or
        // every agent already running would be locked out of a door it was
        // given the key to, with nothing on screen to say why
        let keys = Tokens::default();
        let path = format!(r"\\.\pipe\shikisha-test-restart-{}", std::process::id());
        let mut first = ApiServer::listen(path.clone(), Arc::clone(&keys)).unwrap();
        let token = first.mint("worker");
        first.shutdown();

        let mut second = ApiServer::listen(path, Arc::clone(&keys)).unwrap();
        let rx = std::mem::replace(&mut second.rx, channel().1);
        std::thread::spawn(move || {
            while let Ok(call) = rx.recv() {
                let _ = call.reply.send(Ok(serde_json::json!(call.caller)));
            }
        });
        let mut c = ApiClient::connect(&second.path, &token).unwrap();
        let got = c.call("state", vec![]).unwrap();
        assert_eq!(got["ok"], serde_json::json!(true), "{got}");
        assert_eq!(got["result"], "worker", "誰の鍵かも覚えている");
        second.shutdown();
    }

    #[test]
    fn the_pipe_is_gone_once_the_app_stops_listening() {
        let mut server = served(|_| {});
        assert!(std::fs::File::open(&server.path).is_ok());
        let path = server.path.clone();
        server.shutdown();
        assert!(
            std::fs::File::open(&path).is_err(),
            "閉じたあとのパイプには繋がらない"
        );
    }
}

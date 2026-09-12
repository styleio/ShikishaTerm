//! A tiny SSH server, for checking the SSH tab against something real.
//!
//! The app's own tests talk to a server like this one in-process, which proves
//! the protocol. What they cannot show is the tab: the screen, the keyboard,
//! the state light, the automation. That needs a server the running app can be
//! pointed at, and enabling Windows' own needs an administrator -- so here is
//! one, in the same place the other check tools live (`pty_probe`,
//! `vt_writer`).
//!
//!     cargo run --bin sshd_probe -- 2222 tester hunter2 [folder]
//!
//! It accepts one user and one password, gives out a terminal that echoes what
//! is typed, and answers the file half of the protocol over one folder (a
//! temporary one unless you name it). Nothing here is a real shell, and nothing
//! about it should ever be pointed at a network: it listens on the loopback
//! only, and says so.

use std::sync::Arc;

use russh::keys::ssh_encoding::bytes::Bytes;

#[derive(Clone)]
struct Probe {
    user: String,
    password: String,
    /// The folder the file half serves. Everything the client asks for lands
    /// under it, and nowhere else
    root: std::path::PathBuf,
    /// Channels waiting to be told what they are for. A file connection asks
    /// after the channel is open, so the channel has to be kept until then
    channels: std::sync::Arc<
        tokio::sync::Mutex<std::collections::HashMap<russh::ChannelId, russh::Channel<russh::server::Msg>>>,
    >,
    /// The ones carrying files. Their bytes belong to the file conversation
    /// and to nothing else -- echoing them, the way a terminal does, sends the
    /// client its own first packet back and ends the conversation before it
    /// starts
    files: std::sync::Arc<tokio::sync::Mutex<std::collections::HashSet<russh::ChannelId>>>,
}

impl russh::server::Server for Probe {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}

impl russh::server::Handler for Probe {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<russh::server::Auth, Self::Error> {
        let ok = user == self.user && password == self.password;
        println!("auth {user}: {}", if ok { "accepted" } else { "refused" });
        Ok(match ok {
            true => russh::server::Auth::Accept,
            false => russh::server::Auth::reject(),
        })
    }

    async fn channel_open_session(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        self.channels.lock().await.insert(channel.id(), channel);
        Ok(())
    }

    /// The file half arrives as a request for a subsystem on a channel of its
    /// own, so the channel is taken back out and handed to the file server
    async fn subsystem_request(
        &mut self,
        channel_id: russh::ChannelId,
        name: &str,
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        if name != "sftp" {
            session.channel_failure(channel_id)?;
            return Ok(());
        }
        session.channel_success(channel_id)?;
        self.files.lock().await.insert(channel_id);
        let Some(channel) = self.channels.lock().await.remove(&channel_id) else {
            return Ok(());
        };
        let files = Files {
            root: self.root.clone(),
            open: std::collections::HashMap::new(),
            next: 0,
        };
        println!("sftp on {}", self.root.display());
        // On a task of its own. Awaiting it here would hold up the session
        // loop that feeds this very channel, and the conversation would wait
        // for a packet that cannot arrive until it stops waiting
        tokio::spawn(async move {
            russh_sftp::server::run(channel.into_stream(), files).await;
            println!("sftp: the file conversation ended");
        });
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: russh::ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        println!("pty {term} {cols}x{rows}");
        session.channel_success(channel)?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        channel: russh::ChannelId,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        println!("resized to {cols}x{rows}");
        session.data(
            channel,
            Bytes::from(format!("\r\n[size is now {cols}x{rows}]\r\n$ ").into_bytes()),
        )?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: russh::ChannelId,
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        // This one is a terminal, so it is not waiting to be told what it is.
        // Letting go of it matters: a channel nobody reads from fills up, and a
        // full queue stops the session that feeds every other channel with it
        self.channels.lock().await.remove(&channel);
        session.channel_success(channel)?;
        session.data(
            channel,
            Bytes::from_static(b"the probe server is listening.\r\nnothing here is a real shell.\r\n$ "),
        )?;
        Ok(())
    }

    /// One command, run for real, in the folder this probe serves.
    ///
    /// Real because the thing being checked is git on the far side, and a
    /// canned answer would prove the protocol while proving nothing about the
    /// feature. This binary listens on the loopback only and is not part of
    /// any download; it is a bench tool and belongs on no network.
    async fn exec_request(
        &mut self,
        channel: russh::ChannelId,
        command: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.lock().await.remove(&channel);
        session.channel_success(channel)?;
        let line = String::from_utf8_lossy(command).to_string();
        let shell = if cfg!(windows) { "cmd" } else { "sh" };
        let flag = if cfg!(windows) { "/c" } else { "-c" };
        let out = std::process::Command::new(shell)
            .arg(flag)
            .arg(&line)
            .current_dir(&self.root)
            .output();
        let (code, stdout, stderr) = match out {
            Ok(o) => (o.status.code().unwrap_or(-1), o.stdout, o.stderr),
            Err(e) => (-1, Vec::new(), format!("{e}").into_bytes()),
        };
        if !stdout.is_empty() {
            session.data(channel, Bytes::from(stdout))?;
        }
        if !stderr.is_empty() {
            // Stream 1 is what ssh calls the error half
            session.extended_data(channel, 1, Bytes::from(stderr))?;
        }
        session.exit_status_request(channel, code as u32)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        // Files go to the file conversation, which is reading this channel
        // itself. Only a terminal echoes
        if self.files.lock().await.contains(&channel) {
            return Ok(());
        }
        // Typing is echoed the way a terminal does, so what is on screen is
        // what was typed; Enter starts a new line with a prompt
        let text = String::from_utf8_lossy(data).to_string();
        let out = match text.contains('\r') {
            true => text.replace('\r', "\r\n") + "$ ",
            false => text,
        };
        session.data(channel, Bytes::from(out.into_bytes()))?;
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2222);
    let user = args.get(2).cloned().unwrap_or_else(|| "tester".into());
    let password = args.get(3).cloned().unwrap_or_else(|| "hunter2".into());
    let root = args
        .get(4)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("shikisha-sshd-probe"));
    std::fs::create_dir_all(&root).expect("could not make the folder to serve");

    // The same key every time this folder is served. A new one on each start
    // would look to the app exactly like somebody standing in the middle --
    // which it is right to refuse, and maddening to hit while testing
    let key_path = root.join("probe-host-key");
    let host_key = match std::fs::read_to_string(&key_path)
        .ok()
        .and_then(|t| russh::keys::PrivateKey::from_openssh(&t).ok())
    {
        Some(k) => k,
        None => {
            let k = russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .expect("host key");
            let pem = k
                .to_openssh(russh::keys::ssh_key::LineEnding::LF)
                .expect("write the host key");
            std::fs::write(&key_path, pem.as_bytes()).expect("save the host key");
            k
        }
    };
    let config = Arc::new(russh::server::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
        auth_rejection_time: std::time::Duration::from_millis(200),
        keys: vec![host_key],
        ..Default::default()
    });
    // The loopback, and nothing else. This server accepts one password and
    // hands out a terminal; it has no business being reachable
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("could not listen");
    println!("ssh://{user}@127.0.0.1:{port}  password: {password}");
    println!("files under {}", root.display());
    let mut server = Probe {
        user,
        password,
        root,
        channels: std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        files: std::sync::Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
    };
    use russh::server::Server as _;
    let _ = server.run_on_socket(config, &listener).await;
}

// ── Files ──────────────────────────────────────────────────────────────────
// The same probe, answering the file half of the protocol over one folder.
// Real files, so what the app does is what actually happens: a transfer that
// silently wrote nothing would look exactly like a transfer that worked.

struct Files {
    root: std::path::PathBuf,
    /// Open files and folders, by the handle the client was given
    open: std::collections::HashMap<String, Open>,
    next: u64,
}

enum Open {
    File { path: std::path::PathBuf, write: bool },
    /// A folder, and whether its listing has already been handed over: the
    /// protocol reads a folder by asking again until it is told there is no
    /// more, so somebody has to remember that the answer was given
    Dir { path: std::path::PathBuf, done: bool },
}

impl Files {
    /// Where a path from the wire lands. Everything stays under the folder the
    /// probe was given: this is a toy, and a toy that can be talked into
    /// writing anywhere is not one
    fn at(&self, path: &str) -> std::path::PathBuf {
        let rel = path.trim_start_matches('/').replace('\\', "/");
        let mut out = self.root.clone();
        for part in rel.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    out.pop();
                }
                p => out.push(p),
            }
        }
        if out.starts_with(&self.root) { out } else { self.root.clone() }
    }

    fn hand(&mut self, o: Open) -> String {
        self.next += 1;
        let h = format!("h{}", self.next);
        self.open.insert(h.clone(), o);
        h
    }

    fn attrs(meta: &std::fs::Metadata) -> russh_sftp::protocol::FileAttributes {
        let mut a = russh_sftp::protocol::FileAttributes {
            size: Some(meta.len()),
            ..Default::default()
        };
        a.set_dir(meta.is_dir());
        a.set_regular(meta.is_file());
        a
    }
}

impl russh_sftp::server::Handler for Files {
    type Error = russh_sftp::protocol::StatusCode;

    fn unimplemented(&self) -> Self::Error {
        russh_sftp::protocol::StatusCode::OpUnsupported
    }

    async fn init(
        &mut self,
        _version: u32,
        _extensions: std::collections::HashMap<String, String>,
    ) -> Result<russh_sftp::protocol::Version, Self::Error> {
        println!("sftp init v{_version}");
        Ok(russh_sftp::protocol::Version::new())
    }

    async fn realpath(
        &mut self,
        id: u32,
        path: String,
    ) -> Result<russh_sftp::protocol::Name, Self::Error> {
        let shown = match path.as_str() {
            "" | "." => "/".to_string(),
            p => format!("/{}", p.trim_start_matches('/')),
        };
        Ok(russh_sftp::protocol::Name {
            id,
            files: vec![russh_sftp::protocol::File {
                filename: shown.clone(),
                longname: shown,
                attrs: russh_sftp::protocol::FileAttributes::default(),
            }],
        })
    }

    async fn open(
        &mut self,
        id: u32,
        filename: String,
        pflags: russh_sftp::protocol::OpenFlags,
        _attrs: russh_sftp::protocol::FileAttributes,
    ) -> Result<russh_sftp::protocol::Handle, Self::Error> {
        let path = self.at(&filename);
        let write = pflags.contains(russh_sftp::protocol::OpenFlags::WRITE);
        if write {
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            std::fs::write(&path, b"").map_err(|_| russh_sftp::protocol::StatusCode::Failure)?;
        } else if !path.is_file() {
            return Err(russh_sftp::protocol::StatusCode::NoSuchFile);
        }
        let handle = self.hand(Open::File { path, write });
        Ok(russh_sftp::protocol::Handle { id, handle })
    }

    async fn close(
        &mut self,
        id: u32,
        handle: String,
    ) -> Result<russh_sftp::protocol::Status, Self::Error> {
        self.open.remove(&handle);
        Ok(russh_sftp::protocol::Status {
            id,
            status_code: russh_sftp::protocol::StatusCode::Ok,
            error_message: "ok".into(),
            language_tag: "en".into(),
        })
    }

    async fn read(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        len: u32,
    ) -> Result<russh_sftp::protocol::Data, Self::Error> {
        let Some(Open::File { path, .. }) = self.open.get(&handle) else {
            return Err(russh_sftp::protocol::StatusCode::Failure);
        };
        let all = std::fs::read(path).map_err(|_| russh_sftp::protocol::StatusCode::Failure)?;
        let from = offset as usize;
        if from >= all.len() {
            // The end of a file is said with an error, which is how the
            // protocol says it
            return Err(russh_sftp::protocol::StatusCode::Eof);
        }
        let to = (from + len as usize).min(all.len());
        Ok(russh_sftp::protocol::Data { id, data: all[from..to].to_vec() })
    }

    async fn write(
        &mut self,
        id: u32,
        handle: String,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<russh_sftp::protocol::Status, Self::Error> {
        let Some(Open::File { path, write: true }) = self.open.get(&handle) else {
            return Err(russh_sftp::protocol::StatusCode::PermissionDenied);
        };
        use std::io::{Seek, Write};
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|_| russh_sftp::protocol::StatusCode::Failure)?;
        f.seek(std::io::SeekFrom::Start(offset))
            .and_then(|_| f.write_all(&data))
            .map_err(|_| russh_sftp::protocol::StatusCode::Failure)?;
        Ok(russh_sftp::protocol::Status {
            id,
            status_code: russh_sftp::protocol::StatusCode::Ok,
            error_message: "ok".into(),
            language_tag: "en".into(),
        })
    }

    async fn opendir(
        &mut self,
        id: u32,
        path: String,
    ) -> Result<russh_sftp::protocol::Handle, Self::Error> {
        let at = self.at(&path);
        if !at.is_dir() {
            return Err(russh_sftp::protocol::StatusCode::NoSuchFile);
        }
        let handle = self.hand(Open::Dir { path: at, done: false });
        Ok(russh_sftp::protocol::Handle { id, handle })
    }

    async fn readdir(
        &mut self,
        id: u32,
        handle: String,
    ) -> Result<russh_sftp::protocol::Name, Self::Error> {
        let Some(Open::Dir { path, done }) = self.open.get_mut(&handle) else {
            return Err(russh_sftp::protocol::StatusCode::Failure);
        };
        if *done {
            return Err(russh_sftp::protocol::StatusCode::Eof);
        }
        *done = true;
        let mut files = Vec::new();
        let read = std::fs::read_dir(&*path).map_err(|_| russh_sftp::protocol::StatusCode::Failure)?;
        for e in read.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            files.push(russh_sftp::protocol::File {
                longname: name.clone(),
                filename: name,
                attrs: Self::attrs(&meta),
            });
        }
        Ok(russh_sftp::protocol::Name { id, files })
    }

    async fn stat(
        &mut self,
        id: u32,
        path: String,
    ) -> Result<russh_sftp::protocol::Attrs, Self::Error> {
        let meta = std::fs::metadata(self.at(&path))
            .map_err(|_| russh_sftp::protocol::StatusCode::NoSuchFile)?;
        Ok(russh_sftp::protocol::Attrs { id, attrs: Self::attrs(&meta) })
    }

    async fn lstat(
        &mut self,
        id: u32,
        path: String,
    ) -> Result<russh_sftp::protocol::Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn fstat(
        &mut self,
        id: u32,
        handle: String,
    ) -> Result<russh_sftp::protocol::Attrs, Self::Error> {
        let path = match self.open.get(&handle) {
            Some(Open::File { path, .. }) | Some(Open::Dir { path, .. }) => path.clone(),
            None => return Err(russh_sftp::protocol::StatusCode::Failure),
        };
        let meta =
            std::fs::metadata(path).map_err(|_| russh_sftp::protocol::StatusCode::NoSuchFile)?;
        Ok(russh_sftp::protocol::Attrs { id, attrs: Self::attrs(&meta) })
    }

    async fn mkdir(
        &mut self,
        id: u32,
        path: String,
        _attrs: russh_sftp::protocol::FileAttributes,
    ) -> Result<russh_sftp::protocol::Status, Self::Error> {
        std::fs::create_dir_all(self.at(&path))
            .map_err(|_| russh_sftp::protocol::StatusCode::Failure)?;
        Ok(ok(id))
    }

    async fn rmdir(
        &mut self,
        id: u32,
        path: String,
    ) -> Result<russh_sftp::protocol::Status, Self::Error> {
        std::fs::remove_dir(self.at(&path))
            .map_err(|_| russh_sftp::protocol::StatusCode::Failure)?;
        Ok(ok(id))
    }

    async fn remove(
        &mut self,
        id: u32,
        filename: String,
    ) -> Result<russh_sftp::protocol::Status, Self::Error> {
        std::fs::remove_file(self.at(&filename))
            .map_err(|_| russh_sftp::protocol::StatusCode::Failure)?;
        Ok(ok(id))
    }

    async fn rename(
        &mut self,
        id: u32,
        oldpath: String,
        newpath: String,
    ) -> Result<russh_sftp::protocol::Status, Self::Error> {
        std::fs::rename(self.at(&oldpath), self.at(&newpath))
            .map_err(|_| russh_sftp::protocol::StatusCode::Failure)?;
        Ok(ok(id))
    }
}

fn ok(id: u32) -> russh_sftp::protocol::Status {
    russh_sftp::protocol::Status {
        id,
        status_code: russh_sftp::protocol::StatusCode::Ok,
        error_message: "ok".into(),
        language_tag: "en".into(),
    }
}

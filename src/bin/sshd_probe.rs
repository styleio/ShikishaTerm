//! A tiny SSH server, for checking the SSH tab against something real.
//!
//! The app's own tests talk to a server like this one in-process, which proves
//! the protocol. What they cannot show is the tab: the screen, the keyboard,
//! the state light, the automation. That needs a server the running app can be
//! pointed at, and enabling Windows' own needs an administrator -- so here is
//! one, in the same place the other check tools live (`pty_probe`,
//! `vt_writer`).
//!
//!     cargo run --bin sshd_probe -- 2222 tester hunter2
//!
//! It accepts one user and one password, gives out a terminal, and echoes what
//! is typed with a prompt in front of it. Nothing here is a real shell, and
//! nothing about it should ever be pointed at a network: it listens on the
//! loopback only, and says so.

use std::sync::Arc;

use russh::keys::ssh_encoding::bytes::Bytes;

#[derive(Clone)]
struct Probe {
    user: String,
    password: String,
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
        _channel: russh::Channel<russh::server::Msg>,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
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
        session.channel_success(channel)?;
        session.data(
            channel,
            Bytes::from_static(b"the probe server is listening.\r\nnothing here is a real shell.\r\n$ "),
        )?;
        Ok(())
    }

    async fn data(
        &mut self,
        channel: russh::ChannelId,
        data: &[u8],
        session: &mut russh::server::Session,
    ) -> Result<(), Self::Error> {
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

    let config = Arc::new(russh::server::Config {
        inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
        auth_rejection_time: std::time::Duration::from_millis(200),
        keys: vec![
            russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)
                .expect("host key"),
        ],
        ..Default::default()
    });
    // The loopback, and nothing else. This server accepts one password and
    // hands out a terminal; it has no business being reachable
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("could not listen");
    println!("ssh://{user}@127.0.0.1:{port}  password: {password}");
    let mut server = Probe { user, password };
    use russh::server::Server as _;
    let _ = server.run_on_socket(config, &listener).await;
}

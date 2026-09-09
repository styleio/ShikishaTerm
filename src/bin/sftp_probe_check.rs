//! A one-shot check of the file half, with what actually crosses the wire.
//!
//! The app swallows a failure into a tab's screen, one line at a time, which
//! is the right thing there and useless while finding out *why*. This connects
//! the same way the app does, sends the first packet by hand, and prints what
//! comes back -- so "the library said no" can be told apart from "nothing
//! arrived".
//!
//!     cargo run --bin sftp_probe_check -- 2222 tester hunter2

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port: u16 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(2222);
    let user = args.get(2).cloned().unwrap_or_else(|| "tester".into());
    let password = args.get(3).cloned().unwrap_or_else(|| "hunter2".into());

    struct Anything;
    impl russh::client::Handler for Anything {
        type Error = russh::Error;
        async fn check_server_key(
            &mut self,
            _key: &russh::keys::PublicKeyOrCertificate,
        ) -> Result<bool, Self::Error> {
            Ok(true)
        }
    }

    let config = std::sync::Arc::new(russh::client::Config::default());
    let mut handle = russh::client::connect(config, ("127.0.0.1", port), Anything)
        .await
        .expect("connect");
    let ok = handle
        .authenticate_password(user, password)
        .await
        .expect("auth call")
        .success();
    println!("authenticated: {ok}");

    let channel = handle.channel_open_session().await.expect("channel");
    channel.request_subsystem(true, "sftp").await.expect("subsystem");
    match russh_sftp::client::SftpSession::new(channel.into_stream()).await {
        Err(e) => println!("session: FAILED {e:?}"),
        Ok(sftp) => {
            println!("session: opened");
            match sftp.read_dir("/").await {
                Ok(list) => {
                    for e in list {
                        println!("  {} dir={}", e.file_name(), e.metadata().is_dir());
                    }
                }
                Err(e) => println!("read_dir: FAILED {e:?}"),
            }
            let _ = sftp.close().await;
        }
    }
}

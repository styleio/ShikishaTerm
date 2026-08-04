//! PTY入出力の切り分け用ヘッドレスプローブ (デバッグ用)
//! 使い方: cargo run --bin pty_probe -- <command> [args...]
//! 指定コマンドをPTYで起動し、出力を約10秒キャプチャして表示する。
//! 途中でテスト入力("echo PROBE_OK\r")も書き込む。

use std::io::{Read as _, Write as _};
use std::sync::mpsc;
use std::time::Duration;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: pty_probe <command> [args...]");
        std::process::exit(2);
    }

    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 30,
        cols: 100,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    let mut cmd = CommandBuilder::new(&args[0]);
    cmd.args(&args[1..]);
    cmd.cwd(std::env::current_dir()?);

    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => {
            println!("[probe] spawn OK");
            c
        }
        Err(e) => {
            println!("[probe] spawn FAILED: {e}");
            std::process::exit(1);
        }
    };
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut captured: Vec<u8> = Vec::new();
    for phase in 0..2 {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(200)) {
                // カーソル位置照会(DSR)には本物のターミナル同様応答する
                if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                    println!("[probe] DSR query detected -> replying \\x1b[30;1R");
                    let _ = writer.write_all(b"\x1b[30;1R");
                }
                captured.extend_from_slice(&chunk);
            }
            if let Ok(Some(status)) = child.try_wait() {
                println!("[probe] child exited: {status:?}");
                break;
            }
        }
        if phase == 0 {
            println!("[probe] sending test input: echo PROBE_OK\\r");
            let _ = writer.write_all(b"echo PROBE_OK\r");
        }
    }

    let _ = child.kill();
    println!("[probe] captured {} bytes", captured.len());
    println!("----- raw (escaped) -----");
    let text = String::from_utf8_lossy(&captured);
    for line in text.split(['\n', '\r']).filter(|l| !l.trim().is_empty()) {
        println!("{}", line.escape_debug());
    }
    Ok(())
}

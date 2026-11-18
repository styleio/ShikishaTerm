//! A headless probe for narrowing down PTY input and output problems (for debugging)
//! Usage: cargo run --bin pty_probe -- [--watch] [--resize COLS] <command> [args...]
//! Starts the given command in a PTY, captures its output for about 10 seconds, and prints it.
//! Also writes a test input ("echo PROBE_OK\r") partway through (with --watch, it writes nothing).
//! With --resize COLS, the width is changed to COLS halfway, and what the
//! terminal is sent after the change is printed apart: how a pseudo console
//! answers a new width (whether it draws the screen again) is read off it.

use std::io::{Read as _, Write as _};
use std::sync::mpsc;
use std::time::Duration;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // Watch only: capture what the program says and never write to it. Some of
    // the programs worth probing are agents, and typing into one of those is
    // not a test input -- it is a turn, on somebody's account. Cursor-position
    // queries are still answered: that is what a terminal is, not input.
    let watch_only = args.first().is_some_and(|a| a == "--watch");
    if watch_only {
        args.remove(0);
    }
    let resize: Option<u16> = match args.first().map(String::as_str) {
        Some("--resize") => {
            args.remove(0);
            Some(args.remove(0).parse()?)
        }
        _ => None,
    };
    if args.is_empty() {
        eprintln!("usage: pty_probe [--watch] <command> [args...]");
        std::process::exit(2);
    }

    // The width it starts at: 100, or PROBE_COLS
    let start_cols: u16 = std::env::var("PROBE_COLS").ok().and_then(|c| c.parse().ok()).unwrap_or(100);
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows: 30,
        cols: start_cols,
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
    let mut after_resize: Vec<u8> = Vec::new();
    for phase in 0..2 {
        if phase == 1
            && let Some(cols) = resize
        {
            println!("[probe] width {start_cols} -> {cols}");
            pair.master.resize(PtySize { rows: 30, cols, pixel_width: 0, pixel_height: 0 })?;
            // What it says when it is next written to, a moment after
            if !watch_only {
                std::thread::sleep(Duration::from_secs(2));
                println!("[probe] sending test input: echo PROBE_OK\\r");
                let _ = writer.write_all(b"echo PROBE_OK\r");
            }
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(200)) {
                // Answer a cursor position query (DSR) the way a real terminal would
                if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                    println!("[probe] DSR query detected -> replying \\x1b[30;1R");
                    let _ = writer.write_all(b"\x1b[30;1R");
                }
                // ...and what kind of terminal this is (DA1), which a pseudo
                // console asks as it starts
                if chunk.windows(3).any(|w| w == b"\x1b[c") {
                    let _ = writer.write_all(b"\x1b[?1;2c");
                }
                if phase == 1 && resize.is_some() {
                    after_resize.extend_from_slice(&chunk);
                }
                captured.extend_from_slice(&chunk);
            }
            if let Ok(Some(status)) = child.try_wait() {
                println!("[probe] child exited: {status:?}");
                break;
            }
        }
        if phase == 0 && !watch_only && resize.is_none() {
            println!("[probe] sending test input: echo PROBE_OK\\r");
            let _ = writer.write_all(b"echo PROBE_OK\r");
        }
    }

    let _ = child.kill();
    // Written to files as well when PROBE_DUMP names a folder: what came
    // before the resize and what came after, byte for byte, for a test to
    // read back
    if let (Some(_), Ok(dir)) = (resize, std::env::var("PROBE_DUMP")) {
        let before = &captured[..captured.len() - after_resize.len()];
        std::fs::write(std::path::Path::new(&dir).join("before.bin"), before)?;
        std::fs::write(std::path::Path::new(&dir).join("after.bin"), &after_resize)?;
    }
    if resize.is_some() {
        println!("----- after the resize, as sent ({} bytes) -----", after_resize.len());
        println!("{}", String::from_utf8_lossy(&after_resize).escape_debug());
    }
    println!("[probe] captured {} bytes", captured.len());
    println!("----- raw (escaped) -----");
    let text = String::from_utf8_lossy(&captured);
    for line in text.split(['\n', '\r']).filter(|l| !l.trim().is_empty()) {
        println!("{}", line.escape_debug());
    }
    Ok(())
}

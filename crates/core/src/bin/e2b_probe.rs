//! Does a terminal in a sandbox actually work?
//!
//! Run by hand, against the real service, because that is the only thing that
//! can answer it: the unit tests prove the frames are read correctly, and
//! nothing in a unit test can prove the far end answers the way the schema
//! says it does.
//!
//! One sandbox, one terminal, and it is killed on the way out -- a sandbox
//! costs money for as long as it is alive, so this is written to be run
//! sparingly and to clean up after itself even when it fails.
//!
//!   cargo run -p shikisha-core --bin e2b_probe

use std::io::{Read, Write};

fn main() {
    match probe() {
        Ok(()) => println!("\n-- the terminal in the sandbox works --"),
        Err(e) => {
            eprintln!("\n-- it does not: {e} --");
            std::process::exit(1);
        }
    }
}

fn probe() -> anyhow::Result<()> {
    let key = shikisha_core::e2b::key()
        .ok_or_else(|| anyhow::anyhow!("E2B_API_TOKEN is not set"))?;

    println!("asking for a machine...");
    let sandbox = shikisha_core::e2b::create(&key, "base", 5)?;
    println!("  got {}", sandbox.id);

    // Whatever happens from here, the machine is killed before this returns
    let out = run_on(&sandbox);
    println!("killing it...");
    let killed = shikisha_core::e2b::kill(&key, &sandbox.id);
    out?;
    killed?;
    Ok(())
}

fn run_on(sandbox: &shikisha_core::e2b::Sandbox) -> anyhow::Result<()> {
    println!("opening a terminal...");
    let (pty, mut killer) = shikisha_core::e2b::shell(sandbox, 24, 80, Some("/home/user"))?;
    let mut reader = pty.try_clone_reader()?;
    let mut writer = pty.take_writer()?;

    // What comes back, gathered on a thread of its own: a terminal has no end,
    // so reading it on this one would never return
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => {
                    if tx.send(String::from_utf8_lossy(&buf[..n]).to_string()).is_err() {
                        return;
                    }
                }
            }
        }
    });

    // A word nothing else would print, so seeing it means our typing arrived
    // and the answer came back -- not that a shell said something
    println!("typing...");
    writer.write_all(b"echo shikisha-$((6*7))-ok\n")?;
    writer.flush()?;

    let mut seen = String::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(chunk) => {
                print!("{chunk}");
                let _ = std::io::stdout().flush();
                seen.push_str(&chunk);
                // The echo of what was typed contains the sum unevaluated;
                // only the shell's own answer has it worked out
                if seen.contains("shikisha-42-ok") {
                    println!("\n  the shell answered");
                    // A window this size is what a resize has to survive
                    pty.resize(portable_pty::PtySize {
                        rows: 40,
                        cols: 120,
                        pixel_width: 0,
                        pixel_height: 0,
                    })?;
                    writer.write_all(b"tput cols\n")?;
                    writer.flush()?;
                    return wait_for(&rx, "120", &mut seen);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => anyhow::bail!("the terminal closed"),
        }
    }
    let _ = killer.kill();
    anyhow::bail!("nothing came back in 30 seconds");
}

/// Keep reading until the far end says a thing, or we give up on it.
fn wait_for(
    rx: &std::sync::mpsc::Receiver<String>,
    want: &str,
    seen: &mut String,
) -> anyhow::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let from = seen.len();
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(chunk) => {
                print!("{chunk}");
                let _ = std::io::stdout().flush();
                seen.push_str(&chunk);
                if seen[from..].contains(want) {
                    println!("\n  the new width reached it");
                    return Ok(());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => anyhow::bail!("the terminal closed"),
        }
    }
    anyhow::bail!("the resize never took: {want} was not printed")
}

//! What a pause and a start again do to a machine that is working: does the
//! run begin again, does the work go on, and is its clock right afterwards?
//!
//! The service counts the longest run its account allows from the moment a
//! machine was last started, and pausing and starting again is the way it
//! documents past that. Before this app does that to a machine an AI is
//! working on, it is measured here: a process ticking the time into a file
//! all along, one pause of `PAUSED` seconds, and the machine's clock read
//! against this one's afterwards. Also measured: how far the service lets a
//! run be lengthened, which is the account's longest run.
//!
//! One machine, killed on the way out, even when something fails.
//!
//!   cargo run -p shikisha-core --bin e2b_cycle_probe

use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PAUSED: u64 = 40;

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn main() {
    let key = match shikisha_core::e2b::key() {
        Some(k) => k,
        None => {
            eprintln!("E2B_API_TOKEN is not set");
            std::process::exit(2);
        }
    };
    let asking = shikisha_core::e2b::Asking {
        template: "base".into(),
        minutes: 5,
        marks: shikisha_core::e2b::marks("e2b_cycle_probe"),
        sign_in: None,
        private: false,
    };
    let sandbox = match shikisha_core::e2b::create(&key, &asking) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("no machine: {e:#}");
            std::process::exit(1);
        }
    };
    println!("machine {}", sandbox.id);
    let result = probe(&key, &sandbox);
    let _ = shikisha_core::e2b::kill(&key, &sandbox.id);
    println!("machine killed");
    if let Err(e) = result {
        eprintln!("-- stopped: {e:#} --");
        std::process::exit(1);
    }
}

fn run(s: &shikisha_core::e2b::Sandbox, cmd: &str) -> anyhow::Result<String> {
    Ok(shikisha_core::e2b::exec_within(s, cmd, None, Duration::from_secs(30))?.out.trim().to_string())
}

fn probe(key: &str, s: &shikisha_core::e2b::Sandbox) -> anyhow::Result<()> {
    let w = shikisha_core::e2b::window(key, &s.id)?;
    println!("made:    state {} run {}s long, ends in {}s", w.state, now().saturating_sub(w.started), w.ends.saturating_sub(now()));

    // How far a run can be lengthened: ask for a day, see what it became
    shikisha_core::e2b::keep_up_for(key, &s.id, 24 * 60)?;
    let w = shikisha_core::e2b::window(key, &s.id)?;
    println!("asked for 24h: the run may last {}s from its start (the account's longest run)", w.ends.saturating_sub(w.started));
    shikisha_core::e2b::keep_up_for(key, &s.id, 5)?;

    // Work that goes on through the pause: the time, once a second
    run(s, "nohup sh -c 'while true; do date +%s >> /tmp/ticks; sleep 1; done' >/dev/null 2>&1 & echo started")?;
    std::thread::sleep(Duration::from_secs(5));
    let pid_before = run(s, "pgrep -f 'date +%s' | head -1")?;
    let before = now();
    println!("pausing (a process ticking, pid {pid_before})");
    shikisha_core::e2b::pause(key, &s.id)?;
    let paused_at = now();
    println!("  paused in {}s", paused_at - before);
    std::thread::sleep(Duration::from_secs(PAUSED));
    let resumed = shikisha_core::e2b::connect(key, &s.id, 5)?;
    let back = now();
    println!("  started again in {}s", now().saturating_sub(paused_at + PAUSED));
    let w = shikisha_core::e2b::window(key, &resumed.id)?;
    println!("after:   state {} run {}s long (begun again: {})", w.state, now().saturating_sub(w.started), w.started >= paused_at);

    std::thread::sleep(Duration::from_secs(3));
    let clock: u64 = run(&resumed, "date +%s")?.parse().unwrap_or(0);
    let here = now();
    println!("clock:   the machine says {clock}, this PC {here}: {} s apart", clock as i64 - here as i64);
    let pid_after = run(&resumed, "pgrep -f 'date +%s' | head -1")?;
    println!("process: {} (pid {pid_after})", if pid_after == pid_before { "the same one, still running" } else { "not the same one" });
    let ticks = run(&resumed, "cat /tmp/ticks")?;
    let ticks: Vec<u64> = ticks.lines().filter_map(|l| l.trim().parse().ok()).collect();
    let gap = ticks.windows(2).map(|p| p[1].saturating_sub(p[0])).max().unwrap_or(0);
    println!("ticks:   {} written, the longest gap {}s (the pause was {}s)", ticks.len(), gap, back - paused_at);
    Ok(())
}

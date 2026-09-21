//! Whether each AI CLI is asking this app to report its conversations, and a
//! way to put that back without opening the settings screen.
//!
//! The hook is what lets a CLI say which conversation it is actually running.
//! Without it the app only knows the id it handed over at launch, so a CLI
//! that moves to another conversation on its own -- a person resuming a
//! different one, a `/clear` -- leaves the app's books naming a conversation
//! nobody is having. Everything looks fine until a restart, when the tab comes
//! up clean.
//!
//! That is not a hypothesis: on 2026-09-21 the real install had the hook in
//! none of its CLIs, and four tabs were remembering ids with no record behind
//! them. The settings screen could have said so, but nobody had looked -- so
//! this asks the same question from a terminal, and can answer it.
//!
//!     cargo run -p shikisha-core --bin hook_probe
//!     cargo run -p shikisha-core --bin hook_probe -- --install "C:\google\SHIKISHA-TERM\SHIKISHA-TERM.exe"
//!     cargo run -p shikisha-core --bin hook_probe -- --install <exe> --only claude
//!
//! The program to install is given rather than taken from this process. What
//! goes into the settings is a command line somebody's CLI will run on every
//! turn, and the obvious guess -- "whatever is running now" -- would write
//! THIS tool in, which is nothing anyone wants run on every turn.
//!
//! Installing leaves every other hook in the file exactly as it is (another
//! app's, a person's own), and keeps the previous contents beside it as
//! `.bak`. The plain listing writes nothing at all.

use std::path::PathBuf;

use shikisha_core::agenthook::{self, Status};

/// What the settings say, read against the app this run is about: the one
/// being installed when there is one, and otherwise this build, so a listing
/// on its own still answers "is it there at all"
fn seen(t: &agenthook::Target, program: Option<&std::path::Path>) -> Status {
    match program {
        Some(p) => agenthook::status_of(t, p),
        None => agenthook::status(t),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let after = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .filter(|a| !a.starts_with("--"))
            .cloned()
    };
    let install = args.iter().any(|a| a == "--install");
    let program = after("--install").map(PathBuf::from);
    // A name after --only narrows it to one CLI; without one, every CLI that
    // keeps a config on this machine is brought up to date
    let only = after("--only");

    if install && program.as_ref().is_none_or(|p| !p.is_file()) {
        eprintln!("--install needs the path of the app to be run, and it must exist");
        eprintln!(r#"  e.g. --install "C:\google\SHIKISHA-TERM\SHIKISHA-TERM.exe""#);
        std::process::exit(2);
    }

    let targets = agenthook::targets();
    if targets.is_empty() {
        println!("no CLI on this machine takes a hook");
        return;
    }
    for t in &targets {
        println!("{:<12} {:?}", t.name, seen(t, program.as_deref()));
        println!("             {}", t.file.display());
        let Some(program) = program.as_deref().filter(|_| install) else { continue };
        if only.as_deref().is_some_and(|n| !t.name.to_lowercase().contains(&n.to_lowercase())) {
            continue;
        }
        // A CLI that keeps no config here is not installed on this machine;
        // writing its file would create settings for a program that will never
        // read them
        if matches!(seen(t, Some(program)), Status::NoConfig) {
            println!("             skipped: no config to write into");
            continue;
        }
        match agenthook::install_as(t, program) {
            Ok(()) => println!("             -> installed, now {:?}", seen(t, Some(program))),
            Err(e) => println!("             -> NOT installed: {e}"),
        }
    }
}

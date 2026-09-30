//! The bridge: SHIKISHA-TERM's one program on a server or a MicroVM.
//!
//! Put there only when a person agreed to it (the app asks, per machine). It
//! takes on every job the app needs done on that machine (far-keep plan §3.1),
//! held by one resident process; the app comes in through a door it runs over
//! its own line. Everything it does is in `shikisha-core` (`fardaemon`,
//! `farlink`, `farops`), so the app and the bridge cannot disagree about it.
//!
//!     shikisha-bridge serve      the app's door, on standard input and output
//!     shikisha-bridge daemon     the resident process (started by the door)
//!     shikisha-bridge cli ...    the `shikisha` command of a tab on this machine
//!     shikisha-bridge --version

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-V") | Some("--version") => {
            println!("shikisha-bridge {} ({})", env!("CARGO_PKG_VERSION"), shikisha_core::build_rev());
        }
        Some("cli") => std::process::exit(shikisha_core::farlink::far_cli(&args[1..])),
        Some("serve") => std::process::exit(serve()),
        Some("daemon") => std::process::exit(daemon()),
        _ => {
            eprintln!("shikisha-bridge: started by SHIKISHA-TERM, not by hand (serve | daemon | cli | --version)");
            std::process::exit(2);
        }
    }
}

/// The bridge's folder: the one its program is in
#[cfg(unix)]
fn home() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

#[cfg(unix)]
fn serve() -> i32 {
    match shikisha_core::fardaemon::serve_port(home(), std::io::stdin(), std::io::stdout()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("shikisha-bridge: {e:#}");
            1
        }
    }
}

#[cfg(unix)]
fn daemon() -> i32 {
    match shikisha_core::fardaemon::daemon(home()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("shikisha-bridge: {e:#}");
            1
        }
    }
}

#[cfg(not(unix))]
fn serve() -> i32 {
    eprintln!("shikisha-bridge: serves on Linux and macOS only");
    2
}

#[cfg(not(unix))]
fn daemon() -> i32 {
    serve()
}

//! The bridge: SHIKISHA-TERM's small helper on a server or a MicroVM.
//!
//! Put there only when a person agreed to it (the app asks, per machine), and
//! run only while the app holds a line to it: it reads the line on its input
//! and ends when the line does. Everything it does is in `shikisha-core`
//! (`farlink`, `farops`), so the app and the bridge cannot disagree about it.
//!
//!     shikisha-bridge serve      the line from the app, on standard input and output
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
        _ => {
            eprintln!("shikisha-bridge: started by SHIKISHA-TERM, not by hand (serve | cli | --version)");
            std::process::exit(2);
        }
    }
}

/// The line from the app. The bridge's folder is the one its program is in
#[cfg(unix)]
fn serve() -> i32 {
    let home = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    match shikisha_core::farlink::serve_far(home, std::io::stdin(), std::io::stdout()) {
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

//! SHIKISHA with no window.
//!
//! Everything the app does that drawing is not needed for: the tabs and their
//! pseudo consoles, the state detector, the automation, the board served to
//! whoever connects. Meant for a machine nobody is sitting at -- a VPS, a cloud
//! VM -- where the thing that draws is somewhere else entirely.

/// What it answers when asked, rather than starting up.
const HELP: &str = "\
shikisha-serve -- SHIKISHA with no window

  shikisha-serve            open the tabs in the settings and serve the board

It reads its settings, and keeps everything it is given, under one folder:

  $SHIKISHA_HOME            if that is set
  beside the program        if a `config` folder is sitting there
  $XDG_DATA_HOME/shikisha   otherwise (~/.local/share/shikisha)

The address to open the board at is printed once the remote is on, and
written to the log beside the settings.

  -V, --version             which version this is
  -h, --help                this
";

fn main() -> anyhow::Result<()> {
    // Every option answers and stops, so only the first one is ever read
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-V" | "--version" => {
                println!(
                    "shikisha-serve {} ({})",
                    env!("CARGO_PKG_VERSION"),
                    shikisha_core::build_rev()
                );
                return Ok(());
            }
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            other => {
                eprintln!("shikisha-serve: unknown option: {other}");
                eprintln!("try: shikisha-serve --help");
                std::process::exit(2);
            }
        }
    }
    shikisha_core::serve::run()
}

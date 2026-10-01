//! SHIKISHA with no window.
//!
//! Everything the app does that drawing is not needed for: the tabs and their
//! pseudo consoles, the state detector, the automation, the board served to
//! whoever connects. Meant for a machine nobody is sitting at -- a VPS, a cloud
//! VM -- where the thing that draws is somewhere else entirely.

/// What it answers when asked, rather than starting up.
const HELP: &str = "\
shikisha-server -- SHIKISHA with no window

  shikisha-server           open the tabs in the settings and serve the board
  shikisha-server pair      a code, used once, to add this board to a PC
                            (SHIKISHA-TERM: Settings > Server version > Add)

It reads its settings, and keeps everything it is given, under one folder:

  $SHIKISHA_HOME            if that is set
  beside the program        if a `config` folder is sitting there
  $XDG_DATA_HOME/shikisha   otherwise (~/.local/share/shikisha)

The address to open the board at is printed once the remote is on, and
written to the log beside the settings.

  -V, --version             which version this is
  -h, --help                this
";

/// A code to add this board to a PC with, used once and good for ten
/// minutes, and the QR of the link that carries it for a phone. Written down
/// for the running board to take, in the folder they share (far-keep plan
/// §6.2). Run on the server's own terminal: whoever can run it here is the
/// server's own person
fn pair() {
    use shikisha_core::i18n::{t, tp};
    let code = shikisha_core::pairing::new_code();
    println!("{}", tp("msg.pair.code", &[("code", &code)]));
    match shikisha_core::pairing::board() {
        Some(base) => {
            println!("{}", tp("msg.pair.at", &[("url", &base)]));
            let link = format!("{base}/?pair={}", code.replace('-', ""));
            let qr = shikisha_core::netaddr::qr_text(&link);
            if !qr.is_empty() {
                println!("{}", t("msg.pair.qr"));
                println!("{qr}");
            }
        }
        None => println!("{}", t("msg.pair.no_board")),
    }
    println!("{}", t("msg.pair.once"));
}

fn main() -> anyhow::Result<()> {
    // Every option answers and stops, so only the first one is ever read
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-V" | "--version" => {
                println!(
                    "shikisha-server {} ({})",
                    env!("CARGO_PKG_VERSION"),
                    shikisha_core::build_rev()
                );
                return Ok(());
            }
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "pair" => {
                pair();
                return Ok(());
            }
            other => {
                eprintln!("shikisha-server: unknown option: {other}");
                eprintln!("try: shikisha-server --help");
                std::process::exit(2);
            }
        }
    }
    shikisha_core::serve::run()
}

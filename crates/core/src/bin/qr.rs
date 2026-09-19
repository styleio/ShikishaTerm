//! A link as a square a phone can read.
//!
//!   cargo run -p shikisha-core --bin qr -- "http://100.64.0.1:8787/?t=..."
//!
//! The window draws this on the settings screen. This is the same code for
//! anywhere there is no window: a build running on a server, and a check being
//! run by hand. A pairing link read off the screen and typed into a phone is a
//! link that mostly does not get typed.
fn main() {
    let text: Vec<String> = std::env::args().skip(1).collect();
    let text = text.join(" ");
    if text.trim().is_empty() {
        eprintln!("say what to put in it");
        std::process::exit(2);
    }
    print!("{}", shikisha_core::netaddr::qr_text(&text));
    println!("{text}");
}

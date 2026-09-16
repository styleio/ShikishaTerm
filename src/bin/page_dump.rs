//! Write the board page out exactly as it is served, for looking at it in a
//! browser instead of in the app.
//!
//! The screen rules (docs/design/STYLEGUIDE.md, section 9) ask every session
//! that changes the screen to photograph it and mark it against a list. Doing
//! that through the app means a build, a window, and whatever the screen needs
//! to be showing -- a server to be connected to, a repository mid-merge. This
//! writes the same page to a file, where a browser can open it and a script can
//! put it into the state being judged. tools/shoot.mjs is that script.
//!
//!     cargo run --bin page_dump -- ja        the page in Japanese
//!     cargo run --bin page_dump -- ja light  ...in the light scheme
//!
//! It is a tool for whoever is building this, not part of what is handed out.
fn main() {
    let lang = std::env::args().nth(1).unwrap_or_else(|| "en".into());
    let light = std::env::args().nth(2).as_deref() == Some("light");
    shikisha_core::i18n::init(
        Some(&lang),
        &[std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))],
    );
    let mut html = shikisha_core::shell::page();
    // The scheme is the one in the settings, and a person's own settings are
    // not a test fixture. Asked for by name instead, so both halves of "check
    // it in the light scheme too" can be photographed on any machine
    if light {
        let Some(scheme) = shikisha_core::theme::available()
            .into_iter()
            .find(shikisha_core::theme::is_light)
        else {
            eprintln!("no light scheme is installed");
            std::process::exit(1);
        };
        html = html.replace(
            "</head>",
            &format!("<style>:root{{{}}}</style></head>", scheme.css_vars()),
        );
    }
    print!("{html}");
}

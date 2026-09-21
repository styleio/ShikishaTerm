//! Write the board page out exactly as it is served, for looking at it in a
//! browser instead of in the app.
//!
//! The screen rules (docs/design/STYLEGUIDE.md, section 9) ask every session
//! that changes the screen to photograph it and mark it against a list. Doing
//! that through the app means a build, a window, and whatever the screen needs
//! to be showing -- a server to be connected to, a repository mid-merge. This
//! writes the same page to a file, where a browser can open it and a script can
//! put it into the state being judged. tools/debug/shoot.mjs is that script.
//!
//!     cargo run --bin page_dump -- ja         the page in Japanese
//!     cargo run --bin page_dump -- ja light   ...in the light scheme
//!     cargo run --bin page_dump -- ja remote  ...as a phone is served it
//!     cargo run --bin page_dump -- state      the board state, not the page
//!
//! `light`, `remote` and `state` are words, in any order, after the language.
//! `remote` matters because the page a phone gets is not the window's page at a
//! narrow width: it carries controls the window does not have (RESTART in the
//! bar) and leaves out ones only the window can act on.
//!
//! `state` writes what the app hands the page on its first draw, from the very
//! type the app sends it from (`UiState`), so a page opened outside the app can
//! be given a board to draw with no app running. Nothing is left to a fixture
//! written by hand: a field added to the state is in this the day it is added.
//! tools/check-board.mjs is what asks for it.
//!
//! It is a tool for whoever is building this, not part of what is handed out.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |w: &str| args.iter().any(|a| a == w);
    let light = flag("light");
    let remote = flag("remote");
    let state = flag("state");
    let lang = args
        .iter()
        .find(|a| !matches!(a.as_str(), "light" | "remote" | "state"))
        .cloned()
        .unwrap_or_else(|| "en".into());
    shikisha_core::i18n::init(
        Some(&lang),
        &[std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))],
    );
    // A board with nothing on it yet: what the page is handed first, and the
    // one state every desk passes through on its way to showing anything
    if state {
        print!(
            "{}",
            serde_json::to_string(&shikisha_core::uistate::UiState::default())
                .expect("the state is serializable")
        );
        return;
    }
    let mut html = shikisha_core::shell::served_page(
        false,
        if remote {
            shikisha_core::shell::Served::Remote
        } else {
            shikisha_core::shell::Served::Window
        },
    );
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
            // And the parts the browser draws itself (check boxes, scrollbars)
            // drawn light as well, the way the app serves a light scheme
            &format!("<style>:root{{{}color-scheme:light;}}</style></head>", scheme.css_vars()),
        );
    }
    print!("{html}");
}

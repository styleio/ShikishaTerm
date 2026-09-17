//! Serve the settings page on its own, over a settings file of your choosing,
//! for looking at it in a browser instead of in the app.
//!
//! The screen rules (docs/design/STYLEGUIDE.md, section 9) ask for a photograph
//! of every screen that changed. The settings page is served by the app, and
//! the app edits the settings it runs on -- which, on the machine the work is
//! done on, are somebody's real ones. This serves the same page from the same
//! code over a copy in a temporary folder, so the page can be pressed and saved
//! without touching anything that matters. tools/debug/scenes/ drive it.
//!
//!     cargo run --bin settings_serve -- ja [settings.json]
//!
//! Prints the address, then serves until it is stopped. Without a settings
//! file it starts from an empty one.
//!
//! It is a tool for whoever is building this, not part of what is handed out.
fn main() {
    let lang = std::env::args().nth(1).unwrap_or_else(|| "en".into());
    shikisha_core::i18n::init(
        Some(&lang),
        &[std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))],
    );
    let dir = std::env::temp_dir().join(format!("shikisha-settings-serve-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("could not make a temporary folder");
    let config = dir.join("config.json");
    let start = match std::env::args().nth(2) {
        Some(from) => std::fs::read_to_string(&from).unwrap_or_else(|e| {
            eprintln!("could not read {from}: {e}");
            std::process::exit(1);
        }),
        None => "{}".into(),
    };
    std::fs::write(&config, start).expect("could not write the settings copy");
    let ui = shikisha_core::webui::WebUi::start_with(
        config.clone(),
        std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
        std::sync::Arc::new(std::sync::Mutex::new(None)),
    )
    .expect("the settings page did not start");
    println!("{}", ui.url);
    println!("{}", config.display());
    use std::io::Write as _;
    let _ = std::io::stdout().flush();
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

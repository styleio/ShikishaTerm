//! Installing the phone view on a phone.
//!
//! The phone already gets a whole screen (src/shell.rs) over the private
//! network. What it does not get, in a browser tab, is a place to live: it is
//! one tab among thirty, behind an address bar, found by scrolling. An
//! installed copy is the same page with a name and a picture on the home
//! screen, opening without the browser's furniture around it.
//!
//! Two things make that possible, and both have to be reachable **without the
//! access token**:
//!
//!   - the manifest, which a browser fetches on its own, from its own network
//!     stack, with none of the page's credentials attached;
//!   - the icons, which the launcher fetches later still — possibly days
//!     later, possibly while the app is not even running.
//!
//! That is safe for exactly the reason the shell page itself is served without
//! a token: none of it is state. The manifest says what the app is called and
//! which colour its splash screen is; the icons are a drawing. Every route
//! that can read a tab, send a keystroke or reach a file is still behind the
//! token, one screen further in.
//!
//! **The token is never put in `start_url`.** The page goes to some trouble to
//! get the token out of the address bar and out of anything that persists
//! (see TOKEN in shell.rs); a manifest is about as persistent as a URL gets —
//! it is copied into the launcher's own storage and synced between devices.
//! So the installed app opens the same tokenless shell a fresh tab does, and
//! signs itself in the same way: from what the pairing left behind.
//!
//! Which is why the manifest is only *offered* when the token is the fixed
//! kind (`remote.sticky_token`). A rotating token lives in `sessionStorage`,
//! and a launcher opens a new session every time — an icon installed in that
//! mode would open, ask to be paired again, and be unable to be, because
//! pairing means scanning a QR code, which opens the browser rather than the
//! installed app. An install that cannot work is worse than no install
//! offered, so it is not offered. The routes below still answer, because
//! nothing about them is worth gating and a stale launcher may still ask.

/// The mark, drawn edge to edge, for launchers that show an icon whole.
const ICON_192: &[u8] = include_bytes!("../../../assets/pwa/icon-192.png");
const ICON_512: &[u8] = include_bytes!("../../../assets/pwa/icon-512.png");
/// The same mark inside the safe zone, on a field, for launchers that cut a
/// shape out of what they are given.
const MASKABLE_192: &[u8] = include_bytes!("../../../assets/pwa/maskable-192.png");
const MASKABLE_512: &[u8] = include_bytes!("../../../assets/pwa/maskable-512.png");
/// iOS reads neither `purpose` nor `sizes`; it takes this one and rounds the
/// corners itself.
const APPLE_TOUCH: &[u8] = include_bytes!("../../../assets/pwa/apple-touch-icon.png");

/// Where the manifest lives. Named, rather than spelled out at each use, so
/// the page and the route cannot drift apart.
pub const MANIFEST_PATH: &str = "/manifest.webmanifest";

/// The picture for a path, if that path is one of ours.
///
/// Returns the bytes and how long they may be kept. Icons are the one thing
/// here worth caching hard: a launcher may ask for one long after the app has
/// stopped, and the drawing only changes when the program is replaced.
pub fn icon(path: &str) -> Option<&'static [u8]> {
    Some(match path {
        "/pwa/icon-192.png" => ICON_192,
        "/pwa/icon-512.png" => ICON_512,
        "/pwa/maskable-192.png" => MASKABLE_192,
        "/pwa/maskable-512.png" => MASKABLE_512,
        // At the root, because that is where iOS looks when a page says
        // nothing — and an old bookmark may well say nothing.
        "/apple-touch-icon.png" => APPLE_TOUCH,
        _ => return None,
    })
}

/// The background the splash screen is painted with: the terminal's own, so
/// that opening the app does not flash a colour it never uses again.
fn background() -> String {
    let look = crate::config::load().map(|c| c.appearance).unwrap_or_default();
    look.scheme().bg()
}

/// The manifest itself.
pub fn manifest_json() -> String {
    let bg = background();
    let icon = |path: &str, size: u32, purpose: &str| {
        serde_json::json!({
            "src": path,
            "sizes": format!("{size}x{size}"),
            "type": "image/png",
            "purpose": purpose,
        })
    };
    serde_json::json!({
        "name": "SHIKISHA-TERM",
        // What fits under an icon. The long name is elided to "SHIKISHA-…" on
        // most launchers, which is the half that says nothing.
        "short_name": "SHIKISHA",
        "description": crate::i18n::t("pwa.description"),
        "lang": crate::i18n::lang(),
        // Relative, and deliberately: this app answers on whatever address the
        // private network gave it, and that address is not ours to guess.
        "start_url": "./",
        "scope": "/",
        // No browser chrome, but the tab bar and the terminal both scroll on
        // their own, so the page keeps its own way back rather than borrowing
        // the browser's.
        "display": "standalone",
        "background_color": bg,
        "theme_color": bg,
        "icons": [
            icon("/pwa/icon-192.png", 192, "any"),
            icon("/pwa/icon-512.png", 512, "any"),
            icon("/pwa/maskable-192.png", 192, "maskable"),
            icon("/pwa/maskable-512.png", 512, "maskable"),
        ],
    })
    .to_string()
}

/// The tags the shell page puts in its head.
///
/// `sticky` decides whether an install is offered at all — see the note at the
/// top of this file. The rest is offered either way: a colour for the browser's
/// own bar, and a picture for a bookmark, are worth having in a tab too.
pub fn head(sticky: bool) -> String {
    let bg = background();
    let mut out = format!(
        "<meta name=\"theme-color\" content=\"{bg}\">\n\
         <link rel=\"apple-touch-icon\" href=\"/apple-touch-icon.png\">\n\
         <link rel=\"icon\" type=\"image/png\" sizes=\"192x192\" href=\"/pwa/icon-192.png\">"
    );
    if sticky {
        out.push_str(&format!(
            "\n<link rel=\"manifest\" href=\"{MANIFEST_PATH}\">\n\
             <meta name=\"mobile-web-app-capable\" content=\"yes\">\n\
             <meta name=\"apple-mobile-web-app-capable\" content=\"yes\">\n\
             <meta name=\"apple-mobile-web-app-title\" content=\"SHIKISHA\">\n\
             <!-- The status bar is drawn over the page rather than above it, so\n\
                  the app's own colour runs to the top of the screen. The page\n\
                  keeps clear of the notch with env(safe-area-inset-*). -->\n\
             <meta name=\"apple-mobile-web-app-status-bar-style\" content=\"black-translucent\">"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon the manifest promises has to be a route that answers.
    ///
    /// A manifest naming a picture that 404s is not a small fault: Chrome
    /// refuses the whole install, silently, and the only symptom is that the
    /// "add to home screen" offer never appears.
    #[test]
    fn every_promised_icon_is_served() {
        let m: serde_json::Value = serde_json::from_str(&manifest_json()).unwrap();
        let icons = m["icons"].as_array().unwrap();
        assert_eq!(icons.len(), 4);
        for i in icons {
            let src = i["src"].as_str().unwrap();
            let bytes = icon(src).unwrap_or_else(|| panic!("{src} が配られていない"));
            assert_eq!(&bytes[1..4], b"PNG", "{src} が PNG ではない");
            // The size in the manifest is a promise about the file, not a hint.
            let want: u32 = i["sizes"].as_str().unwrap().split('x').next().unwrap().parse().unwrap();
            let wide = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
            let high = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
            assert_eq!((wide, high), (want, want), "{src} の寸法が宣言と違う");
        }
        assert!(icon("/apple-touch-icon.png").is_some());
        assert!(icon("/pwa/../config.lua").is_none());
    }

    /// The token must never end up somewhere a launcher keeps.
    /// What a browser reads before it will offer to install anything.
    ///
    /// Written down because the list is somebody else's and silent: a missing
    /// field does not produce an error, it produces a browser that never
    /// offers the install and never says why. The one condition not checkable
    /// from here is the address being a secure context, which is the settings
    /// page's business (it says so, and says what to run).
    #[test]
    fn the_manifest_says_everything_a_browser_asks_for() {
        let m: serde_json::Value = serde_json::from_str(&manifest_json()).unwrap();
        for named in ["name", "short_name", "start_url", "display", "icons"] {
            assert!(!m[named].is_null(), "{named} が無い");
        }
        // A window of its own, rather than a tab with the address bar above it
        assert_eq!(m["display"], "standalone");
        // The two sizes every launcher is promised. Smaller ones are allowed
        // to be missing; these two are not
        let sizes: Vec<&str> = m["icons"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|i| i["sizes"].as_str())
            .collect();
        for want in ["192x192", "512x512"] {
            assert!(sizes.contains(&want), "{want} の絵が無い: {sizes:?}");
        }
        // And one a launcher may cut a shape out of without cutting the mark
        assert!(
            m["icons"].as_array().unwrap().iter().any(|i| i["purpose"] == "maskable"),
            "型抜きされる launcher 向けの絵が無い"
        );
    }

    #[test]
    fn the_manifest_carries_no_token() {
        let m = manifest_json();
        assert!(!m.contains("?t="), "start_url にトークンが乗っている");
        assert!(m.contains("\"start_url\":\"./\""));
    }

    /// An install is offered only when the pairing outlives the tab.
    #[test]
    fn install_is_offered_only_when_it_would_work() {
        assert!(!head(false).contains("rel=\"manifest\""));
        assert!(head(true).contains(MANIFEST_PATH));
        // The picture and the colour are worth having in a plain tab too.
        for s in [head(false), head(true)] {
            assert!(s.contains("apple-touch-icon"));
            assert!(s.contains("theme-color"));
        }
    }
}

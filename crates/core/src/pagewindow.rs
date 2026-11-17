//! The windows pages open in a browser that has no window to show them in
//! (the server's), and the tabs made for them.
//!
//! Such a window is said to the loop, which makes a tab for it; the window
//! itself is kept by the browser's host and taken by that tab as its page
//! (`chrome`). Which tab is for which window is written here, by the
//! window's own name -- the name the browser gave it, which no page chooses
//! and no two windows share.
//!
//! By name, and not by where the window was going. Told apart by address, a
//! private tab opened at the same address as a waiting window took that
//! window -- one opened by an ordinary page, in the ordinary storage -- and
//! a form posted into a new window was thrown away because a tab already
//! showed that address.
//!
//! And the tab is known by its desk's own id and its own, never by where
//! the desk stands in the list: desks are put in another order while a
//! window waits, and a tab of the same name on the desk that then stood
//! there would have taken a window a page of another desk opened.

use std::sync::Mutex;

/// What a tab made for a window is marked with in the settings, with the
/// window's name: the tab goes when the window closes itself, as a window
/// does. Taken off every tab at the next start, when no window is left
pub const MARK: &str = "window_of";

/// How many tabs may be waiting to take their windows
const MOST: usize = 64;

/// Each tab made for a window that has not taken it yet: (the desk's own
/// id, the tab's, the window)
static CLAIMS: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

/// The tab `tab` of the desk whose own id is `desk` was made for `window`
pub fn claim(desk: &str, tab: &str, window: &str) {
    let mut claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    claims.retain(|(d, t, w)| !(d == desk && t == tab) && w != window);
    if claims.len() >= MOST {
        claims.remove(0);
    }
    claims.push((desk.to_string(), tab.to_string(), window.to_string()));
}

/// The window that tab was made for, if it has not taken it yet. Asked
/// once: the tab is opening its page, and is told which window is its
/// (`BrowserProfile::window`)
pub fn take(desk: &str, tab: &str) -> Option<String> {
    let mut claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    let at = claims.iter().position(|(d, t, _)| d == desk && t == tab)?;
    Some(claims.remove(at).2)
}

/// Whether a tab was made for `window` and has yet to take it
pub fn claimed(window: &str) -> bool {
    CLAIMS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(_, _, w)| w == window)
}

/// `window` is gone, or is given up on: no tab is to take it
pub fn let_go(window: &str) {
    CLAIMS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, _, w)| w != window);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tab takes the window it was made for, once, and no other tab's
    #[test]
    fn a_tab_takes_the_window_it_was_made_for_and_only_that() {
        claim("desk-b", "receipt", "window-a");
        claim("desk-b", "receipt-2", "window-b");
        assert!(claimed("window-a") && claimed("window-b"));
        assert_eq!(take("desk-b", "other"), None, "a tab not made for a window took one");
        // A tab of the same name on another desk is another tab, wherever
        // either desk stands in the list
        assert_eq!(take("desk-a", "receipt"), None, "a tab of another desk took the window");
        assert_eq!(take("desk-b", "receipt").as_deref(), Some("window-a"));
        assert_eq!(take("desk-b", "receipt"), None, "taken twice");
        assert!(!claimed("window-a"));
        // A window that is gone is nobody's to take
        let_go("window-b");
        assert_eq!(take("desk-b", "receipt-2"), None);
        // Never more than so many waiting
        for n in 0..MOST + 5 {
            claim("desk-c", &format!("t{n}"), &format!("w{n}"));
        }
        assert!(!claimed("w0") && claimed(&format!("w{}", MOST + 4)));
        for n in 0..MOST + 5 {
            let_go(&format!("w{n}"));
        }
    }
}

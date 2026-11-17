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

use std::sync::Mutex;

/// What a tab made for a window is marked with in the settings, with the
/// window's name: the tab goes when the window closes itself, as a window
/// does. Taken off every tab at the next start, when no window is left
pub const MARK: &str = "window_of";

/// How many tabs may be waiting to take their windows
const MOST: usize = 64;

/// Each tab made for a window that has not taken it yet: (the tab's page, the window)
static CLAIMS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// The tab whose page is `tab` was made for `window`
pub fn claim(tab: &str, window: &str) {
    let mut claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    claims.retain(|(t, w)| t != tab && w != window);
    if claims.len() >= MOST {
        claims.remove(0);
    }
    claims.push((tab.to_string(), window.to_string()));
}

/// The window the tab whose page is `tab` was made for, if it has not taken
/// it yet. Asked once: the tab is opening its page
pub fn take(tab: &str) -> Option<String> {
    let mut claims = CLAIMS.lock().unwrap_or_else(|e| e.into_inner());
    let at = claims.iter().position(|(t, _)| t == tab)?;
    Some(claims.remove(at).1)
}

/// Whether a tab was made for `window` and has yet to take it
pub fn claimed(window: &str) -> bool {
    CLAIMS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(_, w)| w == window)
}

/// `window` is gone, or is given up on: no tab is to take it
pub fn let_go(window: &str) {
    CLAIMS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(_, w)| w != window);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tab takes the window it was made for, once, and no other tab's
    #[test]
    fn a_tab_takes_the_window_it_was_made_for_and_only_that() {
        claim("7/receipt", "window-a");
        claim("7/receipt-2", "window-b");
        assert!(claimed("window-a") && claimed("window-b"));
        assert_eq!(take("7/other"), None, "a tab not made for a window took one");
        assert_eq!(take("7/receipt").as_deref(), Some("window-a"));
        assert_eq!(take("7/receipt"), None, "taken twice");
        assert!(!claimed("window-a"));
        // A window that is gone is nobody's to take
        let_go("window-b");
        assert_eq!(take("7/receipt-2"), None);
        // Never more than so many waiting
        for n in 0..MOST + 5 {
            claim(&format!("8/t{n}"), &format!("w{n}"));
        }
        assert!(!claimed("w0") && claimed(&format!("w{}", MOST + 4)));
        for n in 0..MOST + 5 {
            let_go(&format!("w{n}"));
        }
    }
}

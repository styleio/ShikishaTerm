//! What the two doors a DevTools screen connects through agree on.
//!
//! The browser's own DevTools screen (`devtools://devtools/...`) talks the
//! DevTools protocol over a WebSocket it is given the address of. This
//! program gives it a door of its own on the loopback, never the browser's
//! port: one in the window (`src/devtools.rs`, for a page the window draws)
//! and one beside the server's browser (`chrome.rs`). Both let in only that
//! screen, and both hold to the same limits, which are written once here

/// The one origin a connection is answered from
pub const ORIGIN: &str = "devtools://devtools";

/// The most the screen may say in one message. What it sends are commands --
/// a node to inspect, an expression to run -- and a command larger than
/// this is a script pasted into the console, which still fits
pub const ONE_COMMAND: usize = 8 * 1024 * 1024;

/// How far behind a screen may fall before it is let go. A page that says a
/// great deal to a screen that has stopped reading -- a tab asleep, a line
/// gone slow -- would otherwise be kept in memory without end
pub const BEHIND: usize = 64 * 1024 * 1024;

/// How long a screen may take nothing of what is written to it before it is
/// taken for gone
pub const STALLED: std::time::Duration = std::time::Duration::from_secs(30);

/// How much a screen may say before its page has a session for it. It says
/// a handful of commands as it opens; more than this is not a screen opening
pub const EARLY: usize = 16 * 1024 * 1024;

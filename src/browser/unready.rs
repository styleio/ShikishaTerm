//! The window's engine on a system that has none yet.
//!
//! The window is drawn by Chromium. On Windows that is WebView2, which the
//! system carries; elsewhere the program carries its own (CEF), and until it
//! does, a window cannot be opened here. Everything else in `browser` builds
//! and runs the same, so the program around the window can be built and
//! tested on every system meanwhile; this is the one place that says the
//! window itself is not there.

use super::*;

/// Says the window cannot be opened here, and nothing else. The caller hears
/// it the way it hears any window that could not be made: in the log, and as
/// the window closing
pub(super) fn run_window(
    _url: &str,
    _title: &str,
    _wears_the_icon: bool,
    _proxy_tx: Sender<tao::event_loop::EventLoopProxy<Cmd>>,
    _ev_tx: Sender<Ev>,
    _sound_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,
) -> Result<()> {
    Err(anyhow!("this system has no engine to draw the window with yet"))
}

/// No engine is on this system to be asked for its version
pub fn runtime_version() -> Option<String> {
    None
}

/// There is no window to name
pub fn main_hwnd() -> isize {
    0
}

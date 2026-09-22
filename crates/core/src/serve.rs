//! Running with no shell at all.
//!
//! The same loop the window drives, handed something that draws nothing. Tabs
//! are opened, their state is watched, automation fires, and the board is
//! served to whoever connects -- a browser on this machine, a phone, a laptop
//! across a tailnet. What a shell would have provided (a page to click on, a
//! banner, a file dialog, a clipboard) is simply absent, and the traits that
//! stand for those answer honestly instead of pretending.
//!
//! This is what a VPS or a cloud VM runs. There is no second loop here: a
//! runtime with a window and a runtime without one are the same code, given a
//! different shell.

use crate::host::Headless;
use anyhow::Result;

/// The terminal a tab gets when no window is deciding its size. A phone that
/// connects can resize it afterwards; this is only what it starts at
const ROWS: u16 = 43;
const COLS: u16 = 140;

/// Boot the runtime and keep going until it is stopped.
pub fn run() -> Result<()> {
    started(Headless::new(ROWS, COLS))
}

/// The same, with somebody keeping a window over it in another process.
///
/// This is a program split in two on one machine rather than a server nobody
/// is sitting at, and the difference is entirely the minder's: it starts the
/// window, watches it, puts it back when it is taken, and holds the icon that
/// says the work is still going while nothing is on screen. The loop below
/// does not know which of the two it is running
pub fn run_minded(minder: Box<dyn crate::host::Minder>) -> Result<()> {
    started(Headless::new(ROWS, COLS).minded_by(minder))
}

fn started(mut shell: Headless) -> Result<()> {
    // The same carrying-forward the window does before it reads anything: a
    // settings file from an older version is as likely to be on a server
    crate::migrate::prepare();
    crate::runtime::run(&mut shell)
}

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
    let mut shell = Headless::new(ROWS, COLS);
    crate::runtime::run(&mut shell)
}

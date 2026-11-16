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

/// Set when this program is told to stop from outside: `systemctl stop`
/// (SIGTERM), Ctrl+C in the terminal it was started from (SIGINT), that
/// terminal closing (SIGHUP). Left alone, each of them ends the program
/// where it stands: the loop never gets to its way out, where the terminals
/// of AIs set to go on while it is away are let go of rather than ended, a
/// MicroVM is paused or kept up for the time chosen, and what was on screen
/// is written down for the next start. So the signal only raises this, and
/// the loop leaves by its own door (`Mailbox::told_to_end`), within the time
/// the service gives a stop
static TOLD_TO_END: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_stop(_: libc::c_int) {
    TOLD_TO_END.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Whether it was told to stop since this was last asked
pub fn told_to_end() -> bool {
    TOLD_TO_END.swap(false, std::sync::atomic::Ordering::SeqCst)
}

/// Hear the three ways of being told to stop
fn hear_stops() {
    // SAFETY: the handler only stores to an atomic, which is all a signal
    // handler may do
    #[cfg(unix)]
    unsafe {
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            let was = libc::signal(sig, on_stop as extern "C" fn(libc::c_int) as libc::sighandler_t);
            // One it was started not to hear stays unheard: `nohup` asks
            // exactly that of SIGHUP, so that closing the terminal ends nothing
            if was == libc::SIG_IGN {
                libc::signal(sig, libc::SIG_IGN);
            }
        }
    }
}

fn started(mut shell: Headless) -> Result<()> {
    hear_stops();
    // The same carrying-forward the window does before it reads anything: a
    // settings file from an older version is as likely to be on a server
    crate::migrate::prepare();
    crate::runtime::run(&mut shell)
}

//! The runtime half of a program split in two on one machine.
//!
//! One process holds the tabs, the pseudo consoles, the automation and every
//! AI at work in them, and draws nothing. Another process is the window, and
//! it is where nearly all the memory is -- a browser engine, several processes
//! of its own, a few hundred megabytes. Being two programs is what lets the
//! second one be taken without the first one noticing: killed for its memory,
//! or gone in a crash, the work goes on and a new window is put over it.
//!
//! In one process none of that was possible. The window and the runtime failed
//! together because they *were* together, and an allocation that could not be
//! met ended the whole thing -- every conversation in every tab with it.
//!
//! What this is, exactly: a [`Minder`](crate::host::Minder) for the headless
//! runtime. It starts the window, watches it, puts it back when it was taken,
//! and holds the notification-area icon that says the work is still going
//! while no window is up. Everything it notices it says in the words a window
//! would have used -- closed, quit, open -- so the loop reads them in the one
//! place it already reads them and the `resident` setting goes on meaning
//! exactly what it always meant.
//!
//! What it deliberately does NOT do is decide what a ✕ costs. See
//! [`crate::keeper`].

use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

use crate::host::{Minder, Told};
use crate::keeper::{Do, Keeper};
use crate::resident::Pressed;

/// A clock that only goes forwards, counted from the first time it is asked.
/// The keeper wants milliseconds and does not care where zero is
fn now_ms() -> u64 {
    use std::sync::OnceLock;
    static ZERO: OnceLock<std::time::Instant> = OnceLock::new();
    ZERO.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

/// The program that draws. Beside this one, because the two halves are always
/// installed together and always the same version -- a window from one release
/// and a runtime from another do not have to understand each other.
///
/// Falls back to this program itself, which is the ordinary case: the window
/// and the runtime are the same executable started two different ways.
fn window_program() -> std::path::PathBuf {
    let here = std::env::current_exe().unwrap_or_else(|_| crate::config::exe_dir().join(WINDOW));
    window_program_in(&crate::config::exe_dir(), here)
}

/// What the window program is called where it is installed
const WINDOW: &str = if cfg!(windows) { "SHIKISHA-TERM.exe" } else { "SHIKISHA-TERM" };

/// The same, with the folder to look in and the fallback handed over, so the
/// choosing can be read without an installation around it
fn window_program_in(dir: &std::path::Path, current: std::path::PathBuf) -> std::path::PathBuf {
    let beside = dir.join(WINDOW);
    if beside.is_file() { beside } else { current }
}

/// The runtime's half of the pair
pub struct Split {
    /// Where the board is reached, once it is listening. Empty until then,
    /// which is why no window is started before it
    board: RefCell<String>,
    /// The window, while there is one
    window: RefCell<Option<std::process::Child>>,
    /// What has been tried and when to look again
    keep: RefCell<Keeper>,
    /// The icon, and the window nobody sees that carries it
    icon: Option<crate::resident::Standing>,
    /// What the icon has been told and nobody has read yet. Filled from the
    /// thread Windows calls the icon's handler on, emptied by the loop
    presses: Arc<Mutex<Vec<Pressed>>>,
    /// No window, and none wanted until one is asked for. True after a ✕ the
    /// setting said to wait out, and after the keeper has given up trying
    resting: Cell<bool>,
}

impl Split {
    /// Put the icon up and stand ready. No window is started here: there is
    /// nothing to point one at until the board is listening, which the runtime
    /// says with [`Minder::board_is_at`]
    pub fn new() -> Self {
        let presses: Arc<Mutex<Vec<Pressed>>> = Arc::default();
        let heard = Arc::clone(&presses);
        let icon = crate::resident::start(
            "SHIKISHA-TERM",
            &crate::i18n::t("tray.open"),
            &crate::i18n::t("tray.quit"),
            move |p| heard.lock().unwrap().push(p),
        )
        .map_err(|e| crate::append_hook_log(&format!("split: no icon: {e}")))
        .ok();
        Self {
            board: RefCell::default(),
            window: RefCell::default(),
            keep: RefCell::new(Keeper::new()),
            icon,
            presses,
            resting: Cell::new(false),
        }
    }

    /// Start a window on this runtime's board.
    ///
    /// Counted as an attempt whether or not it starts, so a window that cannot
    /// start at all is tried twice and then left to the icon, exactly like one
    /// that starts and dies
    fn open(&self) {
        let board = self.board.borrow().clone();
        if board.is_empty() {
            return;
        }
        self.resting.set(false);
        self.keep.borrow_mut().opened(now_ms());
        let program = window_program();
        match std::process::Command::new(&program).arg("--connect").arg(&board).spawn() {
            Ok(child) => {
                crate::append_hook_log(&format!("split: a window started (pid {})", child.id()));
                *self.window.borrow_mut() = Some(child);
            }
            Err(e) => {
                crate::append_hook_log(&format!("split: no window from {}: {e}", program.display()));
                *self.window.borrow_mut() = None;
            }
        }
    }

    /// Stop trying and say where the way back is. The icon is the only thing
    /// on screen at this point, so it is the only place the offer can be made
    fn offer(&self) {
        self.resting.set(true);
        if let Some(icon) = &self.icon {
            icon.notice(
                &crate::i18n::t("tray.screen_down.title"),
                &crate::i18n::t("tray.screen_down.body"),
            );
        }
    }

    /// Whatever the icon has been told since the last turn
    fn heard(&self) -> Told {
        let mut told = Told::default();
        for p in self.presses.lock().unwrap().drain(..) {
            match p {
                Pressed::Open => told.open = true,
                Pressed::Quit => told.quit = true,
                Pressed::Nothing => {}
            }
        }
        told
    }

    /// Whether the window has stopped being there, and with what
    fn ended(&self) -> Option<Option<i32>> {
        let mut held = self.window.borrow_mut();
        let child = held.as_mut()?;
        match child.try_wait() {
            Ok(None) => None,
            Ok(Some(status)) => {
                *held = None;
                Some(status.code())
            }
            // The process cannot be asked about any more, which is its own
            // kind of gone
            Err(e) => {
                crate::append_hook_log(&format!("split: the window cannot be asked about: {e}"));
                *held = None;
                Some(None)
            }
        }
    }
}

impl Default for Split {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Split {
    /// The runtime is ending, so the window goes with it. Left alone it would
    /// stand there drawing a board that answers nothing, with a ✕ as the only
    /// thing on it that still works
    fn drop(&mut self) {
        if let Some(mut child) = self.window.borrow_mut().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(icon) = &self.icon {
            icon.stop();
        }
    }
}

impl Minder for Split {
    fn board_is_at(&self, url: &str) {
        *self.board.borrow_mut() = url.to_string();
        crate::append_hook_log("split: the board is listening; opening a window on it");
        self.open();
    }

    fn tick(&self) -> Told {
        let told = self.heard();
        // A press for a window is answered here rather than being handed on
        // as `tray_open`: the loop's answer to that is `shell.show()`, which
        // is this same door, and going round by the loop would only mean the
        // press was acted on a turn later
        if told.open {
            self.show();
            return Told { open: false, ..told };
        }
        if told.quit {
            return told;
        }
        let now = now_ms();
        if let Some(code) = self.ended() {
            crate::append_hook_log(&format!("split: the window is gone (code {code:?})"));
            return match self.keep.borrow_mut().parted(code, now) {
                // Handed on as a ✕, which is what it was. What it costs is
                // the setting's answer and the loop's to give
                Do::Closed => {
                    self.resting.set(true);
                    Told { closed: true, ..told }
                }
                Do::Open => {
                    self.open();
                    told
                }
                Do::Later(ms) => {
                    crate::append_hook_log(&format!("split: waiting {ms}ms before trying again"));
                    told
                }
                Do::Offer => {
                    self.offer();
                    told
                }
                Do::Nothing => told,
            };
        }
        if matches!(self.keep.borrow_mut().tick(now), Do::Open) {
            self.open();
        }
        told
    }

    /// A window was asked for -- from the icon, or by the loop answering
    /// `tray_open`. A person asking is never a storm, so what was tried is
    /// forgotten first
    fn show(&self) {
        if self.window.borrow().is_some() {
            return;
        }
        self.keep.borrow_mut().asked();
        self.open();
    }

    /// The ✕, with the setting saying to keep working. There is no window left
    /// to put away -- it has already gone -- so this only stops another being
    /// started in its place
    fn hide(&self) {
        self.resting.set(true);
    }

    /// Said once, the first time, by the program that has the icon
    fn say_where_it_went(&self) {
        let told = crate::config::state_path("tray-noticed");
        if told.exists() {
            return;
        }
        let _ = crate::crypto::write_atomic(&told, "1");
        if let Some(icon) = &self.icon {
            icon.notice("SHIKISHA-TERM", &crate::i18n::t("msg.tray.resident"));
        }
    }

    /// Asked on the machine itself, because there is somebody at it: this half
    /// of the pair has no screen, but it does have a desktop, and quitting
    /// with an AI mid-turn is worth one question
    fn confirm_quit(&self, busy: usize) -> bool {
        if busy == 0 {
            return true;
        }
        let asked = crate::i18n::tp("msg.quit.busy", &[("n", &busy.to_string())]);
        let yes = crate::resident::ask_yes_no("SHIKISHA-TERM", &asked);
        crate::append_hook_log(&format!(
            "split: quit asked with {busy} tab(s) at work: {}",
            if yes { "yes" } else { "no" }
        ));
        yes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window is the program beside this one, never one found on a path
    /// or left over from another install: a window from one release and a
    /// runtime from another do not have to understand each other, and the one
    /// failure that would cause is the one nobody would think to look for --
    /// a window opens, and it is not a window onto this runtime
    #[test]
    fn the_window_is_the_program_installed_beside_this_one() {
        let dir = std::env::temp_dir().join(format!("shikisha-split-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("no folder");
        let fallback = std::path::PathBuf::from("somewhere/else");

        // Nothing beside it yet: this program, started the other way, which is
        // the ordinary case -- the window and the runtime are one executable
        assert_eq!(window_program_in(&dir, fallback.clone()), fallback);

        std::fs::write(dir.join(WINDOW), b"").expect("no file");
        assert_eq!(window_program_in(&dir, fallback), dir.join(WINDOW));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nothing is started before there is a board to point it at. A window
    /// given no address opens on nothing and closes at once, which the keeper
    /// would then read as a window that died
    #[test]
    fn no_window_is_started_before_the_board_is_listening() {
        let split = Split {
            board: RefCell::default(),
            window: RefCell::default(),
            keep: RefCell::new(Keeper::new()),
            icon: None,
            presses: Arc::default(),
            resting: Cell::new(false),
        };
        split.open();
        assert!(split.window.borrow().is_none(), "it started a window onto nowhere");
    }
}

//! The shell a runtime is driven by.
//!
//! Something has to show what is going on and report what a person did. On this
//! machine that is a window; it could as easily be nothing at all, which is what
//! a runtime on a server has. Everything a loop needs from that something is
//! written down here, so the loop can be written once and run either way.
//!
//! What is *not* here is just as deliberate. A shell is never asked what a tab
//! is doing, what a desk holds, or what should happen next -- it measures
//! itself, it draws what it is handed, and it posts what it saw into the
//! mailbox. The thinking is the runtime's.

use crate::mailbox::Mailbox;
use crate::tab::Tab;
use crate::view::{Size, Ui};
use crossterm::event::Event;
use std::time::Duration;

pub trait Shell {
    /// Where reports from this shell land
    fn mail(&mut self) -> &mut Mailbox;

    /// Ask whether to quit with `busy` tabs still working. A shell with nobody
    /// in front of it has nobody to ask, and says yes
    fn confirm_quit(&mut self, busy: usize) -> bool;

    /// Hand a Store update to the platform. It wants a window to put its own
    /// progress on, which is why it is asked of the shell and not done here
    fn install_store_update(&mut self) -> anyhow::Result<()>;

    fn take_keyboard_back(&self);
    fn geom_area(&self) -> (i32, i32, i32, i32);
    fn geom_full(&self) -> (i32, i32, i32, i32);
    fn geom_rows(&self) -> u16;
    fn geom_cols(&self) -> u16;
    fn geom_panes(&self) -> &[shikisha_shared::PaneGeom];
    fn phone_size(&self) -> Option<(u16, u16)>;
    fn set_phone_size(&mut self, size: Option<(u16, u16)>);
    fn is_hidden(&self) -> bool;
    fn last_drawn(&self) -> Option<&crate::uistate::UiState>;
    fn queue_input(&mut self, ev: Event);
    fn inject(&mut self, ev: Event);
    fn toggle_tab_bar(&self);
    fn toggle_side_bar(&self);
    fn take_open_settings(&mut self) -> Option<crate::mailbox::SettingsWanted>;
    fn open_vault(&self);
    fn open_palette(&self);
    /// Bring up the quick commands over the window's page
    fn open_quick(&self);
    /// Bring up the ideas over the window's page
    fn open_ideas(&self);
    fn push_git(&self, json: &str);
    fn push_files(&self, json: &str);
    fn push_issues(&self, json: &str);
    fn push_ideas(&self, json: &str);
    fn push_sftp(&self, json: &str);
    fn push_recorded(&self, line_json: &str);
    /// A line about the run being driven from words, for the strip under
    /// the page it is driving
    fn push_words_note(&self, json: &str);
    fn queue_ui(&mut self, ev: shikisha_shared::Ev);
    fn push_suggested(&self, json: &str);
    fn push_surveyed(&self, json: &str);
    fn push_lua_done(&self, err_json: &str);
    fn push_actions(&self, actions_json: &str);
    fn push_theme(&self);
    /// The settings file was read again. For whatever the shell holds of it
    /// outside the conductor's loop -- the window's keys that work from any
    /// program
    fn settings_reloaded(&self) {}
    fn hide(&mut self);
    fn show(&mut self);
    fn say_where_it_went(&self);
    fn size(&self) -> anyhow::Result<Size>;
    fn poll(&mut self, timeout: Duration, active_tab: Option<&Tab>) -> anyhow::Result<Option<Event>>;
    fn host(&self) -> Option<shikisha_shared::Seat>;

    /// The end of the line a connected client answers browser asks on.
    ///
    /// Only a shell whose pages can be drawn on somebody else's machine has
    /// one. A window draws its own pages, on the machine it is already on
    fn far_pages(&self) -> Option<crate::faraway::Line> {
        None
    }

    /// The board is listening, at this address (the key to it included).
    ///
    /// A window shows the link as a code to scan, on its own settings screen,
    /// and has nothing to add. A runtime with no window has no screen to show it
    /// on, so it says it where the person who started it is looking
    fn board_is_at(&self, _url: &str) {}

    /// Where the next page should be drawn. Meaningless to a shell that can
    /// only draw in one place, which is why it does nothing by default
    fn draw_pages(&self, where_: crate::placed::Draw) {
        let _ = where_;
    }

    fn ask_password(&mut self, title: &str, note: &str) -> anyhow::Result<Option<String>>;
    fn draw(&mut self, tabs: &[Tab], ui: &Ui, flash: Option<&str>) -> anyhow::Result<()>;
}

/// Somebody keeping a window over a runtime that draws nothing itself.
///
/// A runtime with no window is usually a server nobody is sitting at, and then
/// there is nothing to keep. Split in two on somebody's own machine it is the
/// other half of a pair (`split::Split`): it starts the window, watches it,
/// and puts it back when it is taken -- which is the whole reason the two are
/// separate programs.
///
/// Everything it notices is said in the words a window would have used, so the
/// loop reads them where it already reads them and nothing about what a ✕
/// costs is decided twice.
///
/// Every method takes `&self` because the shell is asked these while the loop
/// holds it; what has to change keeps itself.
pub trait Minder {
    /// The board is listening at this address. Nothing can be pointed at it
    /// before this
    fn board_is_at(&self, url: &str);
    /// Once round the loop. What came of it, in the loop's own words
    fn tick(&self) -> Told;
    /// A window was asked for
    fn show(&self);
    /// No window, and none wanted until one is asked for
    fn hide(&self);
    /// Say, once, that the program is still there and where to find it
    fn say_where_it_went(&self);
    /// Whether to stop with this many tabs at work
    fn confirm_quit(&self, busy: usize) -> bool;
}

/// What a minder noticed, in the words the loop already knows
#[derive(Debug, Default, Clone, Copy)]
pub struct Told {
    /// A window's ✕ was pressed
    pub closed: bool,
    /// Ending the program was asked for
    pub quit: bool,
    /// A window was asked for
    pub open: bool,
}

/// A runtime with nothing showing it.
///
/// Every question has an honest answer: it measured a terminal of the size it
/// was told to use, it drew nothing, nobody pressed anything. Automation that
/// needs a person or a page fails where it is written rather than hanging.
pub struct Headless {
    mail: Mailbox,
    rows: u16,
    cols: u16,
    last: Option<crate::uistate::UiState>,
    /// The pages the runtime has open, on this machine or on whoever is
    /// connected. Nothing starts a browser until a page is asked for
    pages: std::rc::Rc<crate::placed::Placed>,
    /// Keystrokes handed to this shell, waiting to be read back.
    ///
    /// A shell with nobody in front of it still receives them: everything a
    /// person does from afar that is not a queue of its own -- typing, picking
    /// a tab, the board's menu -- arrives as the keystroke it stands for, and
    /// is handed here. Dropping them, which is what this did, left a server
    /// that could be watched and not driven
    typed: std::collections::VecDeque<Event>,
    /// Somebody keeping a window over this runtime, when this runtime is half
    /// of a pair. None on a server, where there is nobody to draw for
    minder: Option<Box<dyn Minder>>,
}

/// How big a page is, with no window to fit it into.
///
/// It has to be some size: it is what a picture of the page is taken at and
/// what every coordinate in it is measured against. This is the size the
/// window itself opens at, so a script written at one desk and run on a server
/// is looking at the same page.
const PAGE_SIZE: (i32, i32, i32, i32) = (0, 0, 1280, 900);

impl Headless {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self::browsing(rows, cols, std::rc::Rc::new(crate::placed::Placed::new()))
    }

    /// The same, with its pages opened somewhere of your choosing. For a test
    /// that wants a real browser and nobody's real cookies.
    pub(crate) fn browsing(rows: u16, cols: u16, pages: std::rc::Rc<crate::placed::Placed>) -> Self {
        Self {
            mail: Mailbox::default(),
            rows,
            cols,
            last: None,
            pages,
            typed: std::collections::VecDeque::new(),
            minder: None,
        }
    }

    /// The same runtime, with somebody keeping a window over it. What makes
    /// this half of a split pair rather than a server
    pub fn minded_by(mut self, minder: Box<dyn Minder>) -> Self {
        self.minder = Some(minder);
        self
    }

    /// Take in whatever the pages have said since last time.
    fn hear_pages(&mut self) -> bool {
        let said = self.pages.drain();
        let any = !said.is_empty();
        for ev in said {
            self.mail.page_report(ev);
        }
        any
    }
}

impl Shell for Headless {
    fn mail(&mut self) -> &mut Mailbox {
        &mut self.mail
    }

    /// Nobody to ask, so nothing is in the way of stopping -- unless there
    /// is a window over this runtime, and therefore a desktop to ask on
    fn confirm_quit(&mut self, busy: usize) -> bool {
        match &self.minder {
            Some(m) => m.confirm_quit(busy),
            None => true,
        }
    }

    /// A Store update needs a desktop to show itself on. There is none
    fn install_store_update(&mut self) -> anyhow::Result<()> {
        anyhow::bail!("no desktop here to install a Store update on")
    }

    fn take_keyboard_back(&self) {}
    fn geom_area(&self) -> (i32, i32, i32, i32) { (0, 0, 0, 0) }
    fn geom_full(&self) -> (i32, i32, i32, i32) { (0, 0, 0, 0) }
    fn geom_rows(&self) -> u16 { self.rows }
    fn geom_cols(&self) -> u16 { self.cols }
    fn geom_panes(&self) -> &[shikisha_shared::PaneGeom] { &[] }
    fn phone_size(&self) -> Option<(u16, u16)> { None }
    fn set_phone_size(&mut self, size: Option<(u16, u16)>) { if let Some((r, c)) = size { self.rows = r; self.cols = c; } }
    fn is_hidden(&self) -> bool { true }
    fn last_drawn(&self) -> Option<&crate::uistate::UiState> { self.last.as_ref() }
    /// What a person pressed, for the loop to read on its next turn.
    ///
    /// Both doors lead to the same queue here. The window keeps them apart
    /// because one of them is a real keyboard it is already holding; there is
    /// no keyboard here, so there is nothing to keep apart
    fn queue_input(&mut self, ev: Event) { self.typed.push_back(ev); }
    fn inject(&mut self, ev: Event) { self.typed.push_back(ev); }
    fn toggle_tab_bar(&self) {}
    fn toggle_side_bar(&self) {}
    fn take_open_settings( &mut self, ) -> Option<crate::mailbox::SettingsWanted> { None }
    fn open_vault(&self) {}
    fn open_palette(&self) {}
    fn open_quick(&self) {}
    fn open_ideas(&self) {}
    fn push_git(&self, json: &str) { let _ = json; }
    fn push_files(&self, json: &str) { let _ = json; }
    fn push_issues(&self, json: &str) { let _ = json; }
    fn push_ideas(&self, json: &str) { let _ = json; }
    fn push_sftp(&self, json: &str) { let _ = json; }
    fn push_recorded(&self, line_json: &str) { let _ = line_json; }
    fn push_words_note(&self, json: &str) { let _ = json; }
    fn queue_ui(&mut self, ev: shikisha_shared::Ev) { let _ = ev; }
    fn push_suggested(&self, json: &str) { let _ = json; }
    fn push_surveyed(&self, json: &str) { let _ = json; }
    fn push_lua_done(&self, err_json: &str) { let _ = err_json; }
    fn push_actions(&self, actions_json: &str) { let _ = actions_json; }
    fn push_theme(&self) {}
    /// Nothing to put away on a server. Where there is a window over this
    /// runtime, these are the same three things they have always been -- the
    /// window is simply somewhere else
    fn hide(&mut self) {
        if let Some(m) = &self.minder {
            m.hide();
        }
    }
    fn show(&mut self) {
        if let Some(m) = &self.minder {
            m.show();
        }
    }
    fn say_where_it_went(&self) {
        if let Some(m) = &self.minder {
            m.say_where_it_went();
        }
    }
    fn size(&self) -> anyhow::Result<Size> { Ok(Size { width: self.cols.saturating_mul(8), height: self.rows.saturating_mul(16) }) }
    /// Nobody presses anything here, so this only ever waits -- but it waits
    /// in slices, because the pages are talking on their own threads and a
    /// frame from one of them is worth going round the loop for.
    fn poll(&mut self, timeout: Duration, active_tab: Option<&Tab>) -> anyhow::Result<Option<Event>> {
        let _ = active_tab;
        // What the window over this runtime did since the last turn, put into
        // the mailbox as though a window in this process had done it. The loop
        // reads it a few lines on, in the one place it reads all of this
        if let Some(m) = &self.minder {
            let told = m.tick();
            self.mail.close_requested |= told.closed;
            self.mail.tray_quit |= told.quit;
            self.mail.tray_open |= told.open;
        }
        let until = std::time::Instant::now() + timeout;
        loop {
            // What somebody pressed comes first, and one at a time: the loop
            // acts on one key per turn, exactly as it does at a window
            if let Some(key) = self.typed.pop_front() {
                return Ok(Some(key));
            }
            if self.hear_pages() {
                return Ok(None);
            }
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            std::thread::sleep(left.min(Duration::from_millis(20)));
        }
    }

    /// The browser on this machine.
    ///
    /// Handed over whether or not one is installed: what is here is the
    /// bookkeeping, and the browser behind it is started the first time a page
    /// is actually asked for. Answering `None` would say "this runtime does
    /// not do pages", and it does
    fn far_pages(&self) -> Option<crate::faraway::Line> {
        Some(self.pages.far())
    }

    /// Printed for whoever started it, and written to the log beside the
    /// settings for whoever started it as a service -- what `--help` has always
    /// said happens. Nothing did: a server came up with its board listening
    /// and no way to learn the address short of reading the settings and the
    /// token file and putting the two together
    fn board_is_at(&self, url: &str) {
        let line = crate::i18n::tp("msg.serve.board_at", &[("url", url)]);
        println!("{line}");
        crate::append_hook_log(&line);
        // ...and where there is a window to open on it, it is opened now: the
        // address is the one thing it could not be started without
        if let Some(m) = &self.minder {
            m.board_is_at(url);
        }
    }

    /// Read from the config, and re-read whenever it changes
    fn draw_pages(&self, where_: crate::placed::Draw) {
        self.pages.prefer(where_);
    }

    fn host(&self) -> Option<(std::rc::Rc<dyn shikisha_shared::BrowserHost>, (i32, i32, i32, i32))> {
        Some((
            std::rc::Rc::clone(&self.pages) as std::rc::Rc<dyn shikisha_shared::BrowserHost>,
            PAGE_SIZE,
        ))
    }
    /// A runtime with no window still needs the key to its own secrets, and a
    /// server is where the secrets live -- a login on somebody's laptop does
    /// not carry to it. See `askpass`: a credential the service manager handed
    /// over, or a person at the terminal, and nothing invented in between
    fn ask_password(&mut self, title: &str, note: &str) -> anyhow::Result<Option<String>> {
        Ok(crate::askpass::master(title, note))
    }
    fn draw(&mut self, tabs: &[Tab], ui: &Ui, flash: Option<&str>) -> anyhow::Result<()> {
        // Nothing draws here, but the picture is still built: it is what a
        // viewer on the network is handed, and what a shell would have drawn
        let _ = flash;
        self.last = Some(crate::view::ui_state_of(tabs, ui, None));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// A keystroke handed to a runtime with no window comes back out of it.
    ///
    /// Everything a person does from afar that has no queue of its own --
    /// typing, picking a tab, the board's menu, stopping a run -- arrives as
    /// the keystroke it stands for and is handed to the shell. This shell used
    /// to drop them, which left a server that could be watched and not driven,
    /// and nothing said so.
    #[test]
    fn a_keystroke_reaches_a_runtime_with_no_window() {
        let mut shell = Headless::new(24, 80);
        let typed = |c: char| Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        shell.inject(typed('l'));
        shell.queue_input(typed('s'));

        let wait = Duration::from_millis(50);
        assert_eq!(shell.poll(wait, None).unwrap(), Some(typed('l')));
        assert_eq!(shell.poll(wait, None).unwrap(), Some(typed('s')));
        // One key per turn, as at a window -- and nothing left over
        assert_eq!(shell.poll(wait, None).unwrap(), None);
    }

    /// Waiting for nothing still waits: a loop that spun here would spend a
    /// core on an empty room
    #[test]
    fn with_nothing_pressed_it_waits_out_its_turn() {
        let mut shell = Headless::new(24, 80);
        let began = std::time::Instant::now();
        assert_eq!(shell.poll(Duration::from_millis(120), None).unwrap(), None);
        assert!(
            began.elapsed() >= Duration::from_millis(100),
            "it returns at once though there is nothing: {:?}",
            began.elapsed()
        );
    }
}

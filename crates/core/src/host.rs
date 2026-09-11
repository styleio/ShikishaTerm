//! The shell a runtime is driven by.
//!
//! Something has to show what is going on and report what a person did. On this
//! machine that is a window; it could as easily be nothing at all, which is what
//! a runtime on a server has. Everything a loop needs from that something is
//! written down here, so the loop can be written once and run either way.
//!
//! What is *not* here is just as deliberate. A shell is never asked what a tab
//! is doing, what a workspace holds, or what should happen next -- it measures
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
    fn install_store_update(&mut self, version: &str) -> anyhow::Result<()>;

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
    fn take_open_settings( &mut self, ) -> Option<(Option<String>, bool, Option<String>, Option<u32>)>;
    fn open_vault(&self);
    fn open_palette(&self);
    fn push_git(&self, json: &str);
    fn push_sftp(&self, json: &str);
    fn push_recorded(&self, line_json: &str);
    fn queue_vault(&mut self, ev: shikisha_shared::Ev);
    fn push_suggested(&self, json: &str);
    fn push_surveyed(&self, json: &str);
    fn push_lua_done(&self, err_json: &str);
    fn push_actions(&self, actions_json: &str);
    fn push_theme(&self);
    fn hide(&mut self);
    fn show(&mut self);
    fn say_where_it_went(&self);
    fn size(&self) -> anyhow::Result<Size>;
    fn poll(&mut self, timeout: Duration, active_tab: Option<&Tab>) -> anyhow::Result<Option<Event>>;
    fn host(&self) -> Option<(std::rc::Rc<dyn shikisha_shared::BrowserHost>, (i32, i32, i32, i32))>;
    fn ask_password(&mut self, title: &str, note: &str) -> anyhow::Result<Option<String>>;
    fn draw(&mut self, tabs: &[Tab], ui: &Ui, flash: Option<&str>) -> anyhow::Result<()>;
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
    /// The browser on this machine, and the pages open in it. Nothing starts
    /// one until a page is asked for
    pages: std::rc::Rc<crate::chrome::Pages>,
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
        Self::browsing(rows, cols, std::rc::Rc::new(crate::chrome::Pages::new()))
    }

    /// The same, with its pages opened somewhere of your choosing. For a test
    /// that wants a real browser and nobody's real cookies.
    pub(crate) fn browsing(rows: u16, cols: u16, pages: std::rc::Rc<crate::chrome::Pages>) -> Self {
        Self { mail: Mailbox::default(), rows, cols, last: None, pages }
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

    /// Nobody to ask, so nothing is in the way of stopping
    fn confirm_quit(&mut self, _busy: usize) -> bool {
        true
    }

    /// A Store update needs a desktop to show itself on. There is none
    fn install_store_update(&mut self, _version: &str) -> anyhow::Result<()> {
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
    fn queue_input(&mut self, ev: Event) { let _ = ev; }
    fn inject(&mut self, ev: Event) { let _ = ev; }
    fn toggle_tab_bar(&self) {}
    fn take_open_settings( &mut self, ) -> Option<(Option<String>, bool, Option<String>, Option<u32>)> { None }
    fn open_vault(&self) {}
    fn open_palette(&self) {}
    fn push_git(&self, json: &str) { let _ = json; }
    fn push_sftp(&self, json: &str) { let _ = json; }
    fn push_recorded(&self, line_json: &str) { let _ = line_json; }
    fn queue_vault(&mut self, ev: shikisha_shared::Ev) { let _ = ev; }
    fn push_suggested(&self, json: &str) { let _ = json; }
    fn push_surveyed(&self, json: &str) { let _ = json; }
    fn push_lua_done(&self, err_json: &str) { let _ = err_json; }
    fn push_actions(&self, actions_json: &str) { let _ = actions_json; }
    fn push_theme(&self) {}
    fn hide(&mut self) {}
    fn show(&mut self) {}
    fn say_where_it_went(&self) {}
    fn size(&self) -> anyhow::Result<Size> { Ok(Size { width: self.cols.saturating_mul(8), height: self.rows.saturating_mul(16) }) }
    /// Nobody presses anything here, so this only ever waits -- but it waits
    /// in slices, because the pages are talking on their own threads and a
    /// frame from one of them is worth going round the loop for.
    fn poll(&mut self, timeout: Duration, active_tab: Option<&Tab>) -> anyhow::Result<Option<Event>> {
        let _ = active_tab;
        let until = std::time::Instant::now() + timeout;
        loop {
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

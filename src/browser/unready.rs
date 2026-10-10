//! The window's engine on a system that has none yet.
//!
//! The window is drawn by Chromium: WebView2 on Windows, which the system
//! carries, and CEF on a Mac, which the program carries. Elsewhere there is
//! none, and the window cannot be opened; everything else in `browser` builds
//! and runs the same, so the program around the window can be built and
//! tested meanwhile. This is the one place that says the window is not there:
//! [`Pages::new`] refuses, and nothing below it can ever be reached -- the
//! types have no values, so the compiler holds that promise, not a comment.

use super::window::{Rect4, Spec};
use super::*;

/// No engine is on this system to be asked for its version
pub fn runtime_version() -> Option<String> {
    None
}

/// A value nothing can make
#[derive(Clone, Copy)]
pub(super) enum Never {}

#[derive(Clone)]
pub(crate) struct Port(Never);

pub(crate) struct Heard(#[allow(dead_code)] Never);

impl Port {
    pub fn call(&self, _method: &str, _params_json: &str) {
        match self.0 {}
    }

    pub fn call_result<F: FnOnce(bool, String) + 'static>(&self, _method: &str, _params_json: &str, _done: F) {
        match self.0 {}
    }

    pub fn listen<F: Fn(&serde_json::Value) + 'static>(&self, _event: &str, _on: F) -> Option<Heard> {
        match self.0 {}
    }
}

pub(super) struct Page(Never);

impl Page {
    pub fn port(&self) -> Port {
        match self.0 {}
    }
    pub fn load(&self, _url: &str) -> Result<()> {
        match self.0 {}
    }
    pub fn back(&self) -> Result<()> {
        match self.0 {}
    }
    pub fn forward(&self) -> Result<()> {
        match self.0 {}
    }
    pub fn reload(&self) -> Result<()> {
        match self.0 {}
    }
    pub fn url(&self) -> String {
        match self.0 {}
    }
    pub fn can_back(&self) -> bool {
        match self.0 {}
    }
    pub fn can_forward(&self) -> bool {
        match self.0 {}
    }
    pub fn run_js(&self, _js: &str) {
        match self.0 {}
    }
    pub fn place(&self, _rect: Rect4) {
        match self.0 {}
    }
    pub fn fill(&self, _w: u32, _h: u32) {
        match self.0 {}
    }
    pub fn focus(&self) -> Result<()> {
        match self.0 {}
    }
    pub fn zoom(&self, _factor: f64) {
        match self.0 {}
    }
    pub fn raise(&self) {
        match self.0 {}
    }
    pub fn window_moved(&self) {
        match self.0 {}
    }
    pub fn sound_process(&self) -> u32 {
        match self.0 {}
    }
    pub fn finder(&self, _page: Option<String>, _tell: Sender<Ev>) -> Finder {
        match self.0 {}
    }
}

pub(super) struct Finder(Never);

impl Finder {
    pub fn seek(&self, _view: &Page, _text: &str, _step: shikisha_shared::Seek, _page: Option<String>, _tell: Sender<Ev>) {
        match self.0 {}
    }
}

pub(super) struct Pages(Never);

impl Pages {
    /// Says the window cannot be opened here, and nothing else. The caller
    /// hears it the way it hears any window that could not be made: in the
    /// log, and as the window closing
    pub fn new(_window: std::rc::Rc<tao::window::Window>, _wake: tao::event_loop::EventLoopProxy<Cmd>) -> Result<Self> {
        Err(anyhow!("this system has no engine to draw the window with"))
    }
    pub fn next_turn(&self) -> Option<std::time::Instant> {
        match self.0 {}
    }
    pub fn turn(&mut self) {
        match self.0 {}
    }
    pub fn fill(&mut self, _window: &tao::window::Window, _spec: Spec) -> Result<Page> {
        match self.0 {}
    }
    pub fn child(&mut self, _spec: Spec) -> Result<Page> {
        match self.0 {}
    }
    pub fn forget_store(&mut self, _dir: &std::path::Path) {
        match self.0 {}
    }
}

pub(super) fn cancel_download(_id: &str) {}

/// No browser here, and so nothing of its own to search with either way
pub(super) const FINDS_ITSELF: bool = true;

pub(super) fn find_keys_for(_page: &str, _on: bool) {}

pub(super) fn wind_down() {}

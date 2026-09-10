//! SHIKISHA with no window.
//!
//! Everything the app does that drawing is not needed for: the tabs and their
//! pseudo consoles, the state detector, the automation, the board served to
//! whoever connects. Meant for a machine nobody is sitting at -- a VPS, a cloud
//! VM -- where the thing that draws is somewhere else entirely.

fn main() -> anyhow::Result<()> {
    shikisha_core::serve::run()
}

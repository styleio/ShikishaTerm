//! A Mac's notification.
//!
//! What `wintoast` is on Windows: a banner over whatever is in front, and a
//! line that stays in the Notification Centre until it is read, for the person
//! at this Mac with the window behind something else. Posted the way a Mac's
//! own programs post them (the User Notifications framework), so it is this
//! app's, under its name and icon, and is turned off in System Settings like
//! any other app's.
//!
//! A Mac asks the person once, the first time a notification is posted, whether
//! this app may show them; until they answer yes nothing is shown, and that is
//! theirs to decide. The question is asked when one is first wanted, never at
//! startup: a program told to notify nobody has no business asking.
//!
//! ## Clicking it
//!
//! A click is answered while this program is running: the window comes forward
//! and, if the notification came from one tab, that tab is the one showing --
//! the same as on Windows. The tab travels in the notification's identifier,
//! the one thing a click hands back without asking more of the framework.

use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, ClassType};
use objc2_foundation::{NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// The tab a person clicked a notification for, waiting for the loop to pick it
/// up (0: one that was about no tab -- the window still comes forward)
static CLICKED: Mutex<Option<usize>> = Mutex::new(None);

/// What brings the window forward, given once it exists
static RAISE: OnceLock<Mutex<Box<dyn Fn() + Send>>> = OnceLock::new();

/// Each notification's own number, so a second one about the same tab does not
/// replace the first
static SEQ: AtomicU64 = AtomicU64::new(0);

/// The start of every identifier this program gives a notification
const PREFIX: &str = "shikisha-tab-";

/// The identifier a notification about `tab` is posted under
fn identifier_for(tab: Option<usize>) -> String {
    format!("{PREFIX}{}-{}", tab.unwrap_or(0), SEQ.fetch_add(1, Ordering::Relaxed))
}

/// The tab a clicked notification was about, from its identifier. `None` for
/// one this program did not post
fn tab_of(identifier: &str) -> Option<usize> {
    identifier.strip_prefix(PREFIX)?.split('-').next()?.parse().ok()
}

define_class!(
    // SAFETY: NSObject asks nothing of a subclass, and this one adds no state
    // of its own and has no Drop
    #[unsafe(super(NSObject))]
    #[name = "ShikishaNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// Posted while this app is the one in front: shown all the same. A
        /// Mac keeps them back by default, but the window in front may be
        /// showing another tab than the one that is waiting
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            done: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            done.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        /// A notification was clicked: note which tab it was about, for the
        /// loop to bring forward
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            done: &block2::DynBlock<dyn Fn()>,
        ) {
            let identifier = response.notification().request().identifier().to_string();
            if let Some(tab) = tab_of(&identifier) {
                *CLICKED.lock().unwrap_or_else(|e| e.into_inner()) = Some(tab);
            }
            done.call(());
        }
    }
);

/// Whether this process is an app the system knows: the framework stops the
/// process outright when asked anything by one that is not (a program run
/// outside its `.app`, a test)
fn in_an_app() -> bool {
    NSBundle::mainBundle().bundleIdentifier().is_some()
}

/// Notifications in the Notification Centre, for the runtime (`notify`)
pub struct MacBanners;

impl MacBanners {
    /// Ready to post, with clicks heard from now on. The delegate is set here,
    /// at startup, because a click that starts this app is handed to the
    /// delegate that is there by then
    pub fn new() -> Self {
        if in_an_app() {
            let delegate: Retained<Delegate> = unsafe { msg_send![Delegate::class(), new] };
            UNUserNotificationCenter::currentNotificationCenter().setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            // The centre holds its delegate weakly; this one lives as long as
            // the program does
            std::mem::forget(delegate);
        }
        MacBanners
    }
}

/// What brings the window forward when a notification is clicked
pub fn raise_by(raise: impl Fn() + Send + 'static) {
    let _ = RAISE.set(Mutex::new(Box::new(raise)));
}

impl shikisha_shared::Toasts for MacBanners {
    fn show(&self, title: &str, body: &str, tab: Option<usize>) -> Result<(), String> {
        if !in_an_app() {
            return Err("not running as SHIKISHA-TERM.app, so the Mac has no app to show it for".into());
        }
        let (title, body, id) = (title.to_string(), body.to_string(), identifier_for(tab));
        // Asked every time, and answered at once after the first: the person's
        // answer is the Mac's to keep, and a change in System Settings is
        // honoured from the next one on
        let post = block2::RcBlock::new(move |granted: Bool, error: *mut NSError| {
            if !granted.as_bool() {
                let why = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
                shikisha_core::append_hook_log(&format!(
                    "macnote: notifications are not allowed for SHIKISHA-TERM{}",
                    why.map(|w| format!(" ({w})")).unwrap_or_default()
                ));
                return;
            }
            let content = UNMutableNotificationContent::new();
            content.setTitle(&NSString::from_str(&title));
            content.setBody(&NSString::from_str(&body));
            content.setSound(Some(&UNNotificationSound::defaultSound()));
            let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&NSString::from_str(&id), &content, None);
            let said = block2::RcBlock::new(|error: *mut NSError| {
                if let Some(e) = unsafe { error.as_ref() } {
                    shikisha_core::append_hook_log(&format!("macnote: not shown: {}", e.localizedDescription()));
                }
            });
            UNUserNotificationCenter::currentNotificationCenter().addNotificationRequest_withCompletionHandler(&request, Some(&said));
        });
        UNUserNotificationCenter::currentNotificationCenter().requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &post,
        );
        Ok(())
    }

    fn clicked_tab(&self) -> Option<usize> {
        CLICKED.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    fn raise(&self) {
        if let Some(raise) = RAISE.get() {
            (raise.lock().unwrap_or_else(|e| e.into_inner()))();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tab a click comes back with is the one the notification was posted
    /// about, and two about one tab are two notifications
    #[test]
    fn the_tab_travels_in_the_identifier() {
        let a = identifier_for(Some(3));
        let b = identifier_for(Some(3));
        assert_ne!(a, b, "the second notification would replace the first");
        assert_eq!(tab_of(&a), Some(3));
        assert_eq!(tab_of(&identifier_for(None)), Some(0));
        assert_eq!(tab_of("someone-elses-7"), None);
    }
}

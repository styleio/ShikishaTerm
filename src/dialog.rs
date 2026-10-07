//! A warning or a question in the system's own dialog, asked from any thread.
//!
//! On a Mac only the program's first thread may show a dialog, and that thread
//! is the window's loop once the window is open (`Browser::host`). Asked from
//! anywhere else, the dialog is handed to that loop and waited for; asked on
//! the first thread itself -- before the window, when something has already
//! gone wrong -- it is shown there and then. Windows shows a dialog on
//! whichever thread asks.

use rfd::{MessageButtons, MessageDialogResult, MessageLevel};

/// The program's first thread, noted as `main` starts
static FIRST: std::sync::OnceLock<std::thread::ThreadId> = std::sync::OnceLock::new();

/// Note that this is the program's first thread. Called first thing in `main`
pub fn note_first_thread() {
    let _ = FIRST.set(std::thread::current().id());
}

/// Something went wrong that a person has to be told of, with one button
pub fn warn(title: &str, body: &str) {
    ask(title, body, MessageLevel::Error, MessageButtons::Ok);
}

/// Ask, and wait for the answer
pub fn ask(title: &str, body: &str, level: MessageLevel, buttons: MessageButtons) -> MessageDialogResult {
    let here = FIRST.get().is_none_or(|first| *first == std::thread::current().id());
    if cfg!(target_os = "macos") && !here {
        let asked = rfd::AsyncMessageDialog::new()
            .set_level(level)
            .set_title(title)
            .set_description(body)
            .set_buttons(buttons)
            .show();
        return wait(asked);
    }
    rfd::MessageDialog::new()
        .set_level(level)
        .set_title(title)
        .set_description(body)
        .set_buttons(buttons)
        .show()
}

/// The answer to something that answers later, waited for on this thread
fn wait<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Wake};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::sync::Arc::new(Unpark(std::thread::current())).into();
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Something that answers later is waited for, however many times it
    /// says it is not ready yet
    #[test]
    fn an_answer_that_comes_later_is_waited_for() {
        struct Later(u8, std::sync::Arc<std::sync::Mutex<Option<std::task::Waker>>>);
        impl std::future::Future for Later {
            type Output = &'static str;
            fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<&'static str> {
                if self.0 == 0 {
                    return std::task::Poll::Ready("answered");
                }
                self.0 -= 1;
                *self.1.lock().unwrap() = Some(cx.waker().clone());
                let waker = std::sync::Arc::clone(&self.1);
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    if let Some(w) = waker.lock().unwrap().take() {
                        w.wake();
                    }
                });
                std::task::Poll::Pending
            }
        }
        assert_eq!(wait(Later(3, Default::default())), "answered");
    }
}

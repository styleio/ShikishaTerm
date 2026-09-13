//! Real child-WebView checks. Run alone: the process owns one event loop.
use crate::browser::{Browser, main_hwnd};
use shikisha_shared::BrowserProfile;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn eval(b: &Browser, script: &str) -> String {
    let id = b.eval_in(Some("settings"), script).unwrap();
    let raw = b.wait_result(id, Duration::from_secs(10)).unwrap();
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::String(s)) => s,
        _ => raw,
    }
}

fn press(b: &Browser, key: &str, selector: &str) {
    let point: serde_json::Value = serde_json::from_str(&eval(
        b,
        &format!("return JSON.stringify(probeButton({key:?}, {selector:?}));"),
    ))
    .unwrap();
    mouse(
        point["x"].as_f64().unwrap() as i32,
        point["y"].as_f64().unwrap() as i32,
        2,
    );
    mouse(
        point["x"].as_f64().unwrap() as i32,
        point["y"].as_f64().unwrap() as i32,
        4,
    );
    std::thread::sleep(Duration::from_millis(250));
}

fn mouse(x: i32, y: i32, flags: u32) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetCursorPos, SetForegroundWindow};
    #[link(name = "user32")]
    unsafe extern "system" {
        fn mouse_event(flags: u32, dx: u32, dy: u32, data: u32, extra: usize);
    }
    unsafe {
        let hwnd = main_hwnd() as *mut std::ffi::c_void;
        SetForegroundWindow(hwnd);
        let mut p = POINT { x, y };
        assert_ne!(ClientToScreen(hwnd, &mut p), 0);
        SetCursorPos(p.x, p.y);
        mouse_event(flags, 0, 0, 0, 0);
    }
    std::thread::sleep(Duration::from_millis(250));
}

fn send_key(vk: u8) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn keybd_event(vk: u8, scan: u8, flags: u32, extra: usize);
    }
    unsafe {
        keybd_event(vk, 0, 0, 0);
        keybd_event(vk, 0, 2, 0);
    }
    std::thread::sleep(Duration::from_millis(150));
}

#[test]
#[ignore]
fn settings_confirmations_require_a_person() {
    let language = std::env::var("SHIKISHA_CONFIRM_LANGUAGE").unwrap_or_else(|_| "en".into());
    shikisha_core::i18n::init(
        Some(&language),
        &[std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))],
    );
    let dir = std::env::temp_dir().join(format!("shikisha-confirm-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.json");
    std::fs::write(&config, "{}").unwrap();
    let ui = shikisha_core::webui::WebUi::start_with(
        config,
        Arc::new(Mutex::new(Default::default())),
        Arc::new(Mutex::new(None)),
    )
    .unwrap();
    // Obtain exactly the served settings page, then host it with a request
    // fixture installed before any script can reach the real settings API.
    let html = ureq::get(&ui.url)
        .call()
        .unwrap()
        .body_mut()
        .read_to_string()
        .unwrap();
    ui.shutdown();
    let mut html = html.replacen(
        "<script>",
        &format!(
            "<script>{}</script><script>",
            include_str!("../tests/settings_confirm.js")
        ),
        1,
    );
    if std::env::var_os("SHIKISHA_CONFIRM_LIGHT").is_some() {
        let scheme = shikisha_core::theme::available()
            .into_iter()
            .find(shikisha_core::theme::is_light)
            .unwrap();
        html = html.replace(
            "</head>",
            &format!("<style>:root{{{}}}</style></head>", scheme.css_vars()),
        );
    }
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", server.server_addr());
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            let body = if req.url() == "/blank" {
                "<!doctype html><body>"
            } else {
                &html
            };
            let _ = req.respond(tiny_http::Response::from_string(body).with_header(
                tiny_http::Header::from_bytes("Content-Type", "text/html; charset=utf-8").unwrap(),
            ));
        }
    });
    let b = Browser::spawn(&format!("{url}blank"), "Settings confirmation checks").unwrap();
    let width = if std::env::var_os("SHIKISHA_CONFIRM_NARROW").is_some() {
        390
    } else {
        1280
    };
    b.open_child(
        "settings",
        &url,
        (0, 0, width, 900),
        BrowserProfile::shared_default(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_secs(2));
    for kind in [
        "device",
        "orphans",
        "provider",
        "notify",
        "desk",
        "desk-secrets",
        "folder",
        "secret",
        "tab",
        "close",
    ] {
        let key = eval(&b, &format!("return probePrepare({kind:?});"));
        std::thread::sleep(Duration::from_millis(300));
        let before = eval(&b, "return probeState();");
        b.drain();
        press(&b, &key, "button");
        assert_eq!(
            eval(&b, "return !!document.querySelector('dialog[open]');"),
            "true",
            "{kind}"
        );
        assert_eq!(
            eval(&b, "return JSON.stringify(probeNative);"),
            "[]",
            "{kind}: native confirm used"
        );
        assert_eq!(
            eval(&b, "return probeState();"),
            before,
            "{kind}: acted before confirmation"
        );
        assert_eq!(
            eval(
                &b,
                "const d=document.querySelector('dialog'), p=d.querySelector('.danger').getBoundingClientRect(), c=d.querySelector('.quiet:not(.icon)').getBoundingClientRect(); return p.right >= c.right && p.right <= d.getBoundingClientRect().right;"
            ),
            "true",
            "{kind}: the action must be at the right"
        );
        // What Enter lands on without looking is Cancel: every one of these
        // throws something away
        assert_eq!(
            eval(&b, "return document.activeElement.textContent;"),
            eval(&b, "return T['common.cancel'];"),
            "{kind}: focus starts on the destructive button"
        );
        if kind == "provider" {
            for _ in 0..5 {
                send_key(0x09);
                assert_eq!(
                    eval(&b, "return !!document.activeElement.closest('dialog');"),
                    "true"
                );
            }
            // Releasing a selection outside the frame is not a cancellation.
            let point: serde_json::Value = serde_json::from_str(&eval(&b,
                "const r=document.querySelector('dialog .mbody').getBoundingClientRect(); return JSON.stringify({x:(r.x+12)*devicePixelRatio,y:(r.y+12)*devicePixelRatio});")).unwrap();
            mouse(
                point["x"].as_f64().unwrap() as i32,
                point["y"].as_f64().unwrap() as i32,
                2,
            );
            mouse(2, 2, 4);
            assert_eq!(
                eval(&b, "return !!document.querySelector('dialog[open]');"),
                "true"
            );
            mouse(2, 2, 2);
            mouse(2, 2, 4);
            assert_eq!(
                eval(&b, "return !!document.querySelector('dialog[open]');"),
                "false"
            );
            assert_eq!(eval(&b, "return probeState();"), before);
            press(&b, &key, "button");
        }
        if kind == "desk"
            && let Ok(script) = std::env::var("SHIKISHA_CONFIRM_SHOT_SCRIPT") {
                let output = std::env::var("SHIKISHA_CONFIRM_SHOT_OUTPUT").unwrap();
                assert!(
                    std::process::Command::new("powershell.exe")
                        .args([
                            "-NoProfile",
                            "-File",
                            &script,
                            "-ProcessId",
                            &std::process::id().to_string(),
                            "-Out",
                            &output
                        ])
                        .status()
                        .unwrap()
                        .success()
                );
            }
        press(&b, "common.cancel", "dialog button");
        assert_eq!(
            eval(&b, "return probeState();"),
            before,
            "{kind}: acted after cancel"
        );
        assert!(
            !b.drain()
                .iter()
                .any(|ev| matches!(ev, shikisha_shared::Ev::CloseSettings))
        );
        press(&b, &key, "button");
        // Escape must dismiss only the confirmation, preserving an editor
        // below it and returning focus to the button that opened it.
        send_key(0x1b);
        assert_eq!(
            eval(&b, "return !!document.querySelector('dialog[open]');"),
            "false",
            "{kind}"
        );
        assert_eq!(
            eval(&b, "return probeState();"),
            before,
            "{kind}: Escape acted"
        );
        assert!(
            !b.drain()
                .iter()
                .any(|ev| matches!(ev, shikisha_shared::Ev::CloseSettings))
        );
        press(&b, &key, "button");
        if kind == "provider" {
            // Enter straight away is Cancel...
            send_key(0x0d);
            assert_eq!(eval(&b, "return probeState();"), before, "{kind}: Enter deleted");
            press(&b, &key, "button");
            // ...and the action is one Tab away
            send_key(0x09);
            send_key(0x0d);
        } else {
            press(
                &b,
                if kind == "close" {
                    "settings.back.discard"
                } else {
                    &key
                },
                "dialog .danger",
            );
        }
        if kind == "desk-secrets" {
            assert_eq!(eval(&b, "return probeState();"), before);
            press(&b, "common.cancel", "dialog button");
            assert_eq!(eval(&b, "return probeState();"), before);
            press(&b, &key, "button");
            press(&b, &key, "dialog .danger");
            press(&b, &key, "dialog .danger");
        }
        if kind == "close" {
            assert!(
                b.drain()
                    .iter()
                    .any(|ev| matches!(ev, shikisha_shared::Ev::CloseSettings)),
                "the window was not asked to close settings"
            );
        } else {
            assert_ne!(
                eval(&b, "return probeState();"),
                before,
                "{kind}: confirmation did nothing"
            );
        }
        println!("{kind}: waiting, cancel, Escape and confirm passed");
    }
}
